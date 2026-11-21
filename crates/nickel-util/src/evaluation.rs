//! Confined evaluation of public Nickel data, independent of a domain schema.
use crate::error::{ensure, Error, Result};
use crate::{files, process::capture};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    os::fd::AsRawFd,
    path::{Component, Path, PathBuf},
    process::Command,
    time::Duration,
};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evaluation {
    pub value: serde_json::Value,
    pub modules_sha256: BTreeMap<String, String>,
    pub contract_sha256: Option<String>,
    pub flake_lock_sha256: String,
    pub evaluator: String,
}
fn modules(
    dir: &File,
    relative: &Path,
    destination: &Path,
    hashes: &mut BTreeMap<String, String>,
    count: &mut usize,
    bytes: &mut usize,
    depth: usize,
) -> Result<()> {
    ensure!(depth <= 32, "Nickel module nesting limit exceeded");
    for entry in fs::read_dir(format!("/proc/self/fd/{}", dir.as_raw_fd()))? {
        let entry = entry?;
        let name = entry.file_name();
        if [
            ".git",
            "target",
            "node_modules",
            ".direnv",
            ".zig-cache",
            "zig-out",
            "__pycache__",
        ]
        .iter()
        .any(|excluded| name == *excluded)
        {
            continue;
        }
        *count += 1;
        ensure!(
            *count <= 1000,
            "Too many configuration entries: place modules in a dedicated directory (.sandbox)"
        );
        let kind = entry.file_type()?;
        let rel = relative.join(&name);
        if kind.is_symlink() {
            ensure!(
                !rel.extension().is_some_and(|e| e == "ncl"),
                "Symlinks are not allowed in Nickel modules"
            );
            // Never follow links. An import through an omitted directory cannot resolve in /config.
            continue;
        }
        if kind.is_dir() {
            let child = files::open_child(dir, &name, true)?;
            modules(&child, &rel, destination, hashes, count, bytes, depth + 1)?;
        } else if rel.extension().is_some_and(|e| e == "ncl") {
            ensure!(hashes.len() < 100, "Maximum of 100 Nickel modules");
            let data = files::read_file(files::open_child(dir, &name, false)?, 1024 * 1024)?;
            *bytes += data.len();
            ensure!(*bytes <= 16 * 1024 * 1024, "Nickel modules are too large");
            let target = destination.join(&rel);
            fs::create_dir_all(target.parent().ok_or_else(|| Error::InvalidPath {
                path: target.clone(),
                reason: "Missing parent",
            })?)?;
            fs::write(target, &data)?;
            hashes.insert(
                rel.to_str()
                    .ok_or_else(|| Error::InvalidPath {
                        path: rel.clone(),
                        reason: "Non-UTF-8 Nickel path",
                    })?
                    .into(),
                files::hash(&data),
            );
        }
    }
    Ok(())
}
pub struct EvaluationRequest<'a> {
    pub entry: &'a Path,
    pub contract: Option<&'a Path>,
    /// Explicitly trusted flake directory selecting the pinned evaluator.
    /// Nix copies this directory to its public store. Supply only public recipes;
    /// higher-level adapters should stage their selected inputs first.
    pub nix: &'a Path,
}

/// Public Nickel modules mounted read-only at `/libraries/<name>`.
/// Names are unique ASCII alphanumeric components (also allowing `-` and `_`).
/// Project module directories with a registered name are rejected, preventing
/// Nickel's relative-first import resolution from shadowing that library.
pub struct LibraryBundle<'a> {
    pub name: &'a str,
    pub root: &'a Path,
}

/// A contract module relative to a named library root.
pub struct LibraryContract<'a> {
    pub library: &'a str,
    pub entry: &'a Path,
}

pub struct BundleEvaluationRequest<'a> {
    pub entry: &'a Path,
    /// Explicitly trusted public flake directory; see [`EvaluationRequest::nix`].
    pub nix: &'a Path,
    pub libraries: &'a [LibraryBundle<'a>],
    pub contract: Option<LibraryContract<'a>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundledEvaluation {
    pub evaluation: Evaluation,
    pub libraries_sha256: BTreeMap<String, BTreeMap<String, String>>,
}

pub fn evaluate_bundled(request: &BundleEvaluationRequest<'_>) -> Result<BundledEvaluation> {
    normalized_path(request.entry, false)?;
    normalized_path(request.nix, false)?;
    let libraries = stage_libraries(request)?;
    let evaluation = evaluate_with_libraries(
        &EvaluationRequest {
            entry: request.entry,
            contract: None,
            nix: request.nix,
        },
        Some(&libraries),
    )?;
    Ok(BundledEvaluation {
        evaluation,
        libraries_sha256: libraries.hashes,
    })
}

fn normalized_path(path: &Path, relative: bool) -> Result<()> {
    ensure!(
        !path.as_os_str().is_empty()
            && (!relative || path.is_relative())
            && path
                .components()
                .all(|component| matches!(component, Component::Normal(_) | Component::RootDir))
            && path.components().collect::<PathBuf>().as_os_str() == path.as_os_str(),
        "Nickel path is not normalized: {}",
        path.display()
    );
    Ok(())
}

struct StagedLibraries {
    directory: tempfile::TempDir,
    hashes: BTreeMap<String, BTreeMap<String, String>>,
    contract: Option<PathBuf>,
    contract_sha256: Option<String>,
}

#[derive(Default)]
struct LibraryBudget {
    entries: usize,
    modules: usize,
    bytes: usize,
}

fn stage_libraries(request: &BundleEvaluationRequest<'_>) -> Result<StagedLibraries> {
    ensure!(
        request.libraries.len() <= 8,
        "Maximum of 8 Nickel libraries"
    );
    let mut result = StagedLibraries {
        directory: tempfile::tempdir()?,
        hashes: BTreeMap::new(),
        contract: None,
        contract_sha256: None,
    };
    let mut budget = LibraryBudget::default();
    for library in request.libraries {
        ensure!(
            !library.name.is_empty()
                && library.name.len() <= 64
                && library.name.as_bytes()[0].is_ascii_alphanumeric()
                && library
                    .name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'),
            "Invalid Nickel library name"
        );
        ensure!(
            !result.hashes.contains_key(library.name),
            "Duplicate Nickel library: {}",
            library.name
        );
        normalized_path(library.root, false)?;
        let root = files::open_directory(library.root)?;
        let destination = result.directory.path().join(library.name);
        fs::create_dir(&destination)?;
        let mut hashes = BTreeMap::new();
        library_modules(
            &root,
            Path::new(""),
            &destination,
            &mut hashes,
            &mut budget,
            0,
        )?;
        result.hashes.insert(library.name.into(), hashes);
    }
    if let Some(contract) = &request.contract {
        normalized_path(contract.entry, true)?;
        ensure!(
            contract.entry.extension().is_some_and(|e| e == "ncl"),
            "A .ncl contract is required"
        );
        let name = contract.entry.to_str().ok_or_else(|| Error::InvalidPath {
            path: contract.entry.into(),
            reason: "Non-UTF-8 Nickel path",
        })?;
        let hash = result
            .hashes
            .get(contract.library)
            .and_then(|hashes| hashes.get(name))
            .ok_or_else(|| invalid("Contract missing from Nickel libraries".into()))?;
        result.contract = Some(
            Path::new("/libraries")
                .join(contract.library)
                .join(contract.entry),
        );
        result.contract_sha256 = Some(hash.clone());
    }
    Ok(result)
}

fn library_modules(
    directory: &File,
    relative: &Path,
    destination: &Path,
    hashes: &mut BTreeMap<String, String>,
    budget: &mut LibraryBudget,
    depth: usize,
) -> Result<()> {
    ensure!(depth <= 32, "Nickel library nesting limit exceeded");
    for entry in fs::read_dir(format!("/proc/self/fd/{}", directory.as_raw_fd()))? {
        let entry = entry?;
        budget.entries += 1;
        ensure!(
            budget.entries <= 4096,
            "Maximum of 4096 Nickel library entries"
        );
        let name = entry.file_name();
        let relative = relative.join(&name);
        let kind = entry.file_type()?;
        ensure!(
            !kind.is_symlink(),
            "Symlinks are not allowed in Nickel libraries"
        );
        ensure!(
            kind.is_file() || kind.is_dir(),
            "File type not allowed in Nickel libraries"
        );
        if kind.is_dir() {
            let child = files::open_child(directory, &name, true)?;
            library_modules(&child, &relative, destination, hashes, budget, depth + 1)?;
        } else if relative.extension().is_some_and(|e| e == "ncl") {
            budget.modules += 1;
            ensure!(
                budget.modules <= 2048,
                "Maximum of 2048 Nickel library modules"
            );
            let data = files::read_file(files::open_child(directory, &name, false)?, 1024 * 1024)?;
            budget.bytes += data.len();
            ensure!(
                budget.bytes <= 64 * 1024 * 1024,
                "Nickel libraries are too large"
            );
            let key = relative.to_str().ok_or_else(|| Error::InvalidPath {
                path: relative.clone(),
                reason: "Non-UTF-8 Nickel path",
            })?;
            let target = destination.join(&relative);
            fs::create_dir_all(target.parent().ok_or_else(|| Error::InvalidPath {
                path: target.clone(),
                reason: "Missing parent",
            })?)?;
            fs::write(target, &data)?;
            hashes.insert(key.into(), files::hash(&data));
        }
    }
    Ok(())
}

fn invalid(message: String) -> Error {
    Error::InvalidInput { message }
}

pub fn evaluate(request: &EvaluationRequest<'_>) -> Result<Evaluation> {
    evaluate_with_libraries(request, None)
}

fn evaluate_with_libraries(
    request: &EvaluationRequest<'_>,
    libraries: Option<&StagedLibraries>,
) -> Result<Evaluation> {
    let entry = request.entry;
    ensure!(
        entry.extension().is_some_and(|e| e == "ncl"),
        "A .ncl file is required"
    );
    let entry = files::absolute(entry)?;
    let _entry = files::open_regular(&entry)?;
    let nix_directory = files::absolute(request.nix)?;
    files::open_directory(&nix_directory)?;
    let schema = request
        .contract
        .map(|path| files::read_limited(path, 1024 * 1024))
        .transpose()?;
    let lock = files::read_limited(&nix_directory.join("flake.lock"), 1024 * 1024)?;
    let mut nix = Command::new("nix");
    nix.args([
        "--extra-experimental-features",
        "nix-command flakes",
        "eval",
        "--offline",
        "--no-write-lock-file",
        "--no-update-lock-file",
        "--raw",
    ])
    .arg(format!(
        "path:{}#packages.x86_64-linux.nickel.outPath",
        nix_directory.display()
    ));
    let store = String::from_utf8(capture(nix, Duration::from_secs(60), 1024 * 1024)?)?;
    let store = store.trim();
    ensure!(
        store.starts_with("/nix/store/") && !store.contains('\n'),
        "Invalid evaluator path"
    );
    let binary = Path::new(store).join("bin/nickel");
    ensure!(
        binary.is_file(),
        "Evaluator missing: explicitly build the nix#nickel package"
    );
    let mut query = Command::new("nix-store");
    query.args(["--query", "--requisites", store]);
    let closure = String::from_utf8(capture(query, Duration::from_secs(30), 1024 * 1024)?)?;
    let temp = tempfile::tempdir()?;
    let staged = temp.path().join("config");
    fs::create_dir(&staged)?;
    let mut hashes = BTreeMap::new();
    modules(
        &files::open_directory(entry.parent().ok_or_else(|| Error::InvalidPath {
            path: entry.clone(),
            reason: "Missing parent",
        })?)?,
        Path::new(""),
        &staged,
        &mut hashes,
        &mut 0,
        &mut 0,
        0,
    )?;
    if let Some(libraries) = libraries {
        // Nickel tries the importing module's directory before --import-path.
        // Reserve bundle names throughout the staged project namespace.
        for module in hashes.keys() {
            if let Some(parent) = Path::new(module).parent() {
                for component in parent.components() {
                    if let Component::Normal(name) = component {
                        ensure!(
                            !name
                                .to_str()
                                .is_some_and(|name| libraries.hashes.contains_key(name)),
                            "Project module shadows a Nickel library: {module}"
                        );
                    }
                }
            }
        }
    }
    let schema_path = temp.path().join("schema.ncl");
    if let Some(schema) = &schema {
        fs::write(&schema_path, schema)?;
    }
    let mut cmd = Command::new("prlimit");
    cmd.args([
        "--as=1073741824",
        "--cpu=20",
        "--",
        "bwrap",
        "--unshare-all",
        "--die-with-parent",
        "--new-session",
        "--clearenv",
        "--setenv",
        "HOME",
        "/tmp",
        "--tmpfs",
        "/tmp",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
    ]);
    for path in closure.lines() {
        ensure!(
            path.starts_with("/nix/store/") && Path::new(path).components().count() == 4,
            "Invalid Nix closure"
        );
        cmd.args(["--ro-bind", path, path]);
    }
    cmd.arg("--ro-bind").arg(staged).arg("/config");
    if schema.is_some() {
        cmd.arg("--ro-bind").arg(&schema_path).arg("/schema.ncl");
    }
    if let Some(libraries) = libraries {
        cmd.arg("--ro-bind")
            .arg(libraries.directory.path())
            .arg("/libraries");
    }
    cmd.args(["--chdir", "/config"])
        .arg(&binary)
        .args(["export", "--format", "json"]);
    if schema.is_some() {
        cmd.args(["--apply-contract", "/schema.ncl"]);
    }
    if let Some(libraries) = libraries {
        cmd.args(["--import-path", "/libraries"]);
        if let Some(contract) = &libraries.contract {
            cmd.arg("--apply-contract").arg(contract);
        }
    }
    cmd.arg(
        Path::new("/config").join(entry.file_name().ok_or_else(|| Error::InvalidPath {
            path: entry.clone(),
            reason: "Missing name",
        })?),
    );
    let output =
        capture(cmd, Duration::from_secs(30), 4 * 1024 * 1024).map_err(|source| Error::Nickel {
            source: Box::new(source),
        })?;
    let value = crate::json::parse(&output)?;
    Ok(Evaluation {
        value,
        modules_sha256: hashes,
        contract_sha256: libraries
            .and_then(|libraries| libraries.contract_sha256.clone())
            .or_else(|| schema.as_ref().map(|data| files::hash(data))),
        flake_lock_sha256: files::hash(&lock),
        evaluator: binary.to_string_lossy().into(),
    })
}

/// Create a private entry that deserializes JSON data without interpreting it as
/// Nickel source. The returned directory owns the entry's lifetime.
pub fn json_entry(value: &serde_json::Value) -> Result<tempfile::TempDir> {
    let json = serde_json::to_string(value)?;
    ensure!(json.len() <= 1024 * 1024, "JSON entry exceeds 1 MiB");
    let quoted = serde_json::to_string(&json)?.replace("%{", "\\%{");
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()?;
    fs::write(
        directory.path().join("value.ncl"),
        format!("std.deserialize 'Json {quoted}\n"),
    )?;
    Ok(directory)
}

#[cfg(test)]
mod json_entry_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn json_data_entry_is_private_and_bounded() -> Result<()> {
        let entry = json_entry(&serde_json::json!({"value":"%{import \"/etc/passwd\"}\u{0}\n"}))?;
        ensure!(
            entry.path().metadata()?.permissions().mode() & 0o077 == 0,
            "entry must be private"
        );
        let source = fs::read_to_string(entry.path().join("value.ncl"))?;
        ensure!(source.contains("\\%{"), "interpolation must be escaped");
        ensure!(
            json_entry(&serde_json::Value::String("x".repeat(1024 * 1024))).is_err(),
            "oversized JSON must fail"
        );
        Ok(())
    }
}

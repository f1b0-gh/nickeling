use crate::{
    error::{ensure, Error, Result},
    files,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, PermissionsExt},
    },
    path::Path,
};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sources {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}
pub fn relative_path(s: &str) -> Result<()> {
    if !(!s.is_empty()
        && !s.chars().any(char::is_control)
        && s.split('/').all(|c| !c.is_empty() && c != "." && c != ".."))
    {
        return Err(Error::InvalidConfiguration {
            message: format!("Normalized relative path required: {s}"),
        });
    }
    Ok(())
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileRecord {
    pub sha256: String,
    pub executable: bool,
}
pub type Snapshot = BTreeMap<String, FileRecord>;
pub fn excluded(name: &str, spec: &Sources) -> bool {
    excluded_entry(name, spec, false)
}
/// Filter a directory established by a no-follow type check. Before traversing
/// it, callers must pin a no-follow directory descriptor (for example with
/// `O_DIRECTORY | O_NOFOLLOW`) so a replacement file or link cannot inherit
/// this exemption. Use [`excluded`] for file paths and untyped names.
pub fn excluded_directory(name: &str, spec: &Sources) -> bool {
    excluded_entry(name, spec, true)
}
fn excluded_entry(name: &str, spec: &Sources, directory: bool) -> bool {
    name.split('/').any(|part| {
        let lower = part.to_ascii_lowercase();
        [
            ".git",
            ".hg",
            ".svn",
            ".jj",
            ".codex",
            ".ssh",
            ".aws",
            ".azure",
            ".kube",
            ".gnupg",
            ".pytest_cache",
            ".ruff_cache",
            ".mypy_cache",
            ".venv",
            "venv",
            "node_modules",
            "target",
            "dist",
            "build",
            "zig-cache",
            ".zig-cache",
            "zig-out",
            "__pycache__",
            ".next",
            ".cache",
            "lab-bin",
            ".direnv",
        ]
        .contains(&part)
            || [
                "auth.json",
                "credentials.json",
                "secrets.json",
                "secrets.yaml",
                "secrets.yml",
                ".npmrc",
                ".pypirc",
                ".netrc",
                "id_rsa",
                "id_ed25519",
            ]
            .contains(&lower.as_str())
            || lower.starts_with(".env")
            || [".pem", ".key", ".p12", ".pfx", ".secret"]
                .iter()
                .any(|suffix| lower.ends_with(suffix))
    }) || (!directory
        && name
            .rsplit('/')
            .next()
            .is_some_and(|leaf| leaf.eq_ignore_ascii_case("credentials")))
        || name == "evidence"
        || name.starts_with("evidence/")
        || spec.exclude.iter().any(|p| {
            name == p
                || name
                    .strip_prefix(p)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
}
struct Collector<'a> {
    spec: &'a Sources,
    destination: Option<&'a Path>,
    files: Snapshot,
    total: usize,
    entries: usize,
}
impl Collector<'_> {
    fn visit(&mut self, file: File, name: &str, depth: usize) -> Result<()> {
        ensure!(depth <= 64, "Source nesting limit exceeded");
        self.entries += 1;
        ensure!(self.entries <= 20000, "Too many source entries");
        let meta = file.metadata()?;
        if meta.is_dir() {
            for entry in fs::read_dir(format!("/proc/self/fd/{}", file.as_raw_fd()))? {
                let entry = entry?;
                let child = entry.file_name();
                let child_name = child
                    .to_str()
                    .ok_or_else(|| invalid("Non-UTF-8 source name".into()))?;
                let path = format!("{name}/{child_name}");
                if excluded_entry(&path, self.spec, true) {
                    continue;
                }
                let directory = entry.file_type()?.is_dir();
                if excluded_entry(&path, self.spec, directory) {
                    continue;
                }
                // Pin a directory without following links before accepting a
                // name that would be excluded for a regular credential file.
                let child = if directory {
                    files::open_child(&file, &child, true)?
                } else {
                    files::open_entry(&file, &child)?
                };
                self.visit(child, &path, depth + 1)?;
            }
        } else {
            if self.files.contains_key(name) {
                return Ok(());
            }
            ensure!(self.files.len() < 5000, "Maximum of 5000 source files");
            let data = files::read_file(file, 16 * 1024 * 1024)?;
            self.total += data.len();
            ensure!(
                self.total <= 256 * 1024 * 1024,
                "Total source byte budget exceeded"
            );
            let record = FileRecord {
                sha256: files::hash(&data),
                executable: meta.mode() & 0o111 != 0,
            };
            if let Some(destination) = self.destination {
                let target = destination.join(name);
                let parent = target
                    .parent()
                    .ok_or_else(|| invalid("Missing source parent".into()))?;
                fs::create_dir_all(parent)?;
                // Destination is a newly allocated, host-private staging directory.
                let mut out = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&target)?;
                std::io::Write::write_all(&mut out, &data)?;
                out.set_permissions(fs::Permissions::from_mode(if record.executable {
                    0o755
                } else {
                    0o644
                }))?;
            }
            self.files.insert(name.into(), record);
        }
        Ok(())
    }
}
pub fn snapshot(root: &Path, spec: &Sources, destination: Option<&Path>) -> Result<Snapshot> {
    let root = files::open_directory(root)?;
    if let Some(destination) = destination {
        let dir = files::open_directory(destination)?;
        ensure!(
            fs::read_dir(format!("/proc/self/fd/{}", dir.as_raw_fd()))?
                .next()
                .is_none(),
            "Source staging directory is not empty"
        );
    }
    let mut collector = Collector {
        spec,
        destination,
        files: BTreeMap::new(),
        total: 0,
        entries: 0,
    };
    for name in spec.include.iter().chain(&spec.exclude) {
        relative_path(name)?;
    }
    for name in &spec.include {
        ensure!(
            !excluded_entry(name, spec, true),
            "Source explicitly excluded: {name}"
        );
        let mut current = root.try_clone()?;
        for component in Path::new(name).components() {
            current = files::open_entry(&current, component.as_os_str())?;
        }
        ensure!(
            !excluded_entry(name, spec, current.metadata()?.is_dir()),
            "Source explicitly excluded: {name}"
        );
        collector.visit(current, name, 0)?;
    }
    Ok(collector.files)
}
fn invalid(message: String) -> Error {
    Error::InvalidInput { message }
}

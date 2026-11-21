//! Offline schema bundles. Native provider locks are retained, never interpreted.
use crate::schema::{self, Selection};
use nickel_util::{
    error::{Error, Result},
    files, json, sources,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    ffi::CString,
    fs::File,
    io::Write,
    os::{fd::AsRawFd, unix::ffi::OsStrExt},
    path::{Path, PathBuf},
};

const INPUT_LIMIT: usize = schema::MAX_INPUT_BYTES;
const MODULE_LIMIT: usize = schema::MAX_FILE_BYTES;
const MODULES_LIMIT: usize = schema::MAX_GENERATED_BYTES;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u32,
    pub files: BTreeMap<String, String>,
}

pub(crate) struct Verified {
    pub manifest: Manifest,
    pub selection: Selection,
    pub generated: BTreeMap<String, String>,
}

fn require(condition: bool, message: impl Into<String>) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(Error::InvalidInput {
            message: message.into(),
        })
    }
}

fn parse(bytes: &[u8]) -> Result<Value> {
    let value = json::parse(bytes)?;
    let mut stack = vec![(&value, 0)];
    while let Some((item, depth)) = stack.pop() {
        require(depth <= 64, "Schema input nesting exceeds 64 levels")?;
        match item {
            Value::Array(items) => stack.extend(items.iter().map(|v| (v, depth + 1))),
            Value::Object(items) => stack.extend(items.values().map(|v| (v, depth + 1))),
            _ => (),
        }
    }
    Ok(value)
}

fn generate(inputs: &BTreeMap<String, Vec<u8>>) -> Result<(Selection, BTreeMap<String, String>)> {
    let get = |name: &str| {
        inputs.get(name).ok_or_else(|| Error::InvalidInput {
            message: format!("Missing bundle input: {name}"),
        })
    };
    let selection: Selection = serde_json::from_value(parse(get("selection.json")?)?)?;
    let schema = parse(get("schema.json")?)?;
    let generated = schema::generate(&schema, &selection)?;
    require(
        generated.contains_key("schema.ncl"),
        "Generated bundle lacks schema.ncl",
    )?;
    require(
        generated.len() <= schema::MAX_FILES,
        "Generated bundle exceeds the module budget reserved for provider schemas",
    )?;
    let mut total = 0usize;
    for (name, content) in &generated {
        sources::relative_path(name)?;
        require(
            name.ends_with(".ncl"),
            "Generated module must have .ncl extension",
        )?;
        require(
            content.len() <= MODULE_LIMIT,
            "Generated module exceeds 1 MiB",
        )?;
        total = total.saturating_add(content.len());
    }
    require(
        total <= MODULES_LIMIT,
        "Generated bundle exceeds the byte budget reserved for provider schemas",
    )?;
    if selection
        .providers
        .values()
        .any(|pin| pin.source != "terraform.io/builtin/terraform")
    {
        require(
            !get(".terraform.lock.hcl")?.is_empty(),
            "External providers require a native provider lock",
        )?;
    }
    Ok((selection, generated))
}

fn assemble(
    mut inputs: BTreeMap<String, Vec<u8>>,
    generated: &BTreeMap<String, String>,
) -> BTreeMap<String, Vec<u8>> {
    inputs.extend(
        generated
            .iter()
            .map(|(name, text)| (format!("generated/{name}"), text.as_bytes().to_vec())),
    );
    inputs
}

fn manifest(files: &BTreeMap<String, Vec<u8>>) -> Manifest {
    Manifest {
        version: 1,
        files: files
            .iter()
            .map(|(name, bytes)| (name.clone(), files::hash(bytes)))
            .collect(),
    }
}

/// Convert a frozen schema export without invoking OpenTofu, plugins or network access.
/// `selection` asserts provenance; OpenTofu validates the native lock during init.
pub fn import_schema(
    schema_path: &Path,
    selection_path: &Path,
    provider_lock: Option<&Path>,
    output: &Path,
) -> Result<Manifest> {
    let mut inputs = BTreeMap::from([
        (
            "schema.json".into(),
            files::read_limited(schema_path, INPUT_LIMIT)?,
        ),
        (
            "selection.json".into(),
            files::read_limited(selection_path, INPUT_LIMIT)?,
        ),
    ]);
    if let Some(path) = provider_lock {
        inputs.insert(
            ".terraform.lock.hcl".into(),
            files::read_limited(path, INPUT_LIMIT)?,
        );
    }
    let (_, generated) = generate(&inputs)?;
    let contents = assemble(inputs, &generated);
    let manifest = manifest(&contents);
    publish(output, contents, &manifest)?;
    Ok(manifest)
}

/// Verify both byte integrity and deterministic regeneration of every contract.
pub fn verify(root: &Path) -> Result<Manifest> {
    Ok(load(root)?.manifest)
}

pub(crate) fn load(root: &Path) -> Result<Verified> {
    let manifest: Manifest = serde_json::from_value(json::parse(&files::read_limited(
        &root.join("manifest.json"),
        MODULE_LIMIT,
    )?)?)?;
    require(manifest.version == 1, "Unsupported schema bundle version")?;
    require(
        manifest.files.len() <= schema::MAX_FILES + 3,
        "Too many schema bundle files",
    )?;
    let mut inputs = BTreeMap::new();
    let mut actual = BTreeMap::new();
    let mut total = 0usize;
    for (name, hash) in &manifest.files {
        sources::relative_path(name)?;
        let input = matches!(
            name.as_str(),
            "schema.json" | "selection.json" | ".terraform.lock.hcl"
        );
        require(
            input || (name.starts_with("generated/") && name.ends_with(".ncl")),
            format!("Unknown bundle file: {name}"),
        )?;
        let bytes = files::read_limited(
            &root.join(name),
            if input { INPUT_LIMIT } else { MODULE_LIMIT },
        )?;
        require(
            files::hash(&bytes) == *hash,
            format!("Schema bundle integrity mismatch: {name}"),
        )?;
        if input {
            inputs.insert(name.clone(), bytes);
        } else {
            total = total.saturating_add(bytes.len());
            require(
                total <= MODULES_LIMIT,
                "Generated bundle exceeds the byte budget reserved for provider schemas",
            )?;
            actual.insert(name.clone(), bytes);
        }
    }
    let (selection, generated) = generate(&inputs)?;
    let expected = assemble(inputs, &generated);
    require(
        self::manifest(&expected) == manifest,
        "Bundle manifest differs from deterministic schema generation",
    )?;
    for (name, bytes) in actual {
        require(
            expected.get(&name) == Some(&bytes),
            format!("Generated contract differs from source schema: {name}"),
        )?;
    }
    Ok(Verified {
        manifest,
        selection,
        generated,
    })
}

fn publish(
    output: &Path,
    mut contents: BTreeMap<String, Vec<u8>>,
    manifest: &Manifest,
) -> Result<()> {
    let output = files::absolute(output)?;
    let parent = files::open_directory(output.parent().ok_or_else(|| Error::InvalidInput {
        message: "Bundle output requires a parent directory".into(),
    })?)?;
    let name = output.file_name().ok_or_else(|| Error::InvalidInput {
        message: "Bundle output requires a directory name".into(),
    })?;
    let target_name = CString::new(name.as_bytes())?;
    // Keep the parent pinned throughout creation and publication, even if renamed.
    let pinned = PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd()));
    let temporary = tempfile::Builder::new()
        .prefix(".nickel-tf-")
        .tempdir_in(&pinned)?;
    let mut metadata = serde_json::to_vec_pretty(manifest)?;
    metadata.push(b'\n');
    contents.insert("manifest.json".into(), metadata);
    let mut directories = vec![temporary.path().to_path_buf()];
    for (relative, bytes) in contents {
        let path = temporary.path().join(relative);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
            directories.extend(
                dir.ancestors()
                    .take_while(|d| d.starts_with(temporary.path()))
                    .map(Path::to_path_buf),
            );
        }
        let mut file = File::create(path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    directories.sort();
    directories.dedup();
    for dir in directories.iter().rev() {
        File::open(dir)?.sync_all()?;
    }
    let source_name = CString::new(
        temporary
            .path()
            .file_name()
            .ok_or_else(|| Error::InvalidInput {
                message: "Missing temporary directory name".into(),
            })?
            .as_bytes(),
    )?;
    // SAFETY: parent is a live descriptor and both names are single NUL-terminated
    // components. RENAME_NOREPLACE makes publication atomic and refuses all targets.
    let result = unsafe {
        libc::renameat2(
            parent.as_raw_fd(),
            source_name.as_ptr(),
            parent.as_raw_fd(),
            target_name.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    parent.sync_all()?;
    Ok(())
}

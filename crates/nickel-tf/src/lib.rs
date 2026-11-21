//! Offline Nickel configuration compiler for OpenTofu.
pub mod bundle;
pub mod rendering;
pub mod schema;

use nickel_util::{
    error::{Error, Result},
    evaluation::{evaluate_bundled, BundleEvaluationRequest, LibraryBundle, LibraryContract},
    files, json,
    sources::{self, Snapshot, Sources},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub struct Request {
    pub config: PathBuf,
    /// Trusted public evaluator recipes and static Nickel libraries.
    pub resources: PathBuf,
    /// Frozen provider schema bundle created by `import-schema`.
    pub bundle: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Lock {
    pub version: u32,
    pub selection: schema::Selection,
    pub schema_bundle: bundle::Manifest,
    pub configuration_sha256: String,
    pub rendered_sha256: String,
    pub modules_sha256: BTreeMap<String, String>,
    pub libraries_sha256: BTreeMap<String, BTreeMap<String, String>>,
    pub recipes: Snapshot,
    pub evaluator: String,
    pub flake_lock_sha256: String,
}

pub struct Checked {
    pub configuration: Value,
    pub lock: Lock,
}

/// Type-check and render offline. This does not initialize providers or execute
/// OpenTofu; expression semantics, provider behavior and state belong to OpenTofu.
pub fn check(request: &Request) -> Result<Checked> {
    let config = files::absolute(&request.config)?;
    let resources = files::absolute(&request.resources)?;
    let verified = bundle::load(&files::absolute(&request.bundle)?)?;
    let staging = tempfile::tempdir()?;
    // Only explicitly selected public recipes reach Nix's globally readable store.
    let recipes = sources::snapshot(
        &resources,
        &Sources {
            include: vec!["flake.nix".into(), "flake.lock".into(), "nickel".into()],
            exclude: vec![],
        },
        Some(staging.path()),
    )?;
    let provider_staging = tempfile::tempdir()?;
    for (name, content) in &verified.generated {
        let path = provider_staging.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
    }
    let core = staging.path().join("nickel");
    let libraries = [
        LibraryBundle {
            name: "nickel-tf",
            root: &core,
        },
        LibraryBundle {
            name: "providers",
            root: provider_staging.path(),
        },
    ];
    let evaluated = evaluate_bundled(&BundleEvaluationRequest {
        entry: &config,
        nix: staging.path(),
        libraries: &libraries,
        contract: Some(LibraryContract {
            library: "providers",
            entry: Path::new("schema.ncl"),
        }),
    })?;
    let evaluation = evaluated.evaluation;
    let configuration_sha256 = files::hash(&serde_json::to_vec(&evaluation.value)?);
    let configuration = rendering::render_config(evaluation.value, &verified.selection)?;
    let lock = Lock {
        version: 1,
        selection: verified.selection,
        schema_bundle: verified.manifest,
        configuration_sha256,
        rendered_sha256: files::hash(&serde_json::to_vec(&configuration)?),
        modules_sha256: evaluation.modules_sha256,
        libraries_sha256: evaluated.libraries_sha256,
        recipes,
        evaluator: evaluation.evaluator,
        flake_lock_sha256: evaluation.flake_lock_sha256,
    };
    Ok(Checked {
        configuration,
        lock,
    })
}

/// Render deterministic OpenTofu JSON, rejecting all drift when a lock is given.
pub fn render(request: &Request, lock_path: Option<&Path>) -> Result<Value> {
    let checked = check(request)?;
    if let Some(path) = lock_path {
        let recorded: Lock =
            serde_json::from_value(json::parse(&files::read_limited(path, 4 * 1024 * 1024)?)?)?;
        if recorded != checked.lock {
            return Err(Error::InvalidInput {message: "Configuration lock is stale or incompatible; review changes and create a new lock explicitly".into()});
        }
    }
    Ok(checked.configuration)
}

pub fn lock(request: &Request) -> Result<Lock> {
    Ok(check(request)?.lock)
}

/// Validate already evaluated composition data through the same confined contracts.
pub fn check_value(value: &Value, resources: &Path, bundle: &Path) -> Result<Checked> {
    let entry = nickel_util::evaluation::json_entry(value)?;
    check(&Request {
        config: entry.path().join("value.ncl"),
        resources: resources.into(),
        bundle: bundle.into(),
    })
}

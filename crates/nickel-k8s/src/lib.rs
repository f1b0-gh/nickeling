//! Offline Kubernetes application composition using pinned Nickel contracts.
//! Cluster admission and runtime availability remain separate validation steps.
mod model;
use model::require;
pub use model::Target;
use nickel_util::{
    error::Result,
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
    pub resources: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Lock {
    pub version: u32,
    pub target: Target,
    pub configuration_sha256: String,
    pub manifest_sha256: String,
    pub modules_sha256: BTreeMap<String, String>,
    pub libraries_sha256: BTreeMap<String, BTreeMap<String, String>>,
    pub recipes: Snapshot,
    pub evaluator: String,
    pub flake_lock_sha256: String,
}

pub struct Checked {
    pub target: Target,
    pub manifest: Value,
    pub lock: Lock,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Provenance {
    repository: String,
    revision: String,
    kubernetes_version: String,
    files: BTreeMap<String, String>,
    license_sha256: String,
}

fn verify_upstream(root: &Path) -> Result<()> {
    let path = root.join("nickel/contracts/upstream");
    let provenance: Provenance = serde_json::from_value(json::parse(&files::read_limited(
        &path.join("provenance.json"),
        1024 * 1024,
    )?)?)?;
    require(
        provenance.repository == "https://github.com/nickel-lang/nickel-kubernetes"
            && provenance.revision == "f3a310db82a408d7e1935b98f7ce103263df5035"
            && provenance.kubernetes_version == "1.34.0",
        "Unsupported upstream contract provenance",
    )?;
    require(!provenance.files.is_empty(), "Empty contract manifest")?;
    for (name, expected) in &provenance.files {
        sources::relative_path(name)?;
        require(
            name.ends_with(".ncl"),
            "Contract manifest entries must be Nickel modules",
        )?;
        let actual = files::hash(&files::read_limited(
            &path.join("v1.34.0").join(name),
            1024 * 1024,
        )?);
        require(
            actual == *expected,
            format!("Upstream contract integrity mismatch: {name}"),
        )?;
    }
    require(
        files::hash(&files::read_limited(&path.join("LICENSE"), 1024 * 1024)?)
            == provenance.license_sha256,
        "Upstream license integrity mismatch",
    )
}

pub fn check(request: &Request) -> Result<Checked> {
    let config = files::absolute(&request.config)?;
    let resources = files::absolute(&request.resources)?;
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
    verify_upstream(staging.path())?;
    let root = staging.path().join("nickel");
    let libraries = [LibraryBundle {
        name: "nickel-k8s",
        root: &root,
    }];
    let evaluated = evaluate_bundled(&BundleEvaluationRequest {
        entry: &config,
        nix: staging.path(),
        libraries: &libraries,
        contract: Some(LibraryContract {
            library: "nickel-k8s",
            entry: Path::new("bundle.ncl"),
        }),
    })?;
    let evaluation = evaluated.evaluation;
    let configuration_sha256 = files::hash(&serde_json::to_vec(&evaluation.value)?);
    let bundle: model::Bundle = serde_json::from_value(evaluation.value)?;
    let (target, manifest) = model::normalize(bundle)?;
    let lock = Lock {
        version: 1,
        target: target.clone(),
        configuration_sha256,
        manifest_sha256: files::hash(&serde_json::to_vec(&manifest)?),
        modules_sha256: evaluation.modules_sha256,
        libraries_sha256: evaluated.libraries_sha256,
        recipes,
        evaluator: evaluation.evaluator,
        flake_lock_sha256: evaluation.flake_lock_sha256,
    };
    Ok(Checked {
        target,
        manifest,
        lock,
    })
}

/// Return a deterministic Kubernetes List; an explicit lock rejects any drift.
pub fn render(request: &Request, lock_path: Option<&Path>) -> Result<Value> {
    let checked = check(request)?;
    if let Some(path) = lock_path {
        let recorded: Lock =
            serde_json::from_value(json::parse(&files::read_limited(path, 4 * 1024 * 1024)?)?)?;
        require(recorded==checked.lock,"Manifest lock is stale or incompatible; review changes and create a new lock explicitly")?;
    }
    Ok(checked.manifest)
}

pub fn lock(request: &Request) -> Result<Lock> {
    Ok(check(request)?.lock)
}

/// Validate already evaluated composition data through the same confined contracts.
pub fn check_value(value: &Value, resources: &Path) -> Result<Checked> {
    let entry = nickel_util::evaluation::json_entry(value)?;
    check(&Request {
        config: entry.path().join("value.ncl"),
        resources: resources.into(),
    })
}

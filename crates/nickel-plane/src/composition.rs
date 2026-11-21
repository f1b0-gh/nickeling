use crate::{
    model::{BackendKind, Instance, Unit},
    Error, Result,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};
pub struct CompileRequest {
    pub config: PathBuf,
    pub resources: PathBuf,
    pub tf_resources: Option<PathBuf>,
    pub k8s_resources: Option<PathBuf>,
    pub schemas: BTreeMap<String, PathBuf>,
    pub targets: BTreeSet<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompiledUnit {
    pub artifact: Value,
    pub receipt: Value,
    pub engine_version: Option<String>,
    pub provider_lock: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Compiled {
    pub version: u32,
    pub revision: String,
    pub instance: Instance,
    pub inputs: Value,
    pub units: BTreeMap<String, CompiledUnit>,
}

pub fn compile(request: &CompileRequest) -> Result<Compiled> {
    use nickel_util::{
        evaluation::{evaluate_bundled, BundleEvaluationRequest, LibraryBundle, LibraryContract},
        files,
        sources::{self, Sources},
    };
    let staging = tempfile::tempdir()?;
    let recipes = sources::snapshot(
        &files::absolute(&request.resources)?,
        &Sources {
            include: vec!["flake.nix".into(), "flake.lock".into(), "nickel".into()],
            exclude: vec![],
        },
        Some(staging.path()),
    )?;
    let core = staging.path().join("nickel");
    let tf = request
        .tf_resources
        .as_ref()
        .map(|p| files::absolute(p).map(|p| p.join("nickel")))
        .transpose()?;
    let k8s = request
        .k8s_resources
        .as_ref()
        .map(|p| files::absolute(p).map(|p| p.join("nickel")))
        .transpose()?;
    let mut libraries = vec![LibraryBundle {
        name: "nickel-plane",
        root: &core,
    }];
    if let Some(root) = &tf {
        libraries.push(LibraryBundle {
            name: "nickel-tf",
            root,
        });
    }
    if let Some(root) = &k8s {
        libraries.push(LibraryBundle {
            name: "nickel-k8s",
            root,
        });
    }
    let evaluated = evaluate_bundled(&BundleEvaluationRequest {
        entry: &files::absolute(&request.config)?,
        nix: staging.path(),
        libraries: &libraries,
        contract: Some(LibraryContract {
            library: "nickel-plane",
            entry: std::path::Path::new("instance.ncl"),
        }),
    })?;
    let instance: Instance = serde_json::from_value(evaluated.evaluation.value)?;
    instance.validate(&request.targets)?;
    let inputs = serde_json::json!({"modules":evaluated.evaluation.modules_sha256,"libraries":evaluated.libraries_sha256,"evaluator":evaluated.evaluation.evaluator,"flakeLock":evaluated.evaluation.flake_lock_sha256,"recipes":recipes});
    let mut units = BTreeMap::new();
    for (id, unit) in &instance.units {
        units.insert(
            id.clone(),
            validate_unit(request, unit, &unit.configuration)?,
        );
    }
    let mut compiled = Compiled {
        version: 1,
        revision: String::new(),
        instance,
        inputs,
        units,
    };
    compiled.revision = compiled.digest()?;
    Ok(compiled)
}
impl Compiled {
    pub fn digest(&self) -> Result<String> {
        Ok(nickel_util::files::hash(&serde_json::to_vec(&(
            self.version,
            &self.instance,
            &self.inputs,
            &self.units,
        ))?))
    }
    pub fn verify(&self) -> Result<()> {
        crate::require(
            self.version == 1 && self.revision == self.digest()?,
            "Invalid composition revision receipt",
        )?;
        let targets = self
            .instance
            .units
            .values()
            .map(|u| u.target.clone())
            .collect();
        self.instance.validate(&targets)?;
        crate::require(
            self.units.keys().eq(self.instance.units.keys()),
            "Compiled unit set differs from instance",
        )
    }
}
pub trait Validator {
    fn validate(
        &self,
        unit: &Unit,
        configuration: &Value,
        baseline: &CompiledUnit,
    ) -> Result<CompiledUnit>;
}
impl Validator for CompileRequest {
    fn validate(
        &self,
        unit: &Unit,
        configuration: &Value,
        baseline: &CompiledUnit,
    ) -> Result<CompiledUnit> {
        let result = validate_unit(self, unit, configuration)?;
        let strip = |v: &Value| {
            let mut v = v.clone();
            if let Some(m) = v.as_object_mut() {
                for key in [
                    "configuration_sha256",
                    "rendered_sha256",
                    "manifest_sha256",
                    "modules_sha256",
                ] {
                    m.remove(key);
                }
            }
            v
        };
        crate::require(
            strip(&result.receipt) == strip(&baseline.receipt),
            "Domain compiler inputs changed; recompile the composition",
        )?;
        Ok(result)
    }
}
pub fn validate_unit(
    request: &CompileRequest,
    unit: &Unit,
    configuration: &Value,
) -> Result<CompiledUnit> {
    match unit.backend {
        BackendKind::OpenTofu => {
            #[cfg(feature = "opentofu")]
            {
                let resources = request
                    .tf_resources
                    .as_ref()
                    .ok_or_else(|| Error::Invalid("OpenTofu compiler resources required".into()))?;
                let schema = unit
                    .schema
                    .as_ref()
                    .and_then(|name| request.schemas.get(name))
                    .ok_or_else(|| Error::Invalid("Unknown provider schema bundle".into()))?;
                let checked = nickel_tf::check_value(configuration, resources, schema)?;
                let provider_lock =
                    if let Some(hash) = checked.lock.schema_bundle.files.get(".terraform.lock.hcl")
                    {
                        let bytes = nickel_util::files::read_limited(
                            &schema.join(".terraform.lock.hcl"),
                            16 * 1024 * 1024,
                        )?;
                        crate::require(
                            nickel_util::files::hash(&bytes) == *hash,
                            "Provider lock changed during compilation",
                        )?;
                        Some(String::from_utf8(bytes).map_err(|_| {
                            Error::Invalid("Native provider lock must be UTF-8".into())
                        })?)
                    } else {
                        None
                    };
                Ok(CompiledUnit {
                    artifact: checked.configuration,
                    engine_version: Some(checked.lock.selection.engine.version.clone()),
                    receipt: serde_json::to_value(checked.lock)?,
                    provider_lock,
                })
            }
            #[cfg(not(feature = "opentofu"))]
            {
                let _ = (request, configuration);
                Err(Error::Unsupported(
                    "OpenTofu adapter feature is disabled".into(),
                ))
            }
        }
        BackendKind::Kubernetes => {
            #[cfg(feature = "kubernetes")]
            {
                let resources = request.k8s_resources.as_ref().ok_or_else(|| {
                    Error::Invalid("Kubernetes compiler resources required".into())
                })?;
                let checked = nickel_k8s::check_value(configuration, resources)?;
                Ok(CompiledUnit {
                    artifact: checked.manifest,
                    receipt: serde_json::to_value(checked.lock)?,
                    engine_version: None,
                    provider_lock: None,
                })
            }
            #[cfg(not(feature = "kubernetes"))]
            {
                let _ = (request, configuration);
                Err(Error::Unsupported(
                    "Kubernetes adapter feature is disabled".into(),
                ))
            }
        }
    }
}

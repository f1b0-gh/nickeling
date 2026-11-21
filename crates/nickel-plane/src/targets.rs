use crate::{
    model::{validate_id, BackendKind},
    reconcile::Registry,
    require, Result,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenTofuTarget {
    /// Retain bounded, potentially sensitive diagnostics privately on command failure.
    #[serde(default)]
    pub private_diagnostics: bool,
    pub binary: PathBuf,
    pub stack: String,
    #[serde(default)]
    pub credentials: Option<PathBuf>,
    #[serde(default)]
    pub backend_config: Option<BackendConfig>,
    #[serde(default = "timeout")]
    pub timeout_seconds: u64,
}
fn timeout() -> u64 {
    120
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackendConfig {
    pub kind: String,
    pub configuration: Value,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KubernetesTarget {
    pub server: String,
    pub namespace: String,
    #[serde(default)]
    pub ca_file: Option<PathBuf>,
    /// Offline proof for an explicitly requested legacy binding upgrade; never TLS roots.
    #[serde(default)]
    pub previous_ca_file: Option<PathBuf>,
    #[serde(default)]
    pub token_file: Option<PathBuf>,
    #[serde(default)]
    pub allow_http_loopback: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "backend",
    content = "config",
    rename_all = "lowercase",
    deny_unknown_fields
)]
pub enum Target {
    OpenTofu(OpenTofuTarget),
    Kubernetes(KubernetesTarget),
}
impl Target {
    pub fn kind(&self) -> BackendKind {
        match self {
            Self::OpenTofu(_) => BackendKind::OpenTofu,
            Self::Kubernetes(_) => BackendKind::Kubernetes,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Targets {
    pub version: u32,
    pub targets: BTreeMap<String, Target>,
}
impl Targets {
    pub fn validate(&self) -> Result<()> {
        require(
            self.version == 1 && self.targets.len() <= 64,
            "Unsupported target catalog",
        )?;
        for name in self.targets.keys() {
            validate_id(name)?;
        }
        Ok(())
    }
    pub fn registry(&self) -> Result<Registry> {
        self.validate()?;
        let mut registry = Registry::new();
        for (name, target) in &self.targets {
            let backend = target.backend()?;
            registry.insert(name.clone(), backend);
        }
        Ok(registry)
    }
}

impl Target {
    fn backend(&self) -> Result<Box<dyn crate::backend::Backend>> {
        match self {
            Self::OpenTofu(config) => {
                #[cfg(feature = "opentofu")]
                {
                    Ok(Box::new(crate::adapters::opentofu::OpenTofu::new(
                        config.clone(),
                    )?))
                }
                #[cfg(not(feature = "opentofu"))]
                {
                    let _ = config;
                    Err(crate::Error::Unsupported(
                        "OpenTofu feature disabled".into(),
                    ))
                }
            }
            Self::Kubernetes(config) => {
                #[cfg(feature = "kubernetes")]
                {
                    Ok(Box::new(crate::adapters::kubernetes::Kubernetes::connect(
                        config.clone(),
                    )?))
                }
                #[cfg(not(feature = "kubernetes"))]
                {
                    let _ = config;
                    Err(crate::Error::Unsupported(
                        "Kubernetes feature disabled".into(),
                    ))
                }
            }
        }
    }
}

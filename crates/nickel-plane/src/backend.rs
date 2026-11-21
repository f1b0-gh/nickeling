use crate::{
    model::{BackendKind, OutputSpec, Unit},
    Error, Result,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputValue {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    pub sensitive: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Readiness {
    Ready,
    NotReady,
    Failed,
    #[default]
    Unsupported,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Observation {
    pub exists: bool,
    pub drifted: bool,
    pub readiness: Readiness,
    pub outputs: BTreeMap<String, OutputValue>,
    pub inventory: Value,
    pub preconditions: Value,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChangeSummary {
    pub creates: usize,
    pub updates: usize,
    pub deletes: usize,
    pub replacements: usize,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub field_ownership_transfers: usize,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub recovery_bindings: usize,
    #[serde(default, skip_serializing_if = "is_false")]
    pub deferred_changes: bool,
}
fn is_false(value: &bool) -> bool {
    !*value
}
fn is_zero(value: &usize) -> bool {
    *value == 0
}
impl ChangeSummary {
    pub fn changed(&self) -> bool {
        self.creates
            + self.updates
            + self.deletes
            + self.replacements
            + self.field_ownership_transfers
            + self.recovery_bindings
            > 0
            || self.deferred_changes
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparedChange {
    pub operation: String,
    pub revision: String,
    pub artifact_sha256: String,
    pub target_sha256: String,
    pub summary: ChangeSummary,
    pub payload: Value,
    pub preconditions: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryBinding {
    pub source_operation: String,
    pub api_version: String,
    pub kind: String,
    pub namespace: String,
    pub name: String,
    pub uid: String,
}
impl PreparedChange {
    pub fn recovery_bindings(&self) -> Result<Vec<RecoveryBinding>> {
        if self.summary.recovery_bindings == 0 {
            return Ok(Vec::new());
        }
        crate::require(
            self.summary.recovery_bindings <= 256,
            "Too many recovery bindings",
        )?;
        let values = self.payload["recoveryBindings"]
            .as_array()
            .ok_or_else(|| Error::Invalid("Invalid recovery binding projection".into()))?;
        crate::require(
            values.len() == self.summary.recovery_bindings,
            "Recovery binding projection count differs",
        )?;
        let mut bindings = Vec::with_capacity(values.len());
        for value in values {
            crate::require(
                value.as_object().is_some_and(|fields| {
                    fields.len() == 6
                        && fields.values().all(|value| {
                            value
                                .as_str()
                                .is_some_and(|text| !text.is_empty() && text.len() <= 1024)
                        })
                }),
                "Invalid recovery identity projection",
            )?;
            bindings.push(serde_json::from_value::<RecoveryBinding>(value.clone())?);
        }
        for binding in &bindings {
            crate::require(
                [
                    &binding.source_operation,
                    &binding.api_version,
                    &binding.kind,
                    &binding.namespace,
                    &binding.name,
                    &binding.uid,
                ]
                .iter()
                .all(|value| !value.is_empty() && value.len() <= 1024),
                "Invalid recovery identity projection",
            )?;
        }
        Ok(bindings)
    }
}
#[derive(Clone, Debug)]
pub struct Context {
    pub owner: String,
    pub instance: String,
    pub unit_id: String,
    pub revision: String,
    pub unit: Unit,
    pub artifact: Value,
    pub artifact_sha256: String,
    pub inventory: Value,
    pub interrupted: Option<PreparedChange>,
    /// Bounded immutable recovery inputs; never serialized as public status.
    pub recovery: Option<Vec<RecoveryRecord>>,
    pub work: PathBuf,
    pub engine_version: Option<String>,
    pub provider_lock: Option<Vec<u8>>,
}
#[derive(Clone, Debug)]
pub struct RecoveryRecord {
    pub prepared: PreparedChange,
    pub checkpoint: Option<OperationCheckpoint>,
    pub target_reference: String,
    pub target_identity_sha256: String,
    pub superseded: bool,
}
/// Durable, bounded adapter progress; never part of an approved plan.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationCheckpoint {
    pub sequence: u64,
    pub outstanding_intents: usize,
    pub data: Value,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RecoveryReason {
    UnrecordedIdentity,
    UncertainCreate,
    StaleContinuation,
    ChangedAuthority,
    UnexpectedOwnership,
    InitializationPending,
    RemovalPending,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryDisposition {
    Legacy,
    /// Superseded issued intent remains accounted for without execution authority.
    Tracked,
    Complete,
    Resume,
    ReviewRequired(RecoveryReason),
}
/// Evidence commits are independent from permission to send the next mutation.
pub trait OperationJournal {
    fn checkpoint(&self) -> Option<&OperationCheckpoint>;
    fn guard(&self) -> Result<()>;
    fn commit(
        &mut self,
        progress: &Value,
        outstanding_intents: usize,
        owned_inventory: &Value,
        review: Option<RecoveryReason>,
    ) -> Result<()>;
}
pub trait Backend {
    fn kind(&self) -> BackendKind;
    /// Fingerprint the target and state binding, excluding credential values.
    fn fingerprint(&self) -> Result<String>;
    /// Stable target identity, independent of Kubernetes trust and authentication.
    fn target_identity(&self) -> Result<String> {
        self.fingerprint()
    }
    /// Bind a prepared operation to the connection configuration actually in use.
    fn execution_fingerprint(&self) -> Result<String> {
        self.fingerprint()
    }
    /// Historical proof used only during an explicitly requested binding upgrade.
    fn legacy_fingerprint(&self) -> Result<String> {
        self.fingerprint()
    }
    /// A bounded event hint. Errors trigger periodic resynchronization, never mutation.
    fn poll_events(&mut self, _inventory: &Value) -> Result<bool> {
        Ok(false)
    }
    fn state_binding(&self) -> Result<Option<String>> {
        Ok(None)
    }
    fn observe(&mut self, context: &Context) -> Result<Observation>;
    fn prepare_change(
        &mut self,
        context: &Context,
        observed: &Observation,
        operation: &str,
    ) -> Result<PreparedChange>;
    fn execute_change(&mut self, context: &Context, change: &PreparedChange)
        -> Result<Observation>;
    /// Read-only recovery saves evidence before optional ordinary observation.
    fn recover_change(
        &mut self,
        _context: &Context,
        _change: &PreparedChange,
        _journal: &mut dyn OperationJournal,
    ) -> Result<RecoveryDisposition> {
        Ok(RecoveryDisposition::Legacy)
    }
    fn execute_change_with_journal(
        &mut self,
        context: &Context,
        change: &PreparedChange,
        _journal: &mut dyn OperationJournal,
    ) -> Result<Observation> {
        self.execute_change(context, change)
    }
    /// Observe owned resources without requiring the removed configuration to be healthy.
    fn observe_removal(&mut self, context: &Context) -> Result<Observation> {
        self.observe(context)
    }
    fn prepare_removal(
        &mut self,
        _context: &Context,
        _observed: &Observation,
        _operation: &str,
    ) -> Result<PreparedChange> {
        Err(Error::Unsupported("backend deletion planning".into()))
    }
    fn execute_removal(
        &mut self,
        _context: &Context,
        _change: &PreparedChange,
    ) -> Result<Observation> {
        Err(Error::Unsupported("backend saved deletion".into()))
    }
    fn remove(&mut self, _context: &Context, _observed: &Observation) -> Result<Observation> {
        Err(Error::Unsupported("backend deletion".into()))
    }
}
pub fn select_outputs(
    root: &Value,
    specs: &BTreeMap<String, OutputSpec>,
    sensitive_paths: &[String],
    reference_prefix: &str,
) -> Result<BTreeMap<String, OutputValue>> {
    let mut outputs = BTreeMap::new();
    let mut size = 0usize;
    for (name, spec) in specs {
        let Some(value) = root.pointer(&spec.pointer) else {
            continue;
        };
        let parts = crate::model::pointer_parts(&spec.pointer)?;
        let sensitive = spec.sensitive
            || sensitive_paths.iter().any(|path| {
                crate::model::pointer_parts(path)
                    .is_ok_and(|s| s.starts_with(&parts) || parts.starts_with(&s))
            });
        if !sensitive {
            size = size.saturating_add(serde_json::to_vec(value)?.len());
            crate::require(size <= 1024 * 1024, "Selected outputs exceed 1 MiB")?;
        }
        outputs.insert(
            name.clone(),
            OutputValue {
                value: if sensitive { None } else { Some(value.clone()) },
                sensitive,
                reference: if sensitive {
                    Some(format!("{reference_prefix}/{name}"))
                } else {
                    None
                },
            },
        );
    }
    Ok(outputs)
}

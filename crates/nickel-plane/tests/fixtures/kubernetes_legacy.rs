//! Explicit historical POST-only approval fixtures. New production plans must
//! use the durable compound protocol; these tests retain legacy recovery coverage.
#![allow(dead_code)]
use nickel_plane::{
    adapters::kubernetes::{Api, Kubernetes},
    backend::*,
    model::BackendKind,
    Error, Result,
};
use serde_json::{json, Value};
pub fn prepare<A: Api>(
    k: &mut Kubernetes<A>,
    context: &Context,
    observed: &Observation,
    operation: &str,
) -> Result<PreparedChange> {
    let mut plan = k.prepare_change(context, observed, operation)?;
    if plan.payload["phase"] != "create-initialize-v1" {
        return Ok(plan);
    }
    let objects = plan.payload["objects"]
        .as_object()
        .ok_or_else(|| Error::Invalid("Missing creation descriptors".into()))?;
    let mut existing = context.clone();
    let items = context.artifact["items"]
        .as_array()
        .ok_or_else(|| Error::Invalid("Missing fixture objects".into()))?;
    existing.artifact["items"] = Value::Array(
        items
            .iter()
            .filter(|item| {
                !objects.values().any(|descriptor| {
                    [
                        "/apiVersion",
                        "/kind",
                        "/metadata/name",
                        "/metadata/namespace",
                    ]
                    .iter()
                    .all(|pointer| item.pointer(pointer) == descriptor["body"].pointer(pointer))
                })
            })
            .cloned()
            .collect(),
    );
    let rest = k.prepare_change(&existing, observed, operation)?;
    if rest.summary.field_ownership_transfers > 0 {
        return Ok(rest);
    }
    let owner = nickel_util::files::hash(
        format!("{}/{}/{}", context.owner, context.instance, context.unit_id).as_bytes(),
    );
    let mut actions = Vec::new();
    let mut creates = Vec::new();
    for (path, descriptor) in objects {
        let collection = path
            .rsplit_once('/')
            .ok_or_else(|| Error::Invalid("Invalid fixture creation path".into()))?
            .0;
        actions.push(json!({"method":"POST","path":format!("{collection}?fieldManager=nickel-plane-{}",&owner[..32]),"body":descriptor["body"]}));
        creates.push(path.clone());
    }
    actions.extend(
        rest.payload["actions"]
            .as_array()
            .ok_or_else(|| Error::Invalid("Missing legacy fixture actions".into()))?
            .iter()
            .cloned(),
    );
    let creates_count = plan.summary.creates;
    let replacements = plan.summary.replacements;
    plan.summary = rest.summary;
    plan.summary.creates = creates_count;
    plan.summary.replacements = replacements;
    plan.summary.deferred_changes = false;
    plan.payload = json!({"actions":actions,"creates":creates});
    Ok(plan)
}
/// A historical adapter fixture deliberately retaining default Legacy recovery.
pub struct BackendFixture<A: Api>(pub Kubernetes<A>);
impl<A: Api> Backend for BackendFixture<A> {
    fn kind(&self) -> BackendKind {
        self.0.kind()
    }
    fn fingerprint(&self) -> Result<String> {
        self.0.fingerprint()
    }
    fn target_identity(&self) -> Result<String> {
        self.0.target_identity()
    }
    fn execution_fingerprint(&self) -> Result<String> {
        self.0.execution_fingerprint()
    }
    fn legacy_fingerprint(&self) -> Result<String> {
        self.0.legacy_fingerprint()
    }
    fn observe(&mut self, context: &Context) -> Result<Observation> {
        self.0.observe(context)
    }
    fn prepare_change(
        &mut self,
        context: &Context,
        observed: &Observation,
        operation: &str,
    ) -> Result<PreparedChange> {
        prepare(&mut self.0, context, observed, operation)
    }
    fn execute_change(
        &mut self,
        context: &Context,
        change: &PreparedChange,
    ) -> Result<Observation> {
        self.0.execute_change(context, change)
    }
    fn observe_removal(&mut self, context: &Context) -> Result<Observation> {
        self.0.observe_removal(context)
    }
    fn prepare_removal(
        &mut self,
        context: &Context,
        observed: &Observation,
        operation: &str,
    ) -> Result<PreparedChange> {
        self.0.prepare_removal(context, observed, operation)
    }
    fn execute_removal(
        &mut self,
        context: &Context,
        change: &PreparedChange,
    ) -> Result<Observation> {
        self.0.execute_removal(context, change)
    }
}

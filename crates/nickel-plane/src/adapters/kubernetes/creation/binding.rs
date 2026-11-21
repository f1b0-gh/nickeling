//! Explicit, zero-request-mutation ownership binding for an unrecorded UID.
use super::*;
pub(super) const PHASE: &str = "recovery-bind-v1";
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BindingPlan {
    phase: String,
    source_operation: String,
    source_sequence: u64,
    bindings: BTreeMap<String, Candidate>,
    recovery_bindings: Vec<RecoveryBinding>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Candidate {
    uid: String,
    resource_version: String,
    proof: String,
}
struct Snapshot<'a>(Option<&'a OperationCheckpoint>);
impl OperationJournal for Snapshot<'_> {
    fn checkpoint(&self) -> Option<&OperationCheckpoint> {
        self.0
    }
    fn guard(&self) -> Result<()> {
        Err(Error::Unsupported("Read-only recovery snapshot".into()))
    }
    fn commit(&mut self, _: &Value, _: usize, _: &Value, _: Option<RecoveryReason>) -> Result<()> {
        self.guard()
    }
}
fn source<'a>(context: &'a Context, id: &str) -> Result<&'a RecoveryRecord> {
    context
        .recovery
        .as_ref()
        .and_then(|records| {
            records
                .iter()
                .find(|record| record.prepared.operation == id)
        })
        .ok_or_else(|| Error::Conflict("Recovery source is unavailable".into()))
}
pub(super) fn prepare<A: Api>(
    k: &Kubernetes<A>,
    context: &Context,
    operation: &str,
) -> Result<Option<PreparedChange>> {
    let Some(records) = &context.recovery else {
        return Ok(None);
    };
    require(records.len() <= 17, "Too many recovery sources")?;
    for source in records {
        if source.prepared.payload["phase"] != CREATE {
            continue;
        }
        require(
            source.target_identity_sha256 == k.target_identity()?,
            "Recovery source target changed",
        )?;
        let (creation, progress) = decode(
            context,
            &source.prepared,
            &k.target.namespace,
            &Snapshot(source.checkpoint.as_ref()),
        )?;
        let mut bindings = BTreeMap::new();
        let mut public = Vec::new();
        for (path, step) in &progress.steps {
            if let Step::Candidate {
                uid,
                resource_version,
                proof,
            } = step
            {
                let descriptor = &creation.objects[path];
                bindings.insert(
                    path.clone(),
                    Candidate {
                        uid: uid.clone(),
                        resource_version: resource_version.clone(),
                        proof: proof.clone(),
                    },
                );
                public.push(RecoveryBinding {
                    source_operation: source.prepared.operation.clone(),
                    api_version: text(&descriptor.body, "/apiVersion")?.into(),
                    kind: text(&descriptor.body, "/kind")?.into(),
                    namespace: text(&descriptor.body, "/metadata/namespace")?.into(),
                    name: text(&descriptor.body, "/metadata/name")?.into(),
                    uid: uid.clone(),
                });
            }
        }
        if bindings.is_empty() {
            continue;
        }
        let summary = ChangeSummary {
            recovery_bindings: bindings.len(),
            replacements: bindings
                .keys()
                .filter(|path| creation.objects[*path].predecessor_uid.is_some())
                .count(),
            deferred_changes: source.prepared.summary.deferred_changes,
            ..Default::default()
        };
        let payload = BindingPlan {
            phase: PHASE.into(),
            source_operation: source.prepared.operation.clone(),
            source_sequence: source
                .checkpoint
                .as_ref()
                .map_or(0, |checkpoint| checkpoint.sequence),
            bindings,
            recovery_bindings: public,
        };
        bounded_json(&payload, PROGRESS_LIMIT)?;
        let plan = PreparedChange {
            operation: operation.into(),
            revision: context.revision.clone(),
            artifact_sha256: context.artifact_sha256.clone(),
            target_sha256: k.execution_fingerprint()?,
            summary,
            payload: serde_json::to_value(payload)?,
            preconditions: Value::Null,
        };
        plan.recovery_bindings()?;
        return Ok(Some(plan));
    }
    Ok(None)
}
pub(super) fn execute<A: Api>(
    k: &mut Kubernetes<A>,
    context: &Context,
    change: &PreparedChange,
    journal: &mut dyn OperationJournal,
) -> Result<Observation> {
    authority(k, context, change, journal)?;
    bounded_json(&change.payload, PROGRESS_LIMIT)?;
    let plan: BindingPlan = serde_json::from_value(change.payload.clone())?;
    require(
        plan.phase == PHASE && !plan.bindings.is_empty() && plan.bindings.len() <= 256,
        "Invalid identity-binding plan",
    )?;
    change.recovery_bindings()?;
    let source = source(context, &plan.source_operation)?;
    require(
        source.target_identity_sha256 == k.target_identity()?
            && source
                .checkpoint
                .as_ref()
                .map(|checkpoint| checkpoint.sequence)
                == Some(plan.source_sequence),
        "Identity-binding source changed",
    )?;
    require(
        journal.checkpoint() == source.checkpoint.as_ref(),
        "Identity-binding journal source differs",
    )?;
    let (creation, mut progress) = decode(context, &source.prepared, &k.target.namespace, journal)?;
    let mut entries = inventory(&context.inventory)?;
    let mut public = Vec::new();
    for (path, candidate) in &plan.bindings {
        let descriptor = creation
            .objects
            .get(path)
            .ok_or_else(|| Error::Invalid("Binding path was not approved for creation".into()))?;
        require(
            matches!(progress.steps.get(path),Some(Step::Candidate {uid,resource_version,proof})
            if uid==&candidate.uid && resource_version==&candidate.resource_version && proof==&candidate.proof),
            "Binding candidate changed",
        )?;
        let object = k
            .get(path)?
            .ok_or_else(|| Error::Conflict("Binding candidate disappeared".into()))?;
        let (uid, rv) = identity(context, &source.prepared, descriptor, &object)?;
        require(
            uid == candidate.uid
                && rv == candidate.resource_version
                && image(&object)? == candidate.proof
                && own_fields(&object, &manager(context))? == descriptor.own_fields,
            "Binding candidate changed after review",
        )?;
        public.push(RecoveryBinding {
            source_operation: source.prepared.operation.clone(),
            api_version: text(&descriptor.body, "/apiVersion")?.into(),
            kind: text(&descriptor.body, "/kind")?.into(),
            namespace: text(&descriptor.body, "/metadata/namespace")?.into(),
            name: text(&descriptor.body, "/metadata/name")?.into(),
            uid: uid.clone(),
        });
        record_created(
            &mut entries,
            path,
            descriptor,
            &uid,
            &rv,
            source.prepared.summary.deferred_changes,
        )?;
        if let Some(entry) = entries.get_mut(path) {
            entry.initialization_artifact_sha256 = Some(source.prepared.artifact_sha256.clone());
        }
        progress.steps.insert(
            path.clone(),
            Step::Created {
                uid,
                resource_version: rv,
            },
        );
    }
    require(
        public == plan.recovery_bindings,
        "Public binding identities differ from approval",
    )?;
    authority(k, context, change, journal)?;
    save(journal, &progress, &entries, None)?;
    Ok(Observation {
        inventory: serde_json::to_value(entries)?,
        drifted: true,
        readiness: Readiness::NotReady,
        ..Default::default()
    })
}

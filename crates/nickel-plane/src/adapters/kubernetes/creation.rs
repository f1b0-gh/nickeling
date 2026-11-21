//! Create-only initialization with durable identity and concrete patch checkpoints.
use super::*;
mod binding;
mod initialization;
use crate::store::operations::bounded_json;
const CREATE: &str = "create-initialize-v1";
const PROGRESS_LIMIT: usize = 4 * 1024 * 1024;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Descriptor {
    body: Value,
    own_fields: Value,
    predecessor_uid: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Creation {
    phase: String,
    objects: BTreeMap<String, Descriptor>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(
    tag = "stage",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum Step {
    NotStarted,
    PostIssued,
    Candidate {
        uid: String,
        resource_version: String,
        proof: String,
    },
    Created {
        uid: String,
        resource_version: String,
    },
    PatchPrepared {
        uid: String,
        resource_version: String,
        patch: Value,
        before: String,
        after: String,
    },
    Complete {
        uid: String,
        proof: String,
    },
    Rejected,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Progress {
    version: u32,
    steps: BTreeMap<String, Step>,
}
impl Progress {
    fn outstanding(&self) -> usize {
        self.steps
            .values()
            .filter(|step| matches!(step, Step::PostIssued | Step::Candidate { .. }))
            .count()
    }
    fn complete(&self) -> bool {
        self.steps
            .values()
            .all(|step| matches!(step, Step::Complete { .. }))
    }
}
pub(super) fn handles(change: &PreparedChange) -> bool {
    change.payload["phase"] == CREATE
        || change.payload["phase"] == binding::PHASE
        || change.payload["phase"] == initialization::PHASE
}
fn bounded_id(object: &Value, pointer: &str) -> Result<String> {
    let value = text(object, pointer)?;
    require(
        !value.is_empty() && value.len() <= 1024 && !value.chars().any(char::is_control),
        "Kubernetes identity exceeds its byte budget",
    )?;
    Ok(value.into())
}
fn own_fields(object: &Value, manager: &str) -> Result<Value> {
    let fields = object["metadata"]["managedFields"]
        .as_array()
        .ok_or_else(|| Error::Backend("Missing creation managed fields".into()))?;
    let mut own = fields.iter().filter(|entry| {
        entry["manager"] == manager && entry["subresource"].as_str().unwrap_or_default().is_empty()
    });
    let field = own
        .next()
        .ok_or_else(|| Error::Backend("Missing creation field owner".into()))?;
    require(
        own.next().is_none()
            && field["operation"] == "Update"
            && field["apiVersion"] == object["apiVersion"]
            && field["fieldsType"] == "FieldsV1"
            && field["fieldsV1"].is_object(),
        "Unexpected creation field ownership",
    )?;
    bounded_json(field, 256 * 1024)?;
    let mut field = field.clone();
    field
        .as_object_mut()
        .ok_or_else(|| Error::Backend("Invalid creation field owner".into()))?
        .remove("time");
    Ok(field)
}
fn image(object: &Value) -> Result<String> {
    bounded_json(object, 16 * 1024 * 1024)?;
    let mut value = object.clone();
    let metadata = value["metadata"]
        .as_object_mut()
        .ok_or_else(|| Error::Backend("Invalid creation metadata".into()))?;
    metadata.remove("resourceVersion");
    if let Some(fields) = metadata
        .get_mut("managedFields")
        .and_then(Value::as_array_mut)
    {
        let mut encoded: Vec<String> = fields
            .iter()
            .map(|field| bounded_json(field, 256 * 1024))
            .collect::<Result<_>>()?;
        encoded.sort();
        *fields = encoded.into_iter().map(Value::String).collect();
    }
    Ok(files::hash(
        bounded_json(&value, 16 * 1024 * 1024)?.as_bytes(),
    ))
}
fn identity(
    context: &Context,
    change: &PreparedChange,
    descriptor: &Descriptor,
    object: &Value,
) -> Result<(String, String)> {
    for pointer in [
        "/apiVersion",
        "/kind",
        "/metadata/name",
        "/metadata/namespace",
    ] {
        require(
            object.pointer(pointer) == descriptor.body.pointer(pointer),
            "Creation response identity differs from approved object",
        )?;
    }
    require(
        object["metadata"]["labels"]["nickel-plane.io/owner"] == ownership(context)
            && object["metadata"]["annotations"]["nickel-plane.io/operation"] == change.operation,
        "Creation response ownership differs",
    )?;
    Ok((
        bounded_id(object, "/metadata/uid")?,
        bounded_id(object, "/metadata/resourceVersion")?,
    ))
}
fn decode(
    context: &Context,
    change: &PreparedChange,
    namespace: &str,
    journal: &dyn OperationJournal,
) -> Result<(Creation, Progress)> {
    bounded_json(&change.payload, 16 * 1024 * 1024)?;
    let payload: Creation = serde_json::from_value(change.payload.clone())?;
    require(
        payload.phase == CREATE && !payload.objects.is_empty() && payload.objects.len() <= 256,
        "Invalid creation initializer",
    )?;
    for (path, descriptor) in &payload.objects {
        let (base, plural) = prefix(
            text(&descriptor.body, "/apiVersion")?,
            text(&descriptor.body, "/kind")?,
        )?;
        let name = text(&descriptor.body, "/metadata/name")?;
        validate_name(name)?;
        require(
            text(&descriptor.body, "/metadata/namespace")? == namespace
                && *path == format!("{base}/namespaces/{namespace}/{plural}/{name}"),
            "Creation initializer is outside target",
        )?;
        require(
            descriptor.body["metadata"]["labels"]["nickel-plane.io/owner"] == ownership(context)
                && descriptor.body["metadata"]["annotations"]["nickel-plane.io/operation"]
                    == change.operation,
            "Creation initializer ownership differs",
        )?;
        bounded_json(&descriptor.own_fields, 256 * 1024)?;
    }
    let progress = if let Some(checkpoint) = journal.checkpoint() {
        bounded_json(&checkpoint.data, PROGRESS_LIMIT)?;
        let progress: Progress = serde_json::from_value(checkpoint.data.clone())?;
        require(
            checkpoint.outstanding_intents == progress.outstanding(),
            "Creation intent accounting differs",
        )?;
        progress
    } else {
        Progress {
            version: 1,
            steps: payload
                .objects
                .keys()
                .map(|path| (path.clone(), Step::NotStarted))
                .collect(),
        }
    };
    require(
        progress.version == 1 && progress.steps.keys().eq(payload.objects.keys()),
        "Creation progress does not match approval",
    )?;
    require(
        progress
            .steps
            .values()
            .filter(|step| matches!(step, Step::PatchPrepared { .. }))
            .count()
            <= 1,
        "Multiple active creation continuations",
    )?;
    Ok((payload, progress))
}
fn save(
    journal: &mut dyn OperationJournal,
    progress: &Progress,
    entries: &BTreeMap<String, Entry>,
    review: Option<RecoveryReason>,
) -> Result<()> {
    bounded_json(progress, PROGRESS_LIMIT)?;
    bounded_json(entries, PROGRESS_LIMIT)?;
    journal.commit(
        &serde_json::to_value(progress)?,
        progress.outstanding(),
        &serde_json::to_value(entries)?,
        review,
    )
}
fn record_created(
    entries: &mut BTreeMap<String, Entry>,
    path: &str,
    descriptor: &Descriptor,
    uid: &str,
    rv: &str,
    deferred: bool,
) -> Result<()> {
    if let Some(old) = entries.get(path) {
        require(
            old.uid == uid
                || (old.missing && descriptor.predecessor_uid.as_deref() == Some(&old.uid)),
            "Creation conflicts with retained UID",
        )?;
    } else {
        require(
            descriptor.predecessor_uid.is_none(),
            "Creation predecessor evidence is missing",
        )?;
    }
    entries.insert(
        path.into(),
        Entry {
            path: path.into(),
            uid: uid.into(),
            resource_version: rv.into(),
            kind: text(&descriptor.body, "/kind")?.into(),
            name: text(&descriptor.body, "/metadata/name")?.into(),
            missing: false,
            pending_apply: false,
            pending_initialization: true,
            deferred_changes: deferred,
            initialization_artifact_sha256: None,
            creation_operation: Some(
                text(
                    &descriptor.body,
                    "/metadata/annotations/nickel-plane.io~1operation",
                )?
                .into(),
            ),
            creation_fields_sha256: Some(files::hash(
                bounded_json(&descriptor.own_fields, 256 * 1024)?.as_bytes(),
            )),
        },
    );
    Ok(())
}
pub(super) fn prepare<A: Api>(
    k: &mut Kubernetes<A>,
    context: &Context,
    observed: &Observation,
    operation: &str,
) -> Result<Option<PreparedChange>> {
    if let Some(plan) = binding::prepare(k, context, operation)? {
        return Ok(Some(plan));
    }
    if let Some(plan) = initialization::prepare(k, context, observed, operation)? {
        return Ok(Some(plan));
    }
    let wanted = desired(context, &k.target.namespace)?;
    let present = inventory(&observed.inventory)?;
    let old = inventory(&context.inventory)?;
    let absent: Vec<_> = wanted
        .iter()
        .filter(|(path, _)| present.get(*path).is_none_or(|entry| entry.missing))
        .collect();
    if absent.is_empty() {
        return Ok(None);
    }
    let mut objects = BTreeMap::new();
    let mut descriptor_bytes = 0usize;
    let mut summary = ChangeSummary {
        deferred_changes: wanted
            .keys()
            .any(|path| present.get(path).is_some_and(|entry| !entry.missing))
            || present
                .iter()
                .any(|(path, entry)| !entry.missing && !wanted.contains_key(path)),
        ..Default::default()
    };
    for (path, body) in absent {
        let mut body = body.clone();
        body["metadata"]["annotations"]["nickel-plane.io/operation"] = json!(operation);
        let collection = path
            .rsplit_once('/')
            .ok_or_else(|| Error::Invalid("Invalid creation path".into()))?
            .0;
        k.api.validate_target(&k.target)?;
        let (status, result) = k.api.request(
            "POST",
            &format!("{collection}?fieldManager={}&dryRun=All", manager(context)),
            Some(&body),
        )?;
        require(status == 201, "Kubernetes creation dry-run rejected")?;
        require(
            [
                "/apiVersion",
                "/kind",
                "/metadata/name",
                "/metadata/namespace",
            ]
            .iter()
            .all(|pointer| result.pointer(pointer) == body.pointer(pointer))
                && result["metadata"]["labels"]["nickel-plane.io/owner"] == ownership(context)
                && result["metadata"]["annotations"]["nickel-plane.io/operation"] == operation,
            "Creation dry-run identity differs",
        )?;
        let fields = own_fields(&result, &manager(context))?;
        descriptor_bytes =
            descriptor_bytes.saturating_add(bounded_json(&fields, 256 * 1024)?.len());
        require(
            descriptor_bytes <= PROGRESS_LIMIT,
            "Creation descriptors exceed 4 MiB",
        )?;
        let predecessor_uid = old.get(path).map(|entry| entry.uid.clone());
        if let Some(entry) = old.get(path) {
            require(
                present
                    .get(path)
                    .is_some_and(|observed| observed.missing && observed.uid == entry.uid),
                "Creation predecessor is not missing",
            )?;
        }
        summary.replacements += usize::from(predecessor_uid.is_some());
        objects.insert(
            path.clone(),
            Descriptor {
                body,
                own_fields: fields,
                predecessor_uid,
            },
        );
    }
    summary.creates = objects.len();
    summary.field_ownership_transfers = objects.len();
    // A compact per-path record reserves UID/RV/proof space, plus one largest patch.
    let projected = objects
        .len()
        .saturating_mul(8192)
        .saturating_add(descriptor_bytes.min(256 * 1024))
        .saturating_add(64 * 1024);
    require(
        projected <= PROGRESS_LIMIT,
        "Creation progress exceeds 4 MiB",
    )?;
    let payload = Creation {
        phase: CREATE.into(),
        objects,
    };
    bounded_json(&payload, 16 * 1024 * 1024)?;
    Ok(Some(PreparedChange {
        operation: operation.into(),
        revision: context.revision.clone(),
        artifact_sha256: context.artifact_sha256.clone(),
        target_sha256: k.execution_fingerprint()?,
        summary,
        payload: serde_json::to_value(payload)?,
        preconditions: observed.preconditions.clone(),
    }))
}
fn authority<A: Api>(
    k: &Kubernetes<A>,
    context: &Context,
    change: &PreparedChange,
    journal: &dyn OperationJournal,
) -> Result<()> {
    journal.guard()?;
    require(
        change.revision == context.revision
            && change.artifact_sha256 == context.artifact_sha256
            && change.target_sha256 == k.execution_fingerprint()?,
        "Creation execution authority changed",
    )
}
pub(super) fn execute<A: Api>(
    k: &mut Kubernetes<A>,
    context: &Context,
    change: &PreparedChange,
    journal: &mut dyn OperationJournal,
) -> Result<Observation> {
    if change.payload["phase"] == binding::PHASE {
        return binding::execute(k, context, change, journal);
    }
    if change.payload["phase"] == initialization::PHASE {
        return initialization::execute(k, context, change, journal);
    }
    let (payload, mut progress) = decode(context, change, &k.target.namespace, journal)?;
    authority(k, context, change, journal)?;
    let mut entries = inventory(&context.inventory)?;
    if progress
        .steps
        .values()
        .all(|step| matches!(step, Step::NotStarted))
    {
        let observed = k.observe(context)?;
        require(
            observed.preconditions == change.preconditions,
            "Creation preconditions are stale",
        )?;
        entries = inventory(&observed.inventory)?;
    }
    for (path, descriptor) in &payload.objects {
        loop {
            let step = progress
                .steps
                .get(path)
                .cloned()
                .ok_or_else(|| Error::Invalid("Missing creation progress".into()))?;
            match step {
                Step::NotStarted => {
                    require(
                        k.get(path)?.is_none(),
                        "Refusing to create an existing object",
                    )?;
                    authority(k, context, change, journal)?;
                    progress.steps.insert(path.clone(), Step::PostIssued);
                    save(journal, &progress, &entries, None)?;
                    authority(k, context, change, journal)?;
                    let collection = path
                        .rsplit_once('/')
                        .ok_or_else(|| Error::Invalid("Invalid creation path".into()))?
                        .0;
                    let result = k.api.request(
                        "POST",
                        &format!("{collection}?fieldManager={}", manager(context)),
                        Some(&descriptor.body),
                    );
                    let (status, object) = result?;
                    if status != 201 {
                        if matches!(status, 400 | 401 | 403 | 404 | 405 | 409 | 415 | 422) {
                            progress.steps.insert(path.clone(), Step::Rejected);
                            save(
                                journal,
                                &progress,
                                &entries,
                                Some(RecoveryReason::UnexpectedOwnership),
                            )?;
                        }
                        return Err(Error::Backend(
                            "Kubernetes creation was not confirmed".into(),
                        ));
                    }
                    // Identity validation uses only this response. Do not recheck
                    // execution credentials or cancellation before saving its UID.
                    let (uid, rv) = identity(context, change, descriptor, &object)?;
                    record_created(
                        &mut entries,
                        path,
                        descriptor,
                        &uid,
                        &rv,
                        change.summary.deferred_changes,
                    )?;
                    if let Some(entry) = entries.get_mut(path) {
                        entry.initialization_artifact_sha256 = Some(change.artifact_sha256.clone());
                    }
                    progress.steps.insert(
                        path.clone(),
                        Step::Created {
                            uid,
                            resource_version: rv,
                        },
                    );
                    save(journal, &progress, &entries, None)?;
                    if own_fields(&object, &manager(context))? != descriptor.own_fields {
                        save(
                            journal,
                            &progress,
                            &entries,
                            Some(RecoveryReason::UnexpectedOwnership),
                        )?;
                        return Err(Error::Conflict(
                            "Creation ownership differs from reviewed dry-run".into(),
                        ));
                    }
                }
                Step::Created { uid, .. } => {
                    let object = k
                        .get(path)?
                        .ok_or_else(|| Error::Conflict("Created object disappeared".into()))?;
                    let (actual, rv) = identity(context, change, descriptor, &object)?;
                    require(actual == uid, "Created object UID changed")?;
                    require(
                        own_fields(&object, &manager(context))? == descriptor.own_fields,
                        "Creation field ownership changed",
                    )?;
                    let patch =
                        ownership_transfer(&object, &manager(context))?.ok_or_else(|| {
                            Error::Conflict("Creation ownership cannot be initialized".into())
                        })?;
                    authority(k, context, change, journal)?;
                    let route = format!("{path}?fieldManager={}", manager(context));
                    let result = k.mutate("PATCH", &format!("{route}&dryRun=All"), &patch)?;
                    verify_transfer(&object, &result, &patch)?;
                    progress.steps.insert(
                        path.clone(),
                        Step::PatchPrepared {
                            uid,
                            resource_version: rv,
                            patch,
                            before: image(&object)?,
                            after: image(&result)?,
                        },
                    );
                    save(journal, &progress, &entries, None)?;
                }
                Step::PatchPrepared {
                    uid,
                    resource_version,
                    patch,
                    before,
                    after,
                } => {
                    let object = k
                        .get(path)?
                        .ok_or_else(|| Error::Conflict("Initializer object disappeared".into()))?;
                    require(
                        bounded_id(&object, "/metadata/uid")? == uid,
                        "Initializer UID changed",
                    )?;
                    let current = image(&object)?;
                    if current != after {
                        require(
                            current == before
                                && bounded_id(&object, "/metadata/resourceVersion")?
                                    == resource_version
                                && ownership_transfer(&object, &manager(context))?.as_ref()
                                    == Some(&patch),
                            "Concrete initializer is stale; prepare a fresh plan",
                        )?;
                        authority(k, context, change, journal)?;
                        let route = format!("{path}?fieldManager={}", manager(context));
                        let dry = k.mutate("PATCH", &format!("{route}&dryRun=All"), &patch)?;
                        require(
                            image(&dry)? == after,
                            "Concrete initializer dry-run changed",
                        )?;
                        authority(k, context, change, journal)?;
                        let result = k.mutate("PATCH", &route, &patch)?;
                        require(
                            bounded_id(&result, "/metadata/uid")? == uid
                                && image(&result)? == after,
                            "Initializer response differs from reviewed continuation",
                        )?;
                        if let Some(entry) = entries.get_mut(path) {
                            entry.resource_version =
                                bounded_id(&result, "/metadata/resourceVersion")?;
                        }
                    }
                    let entry = entries
                        .get_mut(path)
                        .ok_or_else(|| Error::Conflict("Initializer lacks retained UID".into()))?;
                    require(entry.uid == uid, "Initializer conflicts with retained UID")?;
                    entry.pending_initialization = false;
                    entry.deferred_changes = false;
                    entry.initialization_artifact_sha256 = None;
                    entry.creation_operation = None;
                    entry.creation_fields_sha256 = None;
                    progress
                        .steps
                        .insert(path.clone(), Step::Complete { uid, proof: after });
                    save(journal, &progress, &entries, None)?;
                }
                Step::Complete { .. } => break,
                Step::PostIssued | Step::Candidate { .. } | Step::Rejected => {
                    return Err(Error::Conflict(
                        "Creation requires a fresh recovery approval".into(),
                    ))
                }
            }
        }
    }
    let mut after = context.clone();
    after.inventory = serde_json::to_value(entries)?;
    after.interrupted = None;
    k.observe(&after)
}
pub(super) fn recover<A: Api>(
    k: &mut Kubernetes<A>,
    context: &Context,
    change: &PreparedChange,
    journal: &mut dyn OperationJournal,
) -> Result<RecoveryDisposition> {
    if change.payload["phase"] == initialization::PHASE {
        return initialization::recover(k, context, change, journal);
    }
    let (payload, mut progress) = decode(context, change, &k.target.namespace, journal)?;
    let mut entries = inventory(&context.inventory)?;
    let mut review = None;
    let superseded = context.recovery.as_ref().is_some_and(|records| {
        records
            .iter()
            .any(|source| source.prepared.operation == change.operation && source.superseded)
    });
    for (path, descriptor) in &payload.objects {
        let step = progress
            .steps
            .get(path)
            .cloned()
            .ok_or_else(|| Error::Invalid("Missing creation progress".into()))?;
        if superseded && !matches!(step, Step::PostIssued | Step::Candidate { .. }) {
            continue;
        }
        let reason = match step {
            Step::NotStarted => {
                if k.get(path)?.is_some() {
                    Some(RecoveryReason::UnexpectedOwnership)
                } else {
                    None
                }
            }
            Step::PostIssued | Step::Candidate { .. } => {
                if let Some(object) = k.get(path)? {
                    let candidate = identity(context, change, descriptor, &object);
                    let fields = own_fields(&object, &manager(context));
                    if let (Ok((uid, rv)), Ok(fields)) = (candidate, fields) {
                        let predecessor = match (&descriptor.predecessor_uid, entries.get(path)) {
                            (None, None) => true,
                            (Some(old), Some(entry)) => {
                                entry.missing && entry.uid == *old && uid != *old
                            }
                            _ => false,
                        };
                        if fields == descriptor.own_fields && predecessor {
                            progress.steps.insert(
                                path.clone(),
                                Step::Candidate {
                                    uid,
                                    resource_version: rv,
                                    proof: image(&object)?,
                                },
                            );
                            Some(RecoveryReason::UnrecordedIdentity)
                        } else {
                            Some(RecoveryReason::UnexpectedOwnership)
                        }
                    } else {
                        Some(RecoveryReason::UnexpectedOwnership)
                    }
                } else {
                    progress.steps.insert(path.clone(), Step::PostIssued);
                    Some(RecoveryReason::UncertainCreate)
                }
            }
            Step::Created { uid, .. } => match k.get(path)? {
                Some(object)
                    if identity(context, change, descriptor, &object)
                        .is_ok_and(|(actual, _)| actual == uid)
                        && own_fields(&object, &manager(context))
                            .is_ok_and(|fields| fields == descriptor.own_fields) =>
                {
                    None
                }
                _ => Some(RecoveryReason::UnexpectedOwnership),
            },
            Step::PatchPrepared {
                uid,
                resource_version,
                before,
                after,
                ..
            } => {
                if let Some(object) = k.get(path)? {
                    let actual = bounded_id(&object, "/metadata/uid")?;
                    if actual != uid {
                        Some(RecoveryReason::UnexpectedOwnership)
                    } else if image(&object)? == after {
                        let entry = entries.get_mut(path).ok_or_else(|| {
                            Error::Conflict("Initializer lacks retained UID".into())
                        })?;
                        require(entry.uid == uid, "Initializer conflicts with retained UID")?;
                        entry.resource_version = bounded_id(&object, "/metadata/resourceVersion")?;
                        entry.pending_initialization = false;
                        entry.deferred_changes = false;
                        entry.initialization_artifact_sha256 = None;
                        entry.creation_operation = None;
                        entry.creation_fields_sha256 = None;
                        progress
                            .steps
                            .insert(path.clone(), Step::Complete { uid, proof: after });
                        None
                    } else if image(&object)? == before
                        && bounded_id(&object, "/metadata/resourceVersion")? == resource_version
                    {
                        None
                    } else {
                        Some(RecoveryReason::StaleContinuation)
                    }
                } else {
                    Some(RecoveryReason::StaleContinuation)
                }
            }
            Step::Complete { uid, .. } => {
                require(
                    entries.get(path).is_some_and(|entry| entry.uid == uid),
                    "Completed initializer lacks retained UID",
                )?;
                None
            }
            Step::Rejected => Some(RecoveryReason::UnexpectedOwnership),
        };
        // Eligible candidates take precedence over another path's refusal. Use the
        // whole saved map so unchanged rereads do not alternate public reasons
        // and invalidate an otherwise exact binding approval.
        let reason = if progress
            .steps
            .values()
            .any(|step| matches!(step, Step::Candidate { .. }))
        {
            Some(RecoveryReason::UnrecordedIdentity)
        } else {
            reason
        };
        if let Some(reason) =
            reason.filter(|reason| !superseded || *reason == RecoveryReason::UnrecordedIdentity)
        {
            if review != Some(RecoveryReason::UnrecordedIdentity) {
                review = Some(reason);
            }
        }
        save(journal, &progress, &entries, reason)?;
    }
    if let Some(reason) = review {
        Ok(RecoveryDisposition::ReviewRequired(reason))
    } else if superseded {
        Ok(RecoveryDisposition::Tracked)
    } else if progress.complete() {
        Ok(RecoveryDisposition::Complete)
    } else {
        Ok(RecoveryDisposition::Resume)
    }
}

//! Freshly reviewed, concrete metadata initialization for an already bound UID.
use super::*;
pub(super) const PHASE: &str = "initialize-known-v1";
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Initializer {
    uid: String,
    resource_version: String,
    patch: Value,
    before: String,
    after: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Plan {
    phase: String,
    objects: BTreeMap<String, Initializer>,
}
pub(super) fn prepare<A: Api>(
    k: &mut Kubernetes<A>,
    context: &Context,
    observed: &Observation,
    operation: &str,
) -> Result<Option<PreparedChange>> {
    let wanted = desired(context, &k.target.namespace)?;
    let present = inventory(&observed.inventory)?;
    let mut objects = BTreeMap::new();
    let mut deferred = false;
    for (path, entry) in &present {
        if !wanted.contains_key(path) || !entry.pending_initialization || entry.missing {
            continue;
        }
        let object = k
            .get(path)?
            .ok_or_else(|| Error::Conflict("Bound initializer object disappeared".into()))?;
        k.owned(context, path, &object, &present)?;
        require(
            bounded_id(&object, "/metadata/resourceVersion")? == entry.resource_version
                && entry.creation_operation.as_deref()
                    == object["metadata"]["annotations"]["nickel-plane.io/operation"].as_str(),
            "Bound initializer snapshot changed",
        )?;
        let fields = own_fields(&object, &manager(context))?;
        require(
            entry.creation_fields_sha256.as_deref()
                == Some(files::hash(bounded_json(&fields, 256 * 1024)?.as_bytes()).as_str()),
            "Bound initializer field ownership differs from the approved creation descriptor",
        )?;
        let patch = ownership_transfer(&object, &manager(context))?
            .ok_or_else(|| Error::Conflict("Bound object has no initializer ownership".into()))?;
        let result = k.mutate(
            "PATCH",
            &format!("{path}?fieldManager={}&dryRun=All", manager(context)),
            &patch,
        )?;
        verify_transfer(&object, &result, &patch)?;
        objects.insert(
            path.clone(),
            Initializer {
                uid: entry.uid.clone(),
                resource_version: entry.resource_version.clone(),
                patch,
                before: image(&object)?,
                after: image(&result)?,
            },
        );
        deferred |= entry.deferred_changes
            || entry.initialization_artifact_sha256.as_deref() != Some(&context.artifact_sha256);
    }
    if objects.is_empty() {
        return Ok(None);
    }
    deferred |= wanted.keys().any(|path| !objects.contains_key(path))
        || present.keys().any(|path| !wanted.contains_key(path));
    let summary = ChangeSummary {
        field_ownership_transfers: objects.len(),
        deferred_changes: deferred,
        ..Default::default()
    };
    let plan = Plan {
        phase: PHASE.into(),
        objects,
    };
    bounded_json(&plan, PROGRESS_LIMIT)?;
    Ok(Some(PreparedChange {
        operation: operation.into(),
        revision: context.revision.clone(),
        artifact_sha256: context.artifact_sha256.clone(),
        target_sha256: k.execution_fingerprint()?,
        summary,
        payload: serde_json::to_value(plan)?,
        preconditions: observed.preconditions.clone(),
    }))
}
fn decode(change: &PreparedChange, journal: &dyn OperationJournal) -> Result<(Plan, Progress)> {
    bounded_json(&change.payload, PROGRESS_LIMIT)?;
    let plan: Plan = serde_json::from_value(change.payload.clone())?;
    require(
        plan.phase == PHASE && !plan.objects.is_empty() && plan.objects.len() <= 256,
        "Invalid known-UID initializer",
    )?;
    let progress = if let Some(checkpoint) = journal.checkpoint() {
        require(
            checkpoint.outstanding_intents == 0,
            "Initializer cannot contain creation intents",
        )?;
        let progress: Progress = serde_json::from_value(checkpoint.data.clone())?;
        require(
            progress.steps.values().all(|step| {
                matches!(
                    step,
                    Step::NotStarted | Step::PatchPrepared { .. } | Step::Complete { .. }
                )
            }),
            "Invalid initializer progress",
        )?;
        progress
    } else {
        Progress {
            version: 1,
            steps: plan
                .objects
                .keys()
                .map(|path| (path.clone(), Step::NotStarted))
                .collect(),
        }
    };
    require(
        progress.version == 1 && progress.steps.keys().eq(plan.objects.keys()),
        "Initializer progress differs from approval",
    )?;
    require(
        progress
            .steps
            .values()
            .filter(|step| matches!(step, Step::PatchPrepared { .. }))
            .count()
            <= 1,
        "Multiple active initializer patches",
    )?;
    for (path, step) in &progress.steps {
        if let Step::PatchPrepared {
            uid,
            resource_version,
            patch,
            before,
            after,
        } = step
        {
            let approved = &plan.objects[path];
            require(
                uid == &approved.uid
                    && resource_version == &approved.resource_version
                    && patch == &approved.patch
                    && before == &approved.before
                    && after == &approved.after,
                "Saved initializer differs from approved concrete patch",
            )?;
        }
    }
    Ok((plan, progress))
}
fn retained(entries: &BTreeMap<String, Entry>, path: &str, patch: &Initializer) -> Result<()> {
    require(
        entries
            .get(path)
            .is_some_and(|entry| entry.uid == patch.uid && !entry.missing),
        "Initializer has no retained UID",
    )
}
fn complete(
    entries: &mut BTreeMap<String, Entry>,
    progress: &mut Progress,
    path: &str,
    patch: &Initializer,
    object: &Value,
) -> Result<()> {
    retained(entries, path, patch)?;
    let entry = entries
        .get_mut(path)
        .ok_or_else(|| Error::Conflict("Initializer lacks inventory".into()))?;
    entry.resource_version = bounded_id(object, "/metadata/resourceVersion")?;
    entry.pending_initialization = false;
    entry.deferred_changes = false;
    entry.initialization_artifact_sha256 = None;
    entry.creation_operation = None;
    entry.creation_fields_sha256 = None;
    progress.steps.insert(
        path.into(),
        Step::Complete {
            uid: patch.uid.clone(),
            proof: patch.after.clone(),
        },
    );
    Ok(())
}
pub(super) fn execute<A: Api>(
    k: &mut Kubernetes<A>,
    context: &Context,
    change: &PreparedChange,
    journal: &mut dyn OperationJournal,
) -> Result<Observation> {
    authority(k, context, change, journal)?;
    let (plan, mut progress) = decode(change, journal)?;
    let mut entries = inventory(&context.inventory)?;
    if progress
        .steps
        .values()
        .all(|step| matches!(step, Step::NotStarted))
    {
        require(
            k.observe(context)?.preconditions == change.preconditions,
            "Known-UID initializer plan is stale",
        )?;
    }
    for (path, patch) in &plan.objects {
        retained(&entries, path, patch)?;
        if matches!(progress.steps.get(path), Some(Step::Complete { .. })) {
            continue;
        }
        if matches!(progress.steps.get(path), Some(Step::NotStarted)) {
            progress.steps.insert(
                path.clone(),
                Step::PatchPrepared {
                    uid: patch.uid.clone(),
                    resource_version: patch.resource_version.clone(),
                    patch: patch.patch.clone(),
                    before: patch.before.clone(),
                    after: patch.after.clone(),
                },
            );
            save(journal, &progress, &entries, None)?;
        }
        let object = k
            .get(path)?
            .ok_or_else(|| Error::Conflict("Known-UID initializer object disappeared".into()))?;
        require(
            bounded_id(&object, "/metadata/uid")? == patch.uid,
            "Known-UID initializer UID changed",
        )?;
        let result = if image(&object)? == patch.after {
            object
        } else {
            require(
                image(&object)? == patch.before
                    && bounded_id(&object, "/metadata/resourceVersion")? == patch.resource_version
                    && ownership_transfer(&object, &manager(context))?.as_ref()
                        == Some(&patch.patch),
                "Concrete known-UID initializer is stale",
            )?;
            authority(k, context, change, journal)?;
            let route = format!("{path}?fieldManager={}", manager(context));
            let dry = k.mutate("PATCH", &format!("{route}&dryRun=All"), &patch.patch)?;
            require(
                image(&dry)? == patch.after,
                "Known-UID initializer dry-run changed",
            )?;
            authority(k, context, change, journal)?;
            let result = k.mutate("PATCH", &route, &patch.patch)?;
            require(
                bounded_id(&result, "/metadata/uid")? == patch.uid
                    && image(&result)? == patch.after,
                "Known-UID initializer response differs",
            )?;
            result
        };
        complete(&mut entries, &mut progress, path, patch, &result)?;
        save(journal, &progress, &entries, None)?;
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
    let (plan, mut progress) = decode(change, journal)?;
    let mut entries = inventory(&context.inventory)?;
    let mut review = false;
    for (path, patch) in &plan.objects {
        retained(&entries, path, patch)?;
        if matches!(progress.steps.get(path), Some(Step::Complete { .. })) {
            continue;
        }
        let object = k.get(path)?;
        let reason = if let Some(object) = object {
            if bounded_id(&object, "/metadata/uid")? != patch.uid {
                Some(RecoveryReason::UnexpectedOwnership)
            } else if image(&object)? == patch.after {
                complete(&mut entries, &mut progress, path, patch, &object)?;
                None
            } else if image(&object)? == patch.before
                && bounded_id(&object, "/metadata/resourceVersion")? == patch.resource_version
            {
                None
            } else {
                Some(RecoveryReason::StaleContinuation)
            }
        } else {
            Some(RecoveryReason::StaleContinuation)
        };
        review |= reason.is_some();
        save(journal, &progress, &entries, reason)?;
    }
    if review {
        Ok(RecoveryDisposition::ReviewRequired(
            RecoveryReason::StaleContinuation,
        ))
    } else if progress.complete() {
        Ok(RecoveryDisposition::Complete)
    } else {
        Ok(RecoveryDisposition::Resume)
    }
}

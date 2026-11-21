//! Durable adapter evidence, separate from permission for the next mutation.
use super::*;
use crate::store::operations::{
    OperationBinding, OperationUpdate, ReplacementUpdate, StoredOperation,
};

pub(super) struct Journal<'a> {
    store: &'a mut Store,
    state: &'a mut UnitState,
    record: StoredOperation,
    binding: OperationBinding,
    options: &'a Options,
    execute: bool,
    revision: String,
    expected_state: Option<UnitState>,
    replacement: Option<PreparedChange>,
}
impl<'a> Journal<'a> {
    pub(super) fn execution(
        store: &'a mut Store,
        state: &'a mut UnitState,
        context: &Context,
        change: &PreparedChange,
        backend: &dyn Backend,
        options: &'a Options,
    ) -> Result<Self> {
        let record = store
            .operation_record(&context.instance, &context.unit_id, &change.operation)?
            .ok_or_else(|| Error::Conflict("Missing running operation".into()))?;
        require(
            !record.accounting_only && record.change == *change && record.phase == "running",
            "Operation is not authorized to run",
        )?;
        let binding = record.binding.clone().unwrap_or(OperationBinding {
            instance: context.instance.clone(),
            unit: context.unit_id.clone(),
            target_reference: context.unit.target.clone(),
            target_identity_sha256: backend.target_identity()?,
        });
        let expected_state = store.unit(&context.instance, &context.unit_id)?;
        Ok(Self {
            store,
            state,
            record,
            binding,
            options,
            execute: true,
            revision: context.revision.clone(),
            expected_state,
            replacement: None,
        })
    }
}
impl OperationJournal for Journal<'_> {
    fn checkpoint(&self) -> Option<&OperationCheckpoint> {
        self.record.checkpoint.as_ref()
    }
    fn guard(&self) -> Result<()> {
        require(
            self.execute && self.options.execute,
            "Read-only recovery cannot execute changes",
        )?;
        if self.options.cancelled.load(Ordering::SeqCst) {
            return Err(Error::Cancelled);
        }
        let authority = self.replacement.as_ref().unwrap_or(&self.record.change);
        self.store
            .ensure_revision(&self.binding.instance, &authority.revision)?;
        let current = self
            .store
            .operation_record(
                &self.binding.instance,
                &self.binding.unit,
                &self.record.change.operation,
            )?
            .ok_or_else(|| Error::Conflict("Running operation disappeared".into()))?;
        require(
            current.change == self.record.change
                && current.checkpoint == self.record.checkpoint
                && current.accounting_only == self.record.accounting_only,
            "Running operation was superseded",
        )?;
        if let Some(replacement) = &self.replacement {
            let selected = self
                .store
                .unit_snapshot(&self.binding.instance, &self.binding.unit)?
                .ok_or_else(|| Error::Conflict("Recovery selection disappeared".into()))?;
            require(
                self.options.approved_operation.as_deref() == Some(&replacement.operation)
                    && selected.prepared.as_ref() == Some(replacement)
                    && selected.recovery.as_ref().is_some_and(|recovery| {
                        recovery.source_operation == current.change.operation
                            && recovery.replacement_operation.as_deref()
                                == Some(&replacement.operation)
                            && recovery.replacement_sequence
                                == Some(
                                    current
                                        .checkpoint
                                        .as_ref()
                                        .map_or(0, |checkpoint| checkpoint.sequence),
                                )
                    }),
                "Recovery approval was superseded",
            )
        } else {
            require(
                !current.accounting_only && current.phase == "running",
                "Running operation was superseded",
            )
        }
    }
    fn commit(
        &mut self,
        progress: &Value,
        outstanding_intents: usize,
        owned_inventory: &Value,
        review: Option<RecoveryReason>,
    ) -> Result<()> {
        // Do not call guard here: a successful HTTP response remains evidence
        // after cancellation or credential replacement forbids the next request.
        let mut next = self.state.clone();
        next.revision = self.revision.clone();
        next.inventory = owned_inventory.clone();
        let review = review.filter(|reason| {
            self.record.phase != "superseded-unresolved"
                || *reason == RecoveryReason::UnrecordedIdentity
                || next
                    .recovery
                    .as_ref()
                    .is_some_and(|old| old.source_operation == self.record.change.operation)
        });
        if let Some(reason) = review.filter(|_| {
            next.recovery
                .as_ref()
                .is_none_or(|old| old.source_operation == self.record.change.operation)
        }) {
            next.recovery = Some(RecoveryState {
                source_operation: self.record.change.operation.clone(),
                reason,
                outstanding_intents,
                replacement_operation: next
                    .recovery
                    .as_ref()
                    .filter(|old| old.source_operation == self.record.change.operation)
                    .and_then(|old| old.replacement_operation.clone()),
                removal_attempted: false,
                replacement_sequence: next
                    .recovery
                    .as_ref()
                    .filter(|old| old.source_operation == self.record.change.operation)
                    .and_then(|old| old.replacement_sequence),
            });
            next.applied = false;
            next.readiness = Readiness::NotReady;
            next.outputs.clear();
        }
        if let Some(replacement) = &self.replacement {
            self.guard()?;
            next.prepared = None;
            next.applied = false;
            next.readiness = Readiness::NotReady;
            next.outputs.clear();
            next.failure = None;
            next.attempts = 0;
            next.next_retry = 0;
            if outstanding_intents == 0 {
                next.recovery = Some(RecoveryState {
                    source_operation: self.record.change.operation.clone(),
                    reason: RecoveryReason::InitializationPending,
                    outstanding_intents: 0,
                    replacement_operation: None,
                    removal_attempted: false,
                    replacement_sequence: None,
                });
            } else if let Some(recovery) = &mut next.recovery {
                recovery.outstanding_intents = outstanding_intents;
                recovery.replacement_operation = None;
                recovery.replacement_sequence = None;
            }
            let phase = if outstanding_intents == 0 {
                "recovered"
            } else if self.record.phase == "superseded-unresolved" {
                "superseded-unresolved"
            } else {
                "review-required"
            };
            let record = self.store.replace_operation(
                &self.binding.instance,
                &self.binding.unit,
                ReplacementUpdate {
                    source: OperationUpdate {
                        expected: &self.record,
                        binding: &self.binding,
                        data: progress,
                        outstanding_intents,
                        state: &next,
                        expected_state: self.expected_state.as_ref(),
                        phase,
                    },
                    replacement,
                    phase: "succeeded",
                },
            )?;
            self.record = record;
            self.expected_state = Some(next.clone());
            *self.state = next;
            return Ok(());
        }
        let phase = if self.record.phase == "superseded-unresolved" {
            "superseded-unresolved"
        } else if next
            .recovery
            .as_ref()
            .is_some_and(|recovery| recovery.source_operation == self.record.change.operation)
        {
            "review-required"
        } else {
            "running"
        };
        let record = self.store.save_operation_state(
            &self.binding.instance,
            &self.binding.unit,
            OperationUpdate {
                expected: &self.record,
                binding: &self.binding,
                data: progress,
                outstanding_intents,
                state: &next,
                expected_state: self.expected_state.as_ref(),
                phase,
            },
        )?;
        self.record = record;
        self.expected_state = Some(next.clone());
        *self.state = next;
        Ok(())
    }
}

pub(super) fn recover_unit(
    store: &mut Store,
    compiled: &Compiled,
    id: &str,
    state: &mut UnitState,
    backends: &mut Registry,
    options: &Options,
) -> Result<Option<RecoveryDisposition>> {
    if recover_removal_attempt(store, compiled, id, state, backends)? {
        return Ok(state
            .recovery
            .as_ref()
            .map(|recovery| RecoveryDisposition::ReviewRequired(recovery.reason)));
    }
    let mut ids = store.unresolved_ids(&compiled.instance.id, id)?;
    if let Some(active) = store.running_record(&compiled.instance.id, id)? {
        ids.push(active.change.operation);
    }
    let mut result = None;
    for operation in ids {
        let record = store
            .operation_record(&compiled.instance.id, id, &operation)?
            .ok_or_else(|| Error::Conflict("Recovery operation disappeared".into()))?;
        let unit = state
            .definition
            .clone()
            .or_else(|| compiled.instance.units.get(id).cloned())
            .ok_or_else(|| Error::Invalid("Missing retained recovery definition".into()))?;
        let effective = state
            .effective
            .clone()
            .or_else(|| compiled.units.get(id).cloned())
            .ok_or_else(|| Error::Invalid("Missing retained recovery artifact".into()))?;
        let target = record
            .binding
            .as_ref()
            .map_or(unit.target.as_str(), |binding| {
                binding.target_reference.as_str()
            });
        let backend = backends
            .get_mut(target)
            .ok_or_else(|| Error::Invalid("Recovery target unavailable".into()))?;
        let identity = backend.target_identity()?;
        if let Some(binding) = &record.binding {
            require(
                binding.target_identity_sha256 == identity,
                "Recovery target identity changed",
            )?;
        }
        let context = Context {
            owner: store.owner.clone(),
            instance: compiled.instance.id.clone(),
            unit_id: id.into(),
            revision: compiled.revision.clone(),
            unit: unit.clone(),
            artifact: effective.artifact,
            artifact_sha256: record.change.artifact_sha256.clone(),
            inventory: state.inventory.clone(),
            interrupted: Some(record.change.clone()),
            recovery: Some(vec![RecoveryRecord {
                prepared: record.change.clone(),
                checkpoint: record.checkpoint.clone(),
                target_reference: target.into(),
                target_identity_sha256: identity.clone(),
                superseded: record.phase == "superseded-unresolved" || record.accounting_only,
            }]),
            work: store.work(&compiled.instance.id, id)?,
            engine_version: effective.engine_version,
            provider_lock: effective.provider_lock.map(String::into_bytes),
        };
        let binding = record.binding.clone().unwrap_or(OperationBinding {
            instance: compiled.instance.id.clone(),
            unit: id.into(),
            target_reference: target.into(),
            target_identity_sha256: identity,
        });
        let change = record.change.clone();
        let expected_state = store.unit(&compiled.instance.id, id)?;
        let mut journal = Journal {
            store,
            state,
            record,
            binding,
            options,
            execute: false,
            revision: compiled.revision.clone(),
            expected_state,
            replacement: None,
        };
        let mut disposition = backend.recover_change(&context, &change, &mut journal)?;
        if disposition == RecoveryDisposition::Resume
            && (change.revision != compiled.revision
                || change.target_sha256 != backend.execution_fingerprint()?)
        {
            let progress = journal
                .checkpoint()
                .ok_or_else(|| Error::Invalid("Recovery progress was not recorded".into()))?
                .clone();
            let inventory = journal.state.inventory.clone();
            journal.commit(
                &progress.data,
                progress.outstanding_intents,
                &inventory,
                Some(RecoveryReason::ChangedAuthority),
            )?;
            disposition = RecoveryDisposition::ReviewRequired(RecoveryReason::ChangedAuthority);
        }
        if disposition == RecoveryDisposition::Resume {
            if let Some(recovery) = journal
                .state
                .recovery
                .as_ref()
                .filter(|recovery| recovery.source_operation == change.operation)
            {
                disposition = RecoveryDisposition::ReviewRequired(recovery.reason);
            }
        }
        if disposition == RecoveryDisposition::Complete {
            journal.state.applied = false;
            journal.state.readiness = Readiness::NotReady;
            journal.state.prepared = None;
            journal.state.failure = None;
            journal.state.attempts = 0;
            journal.state.next_retry = 0;
            if !change.summary.deferred_changes {
                journal.state.applied_sha256 = change.artifact_sha256.clone();
            }
            let operation = journal.record.change.operation.clone();
            finish(
                journal.store,
                &compiled.instance.id,
                id,
                journal.state,
                &operation,
                "recovered",
            )?;
        }
        match disposition {
            RecoveryDisposition::Legacy | RecoveryDisposition::Tracked => {}
            RecoveryDisposition::ReviewRequired(_) => result = Some(disposition),
            RecoveryDisposition::Resume
                if journal.state.recovery.as_ref().is_some_and(|recovery| {
                    recovery.source_operation != change.operation
                        && recovery.reason == RecoveryReason::InitializationPending
                }) =>
            {
                result = Some(disposition)
            }
            _ if !matches!(result, Some(RecoveryDisposition::ReviewRequired(_))) => {
                result = Some(disposition)
            }
            _ => {}
        }
    }
    Ok(result.or_else(|| {
        state
            .recovery
            .as_ref()
            .map(|recovery| RecoveryDisposition::ReviewRequired(recovery.reason))
    }))
}

pub(super) fn finish(
    store: &mut Store,
    instance: &str,
    unit: &str,
    state: &mut UnitState,
    operation: &str,
    phase: &str,
) -> Result<()> {
    let record = store
        .operation_record(instance, unit, operation)?
        .ok_or_else(|| Error::Conflict("Operation disappeared before finalization".into()))?;
    if let (Some(checkpoint), Some(binding)) = (&record.checkpoint, &record.binding) {
        require(
            checkpoint.outstanding_intents == 0,
            "Unresolved creation intents require review",
        )?;
        let mut next = state.clone();
        if next
            .recovery
            .as_ref()
            .is_some_and(|recovery| recovery.source_operation == operation)
        {
            next.recovery = None;
        }
        let expected_state = store.unit(instance, unit)?;
        store.save_operation_state(
            instance,
            unit,
            OperationUpdate {
                expected: &record,
                binding,
                expected_state: expected_state.as_ref(),
                data: &checkpoint.data,
                outstanding_intents: 0,
                state: &next,
                phase,
            },
        )?;
        *state = next;
    } else {
        store.save_unit(instance, unit, state)?;
        store.finish(operation, phase)?;
    }
    Ok(())
}

/// Identity-only recovery plans use retained source context before dependencies.
pub(super) fn review(
    store: &mut Store,
    compiled: &Compiled,
    id: &str,
    state: &mut UnitState,
    backends: &mut Registry,
    options: &Options,
) -> Result<()> {
    let Some(recovery) = state.recovery.clone() else {
        return Ok(());
    };
    let selected = options
        .approved_operation
        .as_ref()
        .and_then(|id| state.prepared.as_ref().filter(|plan| &plan.operation == id))
        .cloned();
    if selected.is_none()
        && !(options.prepare && recovery.reason == RecoveryReason::UnrecordedIdentity)
    {
        return Ok(());
    }
    let source = store
        .operation_record(&compiled.instance.id, id, &recovery.source_operation)?
        .ok_or_else(|| Error::Conflict("Recovery source disappeared".into()))?;
    let binding = source
        .binding
        .clone()
        .ok_or_else(|| Error::Invalid("Recovery source lacks target binding".into()))?;
    let unit = state
        .definition
        .clone()
        .ok_or_else(|| Error::Invalid("Missing recovery unit definition".into()))?;
    let effective = state
        .effective
        .clone()
        .ok_or_else(|| Error::Invalid("Missing recovery artifact".into()))?;
    let backend = backends
        .get_mut(&binding.target_reference)
        .ok_or_else(|| Error::Invalid("Recovery target unavailable".into()))?;
    require(
        backend.target_identity()? == binding.target_identity_sha256,
        "Recovery target identity changed",
    )?;
    let context = Context {
        owner: store.owner.clone(),
        instance: compiled.instance.id.clone(),
        unit_id: id.into(),
        revision: compiled.revision.clone(),
        unit,
        artifact: effective.artifact,
        artifact_sha256: source.change.artifact_sha256.clone(),
        inventory: state.inventory.clone(),
        interrupted: None,
        recovery: Some(vec![RecoveryRecord {
            prepared: source.change.clone(),
            checkpoint: source.checkpoint.clone(),
            target_reference: binding.target_reference.clone(),
            target_identity_sha256: binding.target_identity_sha256.clone(),
            superseded: source.phase == "superseded-unresolved" || source.accounting_only,
        }]),
        work: store.work(&compiled.instance.id, id)?,
        engine_version: effective.engine_version,
        provider_lock: effective.provider_lock.map(String::into_bytes),
    };
    if let Some(plan) = selected {
        require(
            plan.summary.recovery_bindings > 0
                && compiled.instance.policy.mode == Management::Manage
                && options.execute,
            "Identity binding requires its selected manage-mode approval",
        )?;
        require(
            plan.revision == compiled.revision
                && plan.target_sha256 == backend.execution_fingerprint()?,
            "Recovery approval authority changed",
        )?;
        require(
            plan.summary.creates == 0
                && plan.summary.updates == 0
                && plan.summary.deletes == 0
                && plan.summary.field_ownership_transfers == 0
                && (plan.summary.replacements == 0 || compiled.instance.policy.allow_replace),
            "Policy denies recovery identity binding",
        )?;
        plan.recovery_bindings()?;
        let expected_state = store.unit_snapshot(&compiled.instance.id, id)?;
        let mut journal = Journal {
            store,
            state,
            record: source,
            binding,
            options,
            execute: true,
            revision: compiled.revision.clone(),
            expected_state,
            replacement: Some(plan.clone()),
        };
        backend.execute_change_with_journal(&context, &plan, &mut journal)?;
        require(
            journal
                .store
                .operation_record(&compiled.instance.id, id, &plan.operation)?
                .is_some_and(|record| record.phase == "succeeded"),
            "Identity binding did not durably finalize",
        )?;
        return Ok(());
    }
    let operation = store.token()?;
    let observed = Observation {
        inventory: state.inventory.clone(),
        ..Default::default()
    };
    let plan = backend.prepare_change(&context, &observed, &operation)?;
    require(
        plan.summary.recovery_bindings > 0
            && plan.operation == operation
            && plan.revision == compiled.revision
            && plan.artifact_sha256 == context.artifact_sha256
            && plan.target_sha256 == backend.execution_fingerprint()?,
        "Backend returned an invalid identity-binding plan",
    )?;
    plan.recovery_bindings()?;
    let expected_state = store.unit_snapshot(&compiled.instance.id, id)?;
    let mut next = state.clone();
    next.prepared = Some(plan.clone());
    if let Some(recovery) = &mut next.recovery {
        recovery.replacement_operation = Some(operation);
        recovery.replacement_sequence = Some(
            source
                .checkpoint
                .as_ref()
                .map_or(0, |checkpoint| checkpoint.sequence),
        );
    }
    store.associate_replacement(
        &compiled.instance.id,
        id,
        &source,
        &plan,
        &next,
        expected_state.as_ref(),
    )?;
    *state = next;
    Ok(())
}

pub(super) fn associate(
    store: &mut Store,
    instance: &str,
    unit: &str,
    state: &mut UnitState,
    plan: &PreparedChange,
) -> Result<()> {
    let recovery = state
        .recovery
        .as_ref()
        .ok_or_else(|| Error::Conflict("Missing recovery barrier".into()))?;
    let source = store
        .operation_record(instance, unit, &recovery.source_operation)?
        .ok_or_else(|| Error::Conflict("Recovery source disappeared".into()))?;
    let expected_state = store.unit_snapshot(instance, unit)?;
    let mut next = state.clone();
    next.prepared = Some(plan.clone());
    if let Some(recovery) = &mut next.recovery {
        recovery.replacement_operation = Some(plan.operation.clone());
        recovery.replacement_sequence = Some(
            source
                .checkpoint
                .as_ref()
                .map_or(0, |checkpoint| checkpoint.sequence),
        );
    }
    store.associate_replacement(
        instance,
        unit,
        &source,
        plan,
        &next,
        expected_state.as_ref(),
    )?;
    *state = next;
    Ok(())
}
pub(super) fn begin_replacement(
    store: &mut Store,
    instance: &str,
    unit: &str,
    state: &mut UnitState,
    plan: &PreparedChange,
) -> Result<()> {
    let recovery = state
        .recovery
        .as_ref()
        .ok_or_else(|| Error::Conflict("Missing recovery barrier".into()))?;
    let source = store
        .operation_record(instance, unit, &recovery.source_operation)?
        .ok_or_else(|| Error::Conflict("Recovery source disappeared".into()))?;
    let checkpoint = source
        .checkpoint
        .as_ref()
        .ok_or_else(|| Error::Invalid("Missing recovery source checkpoint".into()))?;
    let binding = source
        .binding
        .as_ref()
        .ok_or_else(|| Error::Invalid("Missing recovery source binding".into()))?;
    let expected_state = store.unit_snapshot(instance, unit)?;
    let mut next = state.clone();
    next.recovery = None;
    next.failure = None;
    let phase = if checkpoint.outstanding_intents > 0 {
        "superseded-unresolved"
    } else {
        "recovered"
    };
    store.replace_operation(
        instance,
        unit,
        ReplacementUpdate {
            source: OperationUpdate {
                expected: &source,
                binding,
                data: &checkpoint.data,
                outstanding_intents: checkpoint.outstanding_intents,
                state: &next,
                expected_state: expected_state.as_ref(),
                phase,
            },
            replacement: plan,
            phase: "running",
        },
    )?;
    *state = next;
    Ok(())
}

pub(super) fn outstanding(store: &Store, instance: &str, unit: &str) -> Result<usize> {
    let mut ids = store.unresolved_ids(instance, unit)?;
    if let Some(active) = store.running_record(instance, unit)? {
        ids.push(active.change.operation);
    }
    let mut count = 0usize;
    for id in ids {
        let record = store
            .operation_record(instance, unit, &id)?
            .ok_or_else(|| Error::Conflict("Recovery source disappeared".into()))?;
        count = count
            .checked_add(
                record
                    .checkpoint
                    .map_or(0, |checkpoint| checkpoint.outstanding_intents),
            )
            .ok_or_else(|| Error::Invalid("Outstanding intent budget exceeded".into()))?;
    }
    require(count <= 256, "Outstanding intent budget exceeded")?;
    Ok(count)
}
pub(super) fn stop_for_removal(
    store: &mut Store,
    compiled: &Compiled,
    id: &str,
    state: &mut UnitState,
) -> Result<()> {
    let source = store
        .running_record(&compiled.instance.id, id)?
        .ok_or_else(|| Error::Conflict("Resumable operation disappeared".into()))?;
    let checkpoint = source
        .checkpoint
        .as_ref()
        .ok_or_else(|| Error::Invalid("Missing removal recovery evidence".into()))?;
    let binding = source
        .binding
        .as_ref()
        .ok_or_else(|| Error::Invalid("Missing removal recovery binding".into()))?;
    let expected_state = store.unit_snapshot(&compiled.instance.id, id)?;
    let mut next = state.clone();
    next.revision = compiled.revision.clone();
    next.recovery = Some(RecoveryState {
        source_operation: source.change.operation.clone(),
        reason: RecoveryReason::InitializationPending,
        outstanding_intents: checkpoint.outstanding_intents,
        replacement_operation: None,
        removal_attempted: false,
        replacement_sequence: None,
    });
    next.applied = false;
    next.readiness = Readiness::NotReady;
    store.save_operation_state(
        &compiled.instance.id,
        id,
        OperationUpdate {
            expected: &source,
            binding,
            data: &checkpoint.data,
            outstanding_intents: checkpoint.outstanding_intents,
            state: &next,
            expected_state: expected_state.as_ref(),
            phase: "review-required",
        },
    )?;
    *state = next;
    Ok(())
}

/// A removal attempt occupies the selected source's existing slot. Its immutable
/// approval is referenced exactly, and issued DELETEs are never replayed.
pub(super) fn begin_removal(
    store: &mut Store,
    instance: &str,
    unit: &str,
    state: &mut UnitState,
    plan: &PreparedChange,
) -> Result<()> {
    let recovery = state
        .recovery
        .as_ref()
        .ok_or_else(|| Error::Conflict("Missing removal recovery barrier".into()))?;
    require(!recovery.removal_attempted, "Removal was already attempted")?;
    let source = store
        .operation_record(instance, unit, &recovery.source_operation)?
        .ok_or_else(|| Error::Conflict("Removal source disappeared".into()))?;
    let checkpoint = source
        .checkpoint
        .as_ref()
        .ok_or_else(|| Error::Invalid("Missing removal checkpoint".into()))?;
    let binding = source
        .binding
        .as_ref()
        .ok_or_else(|| Error::Invalid("Missing removal source binding".into()))?;
    let expected_state = store.unit_snapshot(instance, unit)?;
    let mut next = state.clone();
    let recovery = next
        .recovery
        .as_mut()
        .ok_or_else(|| Error::Conflict("Missing removal selection".into()))?;
    recovery.removal_attempted = true;
    recovery.replacement_sequence = Some(
        checkpoint
            .sequence
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("Operation sequence exhausted".into()))?,
    );
    store.replace_operation(
        instance,
        unit,
        ReplacementUpdate {
            source: OperationUpdate {
                expected: &source,
                binding,
                data: &checkpoint.data,
                outstanding_intents: checkpoint.outstanding_intents,
                state: &next,
                expected_state: expected_state.as_ref(),
                phase: &source.phase,
            },
            replacement: plan,
            phase: "in-place-removal",
        },
    )?;
    *state = next;
    Ok(())
}
pub(super) fn finish_removal(
    store: &mut Store,
    instance: &str,
    unit: &str,
    state: &mut UnitState,
    observed: Observation,
) -> Result<()> {
    let recovery = state
        .recovery
        .as_ref()
        .ok_or_else(|| Error::Conflict("Missing attempted removal".into()))?;
    require(recovery.removal_attempted, "Removal has not been attempted")?;
    let source = store
        .operation_record(instance, unit, &recovery.source_operation)?
        .ok_or_else(|| Error::Conflict("Removal source disappeared".into()))?;
    let checkpoint = source
        .checkpoint
        .as_ref()
        .ok_or_else(|| Error::Invalid("Missing removal checkpoint".into()))?;
    let binding = source
        .binding
        .as_ref()
        .ok_or_else(|| Error::Invalid("Missing removal source binding".into()))?;
    let plan = state
        .prepared
        .as_ref()
        .ok_or_else(|| Error::Conflict("Missing attempted removal approval".into()))?;
    let expected_state = store.unit_snapshot(instance, unit)?;
    let mut next = state.clone();
    next.inventory = observed.inventory;
    next.retained = observed.exists;
    next.deleted = !observed.exists && outstanding(store, instance, unit)? == 0;
    next.prepared = None;
    next.applied = false;
    next.readiness = Readiness::NotReady;
    next.failure = None;
    next.attempts = 0;
    next.next_retry = 0;
    if let Some(recovery) = &mut next.recovery {
        recovery.removal_attempted = false;
        recovery.replacement_operation = None;
        recovery.replacement_sequence = None;
        if checkpoint.outstanding_intents == 0 {
            recovery.reason = RecoveryReason::RemovalPending;
        }
    }
    if !next.deleted {
        next.waiting =
            Some("Waiting for unresolved creation recovery or a fresh removal plan".into());
    }
    let phase = if checkpoint.outstanding_intents == 0 {
        "recovered"
    } else {
        source.phase.as_str()
    };
    if checkpoint.outstanding_intents == 0 && next.deleted {
        next.recovery = None;
    }
    store.replace_operation(
        instance,
        unit,
        ReplacementUpdate {
            source: OperationUpdate {
                expected: &source,
                binding,
                data: &checkpoint.data,
                outstanding_intents: checkpoint.outstanding_intents,
                state: &next,
                expected_state: expected_state.as_ref(),
                phase,
            },
            replacement: plan,
            phase: "succeeded",
        },
    )?;
    *state = next;
    Ok(())
}
fn recover_removal_attempt(
    store: &mut Store,
    compiled: &Compiled,
    id: &str,
    state: &mut UnitState,
    backends: &mut Registry,
) -> Result<bool> {
    let Some(recovery) = state
        .recovery
        .as_ref()
        .filter(|recovery| recovery.removal_attempted)
    else {
        return Ok(false);
    };
    let source = store
        .operation_record(&compiled.instance.id, id, &recovery.source_operation)?
        .ok_or_else(|| Error::Conflict("Attempted removal source disappeared".into()))?;
    require(
        source.accounting_only,
        "Attempted removal lacks accounting restriction",
    )?;
    let plan = state
        .prepared
        .as_ref()
        .ok_or_else(|| Error::Conflict("Attempted removal approval disappeared".into()))?;
    require(
        recovery.replacement_operation.as_deref() == Some(&plan.operation)
            && recovery.replacement_sequence
                == source
                    .checkpoint
                    .as_ref()
                    .map(|checkpoint| checkpoint.sequence),
        "Attempted removal selection changed",
    )?;
    let attempt = store
        .operation_record(&compiled.instance.id, id, &plan.operation)?
        .ok_or_else(|| Error::Conflict("Attempted removal record disappeared".into()))?;
    require(
        attempt.phase == "in-place-removal" && attempt.change == *plan,
        "Attempted removal authority changed",
    )?;
    let binding = source
        .binding
        .as_ref()
        .ok_or_else(|| Error::Invalid("Missing removal source binding".into()))?;
    let backend = backends
        .get_mut(&binding.target_reference)
        .ok_or_else(|| Error::Invalid("Removal recovery target unavailable".into()))?;
    require(
        backend.target_identity()? == binding.target_identity_sha256,
        "Removal recovery target identity changed",
    )?;
    let unit = state
        .definition
        .clone()
        .ok_or_else(|| Error::Invalid("Missing removal definition".into()))?;
    let effective = state
        .effective
        .clone()
        .ok_or_else(|| Error::Invalid("Missing removal artifact".into()))?;
    let context = Context {
        owner: store.owner.clone(),
        instance: compiled.instance.id.clone(),
        unit_id: id.into(),
        revision: compiled.revision.clone(),
        unit,
        artifact: effective.artifact,
        artifact_sha256: state.artifact_sha256.clone(),
        inventory: state.inventory.clone(),
        interrupted: None,
        recovery: None,
        work: store.work(&compiled.instance.id, id)?,
        engine_version: effective.engine_version,
        provider_lock: effective.provider_lock.map(String::into_bytes),
    };
    let observed = backend.observe_removal(&context)?;
    state.revision = compiled.revision.clone();
    finish_removal(store, &compiled.instance.id, id, state, observed)?;
    Ok(true)
}

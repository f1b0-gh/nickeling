//! Bounded operation records. Approvals remain immutable as checkpoints advance.
use super::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Write};

pub(crate) const APPROVAL_LIMIT: usize = 16 * 1024 * 1024;
pub(crate) const PROGRESS_LIMIT: usize = 4 * 1024 * 1024;
const BINDING_LIMIT: usize = 64 * 1024;
const ENVELOPE_LIMIT: usize = APPROVAL_LIMIT + PROGRESS_LIMIT + BINDING_LIMIT;

pub(crate) fn bounded_json(value: &impl Serialize, limit: usize) -> Result<String> {
    struct Bounded {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let next = self
                .bytes
                .len()
                .checked_add(bytes.len())
                .filter(|len| *len <= self.limit)
                .ok_or_else(|| io::Error::other("Operation serialization budget exceeded"))?;
            if next > self.bytes.capacity() {
                let capacity = self
                    .bytes
                    .capacity()
                    .saturating_mul(2)
                    .max(next)
                    .min(self.limit);
                self.bytes
                    .try_reserve_exact(capacity - self.bytes.len())
                    .map_err(|_| io::Error::other("Operation serialization allocation refused"))?;
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Bounded {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut writer, value)?;
    String::from_utf8(writer.bytes)
        .map_err(|_| Error::Invalid("Operation encoding is not UTF-8".into()))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct OperationBinding {
    pub instance: String,
    pub unit: String,
    pub target_reference: String,
    pub target_identity_sha256: String,
}
pub(crate) use crate::backend::OperationCheckpoint as Checkpoint;
#[derive(Clone, Debug)]
pub(crate) struct StoredOperation {
    pub change: PreparedChange,
    pub phase: String,
    pub binding: Option<OperationBinding>,
    pub checkpoint: Option<Checkpoint>,
    pub accounting_only: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Envelope {
    journal_version: u32,
    #[serde(default)]
    accounting_only: bool,
    binding: OperationBinding,
    approved_sha256: String,
    prepared: PreparedChange,
    checkpoint: Checkpoint,
}

fn decode(
    instance: &str,
    unit: &str,
    id: &str,
    revision: &str,
    phase: String,
    body: Option<String>,
) -> Result<StoredOperation> {
    let body =
        body.ok_or_else(|| Error::Invalid("Operation envelope exceeds its byte budget".into()))?;
    let value: Value = serde_json::from_str(&body)?;
    let (change, binding, checkpoint, accounting_only) = if value.get("journalVersion").is_some() {
        let envelope: Envelope = serde_json::from_str(&body)?;
        require(
            envelope.journal_version == 1,
            "Unsupported operation journal version",
        )?;
        require(
            envelope.binding.instance == instance && envelope.binding.unit == unit,
            "Operation journal belongs to another unit",
        )?;
        validate_id(&envelope.binding.target_reference)?;
        require(
            !envelope.binding.target_identity_sha256.is_empty(),
            "Missing operation target identity",
        )?;
        bounded_json(&envelope.binding, BINDING_LIMIT)?;
        bounded_json(&envelope.checkpoint.data, PROGRESS_LIMIT)?;
        require(
            envelope.checkpoint.outstanding_intents <= 256,
            "Outstanding intent budget exceeded",
        )?;
        let encoded = bounded_json(&envelope.prepared, APPROVAL_LIMIT)?;
        require(
            files::hash(encoded.as_bytes()) == envelope.approved_sha256,
            "Operation approval digest changed",
        )?;
        (
            envelope.prepared,
            Some(envelope.binding),
            Some(envelope.checkpoint),
            envelope.accounting_only,
        )
    } else {
        require(
            body.len() <= APPROVAL_LIMIT,
            "Operation approval exceeds its byte budget",
        )?;
        (serde_json::from_str(&body)?, None, None, false)
    };
    require(
        change.operation == id && change.revision == revision,
        "Operation row and approval identity differ",
    )?;
    require(
        phase != "superseded-unresolved"
            || checkpoint
                .as_ref()
                .is_some_and(|checkpoint| checkpoint.outstanding_intents > 0),
        "Unresolved operation lacks outstanding checkpoint evidence",
    )?;
    Ok(StoredOperation {
        change,
        phase,
        binding,
        checkpoint,
        accounting_only,
    })
}

fn read_record(
    connection: &Connection,
    instance: &str,
    unit: &str,
    id: &str,
) -> Result<Option<StoredOperation>> {
    let row: Option<(Option<String>, Option<String>, Option<String>)> = connection.query_row(
        "SELECT CASE WHEN length(CAST(revision AS BLOB))<=256 THEN revision ELSE NULL END,CASE WHEN length(CAST(phase AS BLOB))<=64 THEN phase ELSE NULL END,CASE WHEN length(CAST(change AS BLOB))<=?4 THEN change ELSE NULL END FROM operations WHERE instance=?1 AND unit=?2 AND id=?3",
        params![instance,unit,id,ENVELOPE_LIMIT as i64],
        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
    ).optional()?;
    row.map(|(revision, phase, body)| {
        let revision = revision
            .ok_or_else(|| Error::Invalid("Operation revision exceeds its byte budget".into()))?;
        let phase = phase
            .ok_or_else(|| Error::Invalid("Operation phase exceeds its byte budget".into()))?;
        decode(instance, unit, id, &revision, phase, body)
    })
    .transpose()
}

impl Store {
    pub(crate) fn operation_record(
        &self,
        instance: &str,
        unit: &str,
        id: &str,
    ) -> Result<Option<StoredOperation>> {
        read_record(&self.connection, instance, unit, id)
    }
    pub(crate) fn running_record(
        &self,
        instance: &str,
        unit: &str,
    ) -> Result<Option<StoredOperation>> {
        let ids = active_ids(&self.connection, instance, unit)?;
        match ids.into_iter().next() {
            None => Ok(None),
            Some(id) => self.operation_record(instance, unit, &id),
        }
    }
}

pub(crate) struct OperationUpdate<'a> {
    pub expected: &'a StoredOperation,
    pub binding: &'a OperationBinding,
    pub data: &'a Value,
    pub outstanding_intents: usize,
    pub state: &'a UnitState,
    pub expected_state: Option<&'a UnitState>,
    pub phase: &'a str,
}
fn active_ids(connection: &Connection, instance: &str, unit: &str) -> Result<Vec<String>> {
    let mut statement = connection.prepare(
        "SELECT CASE WHEN length(CAST(id AS BLOB))<=256 THEN id ELSE NULL END FROM operations WHERE instance=?1 AND unit=?2 AND phase IN ('running','review-required','superseded-unresolved') AND phase!='superseded-unresolved' LIMIT 2")?;
    let ids: Vec<Option<String>> = statement
        .query_map(params![instance, unit], |row| row.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    require(ids.len() <= 1, "Multiple active operations for a unit")?;
    ids.into_iter()
        .map(|id| {
            id.ok_or_else(|| Error::Invalid("Operation identifier exceeds its byte budget".into()))
        })
        .collect()
}
pub(super) fn admit_phase(
    connection: &Connection,
    instance: &str,
    unit: &str,
    id: &str,
    phase: &str,
) -> Result<()> {
    require(
        matches!(
            phase,
            "prepared"
                | "running"
                | "review-required"
                | "superseded-unresolved"
                | "succeeded"
                | "recovered"
        ),
        "Unsupported operation phase",
    )?;
    if matches!(phase, "running" | "review-required") {
        require(
            active_ids(connection, instance, unit)?
                .iter()
                .all(|existing| existing == id),
            "Another operation is active",
        )?;
    }
    if phase == "superseded-unresolved" {
        let ids = unresolved_ids(connection, instance, unit)?;
        require(
            ids.len() < 16 || ids.iter().any(|existing| existing == id),
            "Unresolved operation budget exceeded",
        )?;
    }
    Ok(())
}
fn unresolved_ids(connection: &Connection, instance: &str, unit: &str) -> Result<Vec<String>> {
    let mut statement = connection.prepare(
        "SELECT CASE WHEN length(CAST(id AS BLOB))<=256 THEN id ELSE NULL END FROM operations WHERE instance=?1 AND unit=?2 AND phase IN ('running','review-required','superseded-unresolved') AND phase='superseded-unresolved' LIMIT 17")?;
    let ids: Vec<Option<String>> = statement
        .query_map(params![instance, unit], |row| row.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    require(ids.len() <= 16, "Unresolved operation budget exceeded")?;
    ids.into_iter()
        .map(|id| {
            id.ok_or_else(|| Error::Invalid("Operation identifier exceeds its byte budget".into()))
        })
        .collect()
}

fn write_operation_state(
    transaction: &Connection,
    instance: &str,
    unit: &str,
    update: OperationUpdate<'_>,
) -> Result<StoredOperation> {
    let OperationUpdate {
        expected,
        binding,
        data,
        outstanding_intents,
        state,
        expected_state,
        phase,
    } = update;
    require(
        binding.instance == instance && binding.unit == unit,
        "Operation journal belongs to another unit",
    )?;
    validate_id(&binding.target_reference)?;
    require(
        !binding.target_identity_sha256.is_empty(),
        "Missing operation target identity",
    )?;
    require(
        outstanding_intents <= 256,
        "Outstanding intent budget exceeded",
    )?;
    require(
        phase != "superseded-unresolved" || outstanding_intents > 0,
        "Resolved source must be retired",
    )?;
    require(
        matches!(
            phase,
            "running" | "review-required" | "superseded-unresolved" | "succeeded" | "recovered"
        ),
        "Unsupported journal transition",
    )?;
    require(
        outstanding_intents == 0
            || matches!(
                phase,
                "running" | "review-required" | "superseded-unresolved"
            ),
        "Unresolved intents cannot be retired",
    )?;
    bounded_json(binding, BINDING_LIMIT)?;
    bounded_json(data, PROGRESS_LIMIT)?;
    let state_data = bounded_json(state, ENVELOPE_LIMIT)?;
    let revision = current_revision(transaction, instance)?;
    require(
        revision == state.revision,
        "Desired revision was superseded",
    )?;
    let previous_state = check_unit_snapshot(transaction, instance, unit, expected_state)?;
    let current = read_record(transaction, instance, unit, &expected.change.operation)?
        .ok_or_else(|| Error::Conflict("Operation no longer exists".into()))?;
    require(
        current.change == expected.change
            && current.phase == expected.phase
            && current.checkpoint == expected.checkpoint
            && current.binding == expected.binding
            && current.accounting_only == expected.accounting_only,
        "Operation checkpoint was superseded",
    )?;
    require(
        current.binding.as_ref().is_none_or(|old| old == binding),
        "Operation target binding changed",
    )?;
    let ids = unresolved_ids(transaction, instance, unit)?;
    let mut total = outstanding_intents;
    for id in &ids {
        if id != &current.change.operation {
            let other = read_record(transaction, instance, unit, id)?
                .ok_or_else(|| Error::Conflict("Unresolved operation disappeared".into()))?;
            total = total
                .checked_add(
                    other
                        .checkpoint
                        .map_or(0, |checkpoint| checkpoint.outstanding_intents),
                )
                .ok_or_else(|| Error::Invalid("Outstanding intent budget exceeded".into()))?;
        }
    }
    // Include the separate active row when updating a superseded record.
    let active = active_ids(transaction, instance, unit)?;
    for id in &active {
        if id != &current.change.operation {
            let other = read_record(transaction, instance, unit, id)?
                .ok_or_else(|| Error::Conflict("Active operation disappeared".into()))?;
            total = total
                .checked_add(
                    other
                        .checkpoint
                        .map_or(0, |checkpoint| checkpoint.outstanding_intents),
                )
                .ok_or_else(|| Error::Invalid("Outstanding intent budget exceeded".into()))?;
        }
    }
    require(total <= 256, "Outstanding intent budget exceeded")?;
    if phase == "superseded-unresolved" && current.phase != phase {
        require(ids.len() < 16, "Unresolved operation budget exceeded")?;
    }
    if matches!(phase, "running" | "review-required") {
        require(
            active.iter().all(|id| id == &current.change.operation),
            "Another operation is active",
        )?;
    }
    let unchanged = previous_state.as_deref() == Some(state_data.as_str())
        && current.phase == phase
        && current
            .checkpoint
            .as_ref()
            .is_some_and(|old| old.data == *data && old.outstanding_intents == outstanding_intents);
    let sequence = if unchanged {
        current.checkpoint.as_ref().map_or(0, |old| old.sequence)
    } else {
        current
            .checkpoint
            .as_ref()
            .map_or(0, |old| old.sequence)
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("Operation sequence exhausted".into()))?
    };
    let checkpoint = Checkpoint {
        sequence,
        outstanding_intents,
        data: data.clone(),
    };
    let approval = bounded_json(&current.change, APPROVAL_LIMIT)?;
    let envelope = Envelope {
        journal_version: 1,
        accounting_only: current.accounting_only,
        binding: binding.clone(),
        approved_sha256: files::hash(approval.as_bytes()),
        prepared: current.change.clone(),
        checkpoint: checkpoint.clone(),
    };
    let encoded = bounded_json(&envelope, ENVELOPE_LIMIT)?;
    transaction.execute(
        "UPDATE operations SET change=?1,phase=?2 WHERE id=?3 AND instance=?4 AND unit=?5",
        params![encoded, phase, current.change.operation, instance, unit],
    )?;
    transaction.execute("INSERT INTO units VALUES(?1,?2,?3) ON CONFLICT(instance,unit) DO UPDATE SET state=excluded.state",
            params![instance,unit,state_data])?;
    Ok(StoredOperation {
        change: current.change,
        phase: phase.into(),
        binding: Some(binding.clone()),
        checkpoint: Some(checkpoint),
        accounting_only: current.accounting_only,
    })
}
impl Store {
    pub(crate) fn save_operation_state(
        &mut self,
        instance: &str,
        unit: &str,
        update: OperationUpdate<'_>,
    ) -> Result<StoredOperation> {
        let transaction = self.connection.transaction()?;
        let result = write_operation_state(&transaction, instance, unit, update)?;
        transaction.commit()?;
        Ok(result)
    }
    pub(crate) fn unresolved_ids(&self, instance: &str, unit: &str) -> Result<Vec<String>> {
        unresolved_ids(&self.connection, instance, unit)
    }
}

pub(crate) struct ReplacementUpdate<'a> {
    pub source: OperationUpdate<'a>,
    pub replacement: &'a PreparedChange,
    pub phase: &'a str,
}
fn same_record(actual: &StoredOperation, expected: &StoredOperation) -> Result<()> {
    require(
        actual.change == expected.change
            && actual.phase == expected.phase
            && actual.binding == expected.binding
            && actual.checkpoint == expected.checkpoint
            && actual.accounting_only == expected.accounting_only,
        "Recovery source was superseded",
    )
}
fn current_revision(connection: &Connection, instance: &str) -> Result<String> {
    let revision: Option<String> = connection.query_row("SELECT CASE WHEN length(CAST(revision AS BLOB))<=256 THEN revision ELSE NULL END FROM instances WHERE id=?1", [instance], |row| row.get(0))?;
    revision.ok_or_else(|| Error::Invalid("Desired revision exceeds its byte budget".into()))
}
fn check_unit_snapshot(
    connection: &Connection,
    instance: &str,
    unit: &str,
    expected: Option<&UnitState>,
) -> Result<Option<String>> {
    let body: Option<Option<String>> = connection.query_row(
        "SELECT CASE WHEN length(CAST(state AS BLOB))<=?3 THEN state ELSE NULL END FROM units WHERE instance=?1 AND unit=?2",
        params![instance,unit,ENVELOPE_LIMIT as i64], |row| row.get(0)).optional()?;
    let actual = body
        .map(|body| {
            let body = body
                .ok_or_else(|| Error::Invalid("Unit state exceeds recovery byte budget".into()))?;
            let state: UnitState = serde_json::from_str(&body)?;
            bounded_json(&state, ENVELOPE_LIMIT)
        })
        .transpose()?;
    let expected = expected
        .map(|state| bounded_json(state, ENVELOPE_LIMIT))
        .transpose()?;
    require(actual == expected, "Unit evidence was superseded")?;
    Ok(actual)
}
fn unit_snapshot(connection: &Connection, instance: &str, unit: &str) -> Result<Option<UnitState>> {
    let body: Option<Option<String>> = connection.query_row(
        "SELECT CASE WHEN length(CAST(state AS BLOB))<=?3 THEN state ELSE NULL END FROM units WHERE instance=?1 AND unit=?2",
        params![instance,unit,ENVELOPE_LIMIT as i64], |row| row.get(0)).optional()?;
    body.map(|body| {
        let body =
            body.ok_or_else(|| Error::Invalid("Unit state exceeds recovery byte budget".into()))?;
        Ok(serde_json::from_str(&body)?)
    })
    .transpose()
}
fn stored_unit(connection: &Connection, instance: &str, unit: &str) -> Result<UnitState> {
    unit_snapshot(connection, instance, unit)?
        .ok_or_else(|| Error::Conflict("Recovery unit state disappeared".into()))
}
impl Store {
    pub(crate) fn unit_snapshot(&self, instance: &str, unit: &str) -> Result<Option<UnitState>> {
        unit_snapshot(&self.connection, instance, unit)
    }
    pub(crate) fn associate_replacement(
        &mut self,
        instance: &str,
        unit: &str,
        source: &StoredOperation,
        replacement: &PreparedChange,
        state: &UnitState,
        expected_state: Option<&UnitState>,
    ) -> Result<()> {
        require(
            state.prepared.as_ref() == Some(replacement),
            "Replacement plan is not selected",
        )?;
        require(
            state.recovery.as_ref().is_some_and(|recovery| {
                recovery.source_operation == source.change.operation
                    && recovery.replacement_operation.as_deref() == Some(&replacement.operation)
                    && recovery.replacement_sequence
                        == Some(
                            source
                                .checkpoint
                                .as_ref()
                                .map_or(0, |checkpoint| checkpoint.sequence),
                        )
            }),
            "Replacement lacks its exact recovery association",
        )?;
        require(
            source.change.operation != replacement.operation,
            "Replacement cannot reuse source approval",
        )?;
        require(
            matches!(
                source.phase.as_str(),
                "review-required" | "superseded-unresolved"
            ) || (source.phase == "recovered"
                && source
                    .checkpoint
                    .as_ref()
                    .is_some_and(|checkpoint| checkpoint.outstanding_intents == 0)
                && state.recovery.as_ref().is_some_and(|recovery| {
                    recovery.reason == crate::backend::RecoveryReason::InitializationPending
                        || (source.accounting_only
                            && recovery.reason == crate::backend::RecoveryReason::RemovalPending)
                })),
            "Source has no recovery barrier",
        )?;
        require(
            replacement.operation.len() <= 256,
            "Operation identifier exceeds its byte budget",
        )?;
        let body = bounded_json(replacement, APPROVAL_LIMIT)?;
        let state_body = bounded_json(state, ENVELOPE_LIMIT)?;
        let transaction = self.connection.transaction()?;
        let revision = current_revision(&transaction, instance)?;
        require(
            replacement.revision == revision && state.revision == revision,
            "Desired revision was superseded",
        )?;
        check_unit_snapshot(&transaction, instance, unit, expected_state)?;
        let actual = read_record(&transaction, instance, unit, &source.change.operation)?
            .ok_or_else(|| Error::Conflict("Recovery source disappeared".into()))?;
        same_record(&actual, source)?;
        if let Some(existing) = read_record(&transaction, instance, unit, &replacement.operation)? {
            require(
                existing.change == *replacement
                    && existing.phase == "prepared"
                    && existing.checkpoint.is_none(),
                "Cannot replace an existing operation approval",
            )?;
        } else {
            transaction.execute(
                "INSERT INTO operations VALUES(?1,?2,?3,?4,'prepared',?5)",
                params![
                    replacement.operation,
                    instance,
                    unit,
                    replacement.revision,
                    body
                ],
            )?;
        }
        transaction.execute("INSERT INTO units VALUES(?1,?2,?3) ON CONFLICT(instance,unit) DO UPDATE SET state=excluded.state",
            params![instance,unit,state_body])?;
        transaction.commit()?;
        Ok(())
    }
    /// Atomically replace authority, or finish a zero-write binding approval.
    pub(crate) fn replace_operation(
        &mut self,
        instance: &str,
        unit: &str,
        update: ReplacementUpdate<'_>,
    ) -> Result<StoredOperation> {
        require(
            matches!(update.phase, "running" | "succeeded" | "in-place-removal"),
            "Invalid replacement phase",
        )?;
        require(
            update.phase != "running"
                || matches!(update.source.phase, "superseded-unresolved" | "recovered"),
            "Replacement cannot leave a second active operation",
        )?;
        let transaction = self.connection.transaction()?;
        let selected = stored_unit(&transaction, instance, unit)?;
        require(
            selected.prepared.as_ref() == Some(update.replacement)
                && selected.recovery.as_ref().is_some_and(|recovery| {
                    recovery.source_operation == update.source.expected.change.operation
                        && recovery.replacement_operation.as_deref()
                            == Some(&update.replacement.operation)
                        && recovery.replacement_sequence
                            == Some(
                                update
                                    .source
                                    .expected
                                    .checkpoint
                                    .as_ref()
                                    .map_or(0, |checkpoint| checkpoint.sequence),
                            )
                }),
            "Recovery replacement approval was superseded",
        )?;
        let replacement = read_record(&transaction, instance, unit, &update.replacement.operation)?
            .ok_or_else(|| Error::Conflict("Recovery replacement is missing".into()))?;
        require(
            (replacement.phase == "prepared"
                || (update.phase == "succeeded"
                    && replacement.phase == "in-place-removal"
                    && selected
                        .recovery
                        .as_ref()
                        .is_some_and(|recovery| recovery.removal_attempted)))
                && replacement.change == *update.replacement
                && replacement.checkpoint.is_none(),
            "Recovery replacement is no longer prepared",
        )?;
        let revision = current_revision(&transaction, instance)?;
        let completing_attempt = update.phase == "succeeded"
            && replacement.phase == "in-place-removal"
            && update.source.expected.accounting_only
            && selected
                .recovery
                .as_ref()
                .is_some_and(|recovery| recovery.removal_attempted);
        require(
            completing_attempt || revision == update.replacement.revision,
            "Desired revision was superseded",
        )?;
        if update.phase == "in-place-removal" {
            require(
                !selected
                    .recovery
                    .as_ref()
                    .is_some_and(|recovery| recovery.removal_attempted)
                    && update
                        .source
                        .state
                        .recovery
                        .as_ref()
                        .is_some_and(|recovery| recovery.removal_attempted)
                    && update.source.state.prepared.as_ref() == Some(update.replacement),
                "Invalid in-place removal transition",
            )?;
        }
        let mut source = write_operation_state(&transaction, instance, unit, update.source)?;
        if update.phase == "in-place-removal" {
            source.accounting_only = true;
            let checkpoint = source
                .checkpoint
                .clone()
                .ok_or_else(|| Error::Invalid("Missing removal source checkpoint".into()))?;
            let binding = source
                .binding
                .clone()
                .ok_or_else(|| Error::Invalid("Missing removal source binding".into()))?;
            let approval = bounded_json(&source.change, APPROVAL_LIMIT)?;
            let envelope = Envelope {
                journal_version: 1,
                accounting_only: true,
                binding,
                approved_sha256: files::hash(approval.as_bytes()),
                prepared: source.change.clone(),
                checkpoint,
            };
            let encoded = bounded_json(&envelope, ENVELOPE_LIMIT)?;
            transaction.execute(
                "UPDATE operations SET change=?1 WHERE id=?2 AND instance=?3 AND unit=?4",
                params![encoded, source.change.operation, instance, unit],
            )?;
        }
        if update.phase == "running" {
            let active: i64 = transaction.query_row("SELECT count(*) FROM (SELECT id FROM operations WHERE instance=?1 AND unit=?2 AND phase IN ('running','review-required','superseded-unresolved') AND phase!='superseded-unresolved' LIMIT 2)", params![instance,unit], |row| row.get(0))?;
            require(active == 0, "Another operation is active")?;
        }
        transaction.execute(
            "UPDATE operations SET phase=?1 WHERE id=?2 AND instance=?3 AND unit=?4",
            params![update.phase, update.replacement.operation, instance, unit],
        )?;
        transaction.commit()?;
        Ok(source)
    }
}

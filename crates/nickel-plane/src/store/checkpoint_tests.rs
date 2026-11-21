#![allow(clippy::unwrap_used)]
use super::*;
use crate::backend::ChangeSummary;
use serde_json::json;

fn fixture() -> (tempfile::TempDir, Store, PreparedChange) {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("store")).unwrap();
    store
        .connection
        .execute(
            "INSERT INTO instances VALUES('instance','revision','{}',0)",
            [],
        )
        .unwrap();
    let change = PreparedChange {
        operation: "operation".into(),
        revision: "revision".into(),
        artifact_sha256: "artifact".into(),
        target_sha256: "execution".into(),
        summary: ChangeSummary::default(),
        payload: json!({"phase":"create-initialize-v1"}),
        preconditions: json!({}),
    };
    store
        .record("instance", "unit", &change, "running")
        .unwrap();
    (temp, store, change)
}

#[test]
fn running_reads_checkpoint_envelope_without_changing_approval() {
    let (_temp, store, change) = fixture();
    let envelope = json!({
        "journalVersion":1,
        "binding":{"instance":"instance","unit":"unit","targetReference":"cluster","targetIdentitySha256":"identity"},
        "approvedSha256":files::hash(&serde_json::to_vec(&change).unwrap()),
        "prepared":change,
        "checkpoint":{"sequence":1,"outstandingIntents":0,"data":{"objects":{}}}
    });
    store
        .connection
        .execute(
            "UPDATE operations SET change=?1 WHERE id='operation'",
            [envelope.to_string()],
        )
        .unwrap();
    assert_eq!(store.running("instance", "unit").unwrap(), Some(change));
}

#[test]
fn record_never_rebinds_an_existing_operation_id() {
    let (_temp, store, mut change) = fixture();
    change.artifact_sha256 = "different-artifact".into();
    assert!(store
        .record("instance", "unit", &change, "prepared")
        .is_err());
    assert_eq!(
        store
            .running("instance", "unit")
            .unwrap()
            .unwrap()
            .artifact_sha256,
        "artifact"
    );
}

fn binding() -> operations::OperationBinding {
    operations::OperationBinding {
        instance: "instance".into(),
        unit: "unit".into(),
        target_reference: "cluster".into(),
        target_identity_sha256: "identity".into(),
    }
}

#[test]
fn checkpoints_commit_inventory_atomically_and_unchanged_reads_keep_sequence() {
    let (temp, mut store, change) = fixture();
    let before = store.running_record("instance", "unit").unwrap().unwrap();
    let mut state = UnitState {
        revision: "revision".into(),
        inventory: json!({"uid":"known"}),
        ..Default::default()
    };
    let progress = json!({"stage":"created","uid":"known"});
    let next = store
        .save_operation_state(
            "instance",
            "unit",
            operations::OperationUpdate {
                expected_state: store.unit("instance", "unit").unwrap().as_ref(),
                expected: &before,
                binding: &binding(),
                data: &progress,
                state: &state,
                outstanding_intents: 0,
                phase: "running",
            },
        )
        .unwrap();
    assert_eq!(next.checkpoint.as_ref().unwrap().sequence, 1);
    let repeated = store
        .save_operation_state(
            "instance",
            "unit",
            operations::OperationUpdate {
                expected_state: store.unit("instance", "unit").unwrap().as_ref(),
                expected: &next,
                binding: &binding(),
                data: &progress,
                state: &state,
                outstanding_intents: 0,
                phase: "running",
            },
        )
        .unwrap();
    assert_eq!(repeated.checkpoint.as_ref().unwrap().sequence, 1);
    assert!(store
        .save_operation_state(
            "instance",
            "unit",
            operations::OperationUpdate {
                expected_state: store.unit("instance", "unit").unwrap().as_ref(),
                expected: &before,
                binding: &binding(),
                data: &json!({"changed":true}),
                state: &state,
                outstanding_intents: 0,
                phase: "running",
            }
        )
        .is_err());
    store.connection.execute_batch("CREATE TEMP TRIGGER fail_unit_update BEFORE UPDATE ON units BEGIN SELECT RAISE(ABORT,'injected unit write failure'); END;").unwrap();
    state.inventory = json!({"uid":"must-not-persist"});
    assert!(store
        .save_operation_state(
            "instance",
            "unit",
            operations::OperationUpdate {
                expected_state: store.unit("instance", "unit").unwrap().as_ref(),
                expected: &next,
                binding: &binding(),
                data: &json!({"stage":"complete"}),
                state: &state,
                outstanding_intents: 0,
                phase: "succeeded",
            }
        )
        .is_err());
    assert_eq!(
        store.unit("instance", "unit").unwrap().unwrap().inventory,
        json!({"uid":"known"})
    );
    assert_eq!(
        store
            .running_record("instance", "unit")
            .unwrap()
            .unwrap()
            .checkpoint
            .unwrap()
            .sequence,
        1
    );
    drop(store);
    let store = Store::open(&temp.path().join("store")).unwrap();
    assert_eq!(store.running("instance", "unit").unwrap(), Some(change));
    assert_eq!(
        store.unit("instance", "unit").unwrap().unwrap().inventory,
        json!({"uid":"known"})
    );
}

#[test]
fn evidence_write_uses_current_state_revision_without_rebinding_old_operation() {
    let (_temp, mut store, change) = fixture();
    let before = store.running_record("instance", "unit").unwrap().unwrap();
    store
        .connection
        .execute(
            "UPDATE instances SET revision='new-revision' WHERE id='instance'",
            [],
        )
        .unwrap();
    let state = UnitState {
        revision: "new-revision".into(),
        inventory: json!({"uid":"learned"}),
        ..Default::default()
    };
    let next = store
        .save_operation_state(
            "instance",
            "unit",
            operations::OperationUpdate {
                expected_state: store.unit("instance", "unit").unwrap().as_ref(),
                expected: &before,
                binding: &binding(),
                data: &json!({"uid":"learned"}),
                state: &state,
                outstanding_intents: 0,
                phase: "review-required",
            },
        )
        .unwrap();
    assert_eq!(next.change, change);
    assert_eq!(
        store.unit("instance", "unit").unwrap().unwrap().revision,
        "new-revision"
    );
}

fn make_unresolved(store: &mut Store, change: &PreparedChange) {
    store
        .record("instance", "unit", change, "prepared")
        .unwrap();
    let record = store
        .operation_record("instance", "unit", &change.operation)
        .unwrap()
        .unwrap();
    let previous = store.unit("instance", "unit").unwrap();
    let state = previous.clone().unwrap_or(UnitState {
        revision: "revision".into(),
        ..Default::default()
    });
    store
        .save_operation_state(
            "instance",
            "unit",
            operations::OperationUpdate {
                expected: &record,
                expected_state: previous.as_ref(),
                binding: &binding(),
                data: &json!({"issued":true}),
                outstanding_intents: 1,
                state: &state,
                phase: "superseded-unresolved",
            },
        )
        .unwrap();
}
#[test]
fn sixteen_unresolved_records_are_separate_from_one_active_and_retire() {
    let (_temp, mut store, change) = fixture();
    for i in 0..16 {
        let mut old = change.clone();
        old.operation = format!("old-{i}");
        make_unresolved(&mut store, &old);
    }
    assert!(store.running_record("instance", "unit").unwrap().is_some());
    assert_eq!(store.unresolved_ids("instance", "unit").unwrap().len(), 16);
    let mut extra = change.clone();
    extra.operation = "overflow".into();
    store.connection.execute("INSERT INTO operations VALUES(?1,'instance','unit','revision','superseded-unresolved',?2)", params![extra.operation,serde_json::to_string(&extra).unwrap()]).unwrap();
    assert!(store.unresolved_ids("instance", "unit").is_err());
    store
        .connection
        .execute("DELETE FROM operations WHERE id='overflow'", [])
        .unwrap();
    let record = store
        .operation_record("instance", "unit", "old-0")
        .unwrap()
        .unwrap();
    let state = store.unit("instance", "unit").unwrap().unwrap();
    store
        .save_operation_state(
            "instance",
            "unit",
            operations::OperationUpdate {
                expected: &record,
                expected_state: Some(&state),
                binding: &binding(),
                data: &json!({"bound":true}),
                outstanding_intents: 0,
                state: &state,
                phase: "recovered",
            },
        )
        .unwrap();
    assert_eq!(store.unresolved_ids("instance", "unit").unwrap().len(), 15);
}

#[test]
fn operation_byte_guards_do_not_skip_oversized_unicode_sql_text() {
    let (_temp, store, _) = fixture();
    let count = (operations::APPROVAL_LIMIT + operations::PROGRESS_LIMIT + 64 * 1024) / 2 + 1;
    store
        .connection
        .execute(
            "UPDATE operations SET change=replace(hex(zeroblob(?1)),'00','é') WHERE id='operation'",
            [count as i64],
        )
        .unwrap();
    let error = store.running("instance", "unit").unwrap_err().to_string();
    assert!(error.contains("byte budget"), "{error}");
    assert!(operations::bounded_json(&"é", 3).is_err());
    assert_eq!(operations::bounded_json(&"e", 3).unwrap(), "\"e\"");
}

#[test]
fn outstanding_intents_are_capped_across_active_and_superseded_rows() {
    let (_temp, mut store, change) = fixture();
    let state = UnitState {
        revision: "revision".into(),
        ..Default::default()
    };
    let before = store.running_record("instance", "unit").unwrap().unwrap();
    let old = store
        .save_operation_state(
            "instance",
            "unit",
            operations::OperationUpdate {
                expected_state: store.unit("instance", "unit").unwrap().as_ref(),
                expected: &before,
                binding: &binding(),
                data: &json!({"intent":"issued"}),
                outstanding_intents: 255,
                state: &state,
                phase: "superseded-unresolved",
            },
        )
        .unwrap();
    let mut active = change.clone();
    active.operation = "new-operation".into();
    store
        .record("instance", "unit", &active, "running")
        .unwrap();
    let current = store.running_record("instance", "unit").unwrap().unwrap();
    assert!(store
        .save_operation_state(
            "instance",
            "unit",
            operations::OperationUpdate {
                expected_state: store.unit("instance", "unit").unwrap().as_ref(),
                expected: &current,
                binding: &binding(),
                data: &json!({"intent":"issued"}),
                outstanding_intents: 2,
                state: &state,
                phase: "running",
            }
        )
        .is_err());
    let current = store
        .save_operation_state(
            "instance",
            "unit",
            operations::OperationUpdate {
                expected_state: store.unit("instance", "unit").unwrap().as_ref(),
                expected: &current,
                binding: &binding(),
                data: &json!({"intent":"issued"}),
                outstanding_intents: 1,
                state: &state,
                phase: "running",
            },
        )
        .unwrap();
    assert!(store
        .save_operation_state(
            "instance",
            "unit",
            operations::OperationUpdate {
                expected_state: store.unit("instance", "unit").unwrap().as_ref(),
                expected: &old,
                binding: &binding(),
                data: &json!({"intent":"unresolved"}),
                outstanding_intents: 255,
                state: &state,
                phase: "recovered",
            }
        )
        .is_err());
    store
        .save_operation_state(
            "instance",
            "unit",
            operations::OperationUpdate {
                expected_state: store.unit("instance", "unit").unwrap().as_ref(),
                expected: &old,
                binding: &binding(),
                data: &json!({"intent":"bound"}),
                outstanding_intents: 0,
                state: &state,
                phase: "recovered",
            },
        )
        .unwrap();
    assert!(store.unresolved_ids("instance", "unit").unwrap().is_empty());
    store
        .save_operation_state(
            "instance",
            "unit",
            operations::OperationUpdate {
                expected_state: store.unit("instance", "unit").unwrap().as_ref(),
                expected: &current,
                binding: &binding(),
                data: &json!({"intent":"issued"}),
                outstanding_intents: 256,
                state: &state,
                phase: "running",
            },
        )
        .unwrap();
}

#[test]
fn explicit_envelopes_never_fall_back_to_legacy_or_cross_unit_approvals() {
    let (_temp, mut store, _) = fixture();
    let state = UnitState {
        revision: "revision".into(),
        ..Default::default()
    };
    let before = store.running_record("instance", "unit").unwrap().unwrap();
    store
        .save_operation_state(
            "instance",
            "unit",
            operations::OperationUpdate {
                expected_state: store.unit("instance", "unit").unwrap().as_ref(),
                expected: &before,
                binding: &binding(),
                data: &json!({}),
                outstanding_intents: 0,
                state: &state,
                phase: "running",
            },
        )
        .unwrap();
    let body: String = store
        .connection
        .query_row(
            "SELECT change FROM operations WHERE id='operation'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    for (pointer, replacement) in [
        ("/journalVersion", json!(2)),
        ("/binding/unit", json!("another")),
        ("/prepared/operation", json!("another")),
        ("/prepared/revision", json!("another")),
        ("/approvedSha256", json!("wrong")),
        ("/checkpoint/outstandingIntents", json!(257)),
    ] {
        let mut altered: serde_json::Value = serde_json::from_str(&body).unwrap();
        *altered.pointer_mut(pointer).unwrap() = replacement;
        store
            .connection
            .execute(
                "UPDATE operations SET change=?1 WHERE id='operation'",
                [altered.to_string()],
            )
            .unwrap();
        assert!(
            store.running_record("instance", "unit").is_err(),
            "{pointer}"
        );
    }
}

#[test]
fn unfinished_queries_use_bounded_recovery_index() {
    let (_temp, store, _) = fixture();
    for suffix in [
        "AND phase!='superseded-unresolved' LIMIT 2",
        "AND phase='superseded-unresolved' LIMIT 17",
    ] {
        let sql = format!("EXPLAIN QUERY PLAN SELECT id FROM operations WHERE instance=?1 AND unit=?2 AND phase IN ('running','review-required','superseded-unresolved') {suffix}");
        let mut query = store.connection.prepare(&sql).unwrap();
        let plans: Vec<String> = query
            .query_map(params!["instance", "unit"], |row| row.get(3))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert!(
            plans
                .iter()
                .any(|plan| plan.contains("operations_recovery")),
            "{plans:?}"
        );
        assert!(
            !plans.iter().any(|plan| plan.contains("SCAN operations")),
            "{plans:?}"
        );
    }
}

#[test]
fn legacy_finish_cannot_discard_a_checkpointed_unresolved_intent() {
    let (_temp, mut store, _) = fixture();
    let state = UnitState {
        revision: "revision".into(),
        ..Default::default()
    };
    let before = store.running_record("instance", "unit").unwrap().unwrap();
    store
        .save_operation_state(
            "instance",
            "unit",
            operations::OperationUpdate {
                expected_state: store.unit("instance", "unit").unwrap().as_ref(),
                expected: &before,
                binding: &binding(),
                data: &json!({"intent":"issued"}),
                outstanding_intents: 1,
                state: &state,
                phase: "review-required",
            },
        )
        .unwrap();
    assert!(store.finish("operation", "recovered").is_err());
    assert!(store.running_record("instance", "unit").unwrap().is_some());
}

#[test]
fn binding_finalization_is_atomic_with_source_inventory_and_exact_approval() {
    let (_temp, mut store, change) = fixture();
    let mut state = UnitState {
        revision: "revision".into(),
        ..Default::default()
    };
    let before = store.running_record("instance", "unit").unwrap().unwrap();
    let source = store
        .save_operation_state(
            "instance",
            "unit",
            operations::OperationUpdate {
                expected_state: store.unit("instance", "unit").unwrap().as_ref(),
                expected: &before,
                binding: &binding(),
                data: &json!({"candidate":"uid"}),
                outstanding_intents: 1,
                state: &state,
                phase: "review-required",
            },
        )
        .unwrap();
    let mut replacement = change.clone();
    replacement.operation = "binding".into();
    state.prepared = Some(replacement.clone());
    state.recovery = Some(crate::reconcile::RecoveryState {
        source_operation: change.operation.clone(),
        reason: crate::backend::RecoveryReason::UnrecordedIdentity,
        outstanding_intents: 1,
        replacement_operation: Some(replacement.operation.clone()),
        removal_attempted: false,
        replacement_sequence: Some(1),
    });
    store
        .associate_replacement(
            "instance",
            "unit",
            &source,
            &replacement,
            &state,
            store.unit("instance", "unit").unwrap().as_ref(),
        )
        .unwrap();
    store.connection.execute_batch("CREATE TEMP TRIGGER fail_binding BEFORE UPDATE ON operations WHEN NEW.id='binding' AND NEW.phase='succeeded' BEGIN SELECT RAISE(ABORT,'injected binding finalization failure'); END;").unwrap();
    let mut next = state.clone();
    next.inventory = json!({"uid":"uid"});
    next.prepared = None;
    next.recovery = None;
    let attempt = |store: &mut Store| {
        store.replace_operation(
            "instance",
            "unit",
            operations::ReplacementUpdate {
                source: operations::OperationUpdate {
                    expected_state: Some(&state),
                    expected: &source,
                    binding: &binding(),
                    data: &json!({"bound":"uid"}),
                    outstanding_intents: 0,
                    state: &next,
                    phase: "recovered",
                },
                replacement: &replacement,
                phase: "succeeded",
            },
        )
    };
    assert!(attempt(&mut store).is_err());
    assert!(store
        .unit("instance", "unit")
        .unwrap()
        .unwrap()
        .inventory
        .is_null());
    assert_eq!(
        store
            .operation_record("instance", "unit", "binding")
            .unwrap()
            .unwrap()
            .phase,
        "prepared"
    );
    assert_eq!(
        store
            .running_record("instance", "unit")
            .unwrap()
            .unwrap()
            .checkpoint
            .unwrap()
            .sequence,
        1
    );
    store
        .connection
        .execute_batch("DROP TRIGGER fail_binding")
        .unwrap();
    attempt(&mut store).unwrap();
    assert_eq!(
        store.unit("instance", "unit").unwrap().unwrap().inventory,
        json!({"uid":"uid"})
    );
    assert!(store.running_record("instance", "unit").unwrap().is_none());
    assert_eq!(
        store
            .operation_record("instance", "unit", "binding")
            .unwrap()
            .unwrap()
            .phase,
        "succeeded"
    );
    assert!(attempt(&mut store).is_err());
}

#[test]
fn direct_record_admission_cannot_exceed_recovery_caps() {
    let (_temp, mut store, change) = fixture();
    let mut extra = change.clone();
    extra.operation = "second-active".into();
    assert!(store.record("instance", "unit", &extra, "running").is_err());
    for i in 0..16 {
        let mut old = change.clone();
        old.operation = format!("unresolved-{i}");
        make_unresolved(&mut store, &old);
    }
    extra.operation = "seventeenth".into();
    assert!(store
        .record("instance", "unit", &extra, "superseded-unresolved")
        .is_err());
    assert!(store.finish("operation", "superseded-unresolved").is_err());
    assert_eq!(store.unresolved_ids("instance", "unit").unwrap().len(), 16);
    assert_eq!(
        store
            .running_record("instance", "unit")
            .unwrap()
            .unwrap()
            .change
            .operation,
        "operation"
    );
}

#[test]
fn interleaved_sources_cannot_overwrite_newer_shared_inventory() {
    let (_temp, mut store, change) = fixture();
    let state = UnitState {
        revision: "revision".into(),
        ..Default::default()
    };
    let mut second = change.clone();
    second.operation = "second".into();
    store
        .record("instance", "unit", &second, "prepared")
        .unwrap();
    let second = store
        .operation_record("instance", "unit", "second")
        .unwrap()
        .unwrap();
    let second = store
        .save_operation_state(
            "instance",
            "unit",
            operations::OperationUpdate {
                expected_state: store.unit("instance", "unit").unwrap().as_ref(),
                expected: &second,
                binding: &binding(),
                data: &json!({"issued":true}),
                outstanding_intents: 1,
                state: &state,
                phase: "superseded-unresolved",
            },
        )
        .unwrap();
    let first = store.running_record("instance", "unit").unwrap().unwrap();
    let mut learned = state.clone();
    learned.inventory = json!({"uid":"preserved"});
    store
        .save_operation_state(
            "instance",
            "unit",
            operations::OperationUpdate {
                expected_state: store.unit("instance", "unit").unwrap().as_ref(),
                expected: &first,
                binding: &binding(),
                data: &json!({"uid":"preserved"}),
                outstanding_intents: 0,
                state: &learned,
                phase: "running",
            },
        )
        .unwrap();
    assert!(store
        .save_operation_state(
            "instance",
            "unit",
            operations::OperationUpdate {
                expected_state: Some(&state),
                expected: &second,
                binding: &binding(),
                data: &json!({"issued":true}),
                outstanding_intents: 1,
                state: &state,
                phase: "superseded-unresolved",
            }
        )
        .is_err());
    assert_eq!(
        store.unit("instance", "unit").unwrap().unwrap().inventory,
        json!({"uid":"preserved"})
    );
}

#[test]
fn raw_duplicate_operation_and_envelope_keys_are_refused() {
    let (_temp, store, change) = fixture();
    let legacy = serde_json::to_string(&change).unwrap();
    let duplicate = legacy.replacen('{', "{\"operation\":\"wrong\",", 1);
    store
        .connection
        .execute(
            "UPDATE operations SET change=?1 WHERE id='operation'",
            [duplicate],
        )
        .unwrap();
    assert!(store.running("instance", "unit").is_err());
    let envelope = json!({"journalVersion":1,"binding":{"instance":"instance","unit":"unit","targetReference":"cluster","targetIdentitySha256":"identity"},
        "approvedSha256":files::hash(legacy.as_bytes()),"prepared":change,"checkpoint":{"sequence":1,"outstandingIntents":0,"data":{}}});
    let duplicate = envelope
        .to_string()
        .replacen('{', "{\"journalVersion\":2,", 1);
    store
        .connection
        .execute(
            "UPDATE operations SET change=?1 WHERE id='operation'",
            [duplicate],
        )
        .unwrap();
    assert!(store.running("instance", "unit").is_err());
}

#[test]
fn unit_evidence_only_changes_advance_sequence_and_missing_is_distinct() {
    let (_temp, mut store, _) = fixture();
    let state = UnitState {
        revision: "revision".into(),
        ..Default::default()
    };
    let original = store.running_record("instance", "unit").unwrap().unwrap();
    assert!(store
        .save_operation_state(
            "instance",
            "unit",
            operations::OperationUpdate {
                expected: &original,
                expected_state: Some(&state),
                binding: &binding(),
                data: &json!({}),
                outstanding_intents: 0,
                state: &state,
                phase: "running",
            }
        )
        .is_err());
    let first = store
        .save_operation_state(
            "instance",
            "unit",
            operations::OperationUpdate {
                expected: &original,
                expected_state: None,
                binding: &binding(),
                data: &json!({}),
                outstanding_intents: 0,
                state: &state,
                phase: "running",
            },
        )
        .unwrap();
    let mut learned = state.clone();
    learned.inventory = json!({"uid":"learned"});
    let second = store
        .save_operation_state(
            "instance",
            "unit",
            operations::OperationUpdate {
                expected: &first,
                expected_state: Some(&state),
                binding: &binding(),
                data: &json!({}),
                outstanding_intents: 0,
                state: &learned,
                phase: "running",
            },
        )
        .unwrap();
    assert_eq!(second.checkpoint.as_ref().unwrap().sequence, 2);
    assert!(store
        .save_operation_state(
            "instance",
            "unit",
            operations::OperationUpdate {
                expected: &first,
                expected_state: Some(&state),
                binding: &binding(),
                data: &json!({}),
                outstanding_intents: 0,
                state: &state,
                phase: "running",
            }
        )
        .is_err());
    let repeated = store
        .save_operation_state(
            "instance",
            "unit",
            operations::OperationUpdate {
                expected: &second,
                expected_state: Some(&learned),
                binding: &binding(),
                data: &json!({}),
                outstanding_intents: 0,
                state: &learned,
                phase: "running",
            },
        )
        .unwrap();
    assert_eq!(repeated.checkpoint.unwrap().sequence, 2);
}

#[test]
fn oversized_metadata_and_unit_snapshot_are_refused_before_decoding() {
    let (_temp, mut store, change) = fixture();
    let original = store.running_record("instance", "unit").unwrap().unwrap();
    let state = UnitState {
        revision: "revision".into(),
        ..Default::default()
    };
    store
        .connection
        .execute(
            "UPDATE instances SET revision=hex(zeroblob(1024)) WHERE id='instance'",
            [],
        )
        .unwrap();
    assert!(store.ensure_revision("instance", "revision").is_err());
    assert!(store
        .record("instance", "unit", &change, "prepared")
        .is_err());
    store
        .connection
        .execute(
            "UPDATE instances SET revision='revision' WHERE id='instance'",
            [],
        )
        .unwrap();
    store
        .connection
        .execute(
            "UPDATE operations SET phase=hex(zeroblob(1024)) WHERE id='operation'",
            [],
        )
        .unwrap();
    assert!(store
        .operation_record("instance", "unit", "operation")
        .unwrap_err()
        .to_string()
        .contains("byte budget"));
    store.connection.execute("UPDATE operations SET phase='running',revision=hex(zeroblob(1024)) WHERE id='operation'",[]).unwrap();
    assert!(store
        .operation_record("instance", "unit", "operation")
        .is_err());
    store
        .connection
        .execute(
            "UPDATE operations SET revision='revision' WHERE id='operation'",
            [],
        )
        .unwrap();
    store
        .connection
        .execute(
            "INSERT INTO units VALUES('instance','unit',hex(zeroblob(?1)))",
            [(operations::APPROVAL_LIMIT + operations::PROGRESS_LIMIT) as i64],
        )
        .unwrap();
    assert!(store.unit("instance", "unit").is_err());
    assert!(store
        .save_operation_state(
            "instance",
            "unit",
            operations::OperationUpdate {
                expected: &original,
                expected_state: None,
                binding: &binding(),
                data: &json!({}),
                outstanding_intents: 0,
                state: &state,
                phase: "running",
            }
        )
        .is_err());
}

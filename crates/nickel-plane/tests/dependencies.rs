#![allow(clippy::unwrap_used)]
use nickel_plane::{
    backend::*,
    composition::{Compiled, CompiledUnit, Validator},
    model::*,
    reconcile::{reconcile, Options, Registry},
    store::Store,
    Error, Result,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
#[derive(Default)]
struct World {
    values: BTreeMap<String, Value>,
    writes: Vec<String>,
    deletes: Vec<String>,
    removal_plans: Vec<String>,
    removal_executions: Vec<String>,
    sensitive: bool,
    missing: bool,
    fail: bool,
    fail_observe: bool,
    fail_after_write: bool,
    observations: Vec<String>,
    fail_after_delete: bool,
    ready: bool,
    readiness: Option<Readiness>,
    cancel: Option<Arc<AtomicBool>>,
}
struct Fake {
    kind: BackendKind,
    world: Arc<Mutex<World>>,
}
impl Backend for Fake {
    fn kind(&self) -> BackendKind {
        self.kind
    }
    fn fingerprint(&self) -> Result<String> {
        Ok(format!("{:?}", self.kind))
    }
    fn observe(&mut self, c: &Context) -> Result<Observation> {
        let mut w = self.world.lock().unwrap();
        w.observations.push(c.unit_id.clone());
        if w.fail_observe {
            return Err(Error::Backend("observation unavailable".into()));
        }
        let value = w.values.get(&c.unit_id);
        Ok(Observation {
            exists: value.is_some(),
            drifted: value != Some(&c.artifact),
            readiness: w.readiness.unwrap_or(if w.ready {
                Readiness::Ready
            } else {
                Readiness::Unsupported
            }),
            outputs: if w.missing {
                BTreeMap::new()
            } else {
                BTreeMap::from([(
                    "message".into(),
                    OutputValue {
                        value: value.and_then(|v| v.get("message")).cloned(),
                        sensitive: w.sensitive,
                        reference: Some("protected://value".into()),
                    },
                )])
            },
            inventory: value.cloned().unwrap_or(Value::Null),
            preconditions: json!(value),
        })
    }
    fn prepare_change(&mut self, c: &Context, o: &Observation, id: &str) -> Result<PreparedChange> {
        Ok(PreparedChange {
            operation: id.into(),
            revision: c.revision.clone(),
            artifact_sha256: c.artifact_sha256.clone(),
            target_sha256: self.fingerprint()?,
            summary: ChangeSummary {
                creates: usize::from(!o.exists),
                updates: usize::from(o.exists),
                ..Default::default()
            },
            payload: Value::Null,
            preconditions: o.preconditions.clone(),
        })
    }
    fn execute_change(&mut self, c: &Context, _: &PreparedChange) -> Result<Observation> {
        let mut w = self.world.lock().unwrap();
        if w.fail {
            return Err(Error::Backend("injected".into()));
        }
        w.writes.push(c.unit_id.clone());
        w.values.insert(c.unit_id.clone(), c.artifact.clone());
        if w.fail_after_write {
            return Err(Error::Backend("interrupted mutation response".into()));
        }
        if let Some(cancel) = &w.cancel {
            cancel.store(true, Ordering::SeqCst)
        }
        drop(w);
        self.observe(c)
    }
    fn remove(&mut self, c: &Context, _: &Observation) -> Result<Observation> {
        let mut w = self.world.lock().unwrap();
        w.deletes.push(c.unit_id.clone());
        w.values.remove(&c.unit_id);
        if w.fail_after_delete {
            return Err(Error::Backend("interrupted deletion response".into()));
        }
        Ok(Observation::default())
    }
    fn prepare_removal(
        &mut self,
        c: &Context,
        o: &Observation,
        id: &str,
    ) -> Result<PreparedChange> {
        self.world.lock().unwrap().removal_plans.push(id.into());
        let mut plan = self.prepare_change(c, o, id)?;
        plan.summary = ChangeSummary {
            deletes: usize::from(o.exists),
            ..Default::default()
        };
        plan.payload = json!({"remove": true});
        Ok(plan)
    }
    fn execute_removal(&mut self, c: &Context, plan: &PreparedChange) -> Result<Observation> {
        assert_eq!(plan.payload["remove"], true);
        self.world
            .lock()
            .unwrap()
            .removal_executions
            .push(plan.operation.clone());
        self.remove(c, &Observation::default())
    }
}
struct Validate;
impl Validator for Validate {
    fn validate(&self, _: &Unit, v: &Value, b: &CompiledUnit) -> Result<CompiledUnit> {
        let mut b = b.clone();
        b.artifact = v.clone();
        Ok(b)
    }
}
type Fixture = (
    tempfile::TempDir,
    Store,
    Compiled,
    Arc<Mutex<World>>,
    Registry,
);
fn fixture() -> Result<Fixture> {
    let tmp = tempfile::tempdir()?;
    let store = Store::open(&tmp.path().join("store"))?;
    let instance: Instance = serde_json::from_value(
        json!({"apiVersion":"nickel-plane/v1","id":"mixed","composition":{"name":"demo","version":"1"},"policy":{"mode":"manage","deletion":"delete","allowDelete":true},"units":{
        "z-infra":{"backend":"opentofu","target":"local","schema":"local","configuration":{"message":"first"},"outputs":{"message":{"pointer":"/outputs/message"}}},
        "a-app":{"backend":"kubernetes","target":"cluster","configuration":{"message":"pending"},"bindings":[{"fromUnit":"z-infra","output":"message","to":"/message","condition":"applied"}]}
        }}),
    )?;
    let units = instance
        .units
        .iter()
        .map(|(id, u)| {
            (
                id.clone(),
                CompiledUnit {
                    artifact: u.configuration.clone(),
                    receipt: Value::Null,
                    engine_version: None,
                    provider_lock: None,
                },
            )
        })
        .collect();
    let mut c = Compiled {
        version: 1,
        revision: String::new(),
        instance,
        inputs: Value::Null,
        units,
    };
    c.revision = c.digest()?;
    let world = Arc::new(Mutex::new(World::default()));
    let registry = Registry::from([
        (
            "local".into(),
            Box::new(Fake {
                kind: BackendKind::OpenTofu,
                world: world.clone(),
            }) as Box<dyn Backend>,
        ),
        (
            "cluster".into(),
            Box::new(Fake {
                kind: BackendKind::Kubernetes,
                world: world.clone(),
            }) as Box<dyn Backend>,
        ),
    ]);
    Ok((tmp, store, c, world, registry))
}
#[test]
fn restart_between_backends_resolves_only_current_applied_outputs() -> Result<()> {
    let (tmp, mut s, mut c, w, mut r) = fixture()?;
    s.submit(&c)?;
    let cancel = Arc::new(AtomicBool::new(false));
    w.lock().unwrap().cancel = Some(cancel.clone());
    assert!(matches!(
        reconcile(
            &mut s,
            &c,
            &Validate,
            &mut r,
            &Options {
                cancelled: cancel.clone(),
                ..Default::default()
            }
        ),
        Err(Error::Cancelled)
    ));
    assert_eq!(w.lock().unwrap().writes, vec!["z-infra"]);
    drop(s);
    let mut s = Store::open(&tmp.path().join("store"))?;
    w.lock().unwrap().cancel = None;
    reconcile(&mut s, &c, &Validate, &mut r, &Options::default())?;
    assert_eq!(w.lock().unwrap().writes, vec!["z-infra", "a-app"]);
    assert_eq!(w.lock().unwrap().values["a-app"]["message"], "first");
    c.instance.units.get_mut("z-infra").unwrap().configuration["message"] = json!("next");
    c.units.get_mut("z-infra").unwrap().artifact = json!({"message":"next"});
    c.revision = c.digest()?;
    s.submit(&c)?;
    w.lock().unwrap().fail = true;
    reconcile(&mut s, &c, &Validate, &mut r, &Options::default())?;
    assert!(!s.unit("mixed", "a-app")?.unwrap().applied);
    assert!(s.unit("mixed", "a-app")?.unwrap().outputs.is_empty());
    assert_eq!(w.lock().unwrap().writes.len(), 2);
    Ok(())
}
#[test]
fn missing_sensitive_and_unsupported_ready_outputs_never_satisfy_dependencies() -> Result<()> {
    for case in ["missing", "sensitive", "ready"] {
        let (_tmp, mut s, mut c, w, mut r) = fixture()?;
        match case {
            "missing" => w.lock().unwrap().missing = true,
            "sensitive" => w.lock().unwrap().sensitive = true,
            _ => c.instance.units.get_mut("a-app").unwrap().bindings[0].condition = Gate::Ready,
        }
        c.revision = c.digest()?;
        s.submit(&c)?;
        reconcile(&mut s, &c, &Validate, &mut r, &Options::default())?;
        assert_eq!(w.lock().unwrap().writes, vec!["z-infra"]);
        let app = s.unit("mixed", "a-app")?.unwrap();
        assert!(!app.applied);
        if case == "sensitive" {
            assert!(app.failure.is_some());
            assert!(!s
                .unit("mixed", "z-infra")?
                .unwrap()
                .public()
                .to_string()
                .contains("first"));
        } else {
            assert!(app.waiting.is_some());
        }
    }
    Ok(())
}
#[test]
fn deletion_uses_reverse_dependencies_and_does_not_replay_completed_removal() -> Result<()> {
    let (_tmp, mut s, c, w, mut r) = fixture()?;
    s.submit(&c)?;
    reconcile(&mut s, &c, &Validate, &mut r, &Options::default())?;
    s.mark_removed("mixed", true)?;
    reconcile(&mut s, &c, &Validate, &mut r, &Options::default())?;
    reconcile(&mut s, &c, &Validate, &mut r, &Options::default())?;
    assert_eq!(w.lock().unwrap().deletes, vec!["a-app", "z-infra"]);
    Ok(())
}
#[test]
fn retained_units_are_never_deleted_and_retry_is_bounded() -> Result<()> {
    let (_tmp, mut s, c, w, mut r) = fixture()?;
    s.submit(&c)?;
    w.lock().unwrap().fail = true;
    for now in [1, 10, 100, 1000, 10000, 100000] {
        reconcile(
            &mut s,
            &c,
            &Validate,
            &mut r,
            &Options {
                now,
                ..Default::default()
            },
        )?;
    }
    assert_eq!(s.unit("mixed", "z-infra")?.unwrap().attempts, 5);
    s.mark_removed("mixed", false)?;
    reconcile(&mut s, &c, &Validate, &mut r, &Options::default())?;
    assert!(w.lock().unwrap().deletes.is_empty());
    Ok(())
}

#[test]
fn continuous_retry_survives_long_outages_and_restart_for_apply_and_removal() -> Result<()> {
    for removal in [false, true] {
        let (tmp, mut s, c, w, mut r) = fixture()?;
        s.submit(&c)?;
        if removal {
            reconcile(&mut s, &c, &Validate, &mut r, &Options::default())?;
            s.mark_removed("mixed", true)?;
        }
        let unit = if removal { "a-app" } else { "z-infra" };
        w.lock().unwrap().fail_observe = true;
        w.lock().unwrap().observations.clear();
        let mut options = Options {
            continuous_retry: true,
            ..Default::default()
        };
        for (index, (now, deadline)) in [
            (1, 3),
            (3, 7),
            (7, 15),
            (15, 31),
            (31, 63),
            (63, 127),
            (127, 255),
            (255, 511),
            (511, 811),
            (811, 1111),
        ]
        .into_iter()
        .enumerate()
        {
            options.now = now;
            reconcile(&mut s, &c, &Validate, &mut r, &options)?;
            let state = s.unit("mixed", unit)?.unwrap();
            assert_eq!(state.attempts, index as u32 + 1);
            assert_eq!(state.next_retry, deadline);
            assert_eq!(w.lock().unwrap().observations.len(), index + 1);
            options.now = deadline - 1;
            reconcile(&mut s, &c, &Validate, &mut r, &options)?;
            assert_eq!(w.lock().unwrap().observations.len(), index + 1);
            assert!(w.lock().unwrap().deletes.is_empty());
        }
        drop(s);
        let mut s = Store::open(&tmp.path().join("store"))?;
        assert_eq!(s.unit("mixed", unit)?.unwrap().attempts, 10);
        assert_eq!(s.unit("mixed", unit)?.unwrap().next_retry, 1111);
        w.lock().unwrap().fail_observe = false;
        options.now = 1110;
        reconcile(&mut s, &c, &Validate, &mut r, &options)?;
        assert_eq!(w.lock().unwrap().observations.len(), 10);
        options.now = 1111;
        reconcile(&mut s, &c, &Validate, &mut r, &options)?;
        let state = s.unit("mixed", unit)?.unwrap();
        assert_eq!(state.attempts, 0);
        assert_eq!(state.next_retry, 0);
        assert!(state.failure.is_none());
        if removal {
            assert!(state.deleted);
            assert_eq!(w.lock().unwrap().deletes, vec!["a-app", "z-infra"]);
        } else {
            assert!(state.applied);
            assert_eq!(w.lock().unwrap().writes, vec!["z-infra", "a-app"]);
        }
    }
    Ok(())
}

#[test]
fn one_shot_removal_still_stops_after_five_failures() -> Result<()> {
    let (_tmp, mut s, c, w, mut r) = fixture()?;
    s.submit(&c)?;
    reconcile(&mut s, &c, &Validate, &mut r, &Options::default())?;
    s.mark_removed("mixed", true)?;
    w.lock().unwrap().fail_observe = true;
    w.lock().unwrap().observations.clear();
    for now in [1, 3, 7, 15, 31, 1000] {
        reconcile(
            &mut s,
            &c,
            &Validate,
            &mut r,
            &Options {
                now,
                ..Default::default()
            },
        )?;
    }
    assert_eq!(s.unit("mixed", "a-app")?.unwrap().attempts, 5);
    assert_eq!(w.lock().unwrap().observations.len(), 5);
    assert!(w.lock().unwrap().deletes.is_empty());
    Ok(())
}

#[test]
fn continuous_retry_recovers_a_running_operation_after_a_persisted_outage() -> Result<()> {
    let (tmp, mut s, c, w, mut r) = fixture()?;
    s.submit(&c)?;
    w.lock().unwrap().fail_after_write = true;
    let mut options = Options {
        continuous_retry: true,
        ..Default::default()
    };
    reconcile(&mut s, &c, &Validate, &mut r, &options)?;
    let operation = s.running("mixed", "z-infra")?.unwrap().operation;
    w.lock().unwrap().fail_observe = true;
    for now in [3, 7, 15, 31, 63] {
        options.now = now;
        reconcile(&mut s, &c, &Validate, &mut r, &options)?;
    }
    assert_eq!(s.unit("mixed", "z-infra")?.unwrap().attempts, 6);
    assert_eq!(s.running("mixed", "z-infra")?.unwrap().operation, operation);
    drop(s);
    let mut s = Store::open(&tmp.path().join("store"))?;
    w.lock().unwrap().fail_observe = false;
    w.lock().unwrap().fail_after_write = false;
    options.now = 126;
    reconcile(&mut s, &c, &Validate, &mut r, &options)?;
    assert!(s.running("mixed", "z-infra")?.is_some());
    options.now = 127;
    reconcile(&mut s, &c, &Validate, &mut r, &options)?;
    assert!(s.running("mixed", "z-infra")?.is_none());
    assert!(s.unit("mixed", "z-infra")?.unwrap().applied);
    assert_eq!(w.lock().unwrap().writes, vec!["z-infra", "a-app"]);
    Ok(())
}

#[test]
fn continuous_retry_saturates_counters_and_deadlines() -> Result<()> {
    let (_tmp, mut s, c, w, mut r) = fixture()?;
    s.submit(&c)?;
    w.lock().unwrap().fail_observe = true;
    let mut options = Options {
        continuous_retry: true,
        ..Default::default()
    };
    reconcile(&mut s, &c, &Validate, &mut r, &options)?;
    let mut state = s.unit("mixed", "z-infra")?.unwrap();
    state.attempts = u32::MAX;
    state.next_retry = 100;
    s.save_unit("mixed", "z-infra", &state)?;
    options.now = 100;
    reconcile(&mut s, &c, &Validate, &mut r, &options)?;
    let state = s.unit("mixed", "z-infra")?.unwrap();
    assert_eq!(state.attempts, u32::MAX);
    assert_eq!(state.next_retry, 400);
    options.now = u64::MAX - 1;
    reconcile(&mut s, &c, &Validate, &mut r, &options)?;
    assert_eq!(s.unit("mixed", "z-infra")?.unwrap().next_retry, u64::MAX);
    Ok(())
}

#[test]
fn planning_removed_units_does_not_execute_deletion() -> Result<()> {
    let (_tmp, mut s, mut c, w, mut r) = fixture()?;
    s.submit(&c)?;
    reconcile(&mut s, &c, &Validate, &mut r, &Options::default())?;
    c.instance.units.remove("a-app");
    c.units.remove("a-app");
    c.revision = c.digest()?;
    s.submit(&c)?;
    reconcile(
        &mut s,
        &c,
        &Validate,
        &mut r,
        &Options {
            execute: false,
            prepare: true,
            ..Default::default()
        },
    )?;
    assert!(w.lock().unwrap().deletes.is_empty());
    let state = s.unit("mixed", "a-app")?.unwrap();
    assert!(state.failure.is_none(), "{:?}", state.failure);
    assert_eq!(state.prepared.unwrap().summary.deletes, 1);
    Ok(())
}

#[test]
fn failed_rollout_reports_failure_without_blocking_correction_or_recovery() -> Result<()> {
    let (_tmp, mut s, mut c, w, mut r) = fixture()?;
    w.lock().unwrap().readiness = Some(Readiness::Failed);
    s.submit(&c)?;
    reconcile(&mut s, &c, &Validate, &mut r, &Options::default())?;
    let state = s.unit("mixed", "z-infra")?.unwrap();
    assert_eq!(state.public()["conditions"]["Failed"], true);
    assert!(!state.inventory.is_null());
    assert_eq!(w.lock().unwrap().writes, vec!["z-infra"]);
    assert!(s.running("mixed", "z-infra")?.is_none());
    reconcile(&mut s, &c, &Validate, &mut r, &Options::default())?;
    assert_eq!(
        s.unit("mixed", "z-infra")?.unwrap().public()["conditions"]["Failed"],
        true
    );
    assert_eq!(w.lock().unwrap().writes, vec!["z-infra"]);

    c.instance.units.get_mut("z-infra").unwrap().configuration["message"] = json!("fixed");
    c.units.get_mut("z-infra").unwrap().artifact = json!({"message":"fixed"});
    c.revision = c.digest()?;
    s.submit(&c)?;
    reconcile(&mut s, &c, &Validate, &mut r, &Options::default())?;
    assert_eq!(w.lock().unwrap().values["z-infra"]["message"], "fixed");
    assert_eq!(w.lock().unwrap().writes, vec!["z-infra", "z-infra"]);
    assert_eq!(
        s.unit("mixed", "z-infra")?.unwrap().public()["conditions"]["Failed"],
        true
    );

    w.lock().unwrap().readiness = Some(Readiness::Ready);
    reconcile(&mut s, &c, &Validate, &mut r, &Options::default())?;
    assert_eq!(
        s.unit("mixed", "z-infra")?.unwrap().public()["conditions"]["Failed"],
        false
    );
    assert_eq!(
        w.lock().unwrap().writes,
        vec!["z-infra", "z-infra", "a-app"]
    );
    Ok(())
}

#[test]
fn approving_a_live_unit_does_not_delete_removed_units() -> Result<()> {
    let (_tmp, mut s, mut c, w, mut r) = fixture()?;
    s.submit(&c)?;
    reconcile(&mut s, &c, &Validate, &mut r, &Options::default())?;
    c.instance.units.remove("a-app");
    c.units.remove("a-app");
    c.instance.units.get_mut("z-infra").unwrap().configuration["message"] = json!("next");
    c.units.get_mut("z-infra").unwrap().artifact = json!({"message":"next"});
    c.revision = c.digest()?;
    s.submit(&c)?;
    reconcile(
        &mut s,
        &c,
        &Validate,
        &mut r,
        &Options {
            execute: false,
            prepare: true,
            ..Default::default()
        },
    )?;
    let operation = s
        .unit("mixed", "z-infra")?
        .unwrap()
        .prepared
        .unwrap()
        .operation;
    reconcile(
        &mut s,
        &c,
        &Validate,
        &mut r,
        &Options {
            approved_operation: Some(operation),
            ..Default::default()
        },
    )?;
    assert_eq!(w.lock().unwrap().values["z-infra"]["message"], "next");
    assert!(w.lock().unwrap().deletes.is_empty());
    Ok(())
}

#[test]
fn removal_approval_executes_only_the_saved_leaf_plan_then_requires_another_plan() -> Result<()> {
    let (_tmp, mut s, c, w, mut r) = fixture()?;
    s.submit(&c)?;
    reconcile(&mut s, &c, &Validate, &mut r, &Options::default())?;
    s.mark_removed("mixed", true)?;
    let plan_options = Options {
        execute: false,
        prepare: true,
        ..Default::default()
    };
    reconcile(&mut s, &c, &Validate, &mut r, &plan_options)?;
    let operation = s
        .unit("mixed", "a-app")?
        .unwrap()
        .prepared
        .unwrap()
        .operation;
    reconcile(
        &mut s,
        &c,
        &Validate,
        &mut r,
        &Options {
            approved_operation: Some(operation.clone()),
            ..Default::default()
        },
    )?;
    {
        let world = w.lock().unwrap();
        assert_eq!(world.deletes, vec!["a-app"]);
        assert_eq!(world.removal_plans, vec![operation.clone()]);
        assert_eq!(world.removal_executions, vec![operation]);
        assert!(world.values.contains_key("z-infra"));
    }
    reconcile(&mut s, &c, &Validate, &mut r, &plan_options)?;
    let operation = s
        .unit("mixed", "z-infra")?
        .unwrap()
        .prepared
        .unwrap()
        .operation;
    reconcile(
        &mut s,
        &c,
        &Validate,
        &mut r,
        &Options {
            approved_operation: Some(operation),
            ..Default::default()
        },
    )?;
    assert_eq!(w.lock().unwrap().deletes, vec!["a-app", "z-infra"]);
    Ok(())
}

#[test]
fn stale_removal_approval_never_substitutes_a_fresh_plan() -> Result<()> {
    let (_tmp, mut s, mut c, w, mut r) = fixture()?;
    s.submit(&c)?;
    reconcile(&mut s, &c, &Validate, &mut r, &Options::default())?;
    c.instance.units.remove("a-app");
    c.units.remove("a-app");
    c.revision = c.digest()?;
    s.submit(&c)?;
    reconcile(
        &mut s,
        &c,
        &Validate,
        &mut r,
        &Options {
            execute: false,
            prepare: true,
            ..Default::default()
        },
    )?;
    let operation = s
        .unit("mixed", "a-app")?
        .unwrap()
        .prepared
        .unwrap()
        .operation;
    w.lock()
        .unwrap()
        .values
        .insert("a-app".into(), json!({"message":"changed remotely"}));
    for now in [1, 3, 7, 15, 31, 63] {
        reconcile(
            &mut s,
            &c,
            &Validate,
            &mut r,
            &Options {
                now,
                continuous_retry: true,
                approved_operation: Some(operation.clone()),
                ..Default::default()
            },
        )?;
    }
    assert_eq!(s.unit("mixed", "a-app")?.unwrap().attempts, 6);
    assert!(s
        .unit("mixed", "a-app")?
        .unwrap()
        .failure
        .unwrap()
        .contains("stale"));
    let world = w.lock().unwrap();
    assert!(world.deletes.is_empty());
    assert_eq!(world.removal_plans.len(), 1);
    Ok(())
}

#[test]
fn interrupted_approved_removal_observes_completion_without_replaying() -> Result<()> {
    let (_tmp, mut s, mut c, w, mut r) = fixture()?;
    s.submit(&c)?;
    reconcile(&mut s, &c, &Validate, &mut r, &Options::default())?;
    c.instance.units.remove("a-app");
    c.units.remove("a-app");
    c.revision = c.digest()?;
    s.submit(&c)?;
    reconcile(
        &mut s,
        &c,
        &Validate,
        &mut r,
        &Options {
            execute: false,
            prepare: true,
            ..Default::default()
        },
    )?;
    let operation = s
        .unit("mixed", "a-app")?
        .unwrap()
        .prepared
        .unwrap()
        .operation;
    let mut options = Options {
        approved_operation: Some(operation),
        ..Default::default()
    };
    w.lock().unwrap().fail_after_delete = true;
    reconcile(&mut s, &c, &Validate, &mut r, &options)?;
    assert!(s.running("mixed", "a-app")?.is_some());
    w.lock().unwrap().fail_after_delete = false;
    options.now = 10;
    reconcile(&mut s, &c, &Validate, &mut r, &options)?;
    assert!(s.unit("mixed", "a-app")?.unwrap().deleted);
    assert!(s.unit("mixed", "a-app")?.unwrap().failure.is_none());
    assert!(s.running("mixed", "a-app")?.is_none());
    assert_eq!(w.lock().unwrap().deletes, vec!["a-app"]);
    assert_eq!(w.lock().unwrap().removal_plans.len(), 1);
    Ok(())
}

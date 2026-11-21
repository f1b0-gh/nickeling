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
    sync::{Arc, Mutex},
};
#[derive(Default)]
struct Remote {
    value: Option<Value>,
    executions: usize,
    observations: usize,
    fail_after_write: bool,
}
struct Fake {
    remote: Arc<Mutex<Remote>>,
}
impl Backend for Fake {
    fn kind(&self) -> BackendKind {
        BackendKind::OpenTofu
    }
    fn fingerprint(&self) -> Result<String> {
        Ok("target".into())
    }
    fn observe(&mut self, c: &Context) -> Result<Observation> {
        let mut r = self
            .remote
            .lock()
            .map_err(|_| Error::Backend("lock".into()))?;
        r.observations += 1;
        Ok(Observation {
            exists: r.value.is_some(),
            drifted: r.value.as_ref() != Some(&c.artifact),
            readiness: Readiness::Ready,
            preconditions: json!({"value":r.value}),
            outputs: BTreeMap::from([(
                "value".into(),
                OutputValue {
                    value: r.value.clone(),
                    sensitive: false,
                    reference: None,
                },
            )]),
            inventory: json!({"owned":true}),
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
        let mut r = self
            .remote
            .lock()
            .map_err(|_| Error::Backend("lock".into()))?;
        r.value = Some(c.artifact.clone());
        r.executions += 1;
        if r.fail_after_write {
            r.fail_after_write = false;
            return Err(Error::Backend("interrupted".into()));
        }
        drop(r);
        self.observe(c)
    }
}
struct Validate;
impl Validator for Validate {
    fn validate(&self, _: &Unit, value: &Value, base: &CompiledUnit) -> Result<CompiledUnit> {
        let mut base = base.clone();
        base.artifact = value.clone();
        Ok(base)
    }
}
fn compiled(manage: bool) -> Result<Compiled> {
    let instance: Instance = serde_json::from_value(
        json!({"apiVersion":"nickel-plane/v1","id":"demo","composition":{"name":"test","version":"1"},"policy":{"mode":if manage{"manage"}else{"observe"}},"units":{"first":{"backend":"opentofu","target":"local","schema":"local","configuration":{"value":1},"outputs":{"value":{"pointer":"/outputs/value"}}}}}),
    )?;
    let mut c = Compiled {
        version: 1,
        revision: String::new(),
        instance,
        inputs: Value::Null,
        units: BTreeMap::from([(
            "first".into(),
            CompiledUnit {
                artifact: json!({"value":1}),
                receipt: Value::Null,
                engine_version: None,
                provider_lock: None,
            },
        )]),
    };
    c.revision = c.digest()?;
    Ok(c)
}
fn registry(remote: Arc<Mutex<Remote>>) -> Registry {
    BTreeMap::from([(
        "local".into(),
        Box::new(Fake { remote }) as Box<dyn Backend>,
    )])
}
#[test]
fn observe_only_and_exclusive_store_never_mutate() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut s = Store::open(&dir.path().join("store"))?;
    assert!(Store::open(&dir.path().join("store")).is_err());
    let c = compiled(false)?;
    s.submit(&c)?;
    let r = Arc::new(Mutex::new(Remote::default()));
    reconcile(
        &mut s,
        &c,
        &Validate,
        &mut registry(r.clone()),
        &Options::default(),
    )?;
    assert_eq!(
        r.lock()
            .map_err(|_| Error::Backend("lock".into()))?
            .executions,
        0
    );
    Ok(())
}
#[test]
fn interrupted_mutation_is_observed_after_restart_without_replay() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let r = Arc::new(Mutex::new(Remote {
        fail_after_write: true,
        ..Default::default()
    }));
    let c = compiled(true)?;
    {
        let mut s = Store::open(&dir.path().join("store"))?;
        s.submit(&c)?;
        reconcile(
            &mut s,
            &c,
            &Validate,
            &mut registry(r.clone()),
            &Options::default(),
        )?;
    }
    let mut s = Store::open(&dir.path().join("store"))?;
    reconcile(
        &mut s,
        &c,
        &Validate,
        &mut registry(r.clone()),
        &Options {
            now: 1000,
            ..Default::default()
        },
    )?;
    let state = s
        .unit("demo", "first")?
        .ok_or_else(|| Error::Invalid("missing state".into()))?;
    assert!(state.applied);
    assert_eq!(
        r.lock()
            .map_err(|_| Error::Backend("lock".into()))?
            .executions,
        1
    );
    assert!(s.running("demo", "first")?.is_none());
    Ok(())
}
#[test]
fn repeated_events_converge_and_old_revision_cannot_overwrite_status() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut s = Store::open(&dir.path().join("store"))?;
    let c = compiled(true)?;
    s.submit(&c)?;
    let r = Arc::new(Mutex::new(Remote::default()));
    let mut backends = registry(r.clone());
    reconcile(&mut s, &c, &Validate, &mut backends, &Options::default())?;
    let old = s
        .unit("demo", "first")?
        .ok_or_else(|| Error::Invalid("missing state".into()))?;
    reconcile(&mut s, &c, &Validate, &mut backends, &Options::default())?;
    assert_eq!(
        r.lock()
            .map_err(|_| Error::Backend("lock".into()))?
            .executions,
        1
    );
    let mut next = c.clone();
    next.instance.parameters = json!({"revision":2});
    next.revision = next.digest()?;
    s.submit(&next)?;
    assert!(s.save_unit("demo", "first", &old).is_err());
    Ok(())
}

#[test]
fn removed_revision_stays_removed_and_public_status_works_with_live_writer() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("store");
    let mut s = Store::open(&path)?;
    let c = compiled(true)?;
    s.submit(&c)?;
    s.mark_removed("demo", false)?;
    s.submit(&c)?;
    assert_eq!(s.removal("demo")?, 1);
    let status = nickel_plane::store::status(&path, "demo")?;
    assert_eq!(status["revision"], c.revision);
    assert_eq!(status["removal"], 1);
    assert_ne!(s.work("a-b", "c")?, s.work("a", "b-c")?);
    s.claim("stack", "a", "b")?;
    assert!(s.claim("stack", "a", "c").is_err());
    Ok(())
}
#[test]
fn store_rejects_links_public_permissions_and_unrelated_databases() -> Result<()> {
    use std::{
        fs,
        os::unix::fs::{symlink, PermissionsExt},
    };
    let tmp = tempfile::tempdir()?;
    let private = tmp.path().join("private");
    fs::create_dir(&private)?;
    fs::set_permissions(&private, fs::Permissions::from_mode(0o700))?;
    symlink(&private, tmp.path().join("link"))?;
    assert!(Store::open(&tmp.path().join("link")).is_err());
    let public = tmp.path().join("public");
    fs::create_dir(&public)?;
    fs::set_permissions(&public, fs::Permissions::from_mode(0o755))?;
    assert!(Store::open(&public).is_err());
    let external = tmp.path().join("external");
    fs::write(&external, "preserved")?;
    fs::hard_link(&external, private.join("store.sqlite"))?;
    assert!(Store::open(&private).is_err());
    assert_eq!(fs::read_to_string(&external)?, "preserved");
    fs::remove_file(private.join("store.sqlite"))?;
    let db = rusqlite::Connection::open(private.join("store.sqlite"))?;
    db.execute("CREATE TABLE unrelated(value TEXT)", [])?;
    drop(db);
    fs::set_permissions(
        private.join("store.sqlite"),
        fs::Permissions::from_mode(0o600),
    )?;
    assert!(Store::open(&private).is_err());
    Ok(())
}

struct Durable {
    base: Fake,
    recover_error: bool,
}
impl Backend for Durable {
    fn kind(&self) -> BackendKind {
        self.base.kind()
    }
    fn fingerprint(&self) -> Result<String> {
        self.base.fingerprint()
    }
    fn observe(&mut self, c: &Context) -> Result<Observation> {
        self.base.observe(c)
    }
    fn prepare_change(&mut self, c: &Context, o: &Observation, id: &str) -> Result<PreparedChange> {
        self.base.prepare_change(c, o, id)
    }
    fn execute_change(&mut self, c: &Context, p: &PreparedChange) -> Result<Observation> {
        self.base.execute_change(c, p)
    }
    fn execute_change_with_journal(
        &mut self,
        _c: &Context,
        _p: &PreparedChange,
        journal: &mut dyn OperationJournal,
    ) -> Result<Observation> {
        journal.guard()?;
        journal.commit(
            &json!({"uid":"response-uid"}),
            0,
            &json!({"uid":"response-uid"}),
            None,
        )?;
        Err(Error::Backend(
            "injected interruption after durable UID".into(),
        ))
    }
    fn recover_change(
        &mut self,
        _c: &Context,
        _p: &PreparedChange,
        journal: &mut dyn OperationJournal,
    ) -> Result<RecoveryDisposition> {
        assert!(journal.guard().is_err(), "read recovery cannot mutate");
        journal.commit(
            &json!({"candidate":"candidate-uid"}),
            1,
            &json!({"uid":"response-uid"}),
            Some(RecoveryReason::UnrecordedIdentity),
        )?;
        if self.recover_error {
            return Err(Error::Backend("injected later read failure".into()));
        }
        Ok(RecoveryDisposition::ReviewRequired(
            RecoveryReason::UnrecordedIdentity,
        ))
    }
}
fn durable_registry(remote: Arc<Mutex<Remote>>, recover_error: bool) -> Registry {
    BTreeMap::from([(
        "local".into(),
        Box::new(Durable {
            base: Fake { remote },
            recover_error,
        }) as Box<dyn Backend>,
    )])
}
#[test]
fn durable_evidence_survives_execution_and_later_recovery_errors() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut store = Store::open(&dir.path().join("store"))?;
    let c = compiled(true)?;
    store.submit(&c)?;
    let remote = Arc::new(Mutex::new(Remote::default()));
    reconcile(
        &mut store,
        &c,
        &Validate,
        &mut durable_registry(remote.clone(), true),
        &Options::default(),
    )?;
    let state = store
        .unit("demo", "first")?
        .ok_or_else(|| Error::Invalid("missing state".into()))?;
    assert_eq!(state.inventory, json!({"uid":"response-uid"}));
    assert!(store.running("demo", "first")?.is_some());
    // Recovery runs despite the ordinary backoff deadline, and its first commit
    // remains durable when a later independent read fails.
    reconcile(
        &mut store,
        &c,
        &Validate,
        &mut durable_registry(remote, true),
        &Options {
            execute: false,
            ..Default::default()
        },
    )?;
    let state = store
        .unit("demo", "first")?
        .ok_or_else(|| Error::Invalid("missing state".into()))?;
    assert_eq!(state.inventory, json!({"uid":"response-uid"}));
    assert_eq!(state.public()["recovery"]["reason"], "unrecordedIdentity");
    Ok(())
}

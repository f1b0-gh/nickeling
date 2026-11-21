//! Compound creation model tests; native API semantics are qualified separately.
#![cfg(feature = "kubernetes")]
#![allow(clippy::unwrap_used)]
use nickel_plane::{
    adapters::kubernetes::{Api, Kubernetes},
    backend::*,
    composition::{Compiled, CompiledUnit, Validator},
    model::*,
    reconcile::{reconcile, Options, Registry},
    store::Store,
    targets::KubernetesTarget,
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
struct Remote {
    objects: BTreeMap<String, Value>,
    posts: usize,
    patches: usize,
    deletes: usize,
    lose_post: bool,
    lose_patch: bool,
    lose_post_at: Option<usize>,
    lose_delete: bool,
    fail_get: bool,
    delay_delete: bool,
    fail_get_path: Option<String>,
    authority: u64,
    rotate_after_post: bool,
    fail_patch: bool,
    corrupt_post_response: Option<String>,
    cancel_after_post: Option<Arc<AtomicBool>>,
}
struct ModelApi(Arc<Mutex<Remote>>);
impl Api for ModelApi {
    fn execution_fingerprint(&self, _: &KubernetesTarget) -> Result<String> {
        Ok(format!(
            "fixture-authority-{}",
            self.0.lock().unwrap().authority
        ))
    }
    fn request(&mut self, method: &str, route: &str, body: Option<&Value>) -> Result<(u16, Value)> {
        let mut remote = self
            .0
            .lock()
            .map_err(|_| Error::Backend("Fixture lock".into()))?;
        let path = route.split('?').next().unwrap();
        if method == "GET" && (remote.fail_get || remote.fail_get_path.as_deref() == Some(path)) {
            return Err(Error::Backend("Fixture GET failure".into()));
        }
        if method == "GET" {
            return Ok(remote
                .objects
                .get(path)
                .map(|object| (200, object.clone()))
                .unwrap_or((404, Value::Null)));
        }
        let dry = route.contains("dryRun=All");
        if method == "PATCH" && !dry && remote.fail_patch {
            return Err(Error::Backend("Fixture PATCH unavailable".into()));
        }
        let body = body.ok_or_else(|| Error::Invalid("Missing fixture body".into()))?;
        if method == "DELETE" {
            let Some(old) = remote.objects.get(path) else {
                return Ok((404, Value::Null));
            };
            if old["metadata"]["uid"] != body["preconditions"]["uid"]
                || old["metadata"]["resourceVersion"] != body["preconditions"]["resourceVersion"]
            {
                return Ok((409, Value::Null));
            }
            if !dry {
                if !remote.delay_delete {
                    remote.objects.remove(path);
                }
                remote.deletes += 1;
                if std::mem::take(&mut remote.lose_delete) {
                    return Err(Error::Backend("Lost deletion response".into()));
                }
            }
            return Ok((200, json!({"status":"Success"})));
        }
        let manager = route
            .split("fieldManager=")
            .nth(1)
            .unwrap_or("fixture")
            .split('&')
            .next()
            .unwrap();
        let destination = if method == "POST" {
            format!("{path}/{}", body["metadata"]["name"].as_str().unwrap())
        } else {
            path.into()
        };
        let mut object = if method == "POST" {
            if remote.objects.contains_key(&destination) {
                return Ok((409, Value::Null));
            }
            let mut object = body.clone();
            object["metadata"]["uid"] = json!(format!("uid-{}", remote.posts + 1));
            object["metadata"]["managedFields"] = json!([{"manager":manager,"operation":"Update","apiVersion":body["apiVersion"],"fieldsType":"FieldsV1","fieldsV1":{"f:data":{"f:value":{},"f:remove":{}}}}]);
            object
        } else if method == "PATCH" {
            let Some(old) = remote.objects.get(path) else {
                return Ok((404, Value::Null));
            };
            if let Some(patch) = body.as_array() {
                let mut object = old.clone();
                for action in patch {
                    let pointer = action["path"].as_str().unwrap();
                    match action["op"].as_str().unwrap() {
                        "test" if object.pointer(pointer) != Some(&action["value"]) => {
                            return Ok((422, Value::Null))
                        }
                        "test" => {}
                        "replace" => {
                            *object.pointer_mut(pointer).unwrap() = action["value"].clone()
                        }
                        _ => return Err(Error::Invalid("Unexpected fixture patch".into())),
                    }
                }
                object
            } else {
                if old["metadata"]["uid"] != body["metadata"]["uid"]
                    || old["metadata"]["resourceVersion"] != body["metadata"]["resourceVersion"]
                    || old["metadata"]["managedFields"][0]["operation"] != "Apply"
                {
                    return Ok((409, Value::Null));
                }
                let mut object = old.clone();
                object["data"] = body["data"].clone();
                object["metadata"]["annotations"] = body["metadata"]["annotations"].clone();
                object
            }
        } else {
            return Err(Error::Invalid("Unsupported fixture method".into()));
        };
        object["metadata"]["resourceVersion"] =
            json!(format!("{}", remote.posts + remote.patches + 1));
        if !dry {
            if method == "POST" {
                remote.posts += 1;
                if remote.rotate_after_post {
                    remote.authority += 1;
                }
            } else {
                remote.patches += 1;
            }
            remote.objects.insert(destination, object.clone());
            if method == "POST" {
                if let Some(cancelled) = &remote.cancel_after_post {
                    cancelled.store(true, Ordering::SeqCst);
                }
            }
            let lose = if method == "POST" {
                std::mem::take(&mut remote.lose_post) || remote.lose_post_at == Some(remote.posts)
            } else {
                std::mem::take(&mut remote.lose_patch)
            };
            if lose {
                return Err(Error::Backend("Lost fixture response".into()));
            }
        }
        if method == "POST" && !dry {
            if remote.corrupt_post_response.as_deref() == Some("identity") {
                object["metadata"]["name"] = json!("wrong-route");
            }
            if remote.corrupt_post_response.as_deref() == Some("fields") {
                object["metadata"]["managedFields"][0]["fieldsV1"]["f:unexpected"] = json!({});
            }
        }
        Ok((if method == "POST" { 201 } else { 200 }, object))
    }
}
struct Validate;
impl Validator for Validate {
    fn validate(
        &self,
        _unit: &Unit,
        configuration: &Value,
        base: &CompiledUnit,
    ) -> Result<CompiledUnit> {
        let mut effective = base.clone();
        effective.artifact = configuration.clone();
        Ok(effective)
    }
}
fn compiled() -> Result<Compiled> {
    let artifact = json!({"apiVersion":"v1","kind":"List","items":[{"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"settings","namespace":"test"},"data":{"value":"one","remove":"old"}}]});
    let instance: Instance = serde_json::from_value(
        json!({"apiVersion":"nickel-plane/v1","id":"demo","composition":{"name":"test","version":"1"},"policy":{"mode":"manage"},"units":{"app":{"backend":"kubernetes","target":"cluster","configuration":artifact}}}),
    )?;
    let mut compiled = Compiled {
        version: 1,
        revision: String::new(),
        instance,
        inputs: Value::Null,
        units: BTreeMap::from([(
            "app".into(),
            CompiledUnit {
                artifact,
                receipt: Value::Null,
                engine_version: None,
                provider_lock: None,
            },
        )]),
    };
    compiled.revision = compiled.digest()?;
    Ok(compiled)
}
fn registry(remote: Arc<Mutex<Remote>>) -> Registry {
    BTreeMap::from([(
        "cluster".into(),
        Box::new(Kubernetes {
            api: ModelApi(remote),
            target: KubernetesTarget {
                server: "https://cluster.example".into(),
                namespace: "test".into(),
                ca_file: None,
                previous_ca_file: None,
                token_file: None,
                allow_http_loopback: false,
            },
        }) as Box<dyn Backend>,
    )])
}
fn prepare(
    store: &mut Store,
    compiled: &Compiled,
    remote: Arc<Mutex<Remote>>,
) -> Result<PreparedChange> {
    reconcile(
        store,
        compiled,
        &Validate,
        &mut registry(remote),
        &Options {
            execute: false,
            prepare: true,
            now: 1000,
            ..Default::default()
        },
    )?;
    let state = store.unit("demo", "app")?.unwrap();
    state.prepared.ok_or_else(|| {
        Error::Invalid(format!(
            "Missing fixture plan: {:?}; recovery: {:?}",
            state.failure, state.recovery
        ))
    })
}
fn apply(
    store: &mut Store,
    compiled: &Compiled,
    remote: Arc<Mutex<Remote>>,
    plan: &PreparedChange,
) -> Result<()> {
    reconcile(
        store,
        compiled,
        &Validate,
        &mut registry(remote),
        &Options {
            approved_operation: Some(plan.operation.clone()),
            now: 1000,
            ..Default::default()
        },
    )
}
const PATH: &str = "/api/v1/namespaces/test/configmaps/settings";
#[test]
fn one_approval_creates_and_initializes_then_omission_uses_normal_ssa() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut store = Store::open(&temp.path().join("store"))?;
    let mut compiled = compiled()?;
    store.submit(&compiled)?;
    let remote = Arc::new(Mutex::new(Remote::default()));
    let plan = prepare(&mut store, &compiled, remote.clone())?;
    assert_eq!(plan.payload["phase"], "create-initialize-v1");
    assert_eq!(plan.summary.creates, 1);
    assert_eq!(plan.summary.field_ownership_transfers, 1);
    apply(&mut store, &compiled, remote.clone(), &plan)?;
    let state = store.unit("demo", "app")?.unwrap();
    assert!(state.applied, "{:?}", state.failure);
    assert_eq!(state.readiness, Readiness::Ready);
    assert!(store.running("demo", "app")?.is_none());
    {
        let remote = remote.lock().unwrap();
        assert_eq!((remote.posts, remote.patches), (1, 1));
        assert_eq!(
            remote.objects[PATH]["metadata"]["managedFields"][0]["operation"],
            "Apply"
        );
    }
    compiled
        .instance
        .units
        .get_mut("app")
        .unwrap()
        .configuration["items"][0]["data"]
        .as_object_mut()
        .unwrap()
        .remove("remove");
    compiled.units.get_mut("app").unwrap().artifact =
        compiled.instance.units["app"].configuration.clone();
    compiled.revision = compiled.digest()?;
    store.submit(&compiled)?;
    let update = prepare(&mut store, &compiled, remote.clone())?;
    assert_eq!(update.summary.field_ownership_transfers, 0);
    apply(&mut store, &compiled, remote.clone(), &update)?;
    let remote = remote.lock().unwrap();
    assert_eq!((remote.posts, remote.patches), (1, 2));
    assert!(remote.objects[PATH]["data"].get("remove").is_none());
    Ok(())
}

#[test]
fn lost_post_uid_requires_public_binding_then_separate_initializer() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("store");
    let compiled = compiled()?;
    let remote = Arc::new(Mutex::new(Remote {
        lose_post: true,
        ..Default::default()
    }));
    {
        let mut store = Store::open(&path)?;
        store.submit(&compiled)?;
        let create = prepare(&mut store, &compiled, remote.clone())?;
        apply(&mut store, &compiled, remote.clone(), &create)?;
        let state = store.unit("demo", "app")?.unwrap();
        assert!(state
            .inventory
            .as_object()
            .is_none_or(|objects| objects.is_empty()));
        assert_eq!(remote.lock().unwrap().posts, 1);
    }
    let mut store = Store::open(&path)?;
    reconcile(
        &mut store,
        &compiled,
        &Validate,
        &mut registry(remote.clone()),
        &Options {
            execute: false,
            now: 1000,
            ..Default::default()
        },
    )?;
    let unbound = store.unit("demo", "app")?.unwrap();
    assert!(unbound
        .inventory
        .as_object()
        .is_none_or(|objects| objects.is_empty()));
    assert_eq!(remote.lock().unwrap().patches, 0);
    let binding = prepare(&mut store, &compiled, remote.clone())?;
    assert_eq!(binding.payload["phase"], "recovery-bind-v1");
    let public = store.unit("demo", "app")?.unwrap().public();
    assert_eq!(public["change"]["recoveryBindings"][0]["uid"], "uid-1");
    assert_eq!(public["change"]["recoveryBindings"][0]["name"], "settings");
    assert!(!public.to_string().contains("fieldsV1"));
    #[cfg(feature = "cli")]
    {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_nickel-plane"))
            .arg("status")
            .arg("--store")
            .arg(&path)
            .args(["--instance", "demo"])
            .output()?;
        assert!(output.status.success());
        let status: Value = serde_json::from_slice(&output.stdout)?;
        assert_eq!(
            status["units"]["app"]["change"]["recoveryBindings"][0]["uid"],
            "uid-1"
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(
            !text.contains("fieldsV1") && !text.contains("checkpoint") && !text.contains("token")
        );
    }
    apply(&mut store, &compiled, remote.clone(), &binding)?;
    {
        let remote = remote.lock().unwrap();
        assert_eq!((remote.posts, remote.patches), (1, 0));
    }
    let bound = store.unit("demo", "app")?.unwrap();
    assert!(!bound.applied);
    assert_eq!(bound.inventory[PATH]["uid"], "uid-1");
    reconcile(
        &mut store,
        &compiled,
        &Validate,
        &mut registry(remote.clone()),
        &Options {
            retry_failed: true,
            now: 2000,
            ..Default::default()
        },
    )?;
    assert_eq!(
        remote.lock().unwrap().patches,
        0,
        "Binding must not authorize automatic initialization"
    );
    let initializer = prepare(&mut store, &compiled, remote.clone())?;
    assert_eq!(initializer.payload["phase"], "initialize-known-v1");
    apply(&mut store, &compiled, remote.clone(), &initializer)?;
    let state = store.unit("demo", "app")?.unwrap();
    assert!(state.applied, "{:?}", state.failure);
    let remote = remote.lock().unwrap();
    assert_eq!((remote.posts, remote.patches), (1, 1));
    Ok(())
}

#[test]
fn an_absent_lost_post_never_replays_without_fresh_approval() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut store = Store::open(&temp.path().join("store"))?;
    let compiled = compiled()?;
    store.submit(&compiled)?;
    let remote = Arc::new(Mutex::new(Remote {
        lose_post: true,
        ..Default::default()
    }));
    let create = prepare(&mut store, &compiled, remote.clone())?;
    apply(&mut store, &compiled, remote.clone(), &create)?;
    remote.lock().unwrap().objects.clear();
    for _ in 0..3 {
        reconcile(
            &mut store,
            &compiled,
            &Validate,
            &mut registry(remote.clone()),
            &Options {
                retry_failed: true,
                now: 1000,
                ..Default::default()
            },
        )?;
    }
    assert_eq!(remote.lock().unwrap().posts, 1);
    let state = store.unit("demo", "app")?.unwrap();
    assert!(!state.applied);
    assert_eq!(state.public()["recovery"]["outstandingIntents"], 1);
    Ok(())
}

#[test]
fn post_uid_is_durable_after_cancellation_and_known_uid_resumes() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("store");
    let mut store = Store::open(&path)?;
    let compiled = compiled()?;
    store.submit(&compiled)?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let remote = Arc::new(Mutex::new(Remote {
        cancel_after_post: Some(cancelled.clone()),
        ..Default::default()
    }));
    let plan = prepare(&mut store, &compiled, remote.clone())?;
    let result = reconcile(
        &mut store,
        &compiled,
        &Validate,
        &mut registry(remote.clone()),
        &Options {
            approved_operation: Some(plan.operation),
            cancelled,
            now: 1000,
            ..Default::default()
        },
    );
    assert!(matches!(result, Err(Error::Cancelled)));
    let state = store.unit("demo", "app")?.unwrap();
    assert_eq!(state.inventory[PATH]["uid"], "uid-1");
    assert!(store.running("demo", "app")?.is_some());
    {
        let remote = remote.lock().unwrap();
        assert_eq!((remote.posts, remote.patches), (1, 0));
    }
    drop(store);
    let mut store = Store::open(&path)?;
    reconcile(
        &mut store,
        &compiled,
        &Validate,
        &mut registry(remote.clone()),
        &Options {
            now: 1000,
            ..Default::default()
        },
    )?;
    let state = store.unit("demo", "app")?.unwrap();
    assert!(state.applied, "{:?}", state.failure);
    let remote = remote.lock().unwrap();
    assert_eq!((remote.posts, remote.patches), (1, 1));
    Ok(())
}

#[test]
fn bound_initializer_refuses_changed_original_field_ownership() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut store = Store::open(&temp.path().join("store"))?;
    let compiled = compiled()?;
    store.submit(&compiled)?;
    let remote = Arc::new(Mutex::new(Remote {
        lose_post: true,
        ..Default::default()
    }));
    let create = prepare(&mut store, &compiled, remote.clone())?;
    apply(&mut store, &compiled, remote.clone(), &create)?;
    let binding = prepare(&mut store, &compiled, remote.clone())?;
    assert_eq!(binding.payload["phase"], "recovery-bind-v1");
    apply(&mut store, &compiled, remote.clone(), &binding)?;
    remote.lock().unwrap().objects.get_mut(PATH).unwrap()["metadata"]["managedFields"][0]
        ["fieldsV1"]["f:data"]["f:foreign"] = json!({});
    assert!(
        prepare(&mut store, &compiled, remote.clone()).is_err(),
        "A changed live fieldset must not replace the approved creation fieldset"
    );
    assert_eq!(remote.lock().unwrap().patches, 0);
    assert_eq!(
        store.unit("demo", "app")?.unwrap().inventory[PATH]["uid"],
        "uid-1"
    );
    Ok(())
}

#[test]
fn mixed_omission_and_creation_stay_deferred_through_all_recovery_paths() -> Result<()> {
    for interruption in ["none", "post", "patch"] {
        let temp = tempfile::tempdir()?;
        let mut store = Store::open(&temp.path().join("store"))?;
        let mut compiled = compiled()?;
        store.submit(&compiled)?;
        let remote = Arc::new(Mutex::new(Remote::default()));
        let initial = prepare(&mut store, &compiled, remote.clone())?;
        apply(&mut store, &compiled, remote.clone(), &initial)?;
        let previous = store.unit("demo", "app")?.unwrap().applied_sha256;
        let configuration = &mut compiled
            .instance
            .units
            .get_mut("app")
            .unwrap()
            .configuration;
        configuration["items"][0]["data"]
            .as_object_mut()
            .unwrap()
            .remove("remove");
        configuration["items"].as_array_mut().unwrap().push(json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"other","namespace":"test"},"data":{"value":"new"}}));
        compiled.units.get_mut("app").unwrap().artifact = configuration.clone();
        compiled.revision = compiled.digest()?;
        store.submit(&compiled)?;
        let create = prepare(&mut store, &compiled, remote.clone())?;
        assert!(create.summary.deferred_changes);
        {
            let mut remote = remote.lock().unwrap();
            remote.lose_post = interruption == "post";
            remote.lose_patch = interruption == "patch";
        }
        apply(&mut store, &compiled, remote.clone(), &create)?;
        if interruption == "post" {
            let binding = prepare(&mut store, &compiled, remote.clone())?;
            assert!(binding.summary.deferred_changes);
            apply(&mut store, &compiled, remote.clone(), &binding)?;
            let initializer = prepare(&mut store, &compiled, remote.clone())?;
            assert!(initializer.summary.deferred_changes);
            apply(&mut store, &compiled, remote.clone(), &initializer)?;
        } else if interruption == "patch" {
            reconcile(
                &mut store,
                &compiled,
                &Validate,
                &mut registry(remote.clone()),
                &Options {
                    execute: false,
                    now: 2000,
                    ..Default::default()
                },
            )?;
        }
        let deferred = store.unit("demo", "app")?.unwrap();
        assert!(!deferred.applied, "{interruption}");
        assert_eq!(deferred.applied_sha256, previous, "{interruption}");
        assert_eq!(
            remote.lock().unwrap().objects[PATH]["data"]["remove"],
            "old"
        );
        let update = prepare(&mut store, &compiled, remote.clone())?;
        assert_eq!(update.summary.creates, 0);
        assert_eq!(update.summary.field_ownership_transfers, 0);
        apply(&mut store, &compiled, remote.clone(), &update)?;
        assert!(
            remote.lock().unwrap().objects[PATH]["data"]
                .get("remove")
                .is_none(),
            "{interruption}"
        );
        assert!(
            store.unit("demo", "app")?.unwrap().applied,
            "{interruption}"
        );
    }
    Ok(())
}

#[test]
fn removal_and_retain_preserve_unknown_intent_and_bind_late_uid_without_initializing() -> Result<()>
{
    for delete in [false, true] {
        let temp = tempfile::tempdir()?;
        let mut store = Store::open(&temp.path().join("store"))?;
        let mut compiled = compiled()?;
        compiled.instance.policy.deletion = Deletion::Delete;
        compiled.instance.policy.allow_delete = true;
        compiled.revision = compiled.digest()?;
        store.submit(&compiled)?;
        let remote = Arc::new(Mutex::new(Remote {
            lose_post: true,
            ..Default::default()
        }));
        let create = prepare(&mut store, &compiled, remote.clone())?;
        apply(&mut store, &compiled, remote.clone(), &create)?;
        let late = remote.lock().unwrap().objects.remove(PATH).unwrap();
        store.mark_removed("demo", delete)?;
        reconcile(
            &mut store,
            &compiled,
            &Validate,
            &mut registry(remote.clone()),
            &Options {
                execute: false,
                now: 1000,
                ..Default::default()
            },
        )?;
        assert!(
            !store.unit("demo", "app")?.unwrap().deleted,
            "An absent uncertain POST cannot prove deletion complete"
        );
        remote.lock().unwrap().objects.insert(PATH.into(), late);
        reconcile(
            &mut store,
            &compiled,
            &Validate,
            &mut registry(remote.clone()),
            &Options {
                execute: false,
                now: 1000,
                ..Default::default()
            },
        )?;
        let candidate = store.unit("demo", "app")?.unwrap();
        assert_eq!(
            candidate.public()["recovery"]["reason"],
            "unrecordedIdentity",
            "delete={delete}"
        );
        assert!(candidate
            .inventory
            .as_object()
            .is_none_or(|objects| objects.is_empty()));
        let binding = prepare(&mut store, &compiled, remote.clone())?;
        assert_eq!(binding.payload["phase"], "recovery-bind-v1");
        apply(&mut store, &compiled, remote.clone(), &binding)?;
        reconcile(
            &mut store,
            &compiled,
            &Validate,
            &mut registry(remote.clone()),
            &Options {
                retry_failed: true,
                now: 2000,
                ..Default::default()
            },
        )?;
        assert_eq!(remote.lock().unwrap().patches, 0);
        assert_eq!(remote.lock().unwrap().deletes, 0);
        if delete {
            let removal = prepare(&mut store, &compiled, remote.clone())?;
            assert_eq!(removal.summary.deletes, 1);
            assert_eq!(removal.summary.field_ownership_transfers, 0);
            apply(&mut store, &compiled, remote.clone(), &removal)?;
            assert_eq!(remote.lock().unwrap().deletes, 1);
            assert!(store.unit("demo", "app")?.unwrap().deleted);
        } else {
            assert!(store.unit("demo", "app")?.unwrap().retained);
            assert!(remote.lock().unwrap().objects.contains_key(PATH));
        }
    }
    Ok(())
}

#[test]
fn approved_replacement_resumes_while_absent_old_intent_stays_tracked_then_late_uid_needs_binding(
) -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut store = Store::open(&temp.path().join("store"))?;
    let mut compiled = compiled()?;
    store.submit(&compiled)?;
    let remote = Arc::new(Mutex::new(Remote {
        lose_post: true,
        ..Default::default()
    }));
    let old = prepare(&mut store, &compiled, remote.clone())?;
    apply(&mut store, &compiled, remote.clone(), &old)?;
    let late = remote.lock().unwrap().objects.remove(PATH).unwrap();
    compiled
        .instance
        .units
        .get_mut("app")
        .unwrap()
        .configuration["items"][0]["metadata"]["name"] = json!("other");
    compiled.units.get_mut("app").unwrap().artifact =
        compiled.instance.units["app"].configuration.clone();
    compiled.revision = compiled.digest()?;
    store.submit(&compiled)?;
    let replacement = prepare(&mut store, &compiled, remote.clone())?;
    let cancelled = Arc::new(AtomicBool::new(false));
    remote.lock().unwrap().cancel_after_post = Some(cancelled.clone());
    assert!(matches!(
        reconcile(
            &mut store,
            &compiled,
            &Validate,
            &mut registry(remote.clone()),
            &Options {
                approved_operation: Some(replacement.operation),
                cancelled,
                now: 1000,
                ..Default::default()
            }
        ),
        Err(Error::Cancelled)
    ));
    reconcile(
        &mut store,
        &compiled,
        &Validate,
        &mut registry(remote.clone()),
        &Options {
            now: 2000,
            ..Default::default()
        },
    )?;
    let state = store.unit("demo", "app")?.unwrap();
    assert!(state.applied, "{:?}; {:?}", state.failure, state.recovery);
    assert_eq!(
        state.inventory["/api/v1/namespaces/test/configmaps/other"]["uid"],
        "uid-2"
    );
    remote.lock().unwrap().objects.insert(PATH.into(), late);
    reconcile(
        &mut store,
        &compiled,
        &Validate,
        &mut registry(remote.clone()),
        &Options {
            execute: false,
            now: 3000,
            ..Default::default()
        },
    )?;
    let candidate = store.unit("demo", "app")?.unwrap();
    assert!(candidate.inventory.get(PATH).is_none());
    assert_eq!(
        candidate.public()["recovery"]["reason"],
        "unrecordedIdentity"
    );
    let binding = prepare(&mut store, &compiled, remote.clone())?;
    assert_eq!(binding.payload["phase"], "recovery-bind-v1");
    assert_eq!(binding.recovery_bindings()?[0].uid, "uid-1");
    let remote = remote.lock().unwrap();
    assert_eq!((remote.posts, remote.patches), (2, 1));
    Ok(())
}

#[test]
fn late_candidate_binding_does_not_strand_active_replacement() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("store");
    let mut store = Store::open(&path)?;
    let mut compiled = compiled()?;
    compiled.instance.policy.allow_delete = true;
    compiled.instance.policy.deletion = Deletion::Delete;
    compiled.revision = compiled.digest()?;
    store.submit(&compiled)?;
    let remote = Arc::new(Mutex::new(Remote {
        lose_post: true,
        ..Default::default()
    }));
    let old = prepare(&mut store, &compiled, remote.clone())?;
    apply(&mut store, &compiled, remote.clone(), &old)?;
    let late = remote.lock().unwrap().objects.remove(PATH).unwrap();
    compiled
        .instance
        .units
        .get_mut("app")
        .unwrap()
        .configuration["items"][0]["metadata"]["name"] = json!("other");
    compiled.units.get_mut("app").unwrap().artifact =
        compiled.instance.units["app"].configuration.clone();
    compiled.revision = compiled.digest()?;
    store.submit(&compiled)?;
    let replacement = prepare(&mut store, &compiled, remote.clone())?;
    let cancelled = Arc::new(AtomicBool::new(false));
    remote.lock().unwrap().cancel_after_post = Some(cancelled.clone());
    assert!(matches!(
        reconcile(
            &mut store,
            &compiled,
            &Validate,
            &mut registry(remote.clone()),
            &Options {
                approved_operation: Some(replacement.operation.clone()),
                cancelled,
                now: 1000,
                ..Default::default()
            }
        ),
        Err(Error::Cancelled)
    ));
    remote.lock().unwrap().objects.insert(PATH.into(), late);
    let binding = prepare(&mut store, &compiled, remote.clone())?;
    assert_eq!(binding.summary.recovery_bindings, 1);
    apply(&mut store, &compiled, remote.clone(), &binding)?;
    drop(store);
    let mut store = Store::open(&path)?;
    assert_eq!(
        store.running("demo", "app")?.unwrap().operation,
        replacement.operation
    );
    apply(&mut store, &compiled, remote.clone(), &replacement)?;
    let state = store.unit("demo", "app")?.unwrap();
    assert!(
        store.running("demo", "app")?.is_none(),
        "{:?}",
        state.failure
    );
    assert!(!state.applied);
    assert_eq!(state.readiness, Readiness::NotReady);
    assert_eq!(state.inventory[PATH]["uid"], "uid-1");
    assert_eq!(
        state.inventory["/api/v1/namespaces/test/configmaps/other"]["uid"],
        "uid-2"
    );
    store.mark_removed("demo", true)?;
    let removal = prepare(&mut store, &compiled, remote.clone())?;
    assert_eq!(removal.summary.deletes, 2);
    apply(&mut store, &compiled, remote.clone(), &removal)?;
    assert!(store.unit("demo", "app")?.unwrap().deleted);
    let remote = remote.lock().unwrap();
    assert_eq!((remote.posts, remote.patches, remote.deletes), (2, 1, 2));
    Ok(())
}

#[test]
fn candidate_binding_survives_later_not_started_conflict_and_unchanged_reads() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut store = Store::open(&temp.path().join("store"))?;
    let mut compiled = compiled()?;
    let mut other = compiled.instance.units["app"].configuration["items"][0].clone();
    other["metadata"]["name"] = json!("z-other");
    compiled
        .instance
        .units
        .get_mut("app")
        .unwrap()
        .configuration["items"]
        .as_array_mut()
        .unwrap()
        .push(other.clone());
    compiled.units.get_mut("app").unwrap().artifact =
        compiled.instance.units["app"].configuration.clone();
    compiled.revision = compiled.digest()?;
    store.submit(&compiled)?;
    let remote = Arc::new(Mutex::new(Remote {
        lose_post: true,
        ..Default::default()
    }));
    let create = prepare(&mut store, &compiled, remote.clone())?;
    apply(&mut store, &compiled, remote.clone(), &create)?;
    remote
        .lock()
        .unwrap()
        .objects
        .insert("/api/v1/namespaces/test/configmaps/z-other".into(), other);
    let binding = prepare(&mut store, &compiled, remote.clone())?;
    assert_eq!(binding.summary.recovery_bindings, 1);
    for _ in 0..2 {
        reconcile(
            &mut store,
            &compiled,
            &Validate,
            &mut registry(remote.clone()),
            &Options {
                execute: false,
                now: 1000,
                ..Default::default()
            },
        )?;
    }
    assert_eq!(
        store.unit("demo", "app")?.unwrap().prepared.as_ref(),
        Some(&binding)
    );
    apply(&mut store, &compiled, remote.clone(), &binding)?;
    let state = store.unit("demo", "app")?.unwrap();
    assert_eq!(state.inventory[PATH]["uid"], "uid-1", "{:?}", state.failure);
    assert!(state
        .inventory
        .get("/api/v1/namespaces/test/configmaps/z-other")
        .is_none());
    let remote = remote.lock().unwrap();
    assert_eq!((remote.posts, remote.patches), (1, 0));
    Ok(())
}

#[test]
fn explicit_known_uid_resume_bypasses_automatic_retry_gate_only() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut store = Store::open(&temp.path().join("store"))?;
    let compiled = compiled()?;
    store.submit(&compiled)?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let remote = Arc::new(Mutex::new(Remote {
        cancel_after_post: Some(cancelled.clone()),
        ..Default::default()
    }));
    let create = prepare(&mut store, &compiled, remote.clone())?;
    assert!(matches!(
        reconcile(
            &mut store,
            &compiled,
            &Validate,
            &mut registry(remote.clone()),
            &Options {
                approved_operation: Some(create.operation.clone()),
                cancelled,
                now: 1000,
                ..Default::default()
            }
        ),
        Err(Error::Cancelled)
    ));
    let mut state = store.unit("demo", "app")?.unwrap();
    state.attempts = 5;
    state.next_retry = 5000;
    store.save_unit("demo", "app", &state)?;
    for execute in [false, true] {
        reconcile(
            &mut store,
            &compiled,
            &Validate,
            &mut registry(remote.clone()),
            &Options {
                execute,
                now: 1000,
                ..Default::default()
            },
        )?;
    }
    assert_eq!(remote.lock().unwrap().patches, 0);
    assert_eq!(store.unit("demo", "app")?.unwrap().attempts, 5);
    apply(&mut store, &compiled, remote.clone(), &create)?;
    assert!(store.unit("demo", "app")?.unwrap().applied);
    assert_eq!(remote.lock().unwrap().patches, 1);
    Ok(())
}

#[test]
fn deletion_at_unresolved_cap_is_non_growing_and_interruption_never_replays() -> Result<()> {
    for lost_delete in [false, true] {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("store");
        let mut store = Store::open(&path)?;
        let mut compiled = compiled()?;
        compiled.instance.policy.allow_delete = true;
        compiled.instance.policy.deletion = Deletion::Delete;
        let template = compiled.instance.units["app"].configuration["items"][0].clone();
        let remote = Arc::new(Mutex::new(Remote::default()));
        for index in 0..16 {
            let mut object = template.clone();
            object["metadata"]["name"] = json!(format!("absent-{index:02}"));
            compiled
                .instance
                .units
                .get_mut("app")
                .unwrap()
                .configuration["items"] = json!([object]);
            compiled.units.get_mut("app").unwrap().artifact =
                compiled.instance.units["app"].configuration.clone();
            compiled.revision = compiled.digest()?;
            store.submit(&compiled)?;
            remote.lock().unwrap().lose_post = true;
            let plan = prepare(&mut store, &compiled, remote.clone())?;
            apply(&mut store, &compiled, remote.clone(), &plan)?;
            remote.lock().unwrap().objects.clear();
        }
        let mut known = template.clone();
        known["metadata"]["name"] = json!("a-known");
        let mut unknown = template.clone();
        unknown["metadata"]["name"] = json!("z-unknown");
        compiled
            .instance
            .units
            .get_mut("app")
            .unwrap()
            .configuration["items"] = json!([known, unknown]);
        compiled.units.get_mut("app").unwrap().artifact =
            compiled.instance.units["app"].configuration.clone();
        compiled.revision = compiled.digest()?;
        store.submit(&compiled)?;
        remote.lock().unwrap().lose_post_at = Some(18);
        let source = prepare(&mut store, &compiled, remote.clone())?;
        apply(&mut store, &compiled, remote.clone(), &source)?;
        let unknown_path = "/api/v1/namespaces/test/configmaps/z-unknown";
        let late = remote.lock().unwrap().objects.remove(unknown_path).unwrap();
        assert_eq!(remote.lock().unwrap().patches, 1);
        let mut extra = template.clone();
        extra["metadata"]["name"] = json!("extra-refused");
        compiled
            .instance
            .units
            .get_mut("app")
            .unwrap()
            .configuration["items"] = json!([extra]);
        compiled.units.get_mut("app").unwrap().artifact =
            compiled.instance.units["app"].configuration.clone();
        compiled.revision = compiled.digest()?;
        store.submit(&compiled)?;
        let growing = prepare(&mut store, &compiled, remote.clone())?;
        apply(&mut store, &compiled, remote.clone(), &growing)?;
        assert_eq!(
            remote.lock().unwrap().posts,
            18,
            "A growing source transition must refuse at the cap"
        );
        assert!(store
            .unit("demo", "app")?
            .unwrap()
            .failure
            .unwrap()
            .contains("budget"));
        store.mark_removed("demo", true)?;
        let deletion = prepare(&mut store, &compiled, remote.clone())?;
        assert_eq!(deletion.summary.deletes, 1);
        remote.lock().unwrap().lose_delete = lost_delete;
        apply(&mut store, &compiled, remote.clone(), &deletion)?;
        assert_eq!(
            remote.lock().unwrap().deletes,
            1,
            "{:?}",
            store.unit("demo", "app")?.unwrap().failure
        );
        drop(store);
        let mut store = Store::open(&path)?;
        if lost_delete {
            assert!(store.record("demo", "app", &deletion, "prepared").is_err());
            assert!(store.finish(&deletion.operation, "recovered").is_err());
            compiled.instance.display_name =
                Some("New revision after lost deletion response".into());
            compiled.revision = compiled.digest()?;
            store.submit(&compiled)?;
            remote.lock().unwrap().fail_get = true;
            reconcile(
                &mut store,
                &compiled,
                &Validate,
                &mut registry(remote.clone()),
                &Options {
                    execute: false,
                    now: 2000,
                    ..Default::default()
                },
            )?;
            assert_eq!(
                store.unit("demo", "app")?.unwrap().prepared.as_ref(),
                Some(&deletion)
            );
            remote.lock().unwrap().fail_get = false;
        }
        reconcile(
            &mut store,
            &compiled,
            &Validate,
            &mut registry(remote.clone()),
            &Options {
                execute: false,
                now: 3000,
                ..Default::default()
            },
        )?;
        let database = rusqlite::Connection::open_with_flags(
            path.join("store.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let counts:(i64,i64,i64)=database.query_row("SELECT sum(phase='superseded-unresolved'),sum(phase IN ('running','review-required')),sum(json_extract(change,'$.checkpoint.outstandingIntents')) FROM operations WHERE phase IN ('running','review-required','superseded-unresolved')",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?;
        assert_eq!(counts, (16, 1, 17));
        let recovered = store.unit("demo", "app")?.unwrap();
        assert!(!recovered.deleted);
        assert!(!recovered.recovery.as_ref().unwrap().removal_attempted);
        apply(&mut store, &compiled, remote.clone(), &deletion)?;
        assert_eq!(remote.lock().unwrap().deletes, 1);
        remote
            .lock()
            .unwrap()
            .objects
            .insert(unknown_path.into(), late);
        let binding = prepare(&mut store, &compiled, remote.clone())?;
        assert_eq!(binding.summary.recovery_bindings, 1);
        assert_eq!(binding.recovery_bindings()?[0].uid, "uid-18");
        apply(&mut store, &compiled, remote.clone(), &binding)?;
        assert_eq!(
            store.unit("demo", "app")?.unwrap().inventory[unknown_path]["uid"],
            "uid-18"
        );
        assert!(!store.unit("demo", "app")?.unwrap().deleted);
        let remote = remote.lock().unwrap();
        assert_eq!((remote.posts, remote.patches, remote.deletes), (18, 1, 1));
    }
    Ok(())
}

#[test]
fn completed_inplace_attempt_with_remaining_uid_requires_reachable_fresh_deletion() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut store = Store::open(&temp.path().join("store"))?;
    let mut compiled = compiled()?;
    compiled.instance.policy.allow_delete = true;
    compiled.instance.policy.deletion = Deletion::Delete;
    compiled.revision = compiled.digest()?;
    store.submit(&compiled)?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let remote = Arc::new(Mutex::new(Remote {
        cancel_after_post: Some(cancelled.clone()),
        ..Default::default()
    }));
    let create = prepare(&mut store, &compiled, remote.clone())?;
    assert!(matches!(
        reconcile(
            &mut store,
            &compiled,
            &Validate,
            &mut registry(remote.clone()),
            &Options {
                approved_operation: Some(create.operation),
                cancelled,
                now: 1000,
                ..Default::default()
            }
        ),
        Err(Error::Cancelled)
    ));
    compiled.instance.display_name = Some("Changed revision".into());
    compiled.revision = compiled.digest()?;
    store.submit(&compiled)?;
    store.mark_removed("demo", true)?;
    remote.lock().unwrap().delay_delete = true;
    let first = prepare(&mut store, &compiled, remote.clone())?;
    apply(&mut store, &compiled, remote.clone(), &first)?;
    assert!(!store.unit("demo", "app")?.unwrap().deleted);
    assert_eq!(remote.lock().unwrap().deletes, 1);
    remote.lock().unwrap().delay_delete = false;
    let second = prepare(&mut store, &compiled, remote.clone())?;
    assert_ne!(first.operation, second.operation);
    apply(&mut store, &compiled, remote.clone(), &second)?;
    assert!(store.unit("demo", "app")?.unwrap().deleted);
    assert_eq!(remote.lock().unwrap().deletes, 2);
    assert_eq!(remote.lock().unwrap().patches, 0);
    Ok(())
}

#[test]
fn missing_owned_uid_requires_replacement_policy_then_exact_predecessor() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut store = Store::open(&temp.path().join("store"))?;
    let mut compiled = compiled()?;
    store.submit(&compiled)?;
    let remote = Arc::new(Mutex::new(Remote::default()));
    let initial = prepare(&mut store, &compiled, remote.clone())?;
    apply(&mut store, &compiled, remote.clone(), &initial)?;
    remote.lock().unwrap().objects.clear();
    let replacement = prepare(&mut store, &compiled, remote.clone())?;
    assert_eq!(replacement.summary.replacements, 1);
    apply(&mut store, &compiled, remote.clone(), &replacement)?;
    assert_eq!(remote.lock().unwrap().posts, 1);
    assert!(store
        .unit("demo", "app")?
        .unwrap()
        .failure
        .unwrap()
        .contains("replacement"));
    compiled.instance.policy.allow_replace = true;
    compiled.revision = compiled.digest()?;
    store.submit(&compiled)?;
    let approved = prepare(&mut store, &compiled, remote.clone())?;
    remote.lock().unwrap().objects.insert(PATH.into(),json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"settings","namespace":"test","uid":"foreign","resourceVersion":"foreign"},"data":{}}));
    apply(&mut store, &compiled, remote.clone(), &approved)?;
    assert_eq!(remote.lock().unwrap().posts, 1);
    assert_eq!(
        store.unit("demo", "app")?.unwrap().inventory[PATH]["uid"],
        "uid-1"
    );
    remote.lock().unwrap().objects.clear();
    reconcile(
        &mut store,
        &compiled,
        &Validate,
        &mut registry(remote.clone()),
        &Options {
            execute: false,
            retry_failed: true,
            now: 2000,
            ..Default::default()
        },
    )?;
    let approved = prepare(&mut store, &compiled, remote.clone())?;
    apply(&mut store, &compiled, remote.clone(), &approved)?;
    let state = store.unit("demo", "app")?.unwrap();
    assert!(state.applied, "{:?}", state.failure);
    assert_eq!(state.inventory[PATH]["uid"], "uid-2");
    Ok(())
}

#[test]
fn candidate_uid_or_resource_version_change_invalidates_exact_binding() -> Result<()> {
    for field in ["uid", "resourceVersion"] {
        let temp = tempfile::tempdir()?;
        let mut store = Store::open(&temp.path().join("store"))?;
        let compiled = compiled()?;
        store.submit(&compiled)?;
        let remote = Arc::new(Mutex::new(Remote {
            lose_post: true,
            ..Default::default()
        }));
        let create = prepare(&mut store, &compiled, remote.clone())?;
        apply(&mut store, &compiled, remote.clone(), &create)?;
        let binding = prepare(&mut store, &compiled, remote.clone())?;
        remote.lock().unwrap().objects.get_mut(PATH).unwrap()["metadata"][field] =
            json!("changed-after-review");
        apply(&mut store, &compiled, remote.clone(), &binding)?;
        let state = store.unit("demo", "app")?.unwrap();
        assert!(state.failure.is_some());
        assert!(state
            .inventory
            .as_object()
            .is_none_or(|entries| entries.is_empty()));
        let remote = remote.lock().unwrap();
        assert_eq!((remote.posts, remote.patches), (1, 0));
    }
    Ok(())
}

#[test]
fn later_read_failure_preserves_earlier_candidate_before_optional_observation() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("store");
    let mut store = Store::open(&path)?;
    let mut compiled = compiled()?;
    let mut later = compiled.instance.units["app"].configuration["items"][0].clone();
    later["metadata"]["name"] = json!("z-later");
    compiled
        .instance
        .units
        .get_mut("app")
        .unwrap()
        .configuration["items"]
        .as_array_mut()
        .unwrap()
        .push(later);
    compiled.units.get_mut("app").unwrap().artifact =
        compiled.instance.units["app"].configuration.clone();
    compiled.revision = compiled.digest()?;
    store.submit(&compiled)?;
    let remote = Arc::new(Mutex::new(Remote {
        lose_post: true,
        ..Default::default()
    }));
    let create = prepare(&mut store, &compiled, remote.clone())?;
    apply(&mut store, &compiled, remote.clone(), &create)?;
    remote.lock().unwrap().fail_get_path =
        Some("/api/v1/namespaces/test/configmaps/z-later".into());
    reconcile(
        &mut store,
        &compiled,
        &Validate,
        &mut registry(remote.clone()),
        &Options {
            execute: false,
            now: 1000,
            ..Default::default()
        },
    )?;
    let state = store.unit("demo", "app")?.unwrap();
    assert_eq!(state.public()["recovery"]["reason"], "unrecordedIdentity");
    assert!(state.failure.is_some());
    drop(store);
    let mut store = Store::open(&path)?;
    remote.lock().unwrap().fail_get_path = None;
    let binding = prepare(&mut store, &compiled, remote.clone())?;
    assert_eq!(binding.recovery_bindings()?[0].uid, "uid-1");
    assert_eq!(remote.lock().unwrap().posts, 1);
    Ok(())
}

#[test]
fn concrete_initializer_staleness_requires_fresh_review_and_lost_postimage_never_replays(
) -> Result<()> {
    for completed in [false, true] {
        let temp = tempfile::tempdir()?;
        let mut store = Store::open(&temp.path().join("store"))?;
        let compiled = compiled()?;
        store.submit(&compiled)?;
        let remote = Arc::new(Mutex::new(Remote {
            fail_patch: !completed,
            lose_patch: completed,
            ..Default::default()
        }));
        let create = prepare(&mut store, &compiled, remote.clone())?;
        apply(&mut store, &compiled, remote.clone(), &create)?;
        {
            let mut remote = remote.lock().unwrap();
            remote.fail_patch = false;
            remote.objects.get_mut(PATH).unwrap()["metadata"]["resourceVersion"] =
                json!("later-version");
            if !completed {
                remote.objects.get_mut(PATH).unwrap()["data"]["foreign"] = json!("later-change");
            }
        }
        reconcile(
            &mut store,
            &compiled,
            &Validate,
            &mut registry(remote.clone()),
            &Options {
                retry_failed: true,
                now: 2000,
                ..Default::default()
            },
        )?;
        assert_eq!(remote.lock().unwrap().patches, usize::from(completed));
        if completed {
            assert!(store.running("demo", "app")?.is_none());
        } else {
            assert_eq!(
                store.unit("demo", "app")?.unwrap().public()["recovery"]["reason"],
                "staleContinuation"
            );
            let initializer = prepare(&mut store, &compiled, remote.clone())?;
            assert_eq!(initializer.payload["phase"], "initialize-known-v1");
            apply(&mut store, &compiled, remote.clone(), &initializer)?;
            assert_eq!(remote.lock().unwrap().patches, 1);
        }
    }
    Ok(())
}

#[test]
fn changed_revision_or_execution_snapshot_preserves_uid_but_invalidates_old_authority() -> Result<()>
{
    for change in ["revision", "credential", "trust", "after-response"] {
        let temp = tempfile::tempdir()?;
        let mut store = Store::open(&temp.path().join("store"))?;
        let mut compiled = compiled()?;
        store.submit(&compiled)?;
        let cancelled = Arc::new(AtomicBool::new(false));
        let remote = Arc::new(Mutex::new(Remote {
            cancel_after_post: if change == "after-response" {
                None
            } else {
                Some(cancelled.clone())
            },
            rotate_after_post: change == "after-response",
            ..Default::default()
        }));
        let create = prepare(&mut store, &compiled, remote.clone())?;
        let result = reconcile(
            &mut store,
            &compiled,
            &Validate,
            &mut registry(remote.clone()),
            &Options {
                approved_operation: Some(create.operation.clone()),
                cancelled,
                now: 1000,
                ..Default::default()
            },
        );
        if change != "after-response" {
            assert!(matches!(result, Err(Error::Cancelled)));
        } else {
            result?;
        }
        assert_eq!(
            store.unit("demo", "app")?.unwrap().inventory[PATH]["uid"],
            "uid-1"
        );
        if change == "revision" {
            compiled.instance.display_name = Some("changed".into());
            compiled.revision = compiled.digest()?;
            store.submit(&compiled)?;
        } else if change != "after-response" {
            remote.lock().unwrap().authority += 1;
        }
        remote.lock().unwrap().rotate_after_post = false;
        apply(&mut store, &compiled, remote.clone(), &create)?;
        assert_eq!(remote.lock().unwrap().patches, 0);
        let fresh = prepare(&mut store, &compiled, remote.clone())?;
        assert_eq!(fresh.payload["phase"], "initialize-known-v1");
        apply(&mut store, &compiled, remote.clone(), &fresh)?;
        assert_eq!(remote.lock().unwrap().patches, 1);
    }
    Ok(())
}

#[test]
fn removed_unit_preserves_unknown_creation_and_requires_binding_before_delete() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut store = Store::open(&temp.path().join("store"))?;
    let mut compiled = compiled()?;
    compiled.instance.policy.allow_delete = true;
    compiled.instance.policy.deletion = Deletion::Delete;
    compiled.revision = compiled.digest()?;
    store.submit(&compiled)?;
    let remote = Arc::new(Mutex::new(Remote {
        lose_post: true,
        ..Default::default()
    }));
    let create = prepare(&mut store, &compiled, remote.clone())?;
    apply(&mut store, &compiled, remote.clone(), &create)?;
    let mut retained = compiled.instance.units.remove("app").unwrap();
    retained.configuration["items"] = json!([]);
    let mut retained_artifact = compiled.units.remove("app").unwrap();
    retained_artifact.artifact = retained.configuration.clone();
    compiled.instance.units.insert("retained".into(), retained);
    compiled.units.insert("retained".into(), retained_artifact);
    compiled.revision = compiled.digest()?;
    store.submit(&compiled)?;
    let binding = prepare(&mut store, &compiled, remote.clone())?;
    assert_eq!(binding.summary.recovery_bindings, 1);
    apply(&mut store, &compiled, remote.clone(), &binding)?;
    let removal = prepare(&mut store, &compiled, remote.clone())?;
    assert_eq!(removal.summary.deletes, 1);
    apply(&mut store, &compiled, remote.clone(), &removal)?;
    assert!(store.unit("demo", "app")?.unwrap().deleted);
    let remote = remote.lock().unwrap();
    assert_eq!((remote.posts, remote.patches, remote.deletes), (1, 0, 1));
    Ok(())
}

#[test]
fn blocked_dependency_keeps_known_uid_evidence_and_prevents_initializer() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut store = Store::open(&temp.path().join("store"))?;
    let mut compiled = compiled()?;
    let desired = compiled.instance.units["app"].configuration.clone();
    let mut provider = compiled.instance.units["app"].clone();
    provider.configuration["items"][0]["metadata"]["name"] = json!("provider");
    let mut artifact = compiled.units["app"].clone();
    artifact.artifact = provider.configuration.clone();
    compiled.instance.units.insert("provider".into(), provider);
    compiled.units.insert("provider".into(), artifact);
    compiled
        .instance
        .units
        .get_mut("app")
        .unwrap()
        .depends_on
        .push(Dependency {
            unit: "provider".into(),
            condition: Gate::Applied,
        });
    compiled
        .instance
        .units
        .get_mut("app")
        .unwrap()
        .configuration["items"] = json!([]);
    compiled.units.get_mut("app").unwrap().artifact =
        compiled.instance.units["app"].configuration.clone();
    compiled.revision = compiled.digest()?;
    store.submit(&compiled)?;
    let remote = Arc::new(Mutex::new(Remote::default()));
    reconcile(
        &mut store,
        &compiled,
        &Validate,
        &mut registry(remote.clone()),
        &Options {
            now: 1000,
            ..Default::default()
        },
    )?;
    assert_eq!(remote.lock().unwrap().posts, 1);
    compiled
        .instance
        .units
        .get_mut("app")
        .unwrap()
        .configuration = desired.clone();
    compiled.units.get_mut("app").unwrap().artifact = desired;
    compiled.revision = compiled.digest()?;
    store.submit(&compiled)?;
    let create = prepare(&mut store, &compiled, remote.clone())?;
    let cancelled = Arc::new(AtomicBool::new(false));
    remote.lock().unwrap().cancel_after_post = Some(cancelled.clone());
    assert!(matches!(
        reconcile(
            &mut store,
            &compiled,
            &Validate,
            &mut registry(remote.clone()),
            &Options {
                approved_operation: Some(create.operation.clone()),
                cancelled,
                now: 1000,
                ..Default::default()
            }
        ),
        Err(Error::Cancelled)
    ));
    remote.lock().unwrap().fail_get_path =
        Some("/api/v1/namespaces/test/configmaps/provider".into());
    apply(&mut store, &compiled, remote.clone(), &create)?;
    let state = store.unit("demo", "app")?.unwrap();
    assert_eq!(state.inventory[PATH]["uid"], "uid-2");
    assert!(state.waiting.is_some());
    assert_eq!(remote.lock().unwrap().patches, 1);
    remote.lock().unwrap().fail_get_path = None;
    reconcile(
        &mut store,
        &compiled,
        &Validate,
        &mut registry(remote.clone()),
        &Options {
            approved_operation: Some(create.operation),
            now: 2000,
            ..Default::default()
        },
    )?;
    assert_eq!(remote.lock().unwrap().patches, 2);
    Ok(())
}

#[test]
fn successful_post_response_identity_is_validated_before_ownership_and_bad_fields_keep_uid(
) -> Result<()> {
    for corruption in ["identity", "fields"] {
        let temp = tempfile::tempdir()?;
        let mut store = Store::open(&temp.path().join("store"))?;
        let compiled = compiled()?;
        store.submit(&compiled)?;
        let remote = Arc::new(Mutex::new(Remote {
            corrupt_post_response: Some(corruption.into()),
            ..Default::default()
        }));
        let create = prepare(&mut store, &compiled, remote.clone())?;
        apply(&mut store, &compiled, remote.clone(), &create)?;
        let state = store.unit("demo", "app")?.unwrap();
        assert!(state.failure.is_some());
        if corruption == "identity" {
            assert!(state
                .inventory
                .as_object()
                .is_none_or(|entries| entries.is_empty()));
        } else {
            assert_eq!(state.inventory[PATH]["uid"], "uid-1");
        }
        let remote = remote.lock().unwrap();
        assert_eq!((remote.posts, remote.patches), (1, 0));
    }
    Ok(())
}

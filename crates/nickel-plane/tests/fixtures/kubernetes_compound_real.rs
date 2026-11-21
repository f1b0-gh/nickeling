//! Actual Store/controller compound protocol, distinct from legacy handoff tests.
#![allow(clippy::unwrap_used)]
use super::{connection, foreign_fields, object};
use nickel_plane::{
    adapters::kubernetes::{Api, Http, Kubernetes},
    backend::*,
    composition::{Compiled, CompiledUnit, Validator},
    model::Instance,
    reconcile::{reconcile, Options, Registry},
    store::Store,
    targets::KubernetesTarget,
    Error, Result,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
#[derive(Default)]
struct Requests {
    posts: usize,
    patches: usize,
    deletes: usize,
    mutation_attempts: usize,
    fail_patch: bool,
    race_post: bool,
    raced_uid: Option<String>,
    lose_post: bool,
    lose_patch: bool,
}
struct ObservedApi {
    http: Http,
    requests: Arc<Mutex<Requests>>,
}
impl Api for ObservedApi {
    fn validate_target(&self, target: &KubernetesTarget) -> Result<()> {
        self.http.validate_target(target)
    }
    fn execution_fingerprint(&self, target: &KubernetesTarget) -> Result<String> {
        self.http.execution_fingerprint(target)
    }
    fn legacy_fingerprint(&self, target: &KubernetesTarget) -> Result<String> {
        self.http.legacy_fingerprint(target)
    }
    fn request(&mut self, method: &str, path: &str, body: Option<&Value>) -> Result<(u16, Value)> {
        let mutation = matches!(method, "POST" | "PATCH" | "DELETE");
        let dry_run = path.contains("dryRun=All");
        if mutation {
            let mut requests = self
                .requests
                .lock()
                .map_err(|_| Error::Backend("Fixture request lock".into()))?;
            requests.mutation_attempts += 1;
            if !dry_run {
                match method {
                    "POST" => requests.posts += 1,
                    "PATCH" => requests.patches += 1,
                    "DELETE" => requests.deletes += 1,
                    _ => {}
                }
                if method == "PATCH" && std::mem::take(&mut requests.fail_patch) {
                    return Err(Error::Backend(
                        "Injected transport failure before PATCH".into(),
                    ));
                }
            }
        }
        let race = method == "POST"
            && !dry_run
            && std::mem::take(
                &mut self
                    .requests
                    .lock()
                    .map_err(|_| Error::Backend("Fixture request lock".into()))?
                    .race_post,
            );
        if race {
            let body = body.ok_or_else(|| Error::Invalid("Missing raced POST body".into()))?;
            let mut foreign = body.clone();
            foreign["metadata"] =
                json!({"name":body["metadata"]["name"],"namespace":body["metadata"]["namespace"]});
            let collection = path.split('?').next().unwrap_or(path);
            let (status, created) = self.http.request(
                "POST",
                &format!("{collection}?fieldManager=foreign"),
                Some(&foreign),
            )?;
            assert_eq!(status, 201);
            self.requests.lock().unwrap().raced_uid =
                Some(created["metadata"]["uid"].as_str().unwrap().into());
        }
        let response = self.http.request(method, path, body)?;
        if !dry_run && (200..300).contains(&response.0) {
            let mut requests = self
                .requests
                .lock()
                .map_err(|_| Error::Backend("Fixture request lock".into()))?;
            let lose = match method {
                "POST" => std::mem::take(&mut requests.lose_post),
                "PATCH" => std::mem::take(&mut requests.lose_patch),
                _ => false,
            };
            if lose {
                return Err(Error::Backend(
                    "Injected lost successful API response".into(),
                ));
            }
        }
        Ok(response)
    }
}
struct Validate;
impl Validator for Validate {
    fn validate(
        &self,
        _: &nickel_plane::model::Unit,
        value: &Value,
        base: &CompiledUnit,
    ) -> Result<CompiledUnit> {
        let mut effective = base.clone();
        effective.artifact = value.clone();
        Ok(effective)
    }
}
fn compiled(name: &str) -> Result<Compiled> {
    let artifact = json!({"apiVersion":"v1","kind":"List","items":[object(name)]});
    let instance: Instance = serde_json::from_value(
        json!({"apiVersion":"nickel-plane/v1","id":name,"composition":{"name":"kind","version":"1"},"policy":{"mode":"manage"},"units":{"app":{"backend":"kubernetes","target":"cluster","configuration":artifact}}}),
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
fn registry(requests: Arc<Mutex<Requests>>) -> Result<Registry> {
    let Kubernetes { api: http, target } = connection()?;
    Ok(BTreeMap::from([(
        "cluster".into(),
        Box::new(Kubernetes {
            api: ObservedApi { http, requests },
            target,
        }) as Box<dyn Backend>,
    )]))
}
fn cycle(
    store: &mut Store,
    compiled: &Compiled,
    requests: Arc<Mutex<Requests>>,
    options: Options,
) -> Result<()> {
    reconcile(
        store,
        compiled,
        &Validate,
        &mut registry(requests)?,
        &options,
    )
}
fn plan(
    store: &mut Store,
    compiled: &Compiled,
    requests: Arc<Mutex<Requests>>,
) -> Result<PreparedChange> {
    cycle(
        store,
        compiled,
        requests,
        Options {
            execute: false,
            prepare: true,
            now: 10000,
            ..Default::default()
        },
    )?;
    let state = store
        .unit(&compiled.instance.id, "app")?
        .ok_or_else(|| Error::Invalid("Missing real fixture state".into()))?;
    state
        .prepared
        .ok_or_else(|| Error::Invalid(format!("Missing real fixture plan: {:?}", state.failure)))
}
fn apply(
    store: &mut Store,
    compiled: &Compiled,
    requests: Arc<Mutex<Requests>>,
    plan: &PreparedChange,
) -> Result<()> {
    cycle(
        store,
        compiled,
        requests,
        Options {
            approved_operation: Some(plan.operation.clone()),
            now: 10000,
            ..Default::default()
        },
    )
}
fn route(name: &str) -> String {
    format!("/api/v1/namespaces/nickeling-api/configmaps/{name}")
}
#[test]
#[ignore = "requires an owned kind cluster; use scripts/test-kind.py"]
fn real_compound_create_then_ssa_update_omission_preserves_foreign_owner() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut store = Store::open(&temp.path().join("store"))?;
    let mut compiled = compiled("compound-normal")?;
    store.submit(&compiled)?;
    let requests = Arc::new(Mutex::new(Requests::default()));
    let create = plan(&mut store, &compiled, requests.clone())?;
    assert_eq!(create.payload["phase"], "create-initialize-v1");
    apply(&mut store, &compiled, requests.clone(), &create)?;
    assert!(store.unit(&compiled.instance.id, "app")?.unwrap().applied);
    let mut inspect = connection()?;
    let path = route("compound-normal");
    let (_, created) = inspect.api.request("GET", &path, None)?;
    assert!(created["metadata"]["managedFields"]
        .as_array()
        .unwrap()
        .iter()
        .any(|field| field["operation"] == "Apply"
            && field["manager"]
                .as_str()
                .unwrap_or_default()
                .starts_with("nickel-plane-")));
    let foreign = json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"compound-normal","namespace":"nickeling-api"},"data":{"foreign":"preserved"}});
    assert_eq!(
        inspect
            .api
            .request(
                "PATCH",
                &format!("{path}?fieldManager=foreign"),
                Some(&foreign)
            )?
            .0,
        200
    );
    let (_, before) = inspect.api.request("GET", &path, None)?;
    let configuration = &mut compiled
        .instance
        .units
        .get_mut("app")
        .unwrap()
        .configuration;
    configuration["items"][0]["data"] = json!({"value":"two"});
    compiled.units.get_mut("app").unwrap().artifact = configuration.clone();
    compiled.revision = compiled.digest()?;
    store.submit(&compiled)?;
    let update = plan(&mut store, &compiled, requests.clone())?;
    assert_eq!(update.summary.field_ownership_transfers, 0);
    apply(&mut store, &compiled, requests.clone(), &update)?;
    let (_, after) = inspect.api.request("GET", &path, None)?;
    assert_eq!(after["data"]["value"], "two");
    assert!(after["data"].get("remove").is_none());
    assert_eq!(after["data"]["foreign"], "preserved");
    assert_eq!(foreign_fields(&before), foreign_fields(&after));
    let requests = requests.lock().unwrap();
    assert_eq!(
        (requests.posts, requests.patches, requests.deletes),
        (1, 2, 0)
    );
    Ok(())
}
#[test]
#[ignore = "requires an owned kind cluster; use scripts/test-kind.py"]
fn real_lost_post_requires_binding_then_fresh_initializer() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("store");
    let compiled = compiled("compound-lost-post")?;
    let requests = Arc::new(Mutex::new(Requests {
        lose_post: true,
        ..Default::default()
    }));
    {
        let mut store = Store::open(&path)?;
        store.submit(&compiled)?;
        let create = plan(&mut store, &compiled, requests.clone())?;
        apply(&mut store, &compiled, requests.clone(), &create)?;
        assert!(store
            .unit(&compiled.instance.id, "app")?
            .unwrap()
            .inventory
            .as_object()
            .is_none_or(|entries| entries.is_empty()));
    }
    let mut store = Store::open(&path)?;
    let binding = plan(&mut store, &compiled, requests.clone())?;
    assert_eq!(binding.payload["phase"], "recovery-bind-v1");
    let mut inspect = connection()?;
    let (_, object) = inspect
        .api
        .request("GET", &route("compound-lost-post"), None)?;
    assert_eq!(
        binding.recovery_bindings()?[0].uid,
        object["metadata"]["uid"].as_str().unwrap()
    );
    let attempts = requests.lock().unwrap().mutation_attempts;
    apply(&mut store, &compiled, requests.clone(), &binding)?;
    assert_eq!(requests.lock().unwrap().mutation_attempts, attempts);
    assert_eq!(requests.lock().unwrap().patches, 0);
    let initializer = plan(&mut store, &compiled, requests.clone())?;
    assert_eq!(initializer.payload["phase"], "initialize-known-v1");
    apply(&mut store, &compiled, requests.clone(), &initializer)?;
    assert!(store.unit(&compiled.instance.id, "app")?.unwrap().applied);
    let requests = requests.lock().unwrap();
    assert_eq!(
        (requests.posts, requests.patches, requests.deletes),
        (1, 1, 0)
    );
    Ok(())
}
#[test]
#[ignore = "requires an owned kind cluster; use scripts/test-kind.py"]
fn real_lost_initializer_response_recovers_known_uid_without_replay() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("store");
    let compiled = compiled("compound-lost-patch")?;
    let requests = Arc::new(Mutex::new(Requests {
        lose_patch: true,
        ..Default::default()
    }));
    {
        let mut store = Store::open(&path)?;
        store.submit(&compiled)?;
        let create = plan(&mut store, &compiled, requests.clone())?;
        apply(&mut store, &compiled, requests.clone(), &create)?;
        assert!(store.running(&compiled.instance.id, "app")?.is_some());
        assert!(store
            .unit(&compiled.instance.id, "app")?
            .unwrap()
            .inventory
            .get(route("compound-lost-patch"))
            .is_some());
    }
    let mut store = Store::open(&path)?;
    for _ in 0..2 {
        cycle(
            &mut store,
            &compiled,
            requests.clone(),
            Options {
                execute: false,
                now: 20000,
                ..Default::default()
            },
        )?;
    }
    assert!(store.running(&compiled.instance.id, "app")?.is_none());
    assert!(store.unit(&compiled.instance.id, "app")?.unwrap().applied);
    let requests = requests.lock().unwrap();
    assert_eq!(
        (requests.posts, requests.patches, requests.deletes),
        (1, 1, 0)
    );
    Ok(())
}

/// Storage/quantity tests use default durable creation, then retain its exact
/// owner and inventory for their focused read/update assertions.
pub(super) fn create_context(context: &mut Context) -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut store = Store::open(&temp.path().join("store"))?;
    let mut compiled = compiled(&context.unit_id)?;
    compiled
        .instance
        .units
        .get_mut("app")
        .unwrap()
        .configuration = context.artifact.clone();
    compiled.units.get_mut("app").unwrap().artifact = context.artifact.clone();
    compiled.revision = compiled.digest()?;
    store.submit(&compiled)?;
    let requests = Arc::new(Mutex::new(Requests::default()));
    let create = plan(&mut store, &compiled, requests.clone())?;
    assert_eq!(create.payload["phase"], "create-initialize-v1");
    apply(&mut store, &compiled, requests.clone(), &create)?;
    // Workload status may race the initializer. Before a concrete patch was
    // saved, the same immutable create approval can resume its known UID. A
    // stale concrete patch instead requires a fresh initializer approval.
    assert_eq!(requests.lock().unwrap().posts, 1);
    let known = store.unit(&compiled.instance.id, "app")?.unwrap().inventory;
    assert!(known.as_object().is_some_and(|entries| !entries.is_empty()));
    for _ in 0..3 {
        let state = store.unit(&compiled.instance.id, "app")?.unwrap();
        if state.inventory.as_object().is_some_and(|entries| {
            entries
                .values()
                .all(|entry| entry["pendingInitialization"] != true)
        }) {
            break;
        }
        let initializer = plan(&mut store, &compiled, requests.clone())?;
        if initializer.payload["phase"] == "create-initialize-v1" {
            assert_eq!(initializer, create, "Only the original approval may resume");
        } else {
            assert_eq!(
                initializer.payload["phase"], "initialize-known-v1",
                "{:?}",
                state.failure
            );
        }
        apply(&mut store, &compiled, requests.clone(), &initializer)?;
        assert_eq!(
            requests.lock().unwrap().posts,
            1,
            "Known UID must never be recreated"
        );
        let current = store.unit(&compiled.instance.id, "app")?.unwrap().inventory;
        for (path, entry) in known.as_object().unwrap() {
            assert_eq!(current[path]["uid"], entry["uid"]);
        }
    }
    let state = store.unit(&compiled.instance.id, "app")?.unwrap();
    assert!(
        state
            .inventory
            .as_object()
            .is_some_and(|entries| !entries.is_empty()
                && entries
                    .values()
                    .all(|entry| entry["pendingInitialization"] != true)),
        "{:?}",
        state.failure
    );
    assert_eq!(requests.lock().unwrap().posts, 1);
    context.owner = store.owner.clone();
    context.instance = compiled.instance.id;
    context.unit_id = "app".into();
    context.revision = compiled.revision;
    context.artifact_sha256 = state.artifact_sha256;
    context.inventory = state.inventory;
    Ok(())
}

#[test]
#[ignore = "requires an owned kind cluster; use scripts/test-kind.py"]
fn real_compound_binding_refuses_changed_candidate_resource_version() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut store = Store::open(&temp.path().join("store"))?;
    let compiled = compiled("compound-stale-binding")?;
    store.submit(&compiled)?;
    let requests = Arc::new(Mutex::new(Requests {
        lose_post: true,
        ..Default::default()
    }));
    let create = plan(&mut store, &compiled, requests.clone())?;
    apply(&mut store, &compiled, requests.clone(), &create)?;
    let binding = plan(&mut store, &compiled, requests.clone())?;
    let mut inspect = connection()?;
    let foreign = json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"compound-stale-binding","namespace":"nickeling-api"},"data":{"foreign":"changed-after-review"}});
    assert_eq!(
        inspect
            .api
            .request(
                "PATCH",
                &format!("{}?fieldManager=foreign", route("compound-stale-binding")),
                Some(&foreign)
            )?
            .0,
        200
    );
    let attempts = requests.lock().unwrap().mutation_attempts;
    apply(&mut store, &compiled, requests.clone(), &binding)?;
    assert_eq!(requests.lock().unwrap().mutation_attempts, attempts);
    let state = store.unit(&compiled.instance.id, "app")?.unwrap();
    assert!(state.failure.is_some());
    assert!(state
        .inventory
        .as_object()
        .is_none_or(|entries| entries.is_empty()));
    let requests = requests.lock().unwrap();
    assert_eq!(
        (requests.posts, requests.patches, requests.deletes),
        (1, 0, 0)
    );
    Ok(())
}

#[test]
#[ignore = "requires an owned kind cluster; use scripts/test-kind.py"]
fn real_compound_refuses_foreign_create_race() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut store = Store::open(&temp.path().join("store"))?;
    let compiled = compiled("compound-create-race")?;
    store.submit(&compiled)?;
    let requests = Arc::new(Mutex::new(Requests {
        race_post: true,
        ..Default::default()
    }));
    let create = plan(&mut store, &compiled, requests.clone())?;
    assert_eq!(create.payload["phase"], "create-initialize-v1");
    let mut inspect = connection()?;
    apply(&mut store, &compiled, requests.clone(), &create)?;
    let state = store.unit(&compiled.instance.id, "app")?.unwrap();
    assert!(state.failure.is_some());
    assert!(state
        .inventory
        .as_object()
        .is_none_or(|entries| entries.is_empty()));
    let (_, actual) = inspect
        .api
        .request("GET", &route("compound-create-race"), None)?;
    assert_eq!(
        requests.lock().unwrap().raced_uid.as_deref(),
        actual["metadata"]["uid"].as_str()
    );
    assert!(actual["metadata"]["labels"]
        .get("nickel-plane.io/owner")
        .is_none());
    let requests = requests.lock().unwrap();
    assert_eq!(
        (requests.posts, requests.patches, requests.deletes),
        (1, 0, 0)
    );
    Ok(())
}

#[test]
#[ignore = "requires an owned kind cluster; use scripts/test-kind.py"]
fn real_compound_copied_markers_require_new_exact_uid_review() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("store");
    let compiled = compiled("compound-copied-markers")?;
    let requests = Arc::new(Mutex::new(Requests {
        lose_post: true,
        ..Default::default()
    }));
    let mut store = Store::open(&path)?;
    store.submit(&compiled)?;
    let create = plan(&mut store, &compiled, requests.clone())?;
    apply(&mut store, &compiled, requests.clone(), &create)?;
    let old_binding = plan(&mut store, &compiled, requests.clone())?;
    let mut inspect = connection()?;
    let object_route = route("compound-copied-markers");
    let (_, original) = inspect.api.request("GET", &object_route, None)?;
    let manager = original["metadata"]["managedFields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| {
            field["manager"]
                .as_str()
                .unwrap_or_default()
                .starts_with("nickel-plane-")
        })
        .unwrap()["manager"]
        .as_str()
        .unwrap();
    let deletion = json!({"apiVersion":"v1","kind":"DeleteOptions","preconditions":{"uid":original["metadata"]["uid"]}});
    assert!((200..300).contains(
        &inspect
            .api
            .request("DELETE", &object_route, Some(&deletion))?
            .0
    ));
    assert_eq!(inspect.api.request("GET", &object_route, None)?.0, 404);
    let mut copied = object("compound-copied-markers");
    copied["metadata"]["labels"] = original["metadata"]["labels"].clone();
    copied["metadata"]["annotations"] = original["metadata"]["annotations"].clone();
    let (status, replacement) = inspect.api.request(
        "POST",
        &format!("/api/v1/namespaces/nickeling-api/configmaps?fieldManager={manager}"),
        Some(&copied),
    )?;
    assert_eq!(status, 201);
    assert_ne!(original["metadata"]["uid"], replacement["metadata"]["uid"]);
    drop(store);
    let mut store = Store::open(&path)?;
    let attempts = requests.lock().unwrap().mutation_attempts;
    apply(&mut store, &compiled, requests.clone(), &old_binding)?;
    assert_eq!(requests.lock().unwrap().mutation_attempts, attempts);
    let state = store.unit(&compiled.instance.id, "app")?.unwrap();
    assert!(state.failure.is_some());
    assert!(state
        .inventory
        .as_object()
        .is_none_or(|entries| entries.is_empty()));
    cycle(
        &mut store,
        &compiled,
        requests.clone(),
        Options {
            retry_failed: true,
            now: 20000,
            ..Default::default()
        },
    )?;
    assert_eq!(requests.lock().unwrap().mutation_attempts, attempts);
    assert!(store
        .unit(&compiled.instance.id, "app")?
        .unwrap()
        .inventory
        .as_object()
        .is_none_or(|entries| entries.is_empty()));
    let binding = plan(&mut store, &compiled, requests.clone())?;
    assert_eq!(binding.payload["phase"], "recovery-bind-v1");
    assert_eq!(
        binding.recovery_bindings()?[0].uid,
        replacement["metadata"]["uid"].as_str().unwrap()
    );
    let attempts = requests.lock().unwrap().mutation_attempts;
    apply(&mut store, &compiled, requests.clone(), &binding)?;
    assert_eq!(requests.lock().unwrap().mutation_attempts, attempts);
    let state = store.unit(&compiled.instance.id, "app")?.unwrap();
    assert_eq!(
        state.inventory[&object_route]["uid"],
        replacement["metadata"]["uid"]
    );
    assert!(state.inventory[&object_route]["pendingInitialization"]
        .as_bool()
        .unwrap());
    let requests = requests.lock().unwrap();
    assert_eq!(
        (requests.posts, requests.patches, requests.deletes),
        (1, 0, 0)
    );
    Ok(())
}

#[test]
#[ignore = "requires an owned kind cluster; use scripts/test-kind.py"]
fn real_compound_stale_initializer_requires_fresh_review() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("store");
    let compiled = compiled("compound-stale-initializer")?;
    let requests = Arc::new(Mutex::new(Requests {
        fail_patch: true,
        ..Default::default()
    }));
    {
        let mut store = Store::open(&path)?;
        store.submit(&compiled)?;
        let create = plan(&mut store, &compiled, requests.clone())?;
        apply(&mut store, &compiled, requests.clone(), &create)?;
        assert!(store.running(&compiled.instance.id, "app")?.is_some());
        assert_eq!(requests.lock().unwrap().patches, 1);
    }
    let mut inspect = connection()?;
    let object_route = route("compound-stale-initializer");
    let foreign = json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"compound-stale-initializer","namespace":"nickeling-api"},"data":{"foreign":"after-checkpoint"}});
    assert_eq!(
        inspect
            .api
            .request(
                "PATCH",
                &format!("{object_route}?fieldManager=foreign"),
                Some(&foreign)
            )?
            .0,
        200
    );
    let (_, before) = inspect.api.request("GET", &object_route, None)?;
    let mut store = Store::open(&path)?;
    let attempts = requests.lock().unwrap().mutation_attempts;
    cycle(
        &mut store,
        &compiled,
        requests.clone(),
        Options {
            retry_failed: true,
            now: 20000,
            ..Default::default()
        },
    )?;
    assert_eq!(requests.lock().unwrap().mutation_attempts, attempts);
    assert_eq!(
        store.unit(&compiled.instance.id, "app")?.unwrap().public()["recovery"]["reason"],
        "staleContinuation"
    );
    let initializer = plan(&mut store, &compiled, requests.clone())?;
    assert_eq!(initializer.payload["phase"], "initialize-known-v1");
    apply(&mut store, &compiled, requests.clone(), &initializer)?;
    assert!(store.unit(&compiled.instance.id, "app")?.unwrap().applied);
    let (_, after) = inspect.api.request("GET", &object_route, None)?;
    assert_eq!(before["data"], after["data"]);
    assert_eq!(foreign_fields(&before), foreign_fields(&after));
    let requests = requests.lock().unwrap();
    assert_eq!(
        (requests.posts, requests.patches, requests.deletes),
        (1, 2, 0)
    );
    Ok(())
}

#[test]
#[ignore = "requires an owned kind cluster; use scripts/test-kind.py"]
fn real_compound_mixed_omission_remains_deferred_across_interruptions() -> Result<()> {
    for interruption in ["none", "post", "patch"] {
        let name = format!("compound-mixed-{interruption}");
        let added = format!("{name}-new");
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("store");
        let mut store = Store::open(&path)?;
        let mut compiled = compiled(&name)?;
        store.submit(&compiled)?;
        let requests = Arc::new(Mutex::new(Requests::default()));
        let initial = plan(&mut store, &compiled, requests.clone())?;
        apply(&mut store, &compiled, requests.clone(), &initial)?;
        assert!(store.unit(&compiled.instance.id, "app")?.unwrap().applied);
        let previous = store
            .unit(&compiled.instance.id, "app")?
            .unwrap()
            .applied_sha256;
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
        configuration["items"]
            .as_array_mut()
            .unwrap()
            .push(object(&added));
        compiled.units.get_mut("app").unwrap().artifact = configuration.clone();
        compiled.revision = compiled.digest()?;
        store.submit(&compiled)?;
        let create = plan(&mut store, &compiled, requests.clone())?;
        assert_eq!(create.payload["phase"], "create-initialize-v1");
        assert!(create.summary.deferred_changes);
        {
            let mut requests = requests.lock().unwrap();
            requests.lose_post = interruption == "post";
            requests.lose_patch = interruption == "patch";
        }
        apply(&mut store, &compiled, requests.clone(), &create)?;
        drop(store);
        let mut store = Store::open(&path)?;
        if interruption == "post" {
            let binding = plan(&mut store, &compiled, requests.clone())?;
            assert!(binding.summary.deferred_changes);
            let attempts = requests.lock().unwrap().mutation_attempts;
            apply(&mut store, &compiled, requests.clone(), &binding)?;
            assert_eq!(requests.lock().unwrap().mutation_attempts, attempts);
            let initializer = plan(&mut store, &compiled, requests.clone())?;
            assert_eq!(initializer.payload["phase"], "initialize-known-v1");
            assert!(initializer.summary.deferred_changes);
            apply(&mut store, &compiled, requests.clone(), &initializer)?;
        } else if interruption == "patch" {
            cycle(
                &mut store,
                &compiled,
                requests.clone(),
                Options {
                    execute: false,
                    now: 20000,
                    ..Default::default()
                },
            )?;
        }
        let state = store.unit(&compiled.instance.id, "app")?.unwrap();
        assert!(!state.applied);
        assert_eq!(state.applied_sha256, previous);
        let mut inspect = connection()?;
        let (_, before) = inspect.api.request("GET", &route(&name), None)?;
        assert_eq!(before["data"]["remove"], "old");
        let update = plan(&mut store, &compiled, requests.clone())?;
        assert_eq!(update.summary.creates, 0);
        assert_eq!(update.summary.field_ownership_transfers, 0);
        apply(&mut store, &compiled, requests.clone(), &update)?;
        let (_, after) = inspect.api.request("GET", &route(&name), None)?;
        assert!(after["data"].get("remove").is_none());
        assert!(store.unit(&compiled.instance.id, "app")?.unwrap().applied);
        let requests = requests.lock().unwrap();
        assert_eq!(requests.posts, 2);
        assert_eq!(requests.deletes, 0);
    }
    Ok(())
}

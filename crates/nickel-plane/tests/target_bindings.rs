#![allow(clippy::unwrap_used)]
#![cfg(feature = "kubernetes")]
#[path = "fixtures/kubernetes_legacy.rs"]
mod legacy;
use nickel_plane::{
    adapters::kubernetes::{Api, Kubernetes},
    backend::Backend,
    composition::{Compiled, CompiledUnit, Validator},
    model::{Instance, Unit},
    reconcile::{reconcile, Options, Registry, TargetBinding},
    store::Store,
    targets::KubernetesTarget,
    Error, Result,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct Remote {
    objects: BTreeMap<String, Value>,
    creates: Vec<String>,
    writes: usize,
    interrupt: bool,
}
struct MemoryApi(Arc<Mutex<Remote>>);
impl Api for MemoryApi {
    fn request(
        &mut self,
        method: &str,
        request_path: &str,
        body: Option<&Value>,
    ) -> Result<(u16, Value)> {
        let dry = request_path.contains("dryRun=All");
        let manager = request_path
            .split("fieldManager=")
            .nth(1)
            .unwrap_or("fixture")
            .split('&')
            .next()
            .unwrap();
        let path = request_path.split('?').next().unwrap();
        let mut remote = self.0.lock().unwrap();
        if method == "GET" {
            return Ok(remote
                .objects
                .get(path)
                .map(|v| (200, v.clone()))
                .unwrap_or((404, Value::Null)));
        }
        let body = body.unwrap();
        let route = if method == "POST" {
            format!("{path}/{}", body["metadata"]["name"].as_str().unwrap())
        } else {
            path.into()
        };
        if method == "POST" && remote.objects.contains_key(&route) {
            return Ok((409, Value::Null));
        }
        if method == "DELETE" {
            let old = remote.objects.get(&route).unwrap();
            if old["metadata"]["uid"] != body["preconditions"]["uid"]
                || old["metadata"]["resourceVersion"] != body["preconditions"]["resourceVersion"]
            {
                return Ok((409, Value::Null));
            }
            if !dry {
                remote.objects.remove(&route);
                remote.writes += 1;
                if std::mem::take(&mut remote.interrupt) {
                    return Err(Error::Backend("interrupted deletion response".into()));
                }
            }
            return Ok((200, json!({})));
        }
        let mut object = if method == "POST" {
            let mut object = body.clone();
            object["metadata"]["uid"] = json!(format!("uid-{}", remote.writes + 1));
            object["metadata"]["managedFields"] = json!([{"manager":manager,"operation":"Update","apiVersion":body["apiVersion"],"fieldsType":"FieldsV1","fieldsV1":{"f:data":{"f:value":{}}}}]);
            object
        } else {
            let Some(old) = remote.objects.get(&route) else {
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
                        _ => return Err(Error::Invalid("Unexpected binding fixture patch".into())),
                    }
                }
                object
            } else {
                if body["metadata"]["uid"] != old["metadata"]["uid"]
                    || body["metadata"]["resourceVersion"] != old["metadata"]["resourceVersion"]
                {
                    return Ok((409, Value::Null));
                }
                let mut object = old.clone();
                object["data"] = body["data"].clone();
                object["metadata"]["annotations"] = body["metadata"]["annotations"].clone();
                object
            }
        };
        object["metadata"]["resourceVersion"] = json!((remote.writes + 1).to_string());
        if !dry {
            remote.writes += 1;
            if method == "POST" {
                remote.creates.push(route.clone());
            }
            remote.objects.insert(route, object.clone());
            if std::mem::take(&mut remote.interrupt) {
                return Err(Error::Backend("interrupted response".into()));
            }
        }
        Ok((if method == "POST" { 201 } else { 200 }, object))
    }
}

struct Validate;
impl Validator for Validate {
    fn validate(&self, _: &Unit, value: &Value, base: &CompiledUnit) -> Result<CompiledUnit> {
        let mut result = base.clone();
        result.artifact = value.clone();
        Ok(result)
    }
}
struct Fixture {
    temp: tempfile::TempDir,
    store: Store,
    compiled: Compiled,
    remote: Arc<Mutex<Remote>>,
    target: KubernetesTarget,
    historical_creation: bool,
}
impl Fixture {
    fn new() -> Result<Self> {
        let temp = tempfile::tempdir()?;
        let ca = temp.path().join("ca");
        // Fake API tests verify binding bytes independently of certificate parsing.
        std::fs::write(&ca, b"old test CA proof")?;
        let artifact = json!({"apiVersion":"v1","kind":"List","items":[{"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"settings","namespace":"test"},"data":{"value":"first"}}]});
        let instance: Instance = serde_json::from_value(
            json!({"apiVersion":"nickel-plane/v1","id":"bindings","composition":{"name":"test","version":"1"},"policy":{"mode":"manage","deletion":"delete","allowDelete":true},"units":{"app":{"backend":"kubernetes","target":"cluster","configuration":artifact}}}),
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
        let mut store = Store::open(&temp.path().join("store"))?;
        store.submit(&compiled)?;
        Ok(Self {
            temp,
            store,
            compiled,
            remote: Arc::default(),
            historical_creation: false,
            target: KubernetesTarget {
                server: "https://cluster.example".into(),
                namespace: "test".into(),
                ca_file: Some(ca),
                previous_ca_file: None,
                token_file: None,
                allow_http_loopback: false,
            },
        })
    }
    fn historical() -> Result<Self> {
        let mut fixture = Self::new()?;
        fixture.historical_creation = true;
        Ok(fixture)
    }
    fn registry(&self) -> Registry {
        let backend = Kubernetes {
            api: MemoryApi(self.remote.clone()),
            target: self.target.clone(),
        };
        let backend: Box<dyn Backend> = if self.historical_creation {
            Box::new(legacy::BackendFixture(backend))
        } else {
            Box::new(backend)
        };
        BTreeMap::from([("cluster".into(), backend)])
    }
    fn run(&mut self, options: Options) -> Result<()> {
        let mut registry = self.registry();
        reconcile(
            &mut self.store,
            &self.compiled,
            &Validate,
            &mut registry,
            &options,
        )
    }
    fn legacy(&mut self) -> Result<()> {
        let mut state = self.store.unit("bindings", "app")?.unwrap();
        state.target_binding = None;
        self.store.save_unit("bindings", "app", &state)
    }
    fn rotate(&mut self) -> Result<PathBuf> {
        let proof = self.temp.path().join("old-ca-proof");
        std::fs::copy(self.target.ca_file.as_ref().unwrap(), &proof)?;
        std::fs::write(self.target.ca_file.as_ref().unwrap(), b"new test CA roots")?;
        Ok(proof)
    }
    fn healthy(&self) -> Result<()> {
        let state = self.store.unit("bindings", "app")?.unwrap();
        assert!(state.failure.is_none(), "{:?}", state.failure);
        Ok(())
    }
}
fn upgrade() -> Options {
    Options {
        upgrade_target_bindings: true,
        retry_failed: true,
        now: 100,
        ..Options::default()
    }
}
fn planning() -> Options {
    Options {
        prepare: true,
        execute: false,
        ..Options::default()
    }
}

#[test]
fn new_binding_survives_rotation_and_restart_without_changing_ownership() -> Result<()> {
    let mut f = Fixture::new()?;
    f.run(Options::default())?;
    let before = f.store.unit("bindings", "app")?.unwrap();
    assert!(before.target_binding.is_some());
    let owner = f.store.owner.clone();
    f.rotate()?;
    let path = f.temp.path().join("store");
    drop(f.store);
    f.store = Store::open(&path)?;
    f.run(Options::default())?;
    f.healthy()?;
    let after = f.store.unit("bindings", "app")?.unwrap();
    assert_eq!(before.inventory, after.inventory);
    assert_eq!(before.target_sha256, after.target_sha256);
    assert_eq!(owner, f.store.owner);
    assert_eq!(f.remote.lock().unwrap().writes, 2);
    Ok(())
}

#[test]
fn legacy_binding_upgrade_is_explicit_verified_and_idempotent() -> Result<()> {
    let mut f = Fixture::historical()?;
    f.run(Options::default())?;
    f.legacy()?;
    let before = f.store.unit("bindings", "app")?.unwrap();
    f.run(Options::default())?;
    assert!(f
        .store
        .unit("bindings", "app")?
        .unwrap()
        .target_binding
        .is_none());
    f.run(upgrade())?;
    f.healthy()?;
    assert!(f
        .store
        .unit("bindings", "app")?
        .unwrap()
        .target_binding
        .is_some());
    f.run(upgrade())?;
    f.rotate()?;
    f.run(Options::default())?;
    f.healthy()?;
    let after = f.store.unit("bindings", "app")?.unwrap();
    assert_eq!(before.target_sha256, after.target_sha256);
    assert_eq!(before.inventory, after.inventory);
    assert_eq!(f.remote.lock().unwrap().writes, 1);
    Ok(())
}

#[test]
fn already_rotated_legacy_state_requires_exact_offline_proof_and_upgrade_flag() -> Result<()> {
    let mut f = Fixture::historical()?;
    f.run(Options::default())?;
    f.legacy()?;
    let proof = f.rotate()?;
    f.run(upgrade())?;
    assert!(f.store.unit("bindings", "app")?.unwrap().failure.is_some());
    assert!(f
        .store
        .unit("bindings", "app")?
        .unwrap()
        .target_binding
        .is_none());
    f.target.previous_ca_file = Some(proof);
    f.run(Options {
        retry_failed: true,
        now: 200,
        ..Options::default()
    })?;
    assert!(f.store.unit("bindings", "app")?.unwrap().failure.is_some());
    f.run(upgrade())?;
    f.healthy()?;
    assert!(f
        .store
        .unit("bindings", "app")?
        .unwrap()
        .target_binding
        .is_some());
    assert_eq!(f.remote.lock().unwrap().writes, 1);
    Ok(())
}

#[test]
fn upgrade_refuses_wrong_ca_endpoint_namespace_and_unknown_binding_version() -> Result<()> {
    for wrong in [
        "ca",
        "endpoint",
        "namespace",
        "version",
        "symlink",
        "oversized",
    ] {
        let mut f = Fixture::historical()?;
        f.run(Options::default())?;
        f.legacy()?;
        match wrong {
            "ca" => {
                let path = f.temp.path().join("wrong");
                std::fs::write(&path, b"wrong proof")?;
                f.target.previous_ca_file = Some(path);
            }
            "endpoint" => f.target.server = "https://other.example".into(),
            "namespace" => f.target.namespace = "other".into(),
            "symlink" => {
                let path = f.temp.path().join("proof-link");
                std::os::unix::fs::symlink(f.target.ca_file.as_ref().unwrap(), &path)?;
                f.target.previous_ca_file = Some(path);
            }
            "oversized" => {
                let path = f.temp.path().join("oversized-proof");
                std::fs::File::create(&path)?.set_len(1024 * 1024 + 1)?;
                f.target.previous_ca_file = Some(path);
            }
            _ => {
                let mut state = f.store.unit("bindings", "app")?.unwrap();
                state.target_binding = Some(TargetBinding {
                    version: 99,
                    identity_sha256: "unknown".into(),
                });
                f.store.save_unit("bindings", "app", &state)?;
            }
        }
        f.run(upgrade())?;
        assert!(
            f.store.unit("bindings", "app")?.unwrap().failure.is_some(),
            "accepted {wrong}"
        );
        assert_eq!(f.remote.lock().unwrap().writes, 1);
    }
    Ok(())
}

#[test]
fn rotation_invalidates_saved_create_and_removal_plans() -> Result<()> {
    let mut f = Fixture::new()?;
    f.run(planning())?;
    let create = f.store.unit("bindings", "app")?.unwrap().prepared.unwrap();
    f.rotate()?;
    f.run(Options {
        approved_operation: Some(create.operation),
        ..Options::default()
    })?;
    assert!(f
        .store
        .unit("bindings", "app")?
        .unwrap()
        .failure
        .as_deref()
        .unwrap()
        .contains("stale"));
    assert_eq!(f.remote.lock().unwrap().writes, 0);
    f.run(Options {
        retry_failed: true,
        ..Options::default()
    })?;
    f.healthy()?;
    f.store.mark_removed("bindings", true)?;
    f.run(planning())?;
    let removal = f.store.unit("bindings", "app")?.unwrap().prepared.unwrap();
    std::fs::write(f.target.ca_file.as_ref().unwrap(), b"third test CA roots")?;
    f.run(Options {
        approved_operation: Some(removal.operation),
        ..Options::default()
    })?;
    assert!(f
        .store
        .unit("bindings", "app")?
        .unwrap()
        .failure
        .as_deref()
        .unwrap()
        .contains("stale"));
    assert_eq!(f.remote.lock().unwrap().writes, 2);
    f.run(Options {
        retry_failed: true,
        ..Options::default()
    })?;
    assert!(f.store.unit("bindings", "app")?.unwrap().deleted);
    Ok(())
}

#[test]
fn legacy_upgrade_preserves_partial_create_journal_and_recovered_uids() -> Result<()> {
    let mut f = Fixture::historical()?;
    let mut second = f.compiled.units["app"].artifact["items"][0].clone();
    second["metadata"]["name"] = json!("second");
    f.compiled.units.get_mut("app").unwrap().artifact["items"]
        .as_array_mut()
        .unwrap()
        .push(second);
    f.compiled
        .instance
        .units
        .get_mut("app")
        .unwrap()
        .configuration = f.compiled.units["app"].artifact.clone();
    f.compiled.revision = f.compiled.digest()?;
    f.store.submit(&f.compiled)?;
    f.remote.lock().unwrap().interrupt = true;
    f.run(Options::default())?;
    let running = f.store.running("bindings", "app")?.unwrap();
    assert_eq!(f.remote.lock().unwrap().creates.len(), 1);
    let (created_path, uid) = {
        let remote = f.remote.lock().unwrap();
        let (path, object) = remote.objects.first_key_value().unwrap();
        (path.clone(), object["metadata"]["uid"].clone())
    };
    f.legacy()?;
    f.target.previous_ca_file = Some(f.rotate()?);
    f.run(upgrade())?;
    f.healthy()?;
    assert!(f.store.running("bindings", "app")?.is_none());
    // The recovered historical Update owner is handed off first. The next
    // ordinary cycle creates the remaining object; it must not recreate the UID.
    assert_eq!(f.remote.lock().unwrap().creates.len(), 1);
    assert_eq!(
        f.remote.lock().unwrap().objects[&created_path]["metadata"]["uid"],
        uid
    );
    f.run(Options {
        retry_failed: true,
        now: 200,
        ..Options::default()
    })?;
    f.healthy()?;
    let remote = f.remote.lock().unwrap();
    assert_eq!(remote.creates.len(), 2);
    assert_eq!(remote.objects[&created_path]["metadata"]["uid"], uid);
    assert_eq!(
        remote
            .creates
            .iter()
            .filter(|path| *path == &created_path)
            .count(),
        1
    );
    assert!(!running.operation.is_empty());
    Ok(())
}

#[test]
fn binding_upgrade_does_not_authorize_a_legacy_prepared_plan() -> Result<()> {
    let mut f = Fixture::historical()?;
    f.run(planning())?;
    f.legacy()?;
    let mut state = f.store.unit("bindings", "app")?.unwrap();
    let plan = state.prepared.as_mut().unwrap();
    // Historical plans used the CA-bound target digest, without auth binding.
    plan.target_sha256 = state.target_sha256.clone();
    let operation = plan.operation.clone();
    f.store.save_unit("bindings", "app", &state)?;
    f.run(Options {
        approved_operation: Some(operation),
        ..upgrade()
    })?;
    let state = f.store.unit("bindings", "app")?.unwrap();
    assert!(state.target_binding.is_some());
    assert!(state.failure.as_deref().unwrap().contains("stale"));
    assert_eq!(f.remote.lock().unwrap().writes, 0);
    f.run(Options {
        retry_failed: true,
        ..Options::default()
    })?;
    f.healthy()?;
    assert_eq!(f.remote.lock().unwrap().writes, 1);
    Ok(())
}

#[test]
fn stable_bindings_still_refuse_endpoint_and_namespace_changes() -> Result<()> {
    for change_namespace in [false, true] {
        let mut f = Fixture::new()?;
        f.run(Options::default())?;
        if change_namespace {
            f.target.namespace = "other".into();
        } else {
            f.target.server = "https://other.example".into();
        }
        f.run(upgrade())?;
        assert!(f.store.unit("bindings", "app")?.unwrap().failure.is_some());
        assert_eq!(f.remote.lock().unwrap().writes, 2);
    }
    Ok(())
}

#[test]
fn saved_plans_and_public_state_expose_only_combined_execution_digests() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut f = Fixture::new()?;
    let token_file = f.temp.path().join("private-token");
    f.target.token_file = Some(token_file.clone());
    let mut previous_execution = None;
    for token in [
        "synthetic-authentication-token-A",
        "synthetic-authentication-token-B",
    ] {
        std::fs::write(&token_file, token)?;
        std::fs::set_permissions(&token_file, std::fs::Permissions::from_mode(0o600))?;
        f.run(planning())?;
        f.healthy()?;
        let state = f.store.unit("bindings", "app")?.unwrap();
        let plan = state.prepared.as_ref().unwrap();
        let token_hash = nickel_util::files::hash(token.as_bytes());
        for serialized in [
            serde_json::to_string(&state)?,
            serde_json::to_string(plan)?,
            state.public().to_string(),
        ] {
            assert!(!serialized.contains(token));
            assert!(!serialized.contains(&token_hash));
        }
        if let Some(previous) = previous_execution {
            assert_ne!(previous, plan.target_sha256);
        }
        previous_execution = Some(plan.target_sha256.clone());
    }
    assert_eq!(f.remote.lock().unwrap().writes, 0);
    Ok(())
}

#[test]
fn legacy_removal_upgrade_requires_proof_and_recovers_interrupted_deletion() -> Result<()> {
    for interrupt in [false, true] {
        let mut f = Fixture::historical()?;
        f.run(Options::default())?;
        f.legacy()?;
        let historical = f.store.unit("bindings", "app")?.unwrap().target_sha256;
        f.store.mark_removed("bindings", true)?;
        let proof = f.rotate()?;
        let wrong = f.temp.path().join("wrong-proof");
        std::fs::write(&wrong, "incorrect historical CA")?;
        f.target.previous_ca_file = Some(wrong);
        f.run(Options {
            prepare: true,
            execute: false,
            ..upgrade()
        })?;
        let state = f.store.unit("bindings", "app")?.unwrap();
        assert!(state.failure.is_some());
        assert!(!state.deleted);
        assert!(state.target_binding.is_none());
        assert_eq!(f.remote.lock().unwrap().writes, 1);

        f.target.previous_ca_file = Some(proof);
        f.run(Options {
            prepare: true,
            execute: false,
            ..upgrade()
        })?;
        f.healthy()?;
        let state = f.store.unit("bindings", "app")?.unwrap();
        assert_eq!(state.target_sha256, historical);
        assert!(state.target_binding.is_some());
        let operation = state.prepared.unwrap().operation;
        assert_eq!(f.remote.lock().unwrap().writes, 1);
        f.remote.lock().unwrap().interrupt = interrupt;
        f.run(Options {
            now: 100,
            approved_operation: Some(operation.clone()),
            ..Options::default()
        })?;
        if interrupt {
            assert!(f.store.running("bindings", "app")?.is_some());
            // Recovery observes completed deletion after another trust rotation;
            // it must not replay the now-stale mutation plan.
            std::fs::write(f.target.ca_file.as_ref().unwrap(), "third test CA")?;
            f.run(Options {
                now: 1000,
                approved_operation: Some(operation),
                ..Options::default()
            })?;
        }
        assert!(f.store.unit("bindings", "app")?.unwrap().deleted);
        assert!(f.store.running("bindings", "app")?.is_none());
        assert_eq!(f.remote.lock().unwrap().writes, 2);
    }
    Ok(())
}

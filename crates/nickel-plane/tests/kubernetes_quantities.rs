#![cfg(feature = "kubernetes")]
#![allow(clippy::unwrap_used)]
use nickel_plane::{
    adapters::kubernetes::{Api, Kubernetes},
    backend::{Backend, Context},
    model::Unit,
    targets::KubernetesTarget,
    Error, Result,
};
use serde_json::{json, Value};

struct ReadOnlyApi {
    path: String,
    object: Value,
    reads: usize,
    mutations: usize,
}
impl Api for ReadOnlyApi {
    fn request(&mut self, method: &str, path: &str, _: Option<&Value>) -> Result<(u16, Value)> {
        if method != "GET" {
            self.mutations += 1;
            return Err(Error::Backend("Observation attempted a mutation".into()));
        }
        self.reads += 1;
        if path == self.path {
            Ok((200, self.object.clone()))
        } else {
            Ok((404, Value::Null))
        }
    }
}

fn observe(desired: Value, mut actual: Value) -> Result<bool> {
    let kind = desired["kind"].as_str().unwrap();
    let path = match kind {
        "Deployment" => "/apis/apps/v1/namespaces/test/deployments/app",
        "StatefulSet" => "/apis/apps/v1/namespaces/test/statefulsets/app",
        "ConfigMap" => "/api/v1/namespaces/test/configmaps/app",
        "PersistentVolumeClaim" => "/api/v1/namespaces/test/persistentvolumeclaims/app",
        _ => return Err(Error::Invalid("Unexpected fixture kind".into())),
    };
    actual["metadata"]["uid"] = json!("owned-uid");
    actual["metadata"]["resourceVersion"] = json!("1");
    actual["metadata"]["labels"]["nickel-plane.io/owner"] =
        json!(&nickel_util::files::hash(b"owner/instance/app")[..32]);
    let unit: Unit = serde_json::from_value(
        json!({"backend":"kubernetes","target":"cluster","configuration":{}}),
    )?;
    let context = Context {
        owner: "owner".into(),
        instance: "instance".into(),
        unit_id: "app".into(),
        revision: "revision".into(),
        unit,
        artifact: json!({"apiVersion":"v1","kind":"List","items":[desired]}),
        artifact_sha256: "artifact".into(),
        inventory: json!({path:{"path":path,"uid":"owned-uid","resourceVersion":"1","kind":kind,"name":"app"}}),
        interrupted: None,
        recovery: None,
        work: std::path::PathBuf::new(),
        engine_version: None,
        provider_lock: None,
    };
    let mut backend = Kubernetes {
        api: ReadOnlyApi {
            path: path.into(),
            object: actual,
            reads: 0,
            mutations: 0,
        },
        target: KubernetesTarget {
            server: "https://cluster.example".into(),
            namespace: "test".into(),
            ca_file: None,
            previous_ca_file: None,
            token_file: None,
            allow_http_loopback: false,
        },
    };
    let observation = backend.observe(&context)?;
    assert!(observation.exists);
    assert_eq!(backend.api.reads, 1);
    assert_eq!(backend.api.mutations, 0);
    Ok(observation.drifted)
}

fn workload(kind: &str) -> Value {
    json!({
        "apiVersion":"apps/v1", "kind":kind,
        "metadata":{"name":"app","namespace":"test"},
        "spec":{"template":{"spec":{"containers":[{
            "name":"app", "image":"example:1",
            "env":[{"name":"TEXT","value":"1000m"}],
            "resources":{
                "limits":{"cpu":"1000m","memory":"1024Mi"},
                "requests":{"cpu":1,"memory":"1.5Gi"}
            }
        }]}}}
    })
}

#[test]
fn canonical_cpu_memory_and_numeric_desired_values_observe_without_drift_or_writes() -> Result<()> {
    for kind in ["Deployment", "StatefulSet"] {
        let desired = workload(kind);
        let mut actual = desired.clone();
        let resources = &mut actual["spec"]["template"]["spec"]["containers"][0]["resources"];
        resources["limits"]["cpu"] = json!("1");
        resources["limits"]["memory"] = json!("1Gi");
        resources["requests"]["cpu"] = json!("1");
        resources["requests"]["memory"] = json!("1536Mi");
        assert!(!observe(desired, actual)?, "{kind} has spurious drift");
    }
    Ok(())
}

#[test]
fn real_resource_changes_and_unrelated_strings_still_drift() -> Result<()> {
    for (pointer, value) in [
        ("/spec/template/spec/containers/0/resources/limits/cpu", "2"),
        (
            "/spec/template/spec/containers/0/resources/limits/memory",
            "2Gi",
        ),
        ("/spec/template/spec/containers/0/env/0/value", "1"),
    ] {
        let desired = workload("Deployment");
        let mut actual = desired.clone();
        *actual.pointer_mut(pointer).unwrap() = json!(value);
        assert!(observe(desired, actual)?, "lost drift at {pointer}");
    }
    let desired = json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"app","namespace":"test"},"data":{"cpu":"1000m","memory":"1024Mi"}});
    let mut actual = desired.clone();
    actual["data"]["cpu"] = json!("1");
    actual["data"]["memory"] = json!("1Gi");
    assert!(observe(desired, actual)?);
    Ok(())
}

#[test]
fn missing_requested_resource_keys_still_drift() -> Result<()> {
    let desired = workload("Deployment");
    let mut actual = desired.clone();
    actual["spec"]["template"]["spec"]["containers"][0]["resources"]["limits"]
        .as_object_mut()
        .unwrap()
        .remove("cpu");
    assert!(observe(desired, actual)?);
    Ok(())
}

fn storage_workload(kind: &str) -> Value {
    let resources =
        json!({"requests":{"ephemeral-storage":"1024Mi"},"limits":{"ephemeral-storage":"1024Mi"}});
    let claim = json!({"spec":{"accessModes":["ReadWriteOnce"],"storageClassName":"",
        "resources":{"requests":{"storage":1073741824_u64},"limits":{"storage":"1024Mi"}}}});
    let mut object = workload(kind);
    object["spec"]["template"]["spec"]["containers"][0]["resources"] = resources.clone();
    object["spec"]["template"]["spec"]["initContainers"] =
        json!([{"name":"init","image":"example:1","resources":resources}]);
    object["spec"]["template"]["spec"]["volumes"] = json!([
        {"name":"scratch","emptyDir":{"sizeLimit":"1024Mi"}},
        {"name":"ephemeral","ephemeral":{"volumeClaimTemplate":claim}}
    ]);
    if kind == "StatefulSet" {
        object["spec"]["volumeClaimTemplates"] = json!([claim]);
    }
    object
}

fn storage_paths(kind: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for bound in ["requests", "limits"] {
        if kind == "PersistentVolumeClaim" {
            paths.push(format!("/spec/resources/{bound}/storage"));
            continue;
        }
        for container in ["containers", "initContainers"] {
            paths.push(format!(
                "/spec/template/spec/{container}/0/resources/{bound}/ephemeral-storage"
            ));
        }
        paths.push(format!("/spec/template/spec/volumes/1/ephemeral/volumeClaimTemplate/spec/resources/{bound}/storage"));
        if kind == "StatefulSet" {
            paths.push(format!(
                "/spec/volumeClaimTemplates/0/spec/resources/{bound}/storage"
            ));
        }
    }
    if kind != "PersistentVolumeClaim" {
        paths.push("/spec/template/spec/volumes/0/emptyDir/sizeLimit".into());
    }
    paths
}

#[test]
fn canonical_storage_observes_without_drift_and_real_changes_still_drift_without_writes(
) -> Result<()> {
    for kind in ["PersistentVolumeClaim", "Deployment", "StatefulSet"] {
        let desired = if kind == "PersistentVolumeClaim" {
            json!({"apiVersion":"v1","kind":kind,"metadata":{"name":"app","namespace":"test"},
                "spec":{"accessModes":["ReadWriteOnce"],"storageClassName":"",
                "resources":{"requests":{"storage":1073741824_u64},"limits":{"storage":"1024Mi"}}}})
        } else {
            storage_workload(kind)
        };
        let mut canonical = desired.clone();
        for pointer in storage_paths(kind) {
            *canonical.pointer_mut(&pointer).unwrap() = json!("1Gi");
        }
        assert!(
            !observe(desired.clone(), canonical.clone())?,
            "{kind}: false canonicalization drift"
        );
        for pointer in storage_paths(kind) {
            for changed in ["1G", "2Gi", "1073741825"] {
                let mut actual = canonical.clone();
                *actual.pointer_mut(&pointer).unwrap() = json!(changed);
                assert!(
                    observe(desired.clone(), actual)?,
                    "{kind} {pointer}: missing drift for {changed}"
                );
            }
            let mut actual = canonical.clone();
            let (parent, key) = pointer.rsplit_once('/').unwrap();
            actual
                .pointer_mut(parent)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(key);
            assert!(
                observe(desired.clone(), actual)?,
                "{kind} {pointer}: missing field must drift"
            );
        }
    }
    Ok(())
}

#[test]
fn storage_shaped_unrelated_strings_still_drift_without_writes() -> Result<()> {
    let mut desired = storage_workload("Deployment");
    desired["metadata"]["annotations"]["storage"] = json!("1024Mi");
    desired["spec"]["template"]["spec"]["containers"][0]["env"][0]["value"] = json!("1024Mi");
    for pointer in [
        "/metadata/annotations/storage",
        "/spec/template/spec/containers/0/env/0/value",
    ] {
        let mut actual = desired.clone();
        *actual.pointer_mut(pointer).unwrap() = json!("1Gi");
        assert!(
            observe(desired.clone(), actual)?,
            "lost literal drift: {pointer}"
        );
    }
    let desired = json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"app","namespace":"test"},"data":{"storage":"1024Mi"}});
    let mut actual = desired.clone();
    actual["data"]["storage"] = json!("1Gi");
    assert!(observe(desired, actual)?);
    Ok(())
}

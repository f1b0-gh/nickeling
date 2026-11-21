//! Real API semantics; invoked only by scripts/test-kind.py in its owned cluster.
#![cfg(feature = "kubernetes")]

#[path = "fixtures/kubernetes_legacy.rs"]
mod legacy;
use nickel_plane::{
    adapters::kubernetes::{Api, Http, Kubernetes},
    backend::{Backend, Context, Readiness},
    model::Unit,
    targets::KubernetesTarget,
    Result,
};
use serde_json::{json, Value};
use std::{env, path::PathBuf};

fn connection() -> Result<Kubernetes<Http>> {
    let fixture = PathBuf::from(env::var("NICKELING_KIND_FIXTURE").map_err(|_| {
        nickel_plane::Error::Invalid("Run scripts/test-kind.py for real API tests".into())
    })?);
    let value: Value = serde_json::from_slice(&std::fs::read(fixture.join("target.json"))?)?;
    let target: KubernetesTarget = serde_json::from_value(value)?;
    assert!(target.server.starts_with("https://127.0.0.1:"));
    assert_eq!(target.namespace, "nickeling-api");
    Kubernetes::connect(target)
}

fn object(name: &str) -> Value {
    json!({"apiVersion":"v1","kind":"ConfigMap", "metadata":{
        "name":name,"namespace":"nickeling-api"},"data":{"value":"one","remove":"old"}})
}

fn foreign_fields(object: &Value) -> Vec<Value> {
    object["metadata"]["managedFields"]
        .as_array()
        .map(|fields| {
            fields
                .iter()
                .filter(|field| field["manager"] == "foreign")
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

fn context(name: &str, artifact: Value) -> Result<Context> {
    let unit: Unit = serde_json::from_value(json!({
        "backend":"kubernetes","target":"cluster","configuration":{}
    }))?;
    Ok(Context {
        owner: "real-api-test".into(),
        instance: "kind".into(),
        unit_id: name.into(),
        revision: "one".into(),
        unit,
        artifact,
        artifact_sha256: "one".into(),
        inventory: Value::Null,
        interrupted: None,
        recovery: None,
        work: PathBuf::from(env::var("NICKELING_KIND_FIXTURE").unwrap_or_default()),
        engine_version: None,
        provider_lock: None,
    })
}

#[test]
#[ignore = "requires an owned kind cluster; use scripts/test-kind.py"]
fn real_controller_create_update_and_omission() -> Result<()> {
    let mut k = connection()?;
    let mut c = context(
        "controller",
        json!({"apiVersion":"v1","kind":"List","items":[object("controller")]}),
    )?;
    let observed = k.observe(&c)?;
    let create = legacy::prepare(&mut k, &c, &observed, "create")?;
    c.inventory = k.execute_change(&c, &create)?.inventory;
    c.artifact["items"][0]["data"]["value"] = json!("two");
    c.artifact_sha256 = "two".into();
    let observed = k.observe(&c)?;
    assert!(observed.drifted);
    let stale = legacy::prepare(&mut k, &c, &observed, "stale-transfer")?;
    assert_eq!(stale.summary.field_ownership_transfers, 1);
    let route = "/api/v1/namespaces/nickeling-api/configmaps/controller";
    let foreign = json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{
        "name":"controller","namespace":"nickeling-api"},"data":{"foreign":"preserved"}});
    assert_eq!(
        k.api
            .request(
                "PATCH",
                &format!("{route}?fieldManager=foreign"),
                Some(&foreign)
            )?
            .0,
        200
    );
    let (_, before) = k.api.request("GET", route, None)?;
    assert!(k.execute_change(&c, &stale).is_err());
    let (_, after_refusal) = k.api.request("GET", route, None)?;
    assert_eq!(
        before["metadata"]["managedFields"],
        after_refusal["metadata"]["managedFields"]
    );
    assert_eq!(before["data"], after_refusal["data"]);
    assert!(!foreign_fields(&before).is_empty());

    let observed = k.observe(&c)?;
    let transfer = legacy::prepare(&mut k, &c, &observed, "transfer")?;
    assert_eq!(transfer.summary.field_ownership_transfers, 1);
    assert_eq!(
        transfer.summary.updates + transfer.summary.creates + transfer.summary.deletes,
        0
    );
    let transferred = k.execute_change(&c, &transfer)?;
    assert!(transferred.drifted);
    assert_eq!(transferred.readiness, Readiness::NotReady);
    c.inventory = transferred.inventory;
    let (_, after_transfer) = k.api.request("GET", route, None)?;
    assert_eq!(before["data"], after_transfer["data"]);
    assert_eq!(before["metadata"]["uid"], after_transfer["metadata"]["uid"]);
    assert_eq!(foreign_fields(&before), foreign_fields(&after_transfer));

    let observed = k.observe(&c)?;
    let update = legacy::prepare(&mut k, &c, &observed, "update")?;
    assert_eq!(update.summary.field_ownership_transfers, 0);
    assert_eq!(update.summary.updates, 1);
    c.inventory = k.execute_change(&c, &update)?.inventory;
    c.artifact["items"][0]["data"]
        .as_object_mut()
        .ok_or_else(|| nickel_plane::Error::Invalid("Expected test data object".into()))?
        .remove("remove");
    c.artifact_sha256 = "three".into();
    let observed = k.observe(&c)?;
    let omit = legacy::prepare(&mut k, &c, &observed, "omit")?;
    c.inventory = k.execute_change(&c, &omit)?.inventory;
    let (_, actual) = k.api.request(
        "GET",
        "/api/v1/namespaces/nickeling-api/configmaps/controller",
        None,
    )?;
    assert_eq!(actual["data"]["value"], "two");
    assert_eq!(actual["data"]["foreign"], "preserved");
    assert_eq!(foreign_fields(&before), foreign_fields(&actual));
    assert!(
        actual["data"].get("remove").is_none(),
        "SSA must remove omitted controller-owned fields"
    );

    Ok(())
}

#[test]
#[ignore = "requires an owned kind cluster; use scripts/test-kind.py"]
fn real_controller_omission_only_stays_pending_until_fresh_apply() -> Result<()> {
    let mut k = connection()?;
    let mut c = context(
        "omission-only",
        json!({"apiVersion":"v1","kind":"List","items":[object("omission-only")]}),
    )?;
    let observed = k.observe(&c)?;
    let create = legacy::prepare(&mut k, &c, &observed, "omission-create")?;
    c.inventory = k.execute_change(&c, &create)?.inventory;
    c.artifact["items"][0]["data"]
        .as_object_mut()
        .ok_or_else(|| nickel_plane::Error::Invalid("Expected test data object".into()))?
        .remove("remove");
    c.artifact_sha256 = "omit-only".into();
    let observed = k.observe(&c)?;
    let transfer = legacy::prepare(&mut k, &c, &observed, "omission-transfer")?;
    assert_eq!(transfer.summary.field_ownership_transfers, 1);
    let pending = k.execute_change(&c, &transfer)?;
    assert!(pending.drifted);
    assert_eq!(pending.readiness, Readiness::NotReady);
    c.inventory = pending.inventory;
    let route = "/api/v1/namespaces/nickeling-api/configmaps/omission-only";
    let (_, still_present) = k.api.request("GET", route, None)?;
    assert_eq!(still_present["data"]["remove"], "old");
    let observed = k.observe(&c)?;
    assert!(observed.drifted);
    let apply = legacy::prepare(&mut k, &c, &observed, "omission-apply")?;
    assert_eq!(apply.summary.field_ownership_transfers, 0);
    assert_eq!(apply.summary.updates, 1);
    let complete = k.execute_change(&c, &apply)?;
    assert!(!complete.drifted);
    assert_eq!(complete.readiness, Readiness::Ready);
    let (_, absent) = k.api.request("GET", route, None)?;
    assert!(absent["data"].get("remove").is_none());
    assert_eq!(absent["data"]["value"], "one");
    Ok(())
}

#[test]
#[ignore = "requires an owned kind cluster; use scripts/test-kind.py"]
fn real_controller_refuses_foreign_create_race() -> Result<()> {
    let mut k = connection()?;
    let race = context(
        "race",
        json!({"apiVersion":"v1","kind":"List","items":[object("race")]}),
    )?;
    let observed = k.observe(&race)?;
    let plan = legacy::prepare(&mut k, &race, &observed, "race-create")?;
    let (code, foreign) = k.api.request(
        "POST",
        "/api/v1/namespaces/nickeling-api/configmaps",
        Some(&object("race")),
    )?;
    assert_eq!(code, 201);
    assert!(k.execute_change(&race, &plan).is_err());
    let (_, untouched) = k.api.request(
        "GET",
        "/api/v1/namespaces/nickeling-api/configmaps/race",
        None,
    )?;
    assert_eq!(foreign["metadata"]["uid"], untouched["metadata"]["uid"]);
    assert!(untouched["metadata"]["labels"]
        .get("nickel-plane.io/owner")
        .is_none());
    Ok(())
}

#[test]
#[ignore = "requires an owned kind cluster; use scripts/test-kind.py"]
fn real_controller_refuses_replacement_uid_even_with_copied_owner_label() -> Result<()> {
    let mut k = connection()?;
    let mut c = context(
        "replacement",
        json!({"apiVersion":"v1","kind":"List","items":[object("replacement")]}),
    )?;
    let observed = k.observe(&c)?;
    let plan = legacy::prepare(&mut k, &c, &observed, "create-replacement")?;
    c.inventory = k.execute_change(&c, &plan)?.inventory;
    let path = "/api/v1/namespaces/nickeling-api/configmaps/replacement";
    let (_, original) = k.api.request("GET", path, None)?;
    let (code, _) = k.api.request("DELETE", path, Some(&json!({"apiVersion":"v1","kind":"DeleteOptions",
        "preconditions":{"uid":original["metadata"]["uid"],"resourceVersion":original["metadata"]["resourceVersion"]}})))?;
    assert!(matches!(code, 200 | 202));
    let mut replacement = object("replacement");
    replacement["metadata"]["labels"] = original["metadata"]["labels"].clone();
    replacement["data"]["value"] = json!("foreign-replacement");
    let (code, created) = k.api.request(
        "POST",
        "/api/v1/namespaces/nickeling-api/configmaps",
        Some(&replacement),
    )?;
    assert_eq!(code, 201);
    assert_ne!(created["metadata"]["uid"], original["metadata"]["uid"]);
    assert!(k.observe(&c).is_err());
    assert!(k.execute_change(&c, &plan).is_err());
    let (_, after) = k.api.request("GET", path, None)?;
    assert_eq!(after["metadata"]["uid"], created["metadata"]["uid"]);
    assert_eq!(after["data"]["value"], "foreign-replacement");
    Ok(())
}

#[test]
#[ignore = "requires an owned kind cluster; use scripts/test-kind.py"]
fn real_controller_recognizes_normalized_cpu_quantity_without_permanent_drift() -> Result<()> {
    let mut k = connection()?;
    let deployment = json!({"apiVersion":"apps/v1","kind":"Deployment","metadata":{
        "name":"quantity","namespace":"nickeling-api"},"spec":{
        "replicas":0,"selector":{"matchLabels":{"app":"quantity"}},"template":{
        "metadata":{"labels":{"app":"quantity"}},"spec":{"containers":[{
        "name":"app","image":"example.invalid/not-pulled:test","resources":{"requests":{"cpu":"1000m"}}}]}}}});
    let mut c = context(
        "quantity",
        json!({"apiVersion":"v1","kind":"List","items":[deployment]}),
    )?;
    compound::create_context(&mut c)?;
    let (_, actual) = k.api.request(
        "GET",
        "/apis/apps/v1/namespaces/nickeling-api/deployments/quantity",
        None,
    )?;
    assert_eq!(
        actual.pointer("/spec/template/spec/containers/0/resources/requests/cpu"),
        Some(&json!("1"))
    );
    assert!(
        !k.observe(&c)?.drifted,
        "Canonical CPU quantity must not cause permanent drift"
    );
    Ok(())
}

fn storage_claim_spec() -> Value {
    // Explicitly disable default StorageClass selection; this test provisions no storage.
    json!({"accessModes":["ReadWriteOnce"],"storageClassName":"",
        "resources":{"requests":{"storage":"1024Mi"},"limits":{"storage":"1024Mi"}}})
}

fn storage_workload(name: &str, kind: &str) -> Value {
    let resources = json!({
        "requests":{"cpu":"1000m","memory":"1024Mi","ephemeral-storage":"1024Mi"},
        "limits":{"cpu":"1000m","memory":"1024Mi","ephemeral-storage":"1024Mi"}
    });
    let mut workload = json!({"apiVersion":"apps/v1","kind":kind,
    "metadata":{"name":name,"namespace":"nickeling-api"},"spec":{
        "replicas":0,"selector":{"matchLabels":{"app":name}},"template":{
            "metadata":{"labels":{"app":name}},"spec":{
                "containers":[{"name":"app","image":"example.invalid/not-pulled:test","resources":resources}],
                "initContainers":[{"name":"init","image":"example.invalid/not-pulled:test","resources":resources}],
                "volumes":[{"name":"scratch","emptyDir":{"sizeLimit":"1024Mi"}},
                    {"name":"ephemeral","ephemeral":{"volumeClaimTemplate":{"spec":storage_claim_spec()}}}]
            }
        }
    }});
    if kind == "StatefulSet" {
        workload["spec"]["serviceName"] = json!("quantity-headless");
        workload["spec"]["volumeClaimTemplates"] = json!([
            {"metadata":{"name":"data"},"spec":storage_claim_spec()}
        ]);
    }
    workload
}

fn assert_canonical_cpu_memory(actual: &Value) {
    for container in ["containers", "initContainers"] {
        for bound in ["requests", "limits"] {
            assert_eq!(
                actual["spec"]["template"]["spec"][container][0]["resources"][bound]["cpu"],
                "1"
            );
            assert_eq!(
                actual["spec"]["template"]["spec"][container][0]["resources"][bound]["memory"],
                "1Gi"
            );
        }
    }
}

const WORKLOAD_STORAGE_PATHS: &[&str] = &[
    "/spec/template/spec/containers/0/resources/requests/ephemeral-storage",
    "/spec/template/spec/containers/0/resources/limits/ephemeral-storage",
    "/spec/template/spec/initContainers/0/resources/requests/ephemeral-storage",
    "/spec/template/spec/initContainers/0/resources/limits/ephemeral-storage",
    "/spec/template/spec/volumes/0/emptyDir/sizeLimit",
    "/spec/template/spec/volumes/1/ephemeral/volumeClaimTemplate/spec/resources/requests/storage",
    "/spec/template/spec/volumes/1/ephemeral/volumeClaimTemplate/spec/resources/limits/storage",
];

fn create_and_check_storage(
    k: &mut Kubernetes<Http>,
    c: &mut Context,
    route: &str,
    paths: &[&str],
) -> Result<Value> {
    compound::create_context(c)?;
    let (status, actual) = k.api.request("GET", route, None)?;
    assert_eq!(status, 200);
    for path in paths {
        assert_eq!(
            actual.pointer(path),
            Some(&json!("1Gi")),
            "API must canonicalize {path}"
        );
    }
    assert!(
        !k.observe(c)?.drifted,
        "Canonical storage quantities must not cause drift"
    );
    Ok(actual)
}

#[test]
#[ignore = "requires an owned kind cluster; use scripts/test-kind.py"]
fn real_controller_recognizes_unbound_pvc_storage_quantities() -> Result<()> {
    let mut k = connection()?;
    let pvc = json!({"apiVersion":"v1","kind":"PersistentVolumeClaim",
        "metadata":{"name":"quantity-pvc","namespace":"nickeling-api"},"spec":storage_claim_spec()});
    let mut c = context(
        "quantity-pvc",
        json!({"apiVersion":"v1","kind":"List","items":[pvc]}),
    )?;
    let route = "/api/v1/namespaces/nickeling-api/persistentvolumeclaims/quantity-pvc";
    create_and_check_storage(
        &mut k,
        &mut c,
        route,
        &[
            "/spec/resources/requests/storage",
            "/spec/resources/limits/storage",
        ],
    )?;
    assert_eq!(k.observe(&c)?.readiness, Readiness::NotReady);
    // An unbound claim is intentionally not resized: this checks drift, not provisioning.
    c.artifact["items"][0]["spec"]["resources"]["requests"]["storage"] = json!("2Gi");
    c.artifact_sha256 = "pvc-size-change".into();
    assert!(k.observe(&c)?.drifted);
    let (_, unchanged) = k.api.request("GET", route, None)?;
    assert_eq!(unchanged["spec"]["resources"]["requests"]["storage"], "1Gi");
    Ok(())
}

#[test]
#[ignore = "requires an owned kind cluster; use scripts/test-kind.py"]
fn real_controller_recognizes_and_updates_deployment_storage_quantities() -> Result<()> {
    let mut k = connection()?;
    let mut c = context(
        "quantity-deployment",
        json!({"apiVersion":"v1","kind":"List",
        "items":[storage_workload("quantity-deployment", "Deployment")]}),
    )?;
    let route = "/apis/apps/v1/namespaces/nickeling-api/deployments/quantity-deployment";
    let actual = create_and_check_storage(&mut k, &mut c, route, WORKLOAD_STORAGE_PATHS)?;
    assert_canonical_cpu_memory(&actual);
    // Wait for the initial Deployment status write before freezing update preconditions.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let observed = k.observe(&c)?;
        assert!(!observed.drifted);
        if observed.readiness == Readiness::Ready {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Zero-replica Deployment did not settle"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    c.artifact["items"][0]["spec"]["template"]["spec"]["volumes"][0]["emptyDir"]["sizeLimit"] =
        json!("2048Mi");
    c.artifact_sha256 = "emptydir-size-change".into();
    let observed = k.observe(&c)?;
    assert!(observed.drifted);
    let update = k.prepare_change(&c, &observed, "storage-update")?;
    assert_eq!(update.summary.field_ownership_transfers, 0);
    assert_eq!(update.summary.updates, 1);
    c.inventory = k.execute_change(&c, &update)?.inventory;
    let (_, actual) = k.api.request("GET", route, None)?;
    assert_eq!(
        actual["spec"]["template"]["spec"]["volumes"][0]["emptyDir"]["sizeLimit"],
        "2Gi"
    );
    assert!(!k.observe(&c)?.drifted);
    Ok(())
}

#[test]
#[ignore = "requires an owned kind cluster; use scripts/test-kind.py"]
fn real_controller_recognizes_statefulset_storage_template_quantities() -> Result<()> {
    let mut k = connection()?;
    let mut c = context(
        "quantity-statefulset",
        json!({"apiVersion":"v1","kind":"List",
        "items":[storage_workload("quantity-statefulset", "StatefulSet")]}),
    )?;
    let route = "/apis/apps/v1/namespaces/nickeling-api/statefulsets/quantity-statefulset";
    let mut paths = WORKLOAD_STORAGE_PATHS.to_vec();
    paths.extend([
        "/spec/volumeClaimTemplates/0/spec/resources/requests/storage",
        "/spec/volumeClaimTemplates/0/spec/resources/limits/storage",
    ]);
    let actual = create_and_check_storage(&mut k, &mut c, route, &paths)?;
    assert_canonical_cpu_memory(&actual);
    // Claim templates are immutable. A changed desired size must remain drift, not be applied.
    c.artifact["items"][0]["spec"]["volumeClaimTemplates"][0]["spec"]["resources"]["requests"]
        ["storage"] = json!("2Gi");
    c.artifact_sha256 = "claim-template-size-change".into();
    assert!(k.observe(&c)?.drifted);
    let (_, unchanged) = k.api.request("GET", route, None)?;
    assert_eq!(
        unchanged["spec"]["volumeClaimTemplates"][0]["spec"]["resources"]["requests"]["storage"],
        "1Gi"
    );
    Ok(())
}

#[path = "fixtures/kubernetes_compound_real.rs"]
mod compound;

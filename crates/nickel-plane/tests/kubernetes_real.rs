//! Real API semantics; invoked only by scripts/test-kind.py in its owned cluster.
#![cfg(feature = "kubernetes")]

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
    let create = k.prepare_change(&c, &observed, "create")?;
    c.inventory = k.execute_change(&c, &create)?.inventory;
    c.artifact["items"][0]["data"]["value"] = json!("two");
    c.artifact_sha256 = "two".into();
    let observed = k.observe(&c)?;
    assert!(observed.drifted);
    let stale = k.prepare_change(&c, &observed, "stale-transfer")?;
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
    let transfer = k.prepare_change(&c, &observed, "transfer")?;
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
    let update = k.prepare_change(&c, &observed, "update")?;
    assert_eq!(update.summary.field_ownership_transfers, 0);
    assert_eq!(update.summary.updates, 1);
    c.inventory = k.execute_change(&c, &update)?.inventory;
    c.artifact["items"][0]["data"]
        .as_object_mut()
        .ok_or_else(|| nickel_plane::Error::Invalid("Expected test data object".into()))?
        .remove("remove");
    c.artifact_sha256 = "three".into();
    let observed = k.observe(&c)?;
    let omit = k.prepare_change(&c, &observed, "omit")?;
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
    let create = k.prepare_change(&c, &observed, "omission-create")?;
    c.inventory = k.execute_change(&c, &create)?.inventory;
    c.artifact["items"][0]["data"]
        .as_object_mut()
        .ok_or_else(|| nickel_plane::Error::Invalid("Expected test data object".into()))?
        .remove("remove");
    c.artifact_sha256 = "omit-only".into();
    let observed = k.observe(&c)?;
    let transfer = k.prepare_change(&c, &observed, "omission-transfer")?;
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
    let apply = k.prepare_change(&c, &observed, "omission-apply")?;
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
    let plan = k.prepare_change(&race, &observed, "race-create")?;
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
    let plan = k.prepare_change(&c, &observed, "create-replacement")?;
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
    let observed = k.observe(&c)?;
    let plan = k.prepare_change(&c, &observed, "quantity-create")?;
    c.inventory = k.execute_change(&c, &plan)?.inventory;
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

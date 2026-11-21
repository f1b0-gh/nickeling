//! Model checks for ownership-transfer planning/recovery; real API semantics are
//! independently exercised by the ignored kind tests in kubernetes_real.rs.
#![cfg(feature = "kubernetes")]
#![allow(clippy::unwrap_used)]
use nickel_plane::{
    adapters::kubernetes::{Api, Kubernetes},
    backend::{Backend, Context, Readiness},
    model::Unit,
    targets::KubernetesTarget,
    Error, Result,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Default)]
struct FieldApi {
    objects: BTreeMap<String, Value>,
    writes: usize,
    interrupt: bool,
    alter_transfer: Option<&'static str>,
}
impl Api for FieldApi {
    fn request(&mut self, method: &str, route: &str, body: Option<&Value>) -> Result<(u16, Value)> {
        let path = route.split('?').next().unwrap();
        let body = body.cloned().unwrap_or(Value::Null);
        if method == "GET" {
            return Ok(self
                .objects
                .get(path)
                .map(|v| (200, v.clone()))
                .unwrap_or((404, Value::Null)));
        }
        let dry = route.contains("dryRun=All");
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
        let mut result = if method == "POST" {
            if self.objects.contains_key(&destination) {
                return Ok((409, Value::Null));
            }
            let mut value = body.clone();
            value["metadata"]["uid"] = json!(format!("uid-{}", self.writes + 1));
            value["metadata"]["managedFields"] = json!([{
                "manager":manager,"operation":"Update","apiVersion":"v1","fieldsType":"FieldsV1",
                "fieldsV1":{"f:data":{"f:value":{},"f:remove":{}}}
            }]);
            value
        } else {
            let Some(old) = self.objects.get(path) else {
                return Ok((404, Value::Null));
            };
            if let Some(ops) = body.as_array() {
                let mut value = old.clone();
                for op in ops {
                    let pointer = op["path"].as_str().unwrap();
                    match op["op"].as_str().unwrap() {
                        "test" if value.pointer(pointer) != Some(&op["value"]) => {
                            return Ok((409, Value::Null))
                        }
                        "test" => {}
                        "replace" => *value.pointer_mut(pointer).unwrap() = op["value"].clone(),
                        _ => {
                            return Err(Error::Invalid(
                                "Unexpected fixture JSON Patch operation".into(),
                            ))
                        }
                    }
                }
                if let Some(field) = self.alter_transfer {
                    if field == "ownership" {
                        value["metadata"]["managedFields"][1]["fieldsV1"] = json!({});
                    } else {
                        value["data"]["foreign"] = json!("unexpected-admission-change");
                    }
                }
                value
            } else {
                if old["metadata"]["uid"] != body["metadata"]["uid"]
                    || old["metadata"]["resourceVersion"] != body["metadata"]["resourceVersion"]
                    || old["metadata"]["managedFields"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|f| f["manager"] == manager && f["operation"] == "Update")
                {
                    return Ok((409, Value::Null));
                }
                let mut value = old.clone();
                // This fixture owns only value/remove; a foreign data key survives.
                for key in ["value", "remove"] {
                    if let Some(wanted) = body["data"].get(key) {
                        value["data"][key] = wanted.clone();
                    } else {
                        value["data"].as_object_mut().unwrap().remove(key);
                    }
                }
                value["metadata"]["annotations"] = body["metadata"]["annotations"].clone();
                value
            }
        };
        if !dry {
            self.writes += 1;
            result["metadata"]["resourceVersion"] = json!(self.writes.to_string());
            self.objects.insert(destination, result.clone());
            if self.interrupt {
                self.interrupt = false;
                return Err(Error::Backend("Interrupted fixture response".into()));
            }
        }
        Ok((200, result))
    }
}
fn fixture() -> Result<(Kubernetes<FieldApi>, Context)> {
    let unit: Unit = serde_json::from_value(
        json!({"backend":"kubernetes","target":"cluster","configuration":{}}),
    )?;
    let mut c = Context {
        owner: "owner".into(),
        instance: "instance".into(),
        unit_id: "app".into(),
        revision: "one".into(),
        unit,
        artifact: json!({"apiVersion":"v1","kind":"List","items":[{"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"settings","namespace":"test"},"data":{"value":"one","remove":"old"}}]}),
        artifact_sha256: "one".into(),
        inventory: Value::Null,
        interrupted: None,
        work: std::env::temp_dir(),
        engine_version: None,
        provider_lock: None,
    };
    let mut k = Kubernetes {
        api: FieldApi::default(),
        target: KubernetesTarget {
            server: "https://cluster.example".into(),
            namespace: "test".into(),
            ca_file: None,
            previous_ca_file: None,
            token_file: None,
            allow_http_loopback: false,
        },
    };
    let observed = k.observe(&c)?;
    let plan = k.prepare_change(&c, &observed, "create")?;
    c.inventory = k.execute_change(&c, &plan)?.inventory;
    let object = k.api.objects.get_mut(PATH).unwrap();
    object["data"]["foreign"] = json!("preserved");
    object["metadata"]["managedFields"].as_array_mut().unwrap().push(json!({
        "manager":"foreign","operation":"Apply","apiVersion":"v1","fieldsType":"FieldsV1","fieldsV1":{"f:data":{"f:foreign":{}}}
    }));
    // Omission alone is not detected by subset observation; the changed artifact
    // triggers this first plan, then durable pending state must trigger the next.
    c.artifact["items"][0]["data"]
        .as_object_mut()
        .unwrap()
        .remove("remove");
    c.artifact_sha256 = "two".into();
    c.revision = "two".into();
    Ok((k, c))
}
const PATH: &str = "/api/v1/namespaces/test/configmaps/settings";

#[test]
fn transfer_is_separately_reviewed_and_pending_survives_until_apply() -> Result<()> {
    let (mut k, mut c) = fixture()?;
    let before = k.api.objects[PATH].clone();
    let observed = k.observe(&c)?;
    let transfer = k.prepare_change(&c, &observed, "transfer")?;
    assert_eq!(
        serde_json::to_value(&transfer.summary)?["fieldOwnershipTransfers"],
        1
    );
    assert_eq!(transfer.summary.updates, 0);
    assert!(transfer.summary.changed());
    assert_eq!(k.api.objects[PATH], before, "preparation must only dry-run");
    let after = k.execute_change(&c, &transfer)?;
    assert!(after.drifted);
    assert_eq!(after.readiness, Readiness::NotReady);
    assert_eq!(k.api.objects[PATH]["data"], before["data"]);
    assert_eq!(
        k.api.objects[PATH]["metadata"]["managedFields"][1],
        before["metadata"]["managedFields"][1]
    );
    c.inventory = serde_json::from_slice(&serde_json::to_vec(&after.inventory)?)?;
    let restarted = k.observe(&c)?;
    assert!(restarted.drifted);
    assert_eq!(restarted.readiness, Readiness::NotReady);
    let apply = k.prepare_change(&c, &restarted, "apply")?;
    assert_eq!(apply.summary.updates, 1);
    let done = k.execute_change(&c, &apply)?;
    assert!(!done.drifted);
    assert_eq!(done.readiness, Readiness::Ready);
    assert!(k.api.objects[PATH]["data"].get("remove").is_none());
    assert_eq!(k.api.objects[PATH]["data"]["foreign"], "preserved");
    c.inventory = done.inventory;
    assert!(!k.observe(&c)?.drifted);
    Ok(())
}

#[test]
fn interrupted_transfer_and_apply_recover_without_losing_pending_configuration() -> Result<()> {
    let (mut k, mut c) = fixture()?;
    let observed = k.observe(&c)?;
    let transfer = k.prepare_change(&c, &observed, "transfer")?;
    k.api.interrupt = true;
    assert!(k.execute_change(&c, &transfer).is_err());
    c.interrupted = Some(transfer);
    let recovered = k.observe(&c)?;
    assert!(recovered.drifted);
    assert_eq!(recovered.readiness, Readiness::NotReady);
    c.inventory = recovered.inventory;
    c.interrupted = None;
    let observed = k.observe(&c)?;
    let apply = k.prepare_change(&c, &observed, "apply")?;
    c.interrupted = Some(apply.clone());
    assert!(
        k.observe(&c)?.drifted,
        "an unexecuted SSA journal cannot clear pending"
    );
    c.interrupted = None;
    k.api.interrupt = true;
    assert!(k.execute_change(&c, &apply).is_err());
    c.interrupted = Some(apply);
    let recovered = k.observe(&c)?;
    assert!(
        !recovered.drifted,
        "completed SSA must clear pending using its exact journal marker"
    );
    assert_eq!(recovered.readiness, Readiness::Ready);
    assert_eq!(k.api.writes, 3);
    Ok(())
}

#[test]
fn stale_transfer_and_ambiguous_managers_are_refused() -> Result<()> {
    let (mut k, c) = fixture()?;
    let observed = k.observe(&c)?;
    let transfer = k.prepare_change(&c, &observed, "transfer")?;
    k.api.objects.get_mut(PATH).unwrap()["metadata"]["resourceVersion"] = json!("changed");
    assert!(k.execute_change(&c, &transfer).is_err());
    assert_eq!(k.api.writes, 1);
    let fields = k.api.objects.get_mut(PATH).unwrap()["metadata"]["managedFields"]
        .as_array_mut()
        .unwrap();
    let mut duplicate = fields[0].clone();
    duplicate["operation"] = json!("Apply");
    fields.push(duplicate);
    let observed = k.observe(&c)?;
    assert!(k
        .prepare_change(&c, &observed, "ambiguous")
        .unwrap_err()
        .to_string()
        .contains("ownership"));
    assert_eq!(k.api.writes, 1);
    Ok(())
}

#[test]
fn partial_multi_object_handoff_retains_every_pending_apply() -> Result<()> {
    let (mut k, mut c) = fixture()?;
    let second = "/api/v1/namespaces/test/configmaps/zzz";
    let mut object = k.api.objects[PATH].clone();
    object["metadata"]["name"] = json!("zzz");
    object["metadata"]["uid"] = json!("uid-second");
    k.api.objects.insert(second.into(), object);
    let mut entry = c.inventory[PATH].clone();
    entry["path"] = json!(second);
    entry["name"] = json!("zzz");
    entry["uid"] = json!("uid-second");
    c.inventory[second] = entry;
    let mut desired = c.artifact["items"][0].clone();
    desired["metadata"]["name"] = json!("zzz");
    c.artifact["items"].as_array_mut().unwrap().push(desired);
    let observed = k.observe(&c)?;
    let transfer = k.prepare_change(&c, &observed, "first-transfer")?;
    assert_eq!(transfer.summary.field_ownership_transfers, 2);
    k.api.interrupt = true;
    assert!(k.execute_change(&c, &transfer).is_err());
    assert_eq!(
        k.api.objects[PATH]["metadata"]["managedFields"][0]["operation"],
        "Apply"
    );
    assert_eq!(
        k.api.objects[second]["metadata"]["managedFields"][0]["operation"],
        "Update"
    );
    c.interrupted = Some(transfer);
    c.inventory = k.observe(&c)?.inventory;
    c.interrupted = None;
    for path in [PATH, second] {
        assert_eq!(c.inventory[path]["pendingApply"], true);
    }
    let observed = k.observe(&c)?;
    let remaining = k.prepare_change(&c, &observed, "remaining-transfer")?;
    assert_eq!(remaining.summary.field_ownership_transfers, 1);
    c.inventory = k.execute_change(&c, &remaining)?.inventory;
    for path in [PATH, second] {
        assert_eq!(c.inventory[path]["pendingApply"], true);
    }
    let observed = k.observe(&c)?;
    let apply = k.prepare_change(&c, &observed, "apply-both")?;
    assert_eq!(apply.summary.updates, 2);
    k.api.interrupt = true;
    assert!(k.execute_change(&c, &apply).is_err());
    c.interrupted = Some(apply.clone());
    let recovered = k.observe(&c)?;
    assert_eq!(recovered.inventory[PATH]["pendingApply"], false);
    assert_eq!(recovered.inventory[second]["pendingApply"], true);
    assert!(recovered.drifted);
    assert_eq!(recovered.readiness, Readiness::NotReady);
    let writes = k.api.writes;
    assert!(
        k.execute_change(&c, &apply).is_err(),
        "partial completion invalidates the previous approval"
    );
    assert_eq!(k.api.writes, writes);
    c.inventory = recovered.inventory;
    c.interrupted = None;
    let observed = k.observe(&c)?;
    let fresh = k.prepare_change(&c, &observed, "fresh-apply")?;
    let done = k.execute_change(&c, &fresh)?;
    assert!(!done.drifted);
    assert_eq!(done.readiness, Readiness::Ready);
    for path in [PATH, second] {
        assert!(k.api.objects[path]["data"].get("remove").is_none());
    }
    Ok(())
}

#[test]
fn pending_configuration_does_not_prevent_explicit_removal() -> Result<()> {
    let (mut k, mut c) = fixture()?;
    let observed = k.observe(&c)?;
    let transfer = k.prepare_change(&c, &observed, "transfer")?;
    c.inventory = k.execute_change(&c, &transfer)?.inventory;
    assert_eq!(c.inventory[PATH]["pendingApply"], true);
    let observed = k.observe_removal(&c)?;
    let remove = k.prepare_removal(&c, &observed, "remove")?;
    assert_eq!(remove.summary.deletes, 1);
    assert_eq!(remove.summary.field_ownership_transfers, 0);
    assert!(remove.payload["actions"]
        .as_array()
        .unwrap()
        .iter()
        .all(|action| action["method"] == "DELETE"));
    Ok(())
}

#[test]
fn incompatible_ownership_and_unexpected_admission_changes_refuse_planning() -> Result<()> {
    for variant in ["version", "format", "budget", "admission", "ownership"] {
        let (mut k, c) = fixture()?;
        let field = &mut k.api.objects.get_mut(PATH).unwrap()["metadata"]["managedFields"][0];
        match variant {
            "version" => field["apiVersion"] = json!("other/v1"),
            "format" => field["fieldsType"] = json!("Unknown"),
            "budget" => field["fieldsV1"] = json!({"f:huge": "x".repeat(256 * 1024)}),
            "admission" => k.api.alter_transfer = Some("workload"),
            "ownership" => k.api.alter_transfer = Some("ownership"),
            _ => unreachable!(),
        }
        let before = k.api.objects[PATH].clone();
        let observed = k.observe(&c)?;
        assert!(
            k.prepare_change(&c, &observed, "refused").is_err(),
            "{variant}"
        );
        assert_eq!(k.api.objects[PATH], before);
        assert_eq!(k.api.writes, 1);
    }
    Ok(())
}

#[test]
fn legacy_summary_remains_readable_without_transfer_count() -> Result<()> {
    let summary: nickel_plane::backend::ChangeSummary = serde_json::from_value(json!({
        "creates":0,"updates":1,"deletes":0,"replacements":0
    }))?;
    assert_eq!(summary.field_ownership_transfers, 0);
    assert!(serde_json::to_value(summary)?
        .get("fieldOwnershipTransfers")
        .is_none());
    Ok(())
}

#[test]
fn unknown_or_null_plan_phase_is_refused_for_execution_and_recovery() -> Result<()> {
    let (mut k, mut c) = fixture()?;
    let observed = k.observe(&c)?;
    let transfer = k.prepare_change(&c, &observed, "transfer")?;
    let before = k.api.objects[PATH].clone();
    for phase in [json!("future-phase"), Value::Null, json!(42)] {
        let mut unknown = transfer.clone();
        unknown.payload["phase"] = phase;
        assert!(k
            .execute_change(&c, &unknown)
            .unwrap_err()
            .to_string()
            .contains("phase"));
        c.interrupted = Some(unknown);
        assert!(k.observe(&c).unwrap_err().to_string().contains("phase"));
        assert_eq!(k.api.objects[PATH], before);
        assert_eq!(k.api.writes, 1);
    }
    Ok(())
}

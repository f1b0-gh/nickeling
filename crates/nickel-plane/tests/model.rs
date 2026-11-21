use nickel_plane::model::*;
use serde_json::{json, Value};
use std::collections::BTreeSet;
fn document() -> Value {
    json!({"apiVersion":"nickel-plane/v1","id":"demo","composition":{"name":"application","version":"1"},"units":{
     "infra":{"backend":"opentofu","target":"local","schema":"local","configuration":{"resource":{}},"outputs":{"endpoint":{"pointer":"/outputs/endpoint"}}},
     "app":{"backend":"kubernetes","target":"cluster","configuration":{"url":"pending"},"bindings":[{"fromUnit":"infra","output":"endpoint","to":"/url","condition":"applied"}]}
    }})
}
#[test]
fn stable_graph_and_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let instance: Instance = serde_json::from_value(document())?;
    assert_eq!(instance.policy.mode, Management::Observe);
    assert_eq!(instance.policy.deletion, Deletion::Retain);
    assert_eq!(
        instance.validate(&BTreeSet::from(["local".into(), "cluster".into()]))?,
        vec!["infra", "app"]
    );
    assert_eq!(
        serde_json::from_value::<Instance>(serde_json::to_value(&instance)?)?,
        instance
    );
    Ok(())
}
#[test]
fn refuses_cycles_missing_outputs_paths_targets_and_incompatible_policies(
) -> Result<(), Box<dyn std::error::Error>> {
    let targets = BTreeSet::from(["local".into(), "cluster".into()]);
    let mut cases = Vec::new();
    let mut v = document();
    v["units"]["infra"]["dependsOn"] = json!([{"unit":"app","condition":"ready"}]);
    cases.push(v);
    let mut v = document();
    v["units"]["app"]["bindings"][0]["output"] = json!("missing");
    cases.push(v);
    let mut v = document();
    v["units"]["app"]["bindings"][0]["to"] = json!("/absent");
    cases.push(v);
    let mut v = document();
    v["units"]["app"]["target"] = json!("unknown");
    cases.push(v);
    let mut v = document();
    v["policy"] = json!({"mode":"observe","deletion":"delete","allowDelete":true});
    cases.push(v);
    for v in cases {
        let instance: Instance = serde_json::from_value(v)?;
        assert!(instance.validate(&targets).is_err());
    }
    let mut v = document();
    v["units"]["infra"]["backend"] = json!("shell");
    assert!(serde_json::from_value::<Instance>(v).is_err());
    Ok(())
}
#[test]
fn refuses_overlapping_bindings_and_path_escape() -> Result<(), Box<dyn std::error::Error>> {
    let mut v = document();
    v["units"]["app"]["configuration"] = json!({"url":{"value":"pending"}});
    v["units"]["app"]["bindings"]
        .as_array_mut()
        .ok_or("bindings")?
        .push(json!({"fromUnit":"infra","output":"endpoint","to":"/url/value"}));
    let instance: Instance = serde_json::from_value(v)?;
    assert!(instance
        .validate(&BTreeSet::from(["local".into(), "cluster".into()]))
        .is_err());
    assert!(pointer_parts("/a~2b").is_err());
    assert!(validate_id("../escape").is_err());
    Ok(())
}
#[test]
fn strict_json_rejects_duplicate_unit_ids_before_deserialization() {
    let input = br#"{"units":{"same":{},"same":{}}}"#;
    assert!(nickel_util::json::parse(input).is_err());
}

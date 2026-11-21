#![allow(clippy::unwrap_used)]

use nickel_tf::{rendering::render_config, schema::Selection};
use serde_json::{json, Value};
use std::{
    io::Write,
    process::{Command, Stdio},
};

fn selection() -> Selection {
    serde_json::from_value(json!({
        "engine": {"name": "opentofu", "version": "1.13.2"},
        "providers": {
            "terraform": {"source": "terraform.io/builtin/terraform", "version": null},
            "random": {"source": "registry.opentofu.org/hashicorp/random", "version": "3.7.2"}
        }
    }))
    .unwrap()
}

fn expression(source: &str) -> Value {
    json!({"__nickel_tf": {"kind": "expression", "source": source}})
}

#[test]
fn expression_positions_escape_literals_and_preserve_null_and_empty_values() {
    let got = render_config(
        json!({
            "resource": {"terraform_data": {"empty": {}, "example": {"input": {
                "text": "${var.name} %{ if true } $${already}", "nil": null, "empty": {},
                "${literal_key}": {"%{literal_key}": 1},
                "deferred": expression("var.name"),
                "reserved": {"__nickel_tf": {"kind": "literal", "value": {
                    "__nickel_tf": {"kind": "expression", "source": "${raw}"}
                }}}
            }}}},
            "locals": {"${literal_key}": "${literal_value}"}
        }),
        &selection(),
    )
    .unwrap();
    assert_eq!(got["resource"]["terraform_data"]["empty"], json!({}));
    assert_eq!(
        got["resource"]["terraform_data"]["example"]["input"],
        json!({
            "text": "$${var.name} %%{ if true } $$${already}", "nil": null, "empty": {},
            "$${literal_key}": {"%%{literal_key}": 1},
            "deferred": "${var.name}",
            "reserved": {"__nickel_tf": {"kind": "expression", "source": "$${raw}"}}
        })
    );
    assert_eq!(got["locals"]["${literal_key}"], "$${literal_value}");
}

#[test]
fn static_arguments_and_variable_defaults_are_not_template_escaped() {
    let got = render_config(json!({
        "variable": {"input": {"type": "string", "default": "${literal} %{ literal }", "description": "${literal}"}},
        "output": {"result": {"value": expression("var.input"), "description": "${literal}", "sensitive": false,
            "depends_on": ["terraform_data.before"]}},
        "provider": {"random": [{"alias": "other", "seed": "${literal}"}]},
        "resource": {"terraform_data": {"after": {"provider": "terraform.default", "depends_on": ["terraform_data.before"],
            "lifecycle": {"ignore_changes": ["input"], "replace_triggered_by": ["terraform_data.before"], "prevent_destroy": true}}}}
    }), &selection()).unwrap();
    assert_eq!(
        got["variable"]["input"]["default"],
        "${literal} %{ literal }"
    );
    assert_eq!(got["output"]["result"]["description"], "${literal}");
    assert_eq!(got["output"]["result"]["value"], "${var.input}");
    assert_eq!(
        got["output"]["result"]["depends_on"],
        json!(["terraform_data.before"])
    );
    assert_eq!(got["provider"]["random"][0]["seed"], "$${literal}");
    assert_eq!(
        got["resource"]["terraform_data"]["after"]["lifecycle"]["replace_triggered_by"],
        json!(["terraform_data.before"])
    );
}

#[test]
fn expression_markers_are_rejected_in_static_contexts_and_malformed_markers_fail() {
    for config in [
        json!({"variable": {"v": {"default": expression("var.x")}}}),
        json!({"output": {"v": {"value": 1, "description": expression("var.x")}}}),
        json!({"resource": {"terraform_data": {"v": {"depends_on": [expression("terraform_data.x")]}}}}),
        json!({"locals": {"v": {"__nickel_tf": {"kind": "unknown"}}}}),
        json!({"locals": {"v": {"__nickel_tf": {"kind": "expression", "source": ""}}}}),
        json!({"locals": {"v": {"__nickel_tf": {"kind": "expression", "source": "x", "extra": 1}}}}),
        json!({"locals": {"v": {"__nickel_tf": {"kind": "expression", "source": "x"}, "extra": 1}}}),
    ] {
        assert!(
            render_config(config.clone(), &selection()).is_err(),
            "accepted {config}"
        );
    }
}

#[test]
fn selection_injects_exact_constraints_and_rejects_conflicts() {
    let got = render_config(json!({}), &selection()).unwrap();
    assert_eq!(
        got["terraform"],
        json!({"required_version": "= 1.13.2", "required_providers": {
            "terraform": {"source": "terraform.io/builtin/terraform"},
            "random": {"source": "registry.opentofu.org/hashicorp/random", "version": "= 3.7.2"}
        }})
    );
    assert_eq!(render_config(got.clone(), &selection()).unwrap(), got);
    for config in [
        json!({"terraform": {"required_version": ">= 1.0"}}),
        json!({"terraform": {"required_providers": {"random": {"source": "registry.opentofu.org/hashicorp/random", "version": "= 3.0.0"}}}}),
        json!({"terraform": {"required_providers": {"other": {"source": "registry.opentofu.org/hashicorp/random", "version": "3.7.2"}}}}),
        json!({"terraform": {"backend": {"local": {}}}}),
        json!({"module": {"x": {"source": "./x"}}}),
        json!({"variable": {"v": {"validation": {}}}}),
        json!({"output": {"v": {"value": 1, "precondition": {}}}}),
        json!({"resource": {"terraform_data": {"v": {"provisioner": []}}}}),
    ] {
        assert!(
            render_config(config.clone(), &selection()).is_err(),
            "accepted {config}"
        );
    }
}

fn nickel(source: &str) -> std::process::Output {
    let bin = std::env::var_os("NICKEL_BIN").unwrap_or_else(|| "nickel".into());
    let mut child = Command::new(bin)
        .args(["export", "--format", "json"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(source.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn nickel_helpers_typecheck_literal_payloads_and_validate_reference_segments() {
    let prefix = "let Core = import \"nickel/contracts.ncl\" in let tf = import \"nickel/expressions.ncl\" in ";
    for body in [
        "(tf.literal 42) | Core.Value Number",
        "(tf.expr \"var.name\") | Core.Value Number",
        "(tf.ref [\"terraform_data\", \"example\", \"output\"]) | Core.Value Number",
        "[1, \"ok\"] | Core.Tuple [Number, String]",
        "[] | Core.Bounds 0 0 (Array Number)",
        "{ input = 1, count = 2 } | Core.Resource { input | Number }",
        "[{ alias = \"other\", region = \"local\" }] | Core.ProviderConfigs { region | String }",
        "(tf.literal { x = { __nickel_tf = { kind = \"expression\", source = \"x\" } } }) | Core.Value { x | Core.Value { __nickel_tf | Core.Value { kind | String, source | String } } }",
    ] {
        let result = nickel(&format!("{prefix}{body}"));
        assert!(result.status.success(), "{body}: {}", String::from_utf8_lossy(&result.stderr));
    }
    for body in [
        "(tf.literal \"wrong\") | Core.Value Number",
        "(tf.literal { __nickel_tf = { kind = \"expression\", source = \"x\" } }) | Core.Value Number",
        "(tf.literal { x = { __nickel_tf = { kind = \"expression\", source = \"x\" } } }) | Core.Value { x | Core.Value Number }",
        "tf.ref [\"terraform_data\", \"bad.name\", \"output\"]",
        "tf.ref []",
        "[1, 2] | Core.Tuple [Number, String]",
        "[1] | Core.Tuple [Number, String]",
        "[] | Core.Bounds 1 2 (Array Number)",
        "[1, 2, 3] | Core.Bounds 1 2 (Array Number)",
        "{ input = 1, typo = 2 } | Core.Resource { input | Number }",
        "{ input = 1, lifecycle = { made_up = true } } | Core.Resource { input | Number }",
        "{ input = 1, count = -1 } | Core.Resource { input | Number }",
        "{ input = 1, count = 1, for_each = {} } | Core.Resource { input | Number }",
        "{ input = 1, lifecycle = {} } | Core.Data { input | Number }",
        "{ default = tf.expr \"var.x\" } | Core.Variable",
        "{ value = 1, precondition = {} } | Core.Output",
        "{ default = 1, validation = {} } | Core.Variable",
    ] {
        let result = nickel(&format!("{prefix}{body}"));
        assert!(!result.status.success(), "accepted {body}");
    }
}

#[test]
fn schema_owner_is_explicit_when_resource_prefix_differs_and_foreign_owners_fail() {
    for wrapper in ["ResourceFor", "DataFor"] {
        for (body, expected) in [
            ("{}", json!({"provider": "other"})),
            ("{ provider = \"other\" }", json!({"provider": "other"})),
            (
                "{ provider = \"other.alias\" }",
                json!({"provider": "other.alias"}),
            ),
        ] {
            let result = nickel(&format!("let Core = import \"nickel/contracts.ncl\" in {body} | Core.{wrapper} \"other\" \"terraform\" {{}}"));
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(
                serde_json::from_slice::<Value>(&result.stdout).unwrap(),
                expected
            );
        }
        let result = nickel(&format!("let Core = import \"nickel/contracts.ncl\" in {{}} | Core.{wrapper} \"terraform\" \"terraform\" {{}}"));
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&result.stdout).unwrap(),
            json!({})
        );
        for address in ["foreign", "foreign.alias", "other.alias.extra", "other."] {
            let result = nickel(&format!("let Core = import \"nickel/contracts.ncl\" in {{ provider = \"{address}\" }} | Core.{wrapper} \"other\" \"terraform\" {{}}"));
            assert!(
                !result.status.success(),
                "accepted foreign or invalid owner {address}"
            );
        }
    }
}

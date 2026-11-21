#![allow(clippy::unwrap_used, clippy::panic)]
use nickel_tf::schema::{generate, validate_selection, Engine, ProviderPin, Selection};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::{fs, path::PathBuf, process::Command};

fn selection() -> Selection {
    Selection {
        engine: Engine {
            name: "opentofu".into(),
            version: "1.13.2".into(),
        },
        providers: BTreeMap::from([(
            "example".into(),
            ProviderPin {
                source: "registry.opentofu.org/acme/example".into(),
                version: Some("2.3.4".into()),
            },
        )]),
    }
}

fn schema(attributes: Value) -> Value {
    json!({"format_version":"1.0", "provider_schemas": {
        "registry.opentofu.org/acme/example": {
            "provider": {"version":0,"block":{}},
            "resource_schemas": {"example_item":{"version":91,"block":{"attributes":attributes}}},
            "data_source_schemas": {"example_lookup":{"version":0,"block":{"attributes":{"key":{"type":"string","required":true}}}}}
        }
    }})
}

#[test]
fn rejects_unsupported_schema_major_and_accepts_minor_additions() {
    let mut input = schema(json!({}));
    input["format_version"] = json!("2.0");
    assert!(generate(&input, &selection()).is_err());
    input["format_version"] = json!("1.99");
    input["future_metadata"] = json!(true);
    assert!(generate(&input, &selection()).is_ok());
}

#[test]
fn selection_requires_exact_versions_canonical_sources_and_unambiguous_names() {
    for (field, bad) in [
        ("engine", "terraform"),
        ("engine_version", ">= 1.0"),
        ("provider_version", "~> 2.3"),
        ("provider_version", "1.0.0-01"),
        ("source", "acme/example"),
        ("source", "registry.opentofu.org/../example"),
        ("source", "registry.opentofu.org/acme--inc/example"),
        ("source", "registry.opentofu.org/acme/e--x"),
        ("name", "bad/name"),
        ("name", "foo_bar"),
        ("name", "Foo"),
        ("name", "foo--bar"),
        ("name", "foo-"),
        ("name", "1example"),
    ] {
        let mut pins = selection();
        match field {
            "engine" => pins.engine.name = bad.into(),
            "engine_version" => pins.engine.version = bad.into(),
            "name" => {
                let pin = pins.providers.remove("example").unwrap();
                pins.providers.insert(bad.into(), pin);
            }
            "source" => pins.providers.get_mut("example").unwrap().source = bad.into(),
            _ => pins.providers.get_mut("example").unwrap().version = Some(bad.into()),
        }
        assert!(validate_selection(&pins).is_err(), "accepted {field} {bad}");
    }
    let mut pins = selection();
    pins.providers
        .insert("another".into(), pins.providers["example"].clone());
    assert!(generate(&schema(json!({})), &pins).is_err());
    pins = selection();
    pins.providers.get_mut("example").unwrap().version = None;
    assert!(generate(&schema(json!({})), &pins).is_err());
}

#[test]
fn selection_accepts_exact_prereleases_and_numeric_source_components() {
    let mut pins = selection();
    let mut pin = pins.providers.remove("example").unwrap();
    pin.version = Some("1.2.3-beta.1+build.01".into());
    pin.source = "registry.opentofu.org/1acme/2example".into();
    pins.providers.insert("example".into(), pin);
    assert!(validate_selection(&pins).is_ok());
}

#[test]
fn refuses_unselected_sources_and_missing_schema_provenance() {
    let mut input = schema(json!({}));
    input["provider_schemas"]["registry.opentofu.org/acme/extra"] =
        json!({"provider":{"version":0,"block":{}}});
    assert!(generate(&input, &selection()).is_err());
    let mut pins = selection();
    pins.providers.get_mut("example").unwrap().source = "registry.opentofu.org/acme/other".into();
    assert!(generate(&schema(json!({})), &pins).is_err());
}

#[test]
fn rejects_invalid_flags_types_and_nesting() {
    for attribute in [
        json!({"type":"string","required":true,"optional":true}),
        json!({"type":"string","required":true,"computed":true}),
        json!({"type":"string"}),
        json!({"type":"string","optional":"yes"}),
        json!({"type":"unknown","optional":true}),
        json!({"type":["list"],"optional":true}),
        json!({"type":["object",[]],"optional":true}),
        json!({"nested_type":{"nesting_mode":"mystery","attributes":{}},"optional":true}),
    ] {
        assert!(
            generate(&schema(json!({"value":attribute})), &selection()).is_err(),
            "accepted {attribute}"
        );
    }
    for nested in [
        json!({"nesting_mode":"mystery","block":{}}),
        json!({"nesting_mode":"list","min_items":3,"max_items":2,"block":{}}),
        json!({"nesting_mode":"list","min_items":-1,"block":{}}),
    ] {
        let mut input = schema(json!({}));
        input["provider_schemas"]["registry.opentofu.org/acme/example"]["resource_schemas"]
            ["example_item"]["block"]["block_types"] = json!({"child":nested});
        assert!(generate(&input, &selection()).is_err(), "accepted {nested}");
    }
}

#[test]
fn output_is_deterministic_and_split_per_provider_schema() {
    let input = schema(json!({"value":{"type":"string","optional":true}}));
    let first = generate(&input, &selection()).unwrap();
    assert_eq!(first, generate(&input, &selection()).unwrap());
    assert!(first.contains_key("schema.ncl"));
    assert!(
        first.len() >= 4,
        "provider, resource, data and root should be split"
    );
    assert!(first
        .keys()
        .all(|key| !key.starts_with('/') && !key.contains("..")));
}

#[test]
fn rejects_excessive_depth_input_size_and_file_count() {
    let mut deep = json!("string");
    for _ in 0..70 {
        deep = json!(["list", deep]);
    }
    assert!(generate(
        &schema(json!({"value":{"type":deep,"optional":true}})),
        &selection()
    )
    .is_err());
    let mut input = schema(json!({}));
    input["description"] = json!("a".repeat(16 * 1024 * 1024));
    assert!(generate(&input, &selection()).is_err());
    input = schema(json!({}));
    let resources = input["provider_schemas"]["registry.opentofu.org/acme/example"]
        ["resource_schemas"]
        .as_object_mut()
        .unwrap();
    for i in 0..2048 {
        resources.insert(format!("example_item_{i}"), json!({"version":0,"block":{}}));
    }
    assert!(generate(&input, &selection()).is_err());
}

#[test]
fn generated_file_limit_reserves_space_for_the_three_core_library_modules() {
    let mut input = schema(json!({}));
    for index in 0..2041 {
        input["provider_schemas"]["registry.opentofu.org/acme/example"]["resource_schemas"]
            .as_object_mut()
            .unwrap()
            .insert(
                format!("example_extra_{index}"),
                json!({"version":0,"block":{}}),
            );
    }
    assert_eq!(generate(&input, &selection()).unwrap().len(), 2045);
    input["provider_schemas"]["registry.opentofu.org/acme/example"]["resource_schemas"]
        ["example_over_budget"] = json!({"version":0,"block":{}});
    assert!(
        generate(&input, &selection()).is_err(),
        "2046 generated files plus three core files exceed the evaluator's 2048-file budget"
    );
}

struct Evaluator(tempfile::TempDir);

impl Evaluator {
    fn new(input: &Value) -> Self {
        Self::with_selection(input, &selection())
    }

    fn with_selection(input: &Value, selection: &Selection) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let core = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("nickel/contracts.ncl");
        for (file, source) in generate(input, selection).unwrap() {
            let target = directory.path().join(file);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            let source =
                source.replace("/libraries/nickel-tf/contracts.ncl", core.to_str().unwrap());
            fs::write(target, source).unwrap();
        }
        fs::write(
            directory.path().join("test.ncl"),
            "(import \"input.json\") | (import \"schema.ncl\")\n",
        )
        .unwrap();
        Self(directory)
    }

    fn check(&self, config: &Value, valid: bool) {
        self.check_as(config, valid, config);
    }

    fn check_as(&self, config: &Value, valid: bool, expected: &Value) {
        fs::write(
            self.0.path().join("input.json"),
            serde_json::to_vec(config).unwrap(),
        )
        .unwrap();
        let binary = std::env::var_os("NICKEL_BIN").unwrap_or_else(|| {
            let pinned = "/nix/store/pac5iqpjwzikq5gagg3vp3bcr6fj6ahh-nickel-1.18.0/bin/nickel";
            if std::path::Path::new(pinned).is_file() {
                pinned.into()
            } else {
                "nickel".into()
            }
        });
        let output = Command::new(binary)
            .args(["export", "--format", "json"])
            .arg(self.0.path().join("test.ncl"))
            .output()
            .unwrap_or_else(|error| {
                panic!("Nickel test requires NICKEL_BIN pointing to Nickel 1.18.0: {error}")
            });
        assert_eq!(
            output.status.success(),
            valid,
            "config {config}; Nickel stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        if valid {
            assert_eq!(
                serde_json::from_slice::<Value>(&output.stdout).unwrap(),
                *expected,
                "contracts must preserve the configuration"
            );
        }
    }
}

#[test]
fn generated_schema_binds_resource_and_data_to_the_provider_that_supplied_it() {
    let mut pins = selection();
    let pin = pins.providers.remove("example").unwrap();
    pins.providers.insert("cloud".into(), pin);
    pins.providers.insert(
        "other".into(),
        ProviderPin {
            source: "registry.opentofu.org/acme/other".into(),
            version: Some("1.0.0".into()),
        },
    );
    let mut input = schema(json!({}));
    input["provider_schemas"]["registry.opentofu.org/acme/other"] =
        json!({"provider":{"version":0,"block":{}}});
    let evaluator = Evaluator::with_selection(&input, &pins);
    evaluator.check_as(
        &json!({"resource":{"example_item":{"one":{}}},"data":{"example_lookup":{"one":{"key":"ok"}}}}), true,
        &json!({"resource":{"example_item":{"one":{"provider":"cloud"}}},"data":{"example_lookup":{"one":{"key":"ok","provider":"cloud"}}}})
    );
    evaluator.check(
        &json!({"resource":{"example_item":{"one":{"provider":"cloud.alias"}}}}),
        true,
    );
    for provider in [
        "example",
        "other",
        "other.alias",
        "cloudish",
        "cloud.",
        "cloud.a.b",
    ] {
        evaluator.check(
            &json!({"resource":{"example_item":{"one":{"provider":provider}}}}),
            false,
        );
        evaluator.check(
            &json!({"data":{"example_lookup":{"one":{"key":"ok","provider":provider}}}}),
            false,
        );
    }
}

#[test]
fn nickel_enforces_required_optional_computed_and_closed_records() {
    let evaluator = Evaluator::new(&schema(json!({
        "name":{"type":"string","required":true},
        "hint":{"type":"string","optional":true},
        "fallback":{"type":"number","optional":true,"computed":true},
        "id":{"type":"string","computed":true}
    })));
    for body in [
        json!({"name":"ok"}),
        json!({"name":"ok","hint":null,"fallback":2}),
        json!({"name":{"__nickel_tf":{"kind":"expression","source":"var.name"}}}),
    ] {
        evaluator.check(&json!({"resource":{"example_item":{"one":body}}}), true);
    }
    for body in [
        json!({}),
        json!({"name":null}),
        json!({"name":42}),
        json!({"name":"ok","hint":false}),
        json!({"name":"ok","id":"forbidden"}),
        json!({"name":"ok","extra":"forbidden"}),
    ] {
        evaluator.check(&json!({"resource":{"example_item":{"one":body}}}), false);
    }
    evaluator.check(&json!({"unsupported":{}}), false);
    evaluator.check(&json!({"resource":{"unknown":{"one":{}}}}), false);
    evaluator.check(
        &json!({"data":{"example_lookup":{"one":{"key":"ok"}}}}),
        true,
    );
    evaluator.check(&json!({"data":{"example_lookup":{"one":{}}}}), false);
}

#[test]
fn nickel_checks_collections_objects_tuples_and_dynamic_nullability() {
    let evaluator = Evaluator::new(&schema(json!({
        "list":{"type":["list","number"],"optional":true},
        "set":{"type":["set","string"],"optional":true},
        "map":{"type":["map","bool"],"optional":true},
        "tuple":{"type":["tuple",["string","number"]],"optional":true},
        "object":{"type":["object",{"name":"string","enabled":"bool"}],"optional":true},
        "anything":{"type":"dynamic","required":true}
    })));
    evaluator.check(
        &json!({"resource":{"example_item":{"one":{
            "list":[1,null,2],"set":["a"],"map":{},"tuple":["a",3],
            "object":{"name":"a","enabled":true},"anything":{}
        }}}}),
        true,
    );
    for (key, value) in [
        ("list", json!(["bad"])),
        ("set", json!([5])),
        ("map", json!({"one":"bad"})),
        ("tuple", json!(["a"])),
        ("tuple", json!(["a", 3, 4])),
        ("tuple", json!([4, "a"])),
        ("object", json!({"name":"a"})),
        ("object", json!({"name":"a","enabled":true,"extra":1})),
        ("anything", Value::Null),
    ] {
        let mut body = json!({"anything":true});
        body[key] = value;
        evaluator.check(&json!({"resource":{"example_item":{"one":body}}}), false);
    }
}

#[test]
fn nickel_checks_nested_attributes_blocks_and_cardinality() {
    let mut input = schema(json!({"nested":{
        "type":"unsupported_ignored_due_to_nested_type",
        "nested_type":{"nesting_mode":"list","attributes":{
            "name":{"type":"string","required":true},
            "computed":{"type":"string","computed":true}
        }},"optional":true
    }}));
    input["provider_schemas"]["registry.opentofu.org/acme/example"]["resource_schemas"]
        ["example_item"]["block"]["block_types"] = json!({
        "children":{"nesting_mode":"list","min_items":1,"max_items":2,"block":{"attributes":{"enabled":{"type":"bool","required":true}}}},
        "single":{"nesting_mode":"single","block":{"attributes":{"value":{"type":"number","optional":true}}}},
        "group":{"nesting_mode":"group","block":{}},
        "map":{"nesting_mode":"map","block":{}},
        "set":{"nesting_mode":"set","block":{}}
    });
    let evaluator = Evaluator::new(&input);
    evaluator.check(&json!({"resource":{"example_item":{"one":{
        "children":[{"enabled":true}],"nested":[{"name":"ok"}],"single":{},"group":{},"map":{"label":{}},"set":[{}]
    }}}}), true);
    for body in [
        json!({}),
        json!({"children":[]}),
        json!({"children":[{}]}),
        json!({"children":[{"enabled":true},{"enabled":true},{"enabled":true}]}),
        json!({"children":[{"enabled":true}],"nested":[{}]}),
        json!({"children":[{"enabled":true}],"nested":[{"name":"ok","computed":"bad"}]}),
        json!({"children":[{"enabled":true}],"single":{"extra":true}}),
        json!({"children":[{"enabled":true}],"map":[{}]}),
        json!({"children":[{"enabled":true}],"map":{"${var.label}":{}}}),
        json!({"children":[{"enabled":true}],"map":{"%{if true}":{}}}),
    ] {
        evaluator.check(&json!({"resource":{"example_item":{"one":body}}}), false);
    }
}

#[test]
fn schema_text_cannot_inject_nickel_code() {
    for name in [
        "evil%{import \"/etc/passwd\"}",
        "name\" = 1, injected",
        "../escape",
    ] {
        let mut attributes = serde_json::Map::new();
        attributes.insert(name.into(), json!({"type":"string","optional":true}));
        assert!(generate(&schema(Value::Object(attributes)), &selection()).is_err());
    }
    let dangerous_key = "literal%{import \"/etc/passwd\"}\n\\end\0\u{0008}\u{000c}";
    let mut members = serde_json::Map::new();
    members.insert(dangerous_key.into(), json!("string"));
    let evaluator = Evaluator::new(&schema(json!({"object":{
        "type":["object",members],"optional":true,
        "description":"%{import \"/etc/passwd\"}\"; evil = true"
    }})));
    let mut value = serde_json::Map::new();
    value.insert(dangerous_key.into(), json!("retained"));
    evaluator.check(
        &json!({"resource":{"example_item":{"one":{"object":value}}}}),
        true,
    );
}

#[test]
fn builtin_provider_is_bound_to_engine_without_requiring_configuration_block() {
    let mut pins = selection();
    pins.providers.clear();
    pins.providers.insert(
        "terraform".into(),
        ProviderPin {
            source: "terraform.io/builtin/terraform".into(),
            version: None,
        },
    );
    let input = json!({"format_version":"1.0","provider_schemas":{"terraform.io/builtin/terraform":{"provider":{"version":0}}}});
    assert!(generate(&input, &pins).is_ok());
    pins.providers.get_mut("terraform").unwrap().version = Some("0.0.0".into());
    assert!(generate(&input, &pins).is_err());
}

#[test]
fn refuses_schema_fields_that_collide_with_core_meta_arguments() {
    for name in [
        "count",
        "for_each",
        "provider",
        "depends_on",
        "lifecycle",
        "connection",
        "provisioner",
        "dynamic",
    ] {
        let mut attributes = serde_json::Map::new();
        attributes.insert(name.into(), json!({"type":"string","optional":true}));
        assert!(
            generate(&schema(Value::Object(attributes)), &selection()).is_err(),
            "accepted resource field {name}"
        );
    }
    for name in ["alias", "version"] {
        let mut input = schema(json!({}));
        input["provider_schemas"]["registry.opentofu.org/acme/example"]["provider"]["block"]
            ["attributes"] = json!({name:{"type":"string","optional":true}});
        assert!(
            generate(&input, &selection()).is_err(),
            "accepted provider field {name}"
        );
    }
}

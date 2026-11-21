#![cfg(all(feature = "opentofu", feature = "kubernetes"))]
use nickel_plane::composition::{compile, CompileRequest};
use nickel_util::files;
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs,
    path::PathBuf,
};
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
#[test]
fn composed_domains_validate_and_lock_transitive_inputs() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("project");
    fs::create_dir(&project)?;
    let tf = root().parent().ok_or("parent")?.join("nickel-tf");
    let k8s = root().parent().ok_or("parent")?.join("nickel-k8s");
    let bundle = temp.path().join("bundle");
    nickel_tf::bundle::import_schema(
        &tf.join("examples/local/schema.json"),
        &tf.join("examples/local/selection.json"),
        None,
        &bundle,
    )?;
    fs::write(project.join("parameters.ncl"), "{name=\"demo\"}")?;
    fs::write(
        project.join("main.ncl"),
        r#"
 let t=import "/libraries/nickel-tf/composition.ncl" in
 let k=import "/libraries/nickel-k8s/composition.ncl" in
 let profiles=import "/libraries/nickel-k8s/profiles.ncl" in
 let p=import "parameters.ncl" in
 {apiVersion="nickel-plane/v1",id=p.name,composition={name="example",version="1"},parameters=p,units={
  infra={backend="opentofu",target="local",schema="local",configuration=t.compose {
   a={resource.terraform_data.a.input="literal"},b={resource.terraform_data.b={}}
  }},
  app={backend="kubernetes",target="cluster",configuration=k.compose (profiles.kubernetes {}) {
   application=[{apiVersion="v1",kind="ConfigMap",metadata={name=p.name,namespace="test"},data={url="pending"}}]
  }}
 }}
 "#,
    )?;
    let req = CompileRequest {
        config: project.join("main.ncl"),
        resources: root(),
        tf_resources: Some(tf),
        k8s_resources: Some(k8s),
        schemas: BTreeMap::from([("local".into(), bundle)]),
        targets: BTreeSet::from(["local".into(), "cluster".into()]),
    };
    let one = compile(&req)?;
    let two = compile(&req)?;
    assert_eq!(one.revision, two.revision);
    assert_eq!(
        one.units["infra"].artifact["resource"]["terraform_data"]["b"],
        json!({})
    );
    assert_eq!(
        one.units["app"].artifact["items"][0]["metadata"]["name"],
        "demo"
    );
    fs::write(project.join("parameters.ncl"), "{name=\"updated\"}")?;
    assert_ne!(compile(&req)?.revision, one.revision);
    assert_eq!(one.revision.len(), 64);
    let lock = temp.path().join("lock.json");
    files::publish_json(&lock, &one, false)?;
    Ok(())
}
#[test]
fn data_bridge_cannot_interpret_nickel_strings_as_source() -> Result<(), Box<dyn Error>> {
    let root = root();
    let tf = root.parent().ok_or("parent")?.join("nickel-tf");
    let tmp = tempfile::tempdir()?;
    let bundle = tmp.path().join("bundle");
    nickel_tf::bundle::import_schema(
        &tf.join("examples/local/schema.json"),
        &tf.join("examples/local/selection.json"),
        None,
        &bundle,
    )?;
    let value = json!({"resource":{"terraform_data":{"test":{"input":"%{import \"/etc/passwd\"}\u{0}\n"}}}});
    let checked = nickel_tf::check_value(&value, &tf, &bundle)?;
    assert_eq!(
        checked.configuration["resource"]["terraform_data"]["test"]["input"],
        "%%{import \"/etc/passwd\"}\u{0}\n"
    );
    assert!(nickel_tf::check_value(
        &json!({"resource":{"terraform_data":{"test":{"output":1}}}}),
        &tf,
        &bundle
    )
    .is_err());
    Ok(())
}
#[test]
fn reusable_compositions_enforce_inputs_and_reject_duplicate_resource_identities(
) -> Result<(), Box<dyn Error>> {
    use nickel_util::evaluation::{evaluate_bundled, BundleEvaluationRequest, LibraryBundle};
    let temp = tempfile::tempdir()?;
    let entry = temp.path().join("main.ncl");
    let tf = root().parent().ok_or("parent")?.join("nickel-tf/nickel");
    let k8s = root().parent().ok_or("parent")?.join("nickel-k8s/nickel");
    let resources = root();
    let libraries = [
        LibraryBundle {
            name: "nickel-tf",
            root: &tf,
        },
        LibraryBundle {
            name: "nickel-k8s",
            root: &k8s,
        },
    ];
    let req = BundleEvaluationRequest {
        entry: &entry,
        nix: &resources,
        libraries: &libraries,
        contract: None,
    };
    fs::write(
        &entry,
        r#"let c=import "/libraries/nickel-tf/composition.ncl" in c.compose {one={resource.terraform_data.item={}},two={resource.terraform_data.item={}}}"#,
    )?;
    assert!(evaluate_bundled(&req).is_err());
    for name in ["nickel-tf", "nickel-k8s"] {
        fs::write(
            &entry,
            format!(
                r#"let c=import "/libraries/{name}/composition.ncl" in (c.define {{name | String}} (fun p => p.name)) {{name=1}}"#
            ),
        )?;
        assert!(evaluate_bundled(&req).is_err());
    }
    let source = r#"let c=import "/libraries/nickel-k8s/composition.ncl" in let p=import "/libraries/nickel-k8s/profiles.ncl" in let item={apiVersion="v1",kind="ConfigMap",metadata={name="same",namespace="demo"}} in c.compose (p.kubernetes {}) {one=[item],two=[item]}"#;
    fs::write(&entry, source)?;
    let checked = evaluate_bundled(&req)?;
    assert!(nickel_k8s::check_value(
        &checked.evaluation.value,
        &root().parent().ok_or("parent")?.join("nickel-k8s")
    )
    .is_err());
    Ok(())
}

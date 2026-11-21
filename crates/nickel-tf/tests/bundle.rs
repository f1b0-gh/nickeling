use nickel_tf::bundle;
use nickel_util::files;
use serde_json::json;
use std::{error::Error, fs, path::Path};

fn fixture(root: &Path) -> Result<(), Box<dyn Error>> {
    fs::write(
        root.join("schema.json"),
        serde_json::to_vec(&json!({
            "format_version":"1.0", "provider_schemas":{
                "terraform.io/builtin/terraform":{
                    "provider":{"version":0,"block":{}},
                    "resource_schemas":{"terraform_data":{"version":0,"block":{
                        "attributes":{"input":{"type":"dynamic","optional":true},"output":{"type":"dynamic","computed":true}}
                    }}}
                }
            }
        }))?,
    )?;
    fs::write(
        root.join("selection.json"),
        serde_json::to_vec(&json!({
            "engine":{"name":"opentofu","version":"1.13.2"},
            "providers":{"terraform":{"source":"terraform.io/builtin/terraform","version":null}}
        }))?,
    )?;
    Ok(())
}

#[test]
fn publishes_complete_bundle_without_overwrite() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    fixture(temp.path())?;
    let out = temp.path().join("bundle");
    let manifest = bundle::import_schema(
        &temp.path().join("schema.json"),
        &temp.path().join("selection.json"),
        None,
        &out,
    )?;
    assert!(out.join("generated/schema.ncl").is_file());
    assert_eq!(
        manifest.files["schema.json"],
        files::hash(&fs::read(temp.path().join("schema.json"))?)
    );
    let before = fs::read(out.join("manifest.json"))?;
    assert!(bundle::import_schema(
        &temp.path().join("schema.json"),
        &temp.path().join("selection.json"),
        None,
        &out
    )
    .is_err());
    assert_eq!(fs::read(out.join("manifest.json"))?, before);
    bundle::verify(&out)?;
    Ok(())
}

#[test]
fn generated_contract_cannot_be_weakened_even_with_updated_hashes() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    fixture(temp.path())?;
    let out = temp.path().join("bundle");
    let mut manifest = bundle::import_schema(
        &temp.path().join("schema.json"),
        &temp.path().join("selection.json"),
        None,
        &out,
    )?;
    fs::write(out.join("generated/schema.ncl"), "Dyn\n")?;
    assert!(bundle::verify(&out).is_err());
    manifest
        .files
        .insert("generated/schema.ncl".into(), files::hash(b"Dyn\n"));
    files::publish_json(&out.join("manifest.json"), &manifest, true)?;
    assert!(bundle::verify(&out).is_err());
    Ok(())
}

#[test]
fn rejects_symlinks_and_unsafe_manifest_paths() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    fixture(temp.path())?;
    let out = temp.path().join("bundle");
    std::os::unix::fs::symlink(
        temp.path().join("schema.json"),
        temp.path().join("linked.json"),
    )?;
    assert!(bundle::import_schema(
        &temp.path().join("linked.json"),
        &temp.path().join("selection.json"),
        None,
        &out
    )
    .is_err());
    let mut manifest = bundle::import_schema(
        &temp.path().join("schema.json"),
        &temp.path().join("selection.json"),
        None,
        &out,
    )?;
    manifest.files.insert("../secret".into(), "x".into());
    files::publish_json(&out.join("manifest.json"), &manifest, true)?;
    assert!(bundle::verify(&out).is_err());
    let linked_parent = temp.path().join("linked-parent");
    std::os::unix::fs::symlink(temp.path(), &linked_parent)?;
    assert!(bundle::import_schema(
        &temp.path().join("schema.json"),
        &temp.path().join("selection.json"),
        None,
        &linked_parent.join("new")
    )
    .is_err());
    Ok(())
}

#[test]
fn external_providers_require_and_preserve_native_lock() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    fixture(temp.path())?;
    let schema = temp.path().join("schema.json");
    let selection = temp.path().join("selection.json");
    for path in [&schema, &selection] {
        let text = fs::read_to_string(path)?
            .replace(
                "terraform.io/builtin/terraform",
                "registry.opentofu.org/example/test",
            )
            .replace("\"version\":null", "\"version\":\"1.2.3\"");
        fs::write(path, text)?;
    }
    let out = temp.path().join("bundle");
    assert!(bundle::import_schema(&schema, &selection, None, &out).is_err());
    let native = temp.path().join("native.lock.hcl");
    let bytes = b"# Native lock remains opaque to the compiler\nprovider \"registry.opentofu.org/example/test\" {\n version = \"1.2.3\"\n hashes = [\"h1:fixture\"]\n}\n";
    fs::write(&native, bytes)?;
    bundle::import_schema(&schema, &selection, Some(&native), &out)?;
    assert_eq!(fs::read(out.join(".terraform.lock.hcl"))?, bytes);
    bundle::verify(&out)?;
    fs::write(out.join(".terraform.lock.hcl"), "changed")?;
    assert!(bundle::verify(&out).is_err());
    Ok(())
}

#[test]
fn bounds_inputs_and_rejects_duplicate_keys_before_publication() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    fixture(temp.path())?;
    let out = temp.path().join("bundle");
    let schema = temp.path().join("schema.json");
    fs::write(
        &schema,
        b"{\"format_version\":\"1.0\",\"format_version\":\"2.0\"}",
    )?;
    assert!(
        bundle::import_schema(&schema, &temp.path().join("selection.json"), None, &out).is_err()
    );
    assert!(!out.exists());
    fs::File::create(&schema)?.set_len(16 * 1024 * 1024 + 1)?;
    assert!(
        bundle::import_schema(&schema, &temp.path().join("selection.json"), None, &out).is_err()
    );
    assert!(!out.exists());
    Ok(())
}

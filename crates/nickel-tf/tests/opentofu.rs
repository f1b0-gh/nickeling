use nickel_tf::{bundle, render, Request};
use nickel_util::files;
use serde_json::{json, Value};
use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn tofu(root: &Path, args: &[&str]) -> Result<Vec<u8>, Box<dyn Error>> {
    tofu_with(root, args, "TOFU_BIN")
}
fn tofu_with(root: &Path, args: &[&str], variable: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let program = std::env::var_os(variable).ok_or_else(|| {
        format!(
            "Set {variable} to the corresponding pinned OpenTofu binary; see README verification"
        )
    })?;
    let output = Command::new(program)
        .current_dir(root)
        .env_clear()
        .env("HOME", root.join("home"))
        .env("PATH", "/usr/bin:/bin")
        .env("TF_CLI_CONFIG_FILE", root.join("empty.tfrc"))
        .env("TF_IN_AUTOMATION", "1")
        .env("CHECKPOINT_DISABLE", "1")
        .env("HTTP_PROXY", "http://127.0.0.1:9")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("NO_PROXY", "")
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "OpenTofu {args:?} failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(output.stdout)
}
#[test]
fn pinned_opentofu_validates_plans_and_resolves_local_expressions() -> Result<(), Box<dyn Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let temp = tempfile::tempdir()?;
    fs::create_dir(temp.path().join("home"))?;
    fs::write(
        temp.path().join("empty.tfrc"),
        "disable_checkpoint = true\n",
    )?;
    let versions: Value = serde_json::from_slice(&tofu(temp.path(), &["version", "-json"])?)?;
    assert_eq!(
        versions["terraform_version"], "1.13.2",
        "Build the pinned .#opentofu and set TOFU_BIN"
    );
    let bundle = temp.path().join("bundle");
    bundle::import_schema(
        &root.join("examples/local/schema.json"),
        &root.join("examples/local/selection.json"),
        None,
        &bundle,
    )?;
    let request = Request {
        config: root.join("examples/local/main.ncl"),
        resources: root,
        bundle,
    };
    let config = render(&request, None)?;
    files::publish_json(&temp.path().join("main.tf.json"), &config, false)?;
    tofu(
        temp.path(),
        &["init", "-backend=false", "-input=false", "-no-color"],
    )?;
    let validation: Value = serde_json::from_slice(&tofu(temp.path(), &["validate", "-json"])?)?;
    assert_eq!(validation["valid"], true);
    tofu(
        temp.path(),
        &["plan", "-input=false", "-no-color", "-out=fixture.plan"],
    )?;
    // Only built-in terraform_data resources, in disposable local state.
    tofu(
        temp.path(),
        &["apply", "-input=false", "-no-color", "fixture.plan"],
    )?;
    let outputs: Value = serde_json::from_slice(&tofu(temp.path(), &["output", "-json"])?)?;
    assert_eq!(
        outputs["settings"]["value"],
        json!({"message":"local OpenTofu fixture","literal_template":"${leave_this_literal}","empty_map":{},"${literal_key}":"key preserved"})
    );
    Ok(())
}

#[test]
fn pinned_external_local_provider_uses_native_readonly_lock() -> Result<(), Box<dyn Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let temp = tempfile::tempdir()?;
    fs::create_dir(temp.path().join("home"))?;
    fs::write(
        temp.path().join("empty.tfrc"),
        "disable_checkpoint = true\n",
    )?;
    let bundle = temp.path().join("bundle");
    let native = root.join("examples/random/.terraform.lock.hcl");
    bundle::import_schema(
        &root.join("examples/random/schema.json"),
        &root.join("examples/random/selection.json"),
        Some(&native),
        &bundle,
    )?;
    let request = Request {
        config: root.join("examples/random/main.ncl"),
        resources: root,
        bundle,
    };
    let config = render(&request, None)?;
    files::publish_json(&temp.path().join("main.tf.json"), &config, false)?;
    assert_eq!(
        config["resource"]["random_id"]["fixture"]["provider"],
        "entropy"
    );
    assert_eq!(
        config["resource"]["random_pet"]["empty"]["provider"],
        "entropy"
    );
    assert!(config["terraform"]["required_providers"]
        .get("random")
        .is_none());
    let lock_bytes = fs::read(&native)?;
    fs::write(temp.path().join(".terraform.lock.hcl"), &lock_bytes)?;
    tofu_with(
        temp.path(),
        &[
            "init",
            "-backend=false",
            "-lockfile=readonly",
            "-input=false",
            "-no-color",
        ],
        "TOFU_LOCAL_BIN",
    )?;
    let validation: Value = serde_json::from_slice(&tofu_with(
        temp.path(),
        &["validate", "-json"],
        "TOFU_LOCAL_BIN",
    )?)?;
    assert_eq!(validation["valid"], true);
    tofu_with(
        temp.path(),
        &["plan", "-input=false", "-no-color", "-out=fixture.plan"],
        "TOFU_LOCAL_BIN",
    )?;
    // Random resources only write to disposable local state, no cloud account.
    tofu_with(
        temp.path(),
        &["apply", "-input=false", "-no-color", "fixture.plan"],
        "TOFU_LOCAL_BIN",
    )?;
    let outputs: Value = serde_json::from_slice(&tofu_with(
        temp.path(),
        &["output", "-json"],
        "TOFU_LOCAL_BIN",
    )?)?;
    let hex = outputs["hex"]["value"]
        .as_str()
        .ok_or("missing hex output")?;
    assert_eq!(hex.len(), 8);
    assert!(hex.bytes().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(
        fs::read(temp.path().join(".terraform.lock.hcl"))?,
        lock_bytes
    );
    Ok(())
}

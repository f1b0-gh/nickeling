#![cfg(feature = "opentofu")]
use nickel_plane::{
    adapters::opentofu::OpenTofu, backend::*, model::Unit, targets::OpenTofuTarget, Error, Result,
};
use serde_json::{json, Value};
use std::{os::unix::fs::PermissionsExt, path::PathBuf};
#[test]
fn native_saved_plans_preserve_state_identity_and_protect_sensitive_outputs() -> Result<()> {
    let temp = tempfile::tempdir()?;
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700))?;
    let binary = std::env::var_os("TOFU_BIN")
        .ok_or_else(|| Error::Invalid("Set TOFU_BIN to pinned OpenTofu 1.13.2".into()))?;
    let target = OpenTofuTarget {
        private_diagnostics: false,
        binary: binary.into(),
        stack: "fixture".into(),
        credentials: None,
        backend_config: None,
        timeout_seconds: 60,
    };
    let mut backend = OpenTofu::new(target)?;
    let unit: Unit = serde_json::from_value(
        json!({"backend":"opentofu","target":"local","schema":"local","configuration":{},"outputs":{"result":{"pointer":"/outputs/result"},"secret":{"pointer":"/outputs/private"}}}),
    )?;
    let artifact = json!({"terraform":{"required_version":"= 1.13.2"},"resource":{"terraform_data":{"fixture":{"input":"local"}}},"output":{"result":{"value":"${terraform_data.fixture.output}"},"private":{"value":"fixture-sensitive-value","sensitive":true}}});
    let mut context = Context {
        owner: "test-owner".into(),
        instance: "fixture".into(),
        unit_id: "state".into(),
        revision: "first".into(),
        unit,
        artifact,
        artifact_sha256: "hash".into(),
        inventory: Value::Null,
        interrupted: None,
        work: temp.path().into(),
        engine_version: Some("1.13.2".into()),
        provider_lock: None,
    };
    let external = tempfile::NamedTempFile::new()?;
    std::fs::write(external.path(), "preserve-hardlink")?;
    std::fs::hard_link(external.path(), context.work.join("main.tf.json"))?;
    assert!(backend.observe(&context).is_err());
    assert_eq!(
        std::fs::read_to_string(external.path())?,
        "preserve-hardlink"
    );
    std::fs::remove_file(context.work.join("main.tf.json"))?;
    let observed = backend.observe(&context)?;
    assert!(!observed.exists);
    let plan = backend.prepare_change(&context, &observed, "first-op")?;
    assert!(plan.summary.creates >= 1);
    let after = backend.execute_change(&context, &plan)?;
    assert!(after.exists && !after.drifted);
    assert_eq!(after.readiness, Readiness::Unsupported);
    assert_eq!(after.outputs["result"].value, Some(json!("local")));
    assert!(after.outputs["secret"].value.is_none());
    assert!(after.outputs["secret"].sensitive);
    let inventory = after.inventory.clone();
    context.inventory = inventory.clone();
    let again = backend.observe(&context)?;
    assert_eq!(again.inventory, inventory);
    assert!(!again.drifted);
    context.artifact["resource"]["terraform_data"]["fixture"]["input"] = json!("updated");
    context.artifact_sha256 = "new-hash".into();
    context.revision = "next".into();
    assert!(backend.execute_change(&context, &plan).is_err());
    let before = backend.observe(&context)?;
    let next = backend.prepare_change(&context, &before, "second-op")?;
    let after = backend.execute_change(&context, &next)?;
    assert_eq!(after.outputs["result"].value, Some(json!("updated")));
    context.inventory = after.inventory;
    let observed = backend.observe(&context)?;
    let destroy = backend.prepare_removal(&context, &observed, "reviewed-destroy")?;
    assert!(destroy.summary.deletes >= 1);
    assert_eq!(destroy.payload["destroy"], true);
    let saved = context.work.join("operation-reviewed-destroy.tfplan");
    let bytes = std::fs::read(&saved)?;
    assert!(
        backend.observe(&context)?.exists,
        "Planning must not destroy resources"
    );
    let removed = backend.execute_removal(&context, &destroy)?;
    assert!(!removed.exists);
    assert_eq!(
        std::fs::read(saved)?,
        bytes,
        "Execute must reuse the reviewed native plan"
    );
    Ok(())
}

#[test]
fn rejects_unlocked_remote_backends() -> Result<()> {
    use nickel_plane::targets::BackendConfig;
    let binary = std::env::var_os("TOFU_BIN")
        .map(PathBuf::from)
        .ok_or_else(|| Error::Invalid("TOFU_BIN required for native qualification".into()))?;
    for (kind, configuration) in [
        ("http", json!({"address":"https://state.example/stack"})),
        ("s3", json!({"bucket":"state","key":"stack"})),
    ] {
        assert!(OpenTofu::new(OpenTofuTarget {
            private_diagnostics: false,
            binary: binary.clone(),
            stack: "fixture".into(),
            credentials: None,
            backend_config: Some(BackendConfig {
                kind: kind.into(),
                configuration
            }),
            timeout_seconds: 30
        })
        .is_err());
    }
    Ok(())
}

#[test]
fn explicit_native_lock_migration_preserves_provider_resource_identity() -> Result<()> {
    let temp = tempfile::tempdir()?;
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700))?;
    let binary = std::env::var_os("TOFU_LOCAL_BIN").ok_or_else(|| {
        Error::Invalid("Set TOFU_LOCAL_BIN to pinned OpenTofu with Random 3.9.1".into())
    })?;
    let mut backend = OpenTofu::new(OpenTofuTarget {
        private_diagnostics: false,
        binary: binary.into(),
        stack: "random".into(),
        credentials: None,
        backend_config: None,
        timeout_seconds: 60,
    })?;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let lock = std::fs::read(root.join("../nickel-tf/examples/random/.terraform.lock.hcl"))?;
    let unit: Unit = serde_json::from_value(
        json!({"backend":"opentofu","target":"local","schema":"random","configuration":{},"outputs":{"value":{"pointer":"/outputs/value"}}}),
    )?;
    let mut c = Context {
        owner: "store".into(),
        instance: "random".into(),
        unit_id: "fixture".into(),
        revision: "first".into(),
        unit,
        artifact: json!({"terraform":{"required_version":"= 1.13.2","required_providers":{"random":{"source":"registry.opentofu.org/hashicorp/random","version":"= 3.9.1"}}},"resource":{"random_id":{"fixture":{"byte_length":4}}},"output":{"value":{"value":"${random_id.fixture.hex}"}}}),
        artifact_sha256: "digest".into(),
        inventory: Value::Null,
        interrupted: None,
        work: temp.path().into(),
        engine_version: Some("1.13.2".into()),
        provider_lock: Some(lock.clone()),
    };
    let o = backend.observe(&c)?;
    let plan = backend.prepare_change(&c, &o, "first")?;
    let applied = backend.execute_change(&c, &plan)?;
    let identity = applied.outputs["value"].value.clone();
    c.inventory = applied.inventory;
    let mut revised = lock;
    revised.extend_from_slice(b"\n# Explicitly reviewed lock revision\n");
    c.provider_lock = Some(revised.clone());
    c.revision = "next".into();
    assert!(
        backend.observe(&c).is_err(),
        "Reconciliation must not silently change the native lock"
    );
    // The documented stopped-controller migration changes only the native lock.
    std::fs::write(c.work.join(".terraform.lock.hcl"), &revised)?;
    let observed = backend.observe(&c)?;
    assert!(!observed.drifted);
    assert_eq!(observed.outputs["value"].value, identity);
    assert!(
        backend.execute_change(&c, &plan).is_err(),
        "Old plans are invalid after lock migration"
    );
    let observed = backend.observe(&c)?;
    assert!(!backend.remove(&c, &observed)?.exists);
    Ok(())
}

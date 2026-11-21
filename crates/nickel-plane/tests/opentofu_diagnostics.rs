#![cfg(feature = "opentofu")]
#![allow(clippy::unwrap_used)]
use nickel_plane::{
    adapters::opentofu::OpenTofu,
    backend::{Backend, Context},
    model::Unit,
    targets::OpenTofuTarget,
    Result,
};
use serde_json::{json, Value};
use std::{os::unix::fs::PermissionsExt, path::Path};
const FAKE: &str = r##"#!/bin/sh
printf '%s\n' "$1" >> calls
case "$1" in
version) printf '{"terraform_version":"1.13.2"}'; exit 0 ;;
show) if [ "$MODE" = show ]; then printf 'sensitive-state-invalid-json';printf 'provider-private-marker' >&2;exit 0;fi
      printf '{"values":{"root_module":{"resources":[]}}}';exit 0 ;;
esac
printf '{"type":"version","ui":"1.2","tofu":"1.13.2"}\n'
if [ "$MODE" = timeout ] && [ "$1" = init ]; then printf 'provider-private-marker' >&2; sleep 10; exit 0;fi
if [ "$MODE" = warnings ] && [ "$1" = init ]; then printf '{"type":"diagnostic","diagnostic":{"severity":"warning","summary":"provider-private-marker"}}\n';exit 0;fi
if [ "$1" = "$MODE" ]; then
  printf '{"type":"diagnostic","diagnostic":{"severity":"error","summary":"Invalid value for input variable","detail":"provider-private-marker"}}\n'
  printf '%s provider-private-marker' "$PROVIDER_TOKEN" >&2
  exit 7
fi
if [ "$1" = plan ];then
  for arg in "$@";do case "$arg" in -out=*) printf fake-plan > "${arg#-out=}";;esac;done
fi
exit 0
"##;
fn context(work: &Path) -> Result<Context> {
    let unit: Unit = serde_json::from_value(
        json!({"backend":"opentofu","target":"local","schema":"local","configuration":{}}),
    )?;
    Ok(Context {
        owner: "owner".into(),
        instance: "fixture".into(),
        unit_id: "state".into(),
        revision: "one".into(),
        unit,
        artifact: json!({"terraform":{"required_version":"= 1.13.2"}}),
        artifact_sha256: "hash".into(),
        inventory: Value::Null,
        interrupted: None,
        recovery: None,
        work: work.into(),
        engine_version: Some("1.13.2".into()),
        provider_lock: None,
    })
}
fn backend(root: &Path, mode: &str, enabled: bool) -> Result<OpenTofu> {
    let binary = root.join("fake-tofu");
    std::fs::write(&binary, FAKE)?;
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))?;
    let credentials = root.join("credentials.json");
    std::fs::write(
        &credentials,
        serde_json::to_vec(&json!({"MODE":mode,"PROVIDER_TOKEN":"credential-private-marker"}))?,
    )?;
    std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600))?;
    OpenTofu::new(OpenTofuTarget {
        binary,
        stack: "fixture".into(),
        credentials: Some(credentials),
        backend_config: None,
        timeout_seconds: 2,
        private_diagnostics: enabled,
    })
}
#[test]
fn opt_in_keeps_private_errors_without_extra_execution_or_public_leaks() -> Result<()> {
    for mode in ["init", "plan", "show"] {
        for enabled in [false, true] {
            let dir = tempfile::tempdir()?;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
            let mut backend = backend(dir.path(), mode, enabled)?;
            let c = context(dir.path())?;
            let error = backend.observe(&c).unwrap_err();
            let text = error.to_string();
            assert!(
                text.contains(if mode == "show" {
                    "invalid JSON"
                } else {
                    "exit 7"
                }),
                "{text}"
            );
            if mode != "show" {
                assert!(text.contains("invalid-input"), "{text}");
            }
            let state = nickel_plane::reconcile::UnitState {
                failure: Some(text.clone()),
                ..Default::default()
            };
            for public in [
                text,
                format!("{error:?}"),
                serde_json::to_string(&state)?,
                state.public().to_string(),
            ] {
                assert!(!public.contains("private-marker"));
                assert!(!public.contains(dir.path().to_str().unwrap()));
                assert!(!public.contains("sensitive-state"));
            }
            let calls = std::fs::read_to_string(dir.path().join("calls"))?;
            assert_eq!(
                calls.lines().count(),
                match mode {
                    "init" => 2,
                    "show" => 3,
                    _ => 4,
                }
            );
            let logs = dir.path().join("opentofu-diagnostics");
            if enabled {
                let entries: Vec<_> = std::fs::read_dir(logs)?.collect();
                assert_eq!(entries.len(), 1);
                let raw = std::fs::read(entries[0].as_ref().unwrap().path())?;
                let raw = String::from_utf8_lossy(&raw);
                assert!(raw.contains("provider-private-marker"));
                assert!(!raw.contains("sensitive-state-invalid-json"));
            } else {
                assert!(!logs.exists());
            }
        }
    }
    Ok(())
}
#[test]
fn quota_refusal_preserves_the_primary_failure_and_existing_artifacts() -> Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
    let mut backend = backend(dir.path(), "init", true)?;
    let c = context(dir.path())?;
    for _ in 0..16 {
        assert!(backend.observe(&c).is_err());
    }
    let message = backend.observe(&c).unwrap_err().to_string();
    assert!(message.contains("exit 7"));
    assert!(message.contains("quota full"));
    assert_eq!(
        std::fs::read_dir(dir.path().join("opentofu-diagnostics"))?.count(),
        16
    );
    Ok(())
}
#[test]
fn pinned_native_invalid_input_produces_safe_category_and_private_json() -> Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
    let binary = std::env::var_os("TOFU_BIN").unwrap();
    let credentials = dir.path().join("credentials.json");
    std::fs::write(
        &credentials,
        serde_json::to_vec(&json!({"TF_VAR_invalid":"\"synthetic-private-number\""}))?,
    )?;
    std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o600))?;
    let mut backend = OpenTofu::new(OpenTofuTarget {
        binary: binary.into(),
        stack: "fixture".into(),
        credentials: Some(credentials),
        backend_config: None,
        timeout_seconds: 30,
        private_diagnostics: true,
    })?;
    let mut c = context(dir.path())?;
    c.artifact["variable"] = json!({"invalid":{"type":"number"}});
    let error = backend.observe(&c).unwrap_err();
    assert!(error.to_string().contains("invalid-input"), "{error}");
    assert!(!error.to_string().contains("synthetic-private-number"));
    assert_eq!(
        std::fs::read_dir(dir.path().join("opentofu-diagnostics"))?.count(),
        1
    );
    Ok(())
}

#[test]
fn reconcile_persists_only_safe_failure_and_keeps_uncertain_apply_journal() -> Result<()> {
    use nickel_plane::{
        composition::{Compiled, CompiledUnit, Validator},
        model::Instance,
        reconcile::{reconcile, Options, Registry},
        store::{status, Store},
    };
    use std::collections::BTreeMap;
    struct Validate;
    impl Validator for Validate {
        fn validate(&self, _: &Unit, _: &Value, base: &CompiledUnit) -> Result<CompiledUnit> {
            Ok(base.clone())
        }
    }
    for mode in ["init", "apply"] {
        let dir = tempfile::tempdir()?;
        let backend = backend(dir.path(), mode, true)?;
        let instance: Instance = serde_json::from_value(
            json!({"apiVersion":"nickel-plane/v1","id":"fixture","composition":{"name":"test","version":"1"},"policy":{"mode":"manage"},"units":{"state":{"backend":"opentofu","target":"local","schema":"local","configuration":{}}}}),
        )?;
        let mut compiled = Compiled {
            version: 1,
            revision: String::new(),
            instance,
            inputs: Value::Null,
            units: BTreeMap::from([(
                "state".into(),
                CompiledUnit {
                    artifact: context(dir.path())?.artifact,
                    receipt: Value::Null,
                    engine_version: Some("1.13.2".into()),
                    provider_lock: None,
                },
            )]),
        };
        compiled.revision = compiled.digest()?;
        let store_path = dir.path().join("store");
        let mut store = Store::open(&store_path)?;
        store.submit(&compiled)?;
        let mut registry =
            Registry::from([("local".into(), Box::new(backend) as Box<dyn Backend>)]);
        reconcile(
            &mut store,
            &compiled,
            &Validate,
            &mut registry,
            &Options {
                execute: true,
                now: 1,
                ..Default::default()
            },
        )?;
        let state = store.unit("fixture", "state")?.unwrap();
        assert!(state.failure.as_ref().unwrap().contains("exit 7"));
        assert!(!status(&store_path, "fixture")?
            .to_string()
            .contains("private-marker"));
        let raw = std::fs::read(store_path.join("store.sqlite"))?;
        assert!(!raw
            .windows(b"private-marker".len())
            .any(|w| w == b"private-marker"));
        if mode == "apply" {
            assert!(store.running("fixture", "state")?.is_some());
        }
    }
    Ok(())
}

#[test]
fn timeout_spawn_and_warning_only_commands_keep_safe_failure_semantics() -> Result<()> {
    for mode in ["timeout", "spawn", "warnings"] {
        let dir = tempfile::tempdir()?;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        let mut backend = backend(dir.path(), mode, true)?;
        let c = context(dir.path())?;
        if mode == "spawn" {
            std::fs::set_permissions(
                dir.path().join("fake-tofu"),
                std::fs::Permissions::from_mode(0o600),
            )?;
        }
        let result = backend.observe(&c);
        if mode == "warnings" {
            assert!(result.is_ok());
            assert!(!dir.path().join("opentofu-diagnostics").exists());
        } else {
            let error = result.unwrap_err().to_string();
            assert!(
                error.contains(if mode == "timeout" {
                    "timeout after 2s"
                } else {
                    "could not start process"
                }),
                "{error}"
            );
            assert!(!error.contains("private-marker"));
            assert!(!error.contains(dir.path().to_str().unwrap()));
            assert_eq!(
                std::fs::read_dir(dir.path().join("opentofu-diagnostics"))?.count(),
                1
            );
        }
    }
    Ok(())
}

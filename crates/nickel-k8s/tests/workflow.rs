use nickel_k8s::{lock, render, Request};
use nickel_util::files;
use serde_json::json;
use std::{error::Error, fs, path::PathBuf};

fn resources() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
fn config(text: &str) -> Result<(tempfile::TempDir, Request), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("app.ncl");
    fs::write(&path, text)?;
    Ok((
        temp,
        Request {
            config: path,
            resources: resources(),
        },
    ))
}
const GENERIC: &str = r#"
let profiles = import "/libraries/nickel-k8s/profiles.ncl" in
{ target = profiles.kubernetes {}, resources = [
 {apiVersion="v1",kind="ConfigMap",metadata={name="settings",namespace="demo"},data={mode="safe"}}
] }
"#;

#[test]
fn renders_standard_resources_and_locks_actual_inputs() -> Result<(), Box<dyn Error>> {
    let (temp, request) = config(GENERIC)?;
    let expected = json!({"apiVersion":"v1","kind":"List","items":[{
        "apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"settings","namespace":"demo"},"data":{"mode":"safe"}
    }]});
    assert_eq!(render(&request, None)?, expected);
    let locked = lock(&request)?;
    let path = temp.path().join("app.lock.json");
    files::publish_json(&path, &locked, false)?;
    assert_eq!(render(&request, Some(&path))?, expected);
    fs::write(&request.config, GENERIC.replace("safe", "changed"))?;
    assert!(render(&request, Some(&path)).is_err());
    Ok(())
}

#[test]
fn upstream_contract_rejects_wrong_service_port_type() -> Result<(), Box<dyn Error>> {
    let text = r#"let p = import "/libraries/nickel-k8s/profiles.ncl" in
    {target=p.kubernetes {},resources=[{apiVersion="v1",kind="Service",metadata={name="api",namespace="demo"},spec={ports=[{port="eighty"}]}}]}"#;
    let (_temp, request) = config(text)?;
    let error = render(&request, None)
        .err()
        .ok_or("invalid port accepted")?;
    assert!(
        matches!(error, nickel_util::error::Error::Nickel { .. }),
        "{error}"
    );
    Ok(())
}

#[test]
fn rejects_duplicates_missing_namespaces_unknown_kinds_and_inline_secrets(
) -> Result<(), Box<dyn Error>> {
    for resources in [
        r#"[{apiVersion="v1",kind="ConfigMap",metadata={name="x",namespace="demo"}}, {apiVersion="v1",kind="ConfigMap",metadata={name="x",namespace="demo"}}]"#,
        r#"[{apiVersion="v1",kind="ConfigMap",metadata={name="x"}}]"#,
        r#"[{apiVersion="v1",kind="ConfigMap",metadata={name="Invalid_Name",namespace="demo"}}]"#,
        r#"[{apiVersion="example.test/v1",kind="Unknown",metadata={name="x",namespace="demo"}}]"#,
        r#"[{apiVersion="v1",kind="Secret",metadata={name="x",namespace="demo"},stringData={password="secret"}}]"#,
    ] {
        let text = format!(
            r#"let p=import "/libraries/nickel-k8s/profiles.ncl" in {{target=p.kubernetes {{}},resources={resources}}}"#
        );
        let (_temp, request) = config(&text)?;
        assert!(render(&request, None).is_err(), "accepted: {resources}");
    }
    Ok(())
}

#[test]
fn refuses_unsupported_versions_and_misspelled_profile_fields() -> Result<(), Box<dyn Error>> {
    for target in [
        r#"{distribution="kubernetes",kubernetesVersion="1.99.0",schemaVersion="1.34.0",ingressClass=null,storageClass=null,networkPolicy=false}"#,
        r#"{distribution="k3s",kubernetesVersion="1.34.0",schemaVersion="1.34.0",k3sVersion="v1.33.0+k3s1",ingressClass=null,storageClass=null,networkPolicy=false}"#,
        r#"{distribution="kubernetes",kubernetesVersion="1.34.0",schemaVersion="1.34.0",ingresClass="typo",storageClass=null,networkPolicy=false}"#,
    ] {
        let text = format!("{{target={target},resources=[]}}");
        let (_temp, request) = config(&text)?;
        assert!(render(&request, None).is_err());
    }
    Ok(())
}

#[test]
fn composes_http_application_for_k3s_with_explicit_capabilities() -> Result<(), Box<dyn Error>> {
    let request = Request {
        config: resources().join("examples/http-k3s.ncl"),
        resources: resources(),
    };
    let manifest = render(&request, None)?;
    let items = manifest["items"].as_array().ok_or("items missing")?;
    for kind in [
        "Deployment",
        "Service",
        "ConfigMap",
        "Ingress",
        "PersistentVolumeClaim",
        "NetworkPolicy",
    ] {
        assert!(items.iter().any(|r| r["kind"] == kind), "missing {kind}");
    }
    let deploy = items
        .iter()
        .find(|r| r["kind"] == "Deployment")
        .ok_or("deployment missing")?;
    assert_eq!(
        deploy["spec"]["template"]["spec"]["containers"][0]["envFrom"][1]["secretRef"]["name"],
        "web-secrets"
    );
    let ingress = items
        .iter()
        .find(|r| r["kind"] == "Ingress")
        .ok_or("ingress missing")?;
    assert_eq!(ingress["spec"]["ingressClassName"], "traefik");
    let pvc = items
        .iter()
        .find(|r| r["kind"] == "PersistentVolumeClaim")
        .ok_or("PVC missing")?;
    assert_eq!(pvc["spec"]["storageClassName"], "local-path");
    Ok(())
}

#[test]
fn locks_cover_imported_modules_and_transitive_library_files() -> Result<(), Box<dyn Error>> {
    let text = GENERIC.replace("mode=\"safe\"", "mode=import \"mode.ncl\"");
    let (temp, mut request) = config(&text)?;
    fs::write(temp.path().join("mode.ncl"), "\"safe\"")?;
    let path = temp.path().join("app.lock.json");
    files::publish_json(&path, &lock(&request)?, false)?;
    fs::write(temp.path().join("mode.ncl"), "\"changed\"")?;
    assert!(render(&request, Some(&path)).is_err());
    fs::write(temp.path().join("mode.ncl"), "\"safe\"")?;
    let library = tempfile::tempdir()?;
    nickel_util::sources::snapshot(
        &resources(),
        &nickel_util::sources::Sources {
            include: vec!["flake.nix".into(), "flake.lock".into(), "nickel".into()],
            exclude: vec![],
        },
        Some(library.path()),
    )?;
    request.resources = library.path().to_path_buf();
    assert!(render(&request, Some(&path)).is_ok());
    let helper = library.path().join("nickel/helpers.ncl");
    fs::write(
        &helper,
        format!(
            "{}\n# Changed library revision\n",
            fs::read_to_string(&helper)?
        ),
    )?;
    assert!(render(&request, Some(&path)).is_err());
    Ok(())
}

#[test]
fn resource_order_is_deterministic_and_profiles_share_composition() -> Result<(), Box<dyn Error>> {
    let original = fs::read_to_string(resources().join("examples/http-k3s.ncl"))?;
    let (_first, request1) = config(&original)?;
    let (_second, request2) = config(&original.replace("profiles.k3s", "profiles.kubernetes"))?;
    assert_eq!(render(&request1, None)?, render(&request2, None)?);
    let prefix = r#"let p=import "/libraries/nickel-k8s/profiles.ncl" in {target=p.kubernetes {},resources="#;
    let one = r#"{apiVersion="v1",kind="ConfigMap",metadata={name="a",namespace="demo"}}"#;
    let two = one.replace("name=\"a\"", "name=\"b\"");
    let (_a, a) = config(&format!("{prefix}[{one},{two}]}}"))?;
    let (_b, b) = config(&format!("{prefix}[{two},{one}]}}"))?;
    assert_eq!(render(&a, None)?, render(&b, None)?);
    Ok(())
}

#[test]
fn profile_capabilities_are_checked_for_raw_resources() -> Result<(), Box<dyn Error>> {
    let text = r#"let p=import "/libraries/nickel-k8s/profiles.ncl" in
    {target=p.k3s {},resources=[{apiVersion="networking.k8s.io/v1",kind="NetworkPolicy",metadata={name="deny",namespace="demo"},spec={podSelector={},policyTypes=["Ingress","Egress"]}}]}"#;
    let (_temp, request) = config(text)?;
    let error = render(&request, None)
        .err()
        .ok_or("missing capability accepted")?;
    assert!(
        error.to_string().contains("networkPolicy capability"),
        "{error}"
    );
    Ok(())
}

#[test]
fn rejects_service_names_that_begin_with_a_digit() -> Result<(), Box<dyn Error>> {
    let text = r#"let p=import "/libraries/nickel-k8s/profiles.ncl" in
    {target=p.kubernetes {},resources=[{apiVersion="v1",kind="Service",metadata={name="1api",namespace="demo"},spec={ports=[{port=80}]}}]}"#;
    let (_temp, request) = config(text)?;
    assert!(
        render(&request, None).is_err(),
        "Service names must start with a letter on the pinned default API"
    );
    Ok(())
}

#[test]
fn accepts_kubernetes_subdomain_names_longer_than_one_dns_label() -> Result<(), Box<dyn Error>> {
    let name = "a".repeat(64);
    let text = GENERIC.replace("settings", &name);
    let (_temp, request) = config(&text)?;
    let manifest = render(&request, None)?;
    assert_eq!(manifest["items"][0]["metadata"]["name"], name);
    Ok(())
}

#[test]
fn claim_templates_obey_storage_capabilities() -> Result<(), Box<dyn Error>> {
    let stateful = r#"{
        apiVersion="apps/v1",kind="StatefulSet",metadata={name="db",namespace="demo"},
        spec={serviceName="db",selector.matchLabels.app="db",
            template={metadata.labels.app="db",spec.containers=[{name="db",image="registry.example.com/db:1"}]},
            volumeClaimTemplates=[{metadata.name="data",spec={accessModes=["ReadWriteOnce"],storageClassName="local-path",resources.requests.storage="1Gi"}}]}
    }"#;
    let ephemeral = r#"{
        apiVersion="apps/v1",kind="Deployment",metadata={name="api",namespace="demo"},
        spec={selector.matchLabels.app="api",template={metadata.labels.app="api",spec={
            containers=[{name="api",image="registry.example.com/api:1"}],
            volumes=[{name="scratch",ephemeral.volumeClaimTemplate={spec={accessModes=["ReadWriteOnce"],storageClassName="local-path",resources.requests.storage="1Gi"}}}]
        }}}
    }"#;
    for resource in [stateful, ephemeral] {
        let text = format!(
            r#"let p=import "/libraries/nickel-k8s/profiles.ncl" in {{target=p.k3s {{}},resources=[{resource}]}}"#
        );
        let (_temp, request) = config(&text)?;
        assert!(
            render(&request, None).is_err(),
            "claim creation requires a declared storage class"
        );
        let declared = text.replace("p.k3s {}", "p.k3s {storageClass=\"local-path\"}");
        fs::write(&request.config, &declared)?;
        assert!(
            render(&request, None).is_ok(),
            "matching declared claim class must work"
        );
        fs::write(
            &request.config,
            declared.replace(
                "storageClassName=\"local-path\"",
                "storageClassName=\"undeclared\"",
            ),
        )?;
        assert!(render(&request, None).is_err());
        fs::write(
            &request.config,
            declared.replace("storageClassName=\"local-path\",", ""),
        )?;
        assert!(
            render(&request, None).is_err(),
            "implicit default storage classes must be explicit"
        );
    }
    Ok(())
}

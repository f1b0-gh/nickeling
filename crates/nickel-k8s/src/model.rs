use nickel_util::error::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub(crate) fn require(condition: bool, message: impl Into<String>) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(Error::InvalidInput {
            message: message.into(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Target {
    pub distribution: String,
    pub kubernetes_version: String,
    pub schema_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub k3s_version: Option<String>,
    pub ingress_class: Option<String>,
    pub storage_class: Option<String>,
    pub network_policy: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Bundle {
    pub target: Target,
    pub resources: Vec<Value>,
}

fn dns_name(value: &str, max: usize, subdomain: bool) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.split('.').all(|label| {
            !label.is_empty()
                && label
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && label
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        && (subdomain || !value.contains('.'))
}

fn string<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::InvalidInput {
            message: format!("Required string field: {field}"),
        })
}

fn storage_class(spec: &Value, target: &Target) -> Result<()> {
    let class = target
        .storage_class
        .as_deref()
        .ok_or_else(|| Error::InvalidInput {
            message: "Declare an available storageClass in target".into(),
        })?;
    require(
        spec["storageClassName"].as_str() == Some(class),
        "Storage class must match the declared target capability",
    )
}

fn workload_claims(resource: &Value, target: &Target) -> Result<()> {
    if let Some(claims) = resource["spec"]["volumeClaimTemplates"].as_array() {
        for claim in claims {
            storage_class(&claim["spec"], target)?;
        }
    }
    if let Some(volumes) = resource["spec"]["template"]["spec"]["volumes"].as_array() {
        for volume in volumes {
            let claim = &volume["ephemeral"]["volumeClaimTemplate"];
            if !claim.is_null() {
                storage_class(&claim["spec"], target)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn normalize(mut bundle: Bundle) -> Result<(Target, Value)> {
    let target = &bundle.target;
    require(
        target.schema_version == "1.34.0",
        "Supported Kubernetes schema version: 1.34.0",
    )?;
    require(
        match target.distribution.as_str() {
            "kubernetes" => target.k3s_version.is_none() && target.kubernetes_version == "1.34.0",
            "k3s" => {
                target.k3s_version.as_deref() == Some("v1.34.1+k3s1")
                    && target.kubernetes_version == "1.34.1"
            }
            _ => false,
        },
        "Unsupported distribution/version combination",
    )?;
    for class in [&target.ingress_class, &target.storage_class]
        .into_iter()
        .flatten()
    {
        require(
            dns_name(class, 253, true),
            "Invalid ingress/storage class name",
        )?;
    }
    require(
        bundle.resources.len() <= 256,
        "Maximum 256 resources per bundle",
    )?;
    let mut identities = BTreeSet::new();
    for resource in &bundle.resources {
        let api = string(resource, "apiVersion")?;
        let kind = string(resource, "kind")?;
        let expected = match kind {
            "Deployment" | "StatefulSet" => "apps/v1",
            "Service" | "ConfigMap" | "Namespace" | "PersistentVolumeClaim" | "ServiceAccount" => {
                "v1"
            }
            "NetworkPolicy" | "Ingress" => "networking.k8s.io/v1",
            _ => {
                return Err(Error::InvalidInput {
                    message: format!("Unsupported resource kind: {kind}"),
                })
            }
        };
        require(
            api == expected,
            format!("Unsupported apiVersion for {kind}"),
        )?;
        let meta = &resource["metadata"];
        let name = string(meta, "name")?;
        let label = matches!(kind, "Namespace" | "Service");
        require(
            dns_name(name, if label { 63 } else { 253 }, !label)
                && (kind != "Service"
                    || name.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)),
            format!("Invalid {kind} name: {name}"),
        )?;
        let namespace = if kind == "Namespace" {
            require(
                meta.get("namespace").is_none(),
                "Namespace resources cannot have metadata.namespace",
            )?;
            ""
        } else {
            let namespace = string(meta, "namespace")?;
            require(
                dns_name(namespace, 63, false),
                "Explicit valid namespace required",
            )?;
            namespace
        };
        require(
            identities.insert((api, kind, namespace, name)),
            format!("Duplicate resource: {kind} {namespace}/{name}"),
        )?;
        if matches!(
            kind,
            "Deployment"
                | "StatefulSet"
                | "Service"
                | "Ingress"
                | "NetworkPolicy"
                | "PersistentVolumeClaim"
        ) {
            require(
                resource["spec"].is_object(),
                format!("{kind} requires spec"),
            )?;
        }
        match kind {
            "Ingress" => {
                let class = target
                    .ingress_class
                    .as_deref()
                    .ok_or_else(|| Error::InvalidInput {
                        message: "Declare an available ingressClass in target".into(),
                    })?;
                require(
                    resource["spec"]["ingressClassName"].as_str() == Some(class),
                    "Ingress class must match the declared target capability",
                )?;
            }
            "PersistentVolumeClaim" => storage_class(&resource["spec"], target)?,
            "Deployment" | "StatefulSet" => workload_claims(resource, target)?,
            "NetworkPolicy" => require(
                target.network_policy,
                "Declare networkPolicy capability before generating policies",
            )?,
            _ => (),
        }
    }
    bundle.resources.sort_by_cached_key(|r| {
        (
            r["apiVersion"].as_str().unwrap_or_default().to_owned(),
            r["kind"].as_str().unwrap_or_default().to_owned(),
            r["metadata"]["namespace"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            r["metadata"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        )
    });
    Ok((
        bundle.target,
        json!({"apiVersion":"v1","kind":"List","items":bundle.resources}),
    ))
}

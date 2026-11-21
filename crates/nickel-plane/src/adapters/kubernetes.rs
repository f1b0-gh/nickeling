//! Namespaced Kubernetes execution with UID ownership and optimistic concurrency.
use crate::{backend::*, model::BackendKind, require, targets::KubernetesTarget, Error, Result};
use nickel_util::files;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, os::unix::fs::MetadataExt, time::Duration};
mod quantity;
pub trait Api {
    fn request(&mut self, method: &str, path: &str, body: Option<&Value>) -> Result<(u16, Value)>;
    fn validate_target(&self, _target: &KubernetesTarget) -> Result<()> {
        Ok(())
    }
    fn execution_fingerprint(&self, target: &KubernetesTarget) -> Result<String> {
        ConnectionSnapshot::read(target)?.fingerprint()
    }
    fn legacy_fingerprint(&self, target: &KubernetesTarget) -> Result<String> {
        ConnectionSnapshot::read(target)?.legacy_fingerprint()
    }
}
pub struct Kubernetes<A: Api> {
    pub api: A,
    pub target: KubernetesTarget,
}
pub struct Http {
    agent: ureq::Agent,
    snapshot: ConnectionSnapshot,
}
struct ConnectionSnapshot {
    target: KubernetesTarget,
    ca: Option<Vec<u8>>,
    token: Option<String>,
}
impl ConnectionSnapshot {
    fn read(target: &KubernetesTarget) -> Result<Self> {
        let ca = target
            .ca_file
            .as_ref()
            .map(|path| files::read_limited(path, 1024 * 1024))
            .transpose()?;
        let token = target
            .token_file
            .as_ref()
            .map(|path| {
                let opened = files::open_regular(path)?;
                require(
                    opened.metadata()?.mode() & 0o077 == 0,
                    "Credential file must be private",
                )?;
                let token = String::from_utf8(files::read_file(opened, 64 * 1024)?)
                    .map_err(|_| Error::Invalid("Invalid bearer token encoding".into()))?;
                require(
                    !token.trim().is_empty() && !token.trim().contains(['\r', '\n']),
                    "Invalid bearer token",
                )?;
                Ok::<_, Error>(token)
            })
            .transpose()?;
        Ok(Self {
            target: target.clone(),
            ca,
            token,
        })
    }
    fn validate(&self, target: &KubernetesTarget) -> Result<()> {
        require(
            self.target == *target,
            "Kubernetes target changed; reconnect before continuing",
        )?;
        let current = Self::read(target)?;
        require(
            self.ca == current.ca && self.token == current.token,
            "Kubernetes trust or credentials changed; reconnect before continuing",
        )
    }
    fn fingerprint(&self) -> Result<String> {
        let trust = if let Some(ca) = &self.ca {
            json!({"mode":"explicit","sha256":files::hash(ca)})
        } else {
            // These are the same roots ureq's rustls connector uses for WebPki.
            let anchors: Vec<_> = webpki_roots::TLS_SERVER_ROOTS
                .iter()
                .map(|root| {
                    (
                        root.subject.as_ref(),
                        root.subject_public_key_info.as_ref(),
                        root.name_constraints.as_ref().map(|value| value.as_ref()),
                    )
                })
                .collect();
            json!({"mode":"webpki","sha256":files::hash(&serde_json::to_vec(&anchors)?)})
        };
        Ok(files::hash(&serde_json::to_vec(&json!({
            "domain":"nickel-plane/kubernetes-execution/v1",
            "server":self.target.server,"namespace":self.target.namespace,
            "allowHttpLoopback":self.target.allow_http_loopback,
            "trust":trust,"token":self.token.as_ref().map(|token| files::hash(token.as_bytes()))
        }))?))
    }
    fn legacy_fingerprint(&self) -> Result<String> {
        Ok(files::hash(&serde_json::to_vec(&json!({
            "server":self.target.server,"namespace":self.target.namespace,
            "ca":self.ca.as_ref().map(|bytes| files::hash(bytes))
        }))?))
    }
}
impl Api for Http {
    fn validate_target(&self, target: &KubernetesTarget) -> Result<()> {
        self.snapshot.validate(target)
    }
    fn execution_fingerprint(&self, target: &KubernetesTarget) -> Result<String> {
        self.snapshot.validate(target)?;
        self.snapshot.fingerprint()
    }
    fn legacy_fingerprint(&self, target: &KubernetesTarget) -> Result<String> {
        self.snapshot.validate(target)?;
        self.snapshot.legacy_fingerprint()
    }
    fn request(&mut self, method: &str, path: &str, body: Option<&Value>) -> Result<(u16, Value)> {
        self.snapshot.validate(&self.snapshot.target)?;
        require(
            path.starts_with('/') && !path.starts_with("//"),
            "Invalid API path",
        )?;
        let mut request = ureq::http::Request::builder()
            .method(method)
            .uri(format!(
                "{}{path}",
                self.snapshot.target.server.trim_end_matches('/')
            ))
            .header("Accept", "application/json")
            .header(
                "Content-Type",
                if method == "PATCH" {
                    "application/apply-patch+yaml"
                } else {
                    "application/json"
                },
            );
        if let Some(token) = &self.snapshot.token {
            request = request.header("Authorization", format!("Bearer {}", token.trim()));
        }
        let bytes = body
            .map(serde_json::to_vec)
            .transpose()?
            .unwrap_or_default();
        let request = request
            .body(bytes)
            .map_err(|_| Error::Backend("Kubernetes request encoding".into()))?;
        let request = self
            .agent
            .configure_request(request)
            .timeout_global(Some(Duration::from_secs(if path.contains("watch=true") {
                3
            } else {
                30
            })))
            .build();
        let mut response = self.agent.run(request).map_err(|error| {
            Error::Backend(
                match error {
                    ureq::Error::Timeout(_) => "Kubernetes transport timeout",
                    ureq::Error::Io(_) => "Kubernetes transport I/O",
                    ureq::Error::Protocol(_) => "Kubernetes transport protocol",
                    ureq::Error::HostNotFound => "Kubernetes transport DNS",
                    ureq::Error::Tls(_) | ureq::Error::Rustls(_) => "Kubernetes transport TLS",
                    _ => "Kubernetes transport",
                }
                .into(),
            )
        })?;
        let status = response.status().as_u16();
        let bytes = response
            .body_mut()
            .with_config()
            .limit(4 * 1024 * 1024)
            .read_to_vec()
            .map_err(|_| Error::Backend("Kubernetes response limit/transport".into()))?;
        if !(200..300).contains(&status) {
            return Ok((status, Value::Null));
        }
        let value = if bytes.is_empty() {
            Value::Null
        } else if path.contains("watch=true") {
            let mut events = Vec::new();
            for line in bytes.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
                require(events.len() < 4096, "Kubernetes watch event limit")?;
                events.push(
                    nickel_util::json::parse(line)
                        .map_err(|_| Error::Backend("Kubernetes watch JSON".into()))?,
                );
            }
            Value::Array(events)
        } else {
            nickel_util::json::parse(&bytes)
                .map_err(|_| Error::Backend("Kubernetes response JSON".into()))?
        };
        Ok((status, value))
    }
}
impl Kubernetes<Http> {
    pub fn connect(target: KubernetesTarget) -> Result<Self> {
        let uri: ureq::http::Uri = target
            .server
            .parse()
            .map_err(|_| Error::Invalid("Invalid Kubernetes server URL".into()))?;
        let local = target.allow_http_loopback
            && uri.scheme_str() == Some("http")
            && matches!(uri.host(), Some("127.0.0.1" | "[::1]"));
        require((uri.scheme_str()==Some("https")||local)&&!target.server.contains(['@','?','#'])&&matches!(uri.path(),""|"/"),"Kubernetes requires an explicit HTTPS server; HTTP is restricted to opted-in loopback fixtures")?;
        validate_name(&target.namespace)?;
        let snapshot = ConnectionSnapshot::read(&target)?;
        let mut config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .proxy(None)
            .timeout_global(Some(Duration::from_secs(30)));
        if let Some(bytes) = &snapshot.ca {
            let cert = ureq::tls::Certificate::from_pem(bytes)
                .map_err(|_| Error::Invalid("Invalid Kubernetes CA certificate".into()))?;
            config = config.tls_config(
                ureq::tls::TlsConfig::builder()
                    .root_certs(ureq::tls::RootCerts::new_with_certs(&[cert]))
                    .build(),
            );
        }
        let api = Http {
            agent: config.build().new_agent(),
            snapshot,
        };
        Ok(Self { api, target })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Entry {
    path: String,
    uid: String,
    resource_version: String,
    kind: String,
    name: String,
    #[serde(default)]
    missing: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Action {
    method: String,
    path: String,
    body: Value,
}
fn validate_name(name: &str) -> Result<()> {
    require(
        !name.is_empty()
            && name.len() <= 253
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.'),
        "Invalid Kubernetes object name",
    )
}
fn text<'a>(value: &'a Value, pointer: &str) -> Result<&'a str> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Backend(format!("Kubernetes field absent: {pointer}")))
}
fn prefix(api: &str, kind: &str) -> Result<(&'static str, &'static str)> {
    match (api, kind) {
        ("v1", "ConfigMap") => Ok(("/api/v1", "configmaps")),
        ("v1", "Service") => Ok(("/api/v1", "services")),
        ("v1", "ServiceAccount") => Ok(("/api/v1", "serviceaccounts")),
        ("v1", "PersistentVolumeClaim") => Ok(("/api/v1", "persistentvolumeclaims")),
        ("apps/v1", "Deployment") => Ok(("/apis/apps/v1", "deployments")),
        ("apps/v1", "StatefulSet") => Ok(("/apis/apps/v1", "statefulsets")),
        ("networking.k8s.io/v1", "NetworkPolicy") => {
            Ok(("/apis/networking.k8s.io/v1", "networkpolicies"))
        }
        ("networking.k8s.io/v1", "Ingress") => Ok(("/apis/networking.k8s.io/v1", "ingresses")),
        _ => Err(Error::Unsupported(
            "Kubernetes execution supports the documented namespaced kinds only".into(),
        )),
    }
}
fn ownership(context: &Context) -> String {
    files::hash(format!("{}/{}/{}", context.owner, context.instance, context.unit_id).as_bytes())
        [..32]
        .into()
}
fn manager(context: &Context) -> String {
    format!("nickel-plane-{}", ownership(context))
}
fn inventory(value: &Value) -> Result<BTreeMap<String, Entry>> {
    if value.is_null() {
        Ok(BTreeMap::new())
    } else {
        Ok(serde_json::from_value(value.clone())?)
    }
}
fn desired(context: &Context, namespace: &str) -> Result<BTreeMap<String, Value>> {
    let items = context.artifact["items"]
        .as_array()
        .ok_or_else(|| Error::Invalid("Expected Kubernetes List".into()))?;
    require(items.len() <= 256, "Too many Kubernetes objects")?;
    let mut result = BTreeMap::new();
    for item in items {
        let name = text(item, "/metadata/name")?;
        validate_name(name)?;
        require(
            text(item, "/metadata/namespace")? == namespace,
            "Kubernetes object is outside target namespace",
        )?;
        let (base, plural) = prefix(text(item, "/apiVersion")?, text(item, "/kind")?)?;
        let path = format!("{base}/namespaces/{namespace}/{plural}/{name}");
        let mut item = item.clone();
        for key in [
            "uid",
            "resourceVersion",
            "managedFields",
            "creationTimestamp",
            "deletionTimestamp",
            "generation",
        ] {
            require(
                item["metadata"].get(key).is_none(),
                "Server-owned metadata cannot be configured",
            )?;
        }
        for group in ["labels", "annotations"] {
            if let Some(fields) = item["metadata"][group].as_object() {
                require(
                    !fields.keys().any(|key| key.starts_with("nickel-plane.io/")),
                    "Reserved controller metadata",
                )?;
            }
        }
        item["metadata"]["labels"]["nickel-plane.io/owner"] = json!(ownership(context));
        require(
            result.insert(path, item).is_none(),
            "Duplicate Kubernetes object identity",
        )?;
    }
    Ok(result)
}
fn subset(desired: &Value, actual: &Value) -> bool {
    fn compare(want: &Value, have: &Value, path: &str, kind: &str) -> bool {
        match want {
            Value::Object(fields) => have.as_object().is_some_and(|object| {
                fields.iter().all(|(key, value)| {
                    let next = format!("{path}/{key}");
                    if let Some(actual) = object.get(key) {
                        return compare(value, actual, &next, kind);
                    }
                    // Supported native APIs omit empty slices and these empty maps.
                    // Do not erase empty selector/affinity structs: omission can have
                    // different semantics, even though both serialize as records.
                    value.is_null()
                        || value.as_array().is_some_and(Vec::is_empty)
                        || (value.as_object().is_some_and(serde_json::Map::is_empty)
                            && (matches!(
                                key.as_str(),
                                "labels"
                                    | "annotations"
                                    | "matchLabels"
                                    | "nodeSelector"
                                    | "limits"
                                    | "requests"
                            ) || (kind == "ConfigMap"
                                && matches!(next.as_str(), "/data" | "/binaryData"))
                                || (kind == "Service" && next == "/spec/selector")))
                })
            }),
            Value::Array(items) => have.as_array().is_some_and(|actual| {
                items.len() == actual.len()
                    && items
                        .iter()
                        .zip(actual)
                        .enumerate()
                        .all(|(i, (a, b))| compare(a, b, &format!("{path}/{i}"), kind))
            }),
            _ => {
                if want == have {
                    return true;
                }
                let want = match want {
                    Value::String(value) => std::borrow::Cow::Borrowed(value.as_str()),
                    Value::Number(value) => std::borrow::Cow::Owned(value.to_string()),
                    _ => return false,
                };
                let have = match have {
                    Value::String(value) => std::borrow::Cow::Borrowed(value.as_str()),
                    Value::Number(value) => std::borrow::Cow::Owned(value.to_string()),
                    _ => return false,
                };
                quantity::equivalent(kind, path, &want, &have)
            }
        }
    }
    compare(
        desired,
        actual,
        "",
        desired["kind"].as_str().unwrap_or_default(),
    )
}

fn ready(object: &Value) -> Result<Readiness> {
    if !object["metadata"]["deletionTimestamp"].is_null() {
        return Ok(Readiness::NotReady);
    }
    let kind = object["kind"].as_str().unwrap_or_default();
    let yes = match kind {
        "ConfigMap" | "ServiceAccount" | "NetworkPolicy" => true,
        "PersistentVolumeClaim" => object["status"]["phase"] == "Bound",
        "Ingress" => object["status"]["loadBalancer"]["ingress"]
            .as_array()
            .is_some_and(|a| !a.is_empty()),
        "Service" => {
            if object["spec"]["type"] == "ExternalName" {
                object["spec"]["externalName"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty())
            } else if object["spec"]["type"] == "LoadBalancer" {
                object["status"]["loadBalancer"]["ingress"]
                    .as_array()
                    .is_some_and(|a| !a.is_empty())
            } else {
                object["spec"]["clusterIP"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty())
            }
        }
        "Deployment" => {
            if object["status"]["observedGeneration"].as_u64().unwrap_or(0)
                < object["metadata"]["generation"].as_u64().unwrap_or(1)
            {
                return Ok(Readiness::NotReady);
            }
            if object["status"]["conditions"]
                .as_array()
                .is_some_and(|conditions| {
                    conditions.iter().any(|condition| {
                        condition["type"] == "Progressing" && condition["status"] == "False"
                    })
                })
            {
                return Ok(Readiness::Failed);
            }
            let replicas = object["spec"]["replicas"].as_u64().unwrap_or(1);
            [
                "replicas",
                "readyReplicas",
                "updatedReplicas",
                "availableReplicas",
            ]
            .iter()
            .all(|field| object["status"][field].as_u64().unwrap_or(0) == replicas)
        }
        "StatefulSet" => {
            let replicas = object["spec"]["replicas"].as_u64().unwrap_or(1);
            object["status"]["observedGeneration"].as_u64().unwrap_or(0)
                >= object["metadata"]["generation"].as_u64().unwrap_or(1)
                && object["status"]["readyReplicas"].as_u64().unwrap_or(0) >= replicas
                && object["status"]["updatedReplicas"].as_u64().unwrap_or(0) >= replicas
        }
        _ => return Ok(Readiness::Unsupported),
    };
    Ok(if yes {
        Readiness::Ready
    } else {
        Readiness::NotReady
    })
}
impl<A: Api> Kubernetes<A> {
    fn get(&mut self, path: &str) -> Result<Option<Value>> {
        self.api.validate_target(&self.target)?;
        let (status, value) = self.api.request("GET", path, None)?;
        match status {
            200 => Ok(Some(value)),
            404 => Ok(None),
            409 => Err(Error::Conflict("Kubernetes read conflict".into())),
            _ => Err(Error::Backend(format!("Kubernetes GET HTTP {status}"))),
        }
    }
    fn mutate(&mut self, method: &str, path: &str, body: &Value) -> Result<Value> {
        self.api.validate_target(&self.target)?;
        let (status, value) = self.api.request(method, path, Some(body))?;
        match status {
            200..=299 => Ok(value),
            409 => Err(Error::Conflict(
                "Kubernetes ownership/admission/precondition conflict".into(),
            )),
            _ => Err(Error::Backend(format!("Kubernetes {method} HTTP {status}"))),
        }
    }
    fn owned(
        &self,
        context: &Context,
        path: &str,
        object: &Value,
        previous: &BTreeMap<String, Entry>,
    ) -> Result<()> {
        let intended_create = context.interrupted.as_ref().is_some_and(|operation| {
            object["metadata"]["annotations"]["nickel-plane.io/operation"] == operation.operation
                && operation.payload["creates"]
                    .as_array()
                    .is_some_and(|keys| keys.iter().any(|k| k.as_str() == Some(path)))
        });
        let matches = previous.get(path).map_or(intended_create, |old| {
            object["metadata"]["uid"] == old.uid || (old.missing && intended_create)
        });
        if !matches || object["metadata"]["labels"]["nickel-plane.io/owner"] != ownership(context) {
            return Err(Error::Conflict(
                "Kubernetes object is unowned or its UID changed".into(),
            ));
        }
        Ok(())
    }
}
impl<A: Api> Backend for Kubernetes<A> {
    fn kind(&self) -> BackendKind {
        BackendKind::Kubernetes
    }
    fn fingerprint(&self) -> Result<String> {
        self.api.validate_target(&self.target)?;
        self.api.legacy_fingerprint(&self.target)
    }
    fn target_identity(&self) -> Result<String> {
        self.api.validate_target(&self.target)?;
        Ok(files::hash(&serde_json::to_vec(&json!({
            "domain":"nickel-plane/kubernetes-target/v1",
            "server":self.target.server,"namespace":self.target.namespace
        }))?))
    }
    fn execution_fingerprint(&self) -> Result<String> {
        self.api.execution_fingerprint(&self.target)
    }
    fn legacy_fingerprint(&self) -> Result<String> {
        self.api.validate_target(&self.target)?;
        let Some(path) = &self.target.previous_ca_file else {
            return self.fingerprint();
        };
        let ca = files::hash(&files::read_limited(path, 1024 * 1024)?);
        Ok(files::hash(&serde_json::to_vec(&json!({
            "server":self.target.server,"namespace":self.target.namespace,"ca":ca
        }))?))
    }
    fn poll_events(&mut self, value: &Value) -> Result<bool> {
        self.api.validate_target(&self.target)?;
        // Watch one known object as a scheduling hint, bounded by server timeout.
        // Full periodic observation covers every object and missed/expired events.
        let entries = inventory(value)?;
        let Some(entry) = entries.values().find(|e| !e.missing) else {
            return Ok(false);
        };
        require(
            entry
                .resource_version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "Invalid resourceVersion",
        )?;
        validate_name(&entry.name)?;
        let collection = entry
            .path
            .rsplit_once('/')
            .ok_or_else(|| Error::Invalid("Invalid inventory path".into()))?
            .0;
        let path=format!("{collection}?watch=true&timeoutSeconds=1&allowWatchBookmarks=true&fieldSelector=metadata.name%3D{}&resourceVersion={}",entry.name,entry.resource_version);
        let (status, events) = self.api.request("GET", &path, None)?;
        if status == 410 {
            return Ok(true);
        }
        require(status == 200, "Kubernetes watch rejected")?;
        Ok(events
            .as_array()
            .is_some_and(|a| a.iter().any(|e| e["type"] != "BOOKMARK")))
    }
    fn observe(&mut self, context: &Context) -> Result<Observation> {
        self.api.validate_target(&self.target)?;
        let wanted = desired(context, &self.target.namespace)?;
        let previous = inventory(&context.inventory)?;
        let mut actual = BTreeMap::new();
        let mut preconditions = BTreeMap::<String, Value>::new();
        let mut outputs = json!({"resources":{}});
        let mut exists = true;
        let mut drifted = false;
        let mut readiness = Readiness::Ready;
        let mut keys: std::collections::BTreeSet<_> =
            wanted.keys().chain(previous.keys()).cloned().collect();
        if let Some(operation) = &context.interrupted {
            if let Some(creates) = operation.payload["creates"].as_array() {
                for path in creates {
                    keys.insert(
                        path.as_str()
                            .ok_or_else(|| {
                                Error::Invalid("Invalid interrupted creation path".into())
                            })?
                            .to_string(),
                    );
                }
            }
        }
        for path in keys {
            if let Some(object) = self.get(&path)? {
                self.owned(context, &path, &object, &previous)?;
                let entry = Entry {
                    path: path.clone(),
                    uid: text(&object, "/metadata/uid")?.into(),
                    resource_version: text(&object, "/metadata/resourceVersion")?.into(),
                    kind: text(&object, "/kind")?.into(),
                    name: text(&object, "/metadata/name")?.into(),
                    missing: false,
                };
                preconditions.insert(
                    path.clone(),
                    json!({"uid":entry.uid,"resourceVersion":entry.resource_version}),
                );
                outputs["resources"][&entry.kind][&entry.name] = object.clone();
                if let Some(want) = wanted.get(&path) {
                    drifted |= !subset(want, &object);
                    let r = ready(&object)?;
                    if r != Readiness::Ready && readiness != Readiness::Failed {
                        readiness = r;
                    }
                } else {
                    drifted = true;
                }
                actual.insert(path, entry);
            } else {
                preconditions.insert(path.clone(), Value::Null);
                if wanted.contains_key(&path) {
                    exists = false;
                    drifted = true;
                    if readiness != Readiness::Failed {
                        readiness = Readiness::NotReady;
                    }
                    if let Some(old) = previous.get(&path) {
                        let mut history = old.clone();
                        history.missing = true;
                        actual.insert(path, history);
                    }
                }
            }
        }
        Ok(Observation {
            exists,
            drifted,
            readiness,
            outputs: select_outputs(
                &outputs,
                &context.unit.outputs,
                &[],
                &format!("kubernetes://{}/{}", context.instance, context.unit_id),
            )?,
            inventory: serde_json::to_value(actual)?,
            preconditions: serde_json::to_value(preconditions)?,
        })
    }
    fn prepare_change(
        &mut self,
        context: &Context,
        observed: &Observation,
        operation: &str,
    ) -> Result<PreparedChange> {
        let wanted = desired(context, &self.target.namespace)?;
        let old = inventory(&context.inventory)?;
        let present = inventory(&observed.inventory)?;
        let mut actions = Vec::new();
        let mut creates = Vec::new();
        let mut summary = ChangeSummary::default();
        for (path, body) in &wanted {
            let mut body = body.clone();
            body["metadata"]["annotations"]["nickel-plane.io/operation"] = json!(operation);
            let (method, route) = if let Some(entry) = present.get(path).filter(|e| !e.missing) {
                body["metadata"]["uid"] = json!(entry.uid);
                body["metadata"]["resourceVersion"] = json!(entry.resource_version);
                summary.updates += 1;
                ("PATCH", path.clone())
            } else {
                summary.creates += 1;
                summary.replacements += usize::from(old.contains_key(path));
                creates.push(path.clone());
                (
                    "POST",
                    path.rsplit_once('/')
                        .ok_or_else(|| Error::Invalid("Invalid object path".into()))?
                        .0
                        .into(),
                )
            };
            let route = format!("{route}?fieldManager={}&dryRun=All", manager(context));
            self.mutate(method, &route, &body)?;
            actions.push(Action {
                method: method.into(),
                path: route.replace("&dryRun=All", ""),
                body,
            });
        }
        for (path, entry) in &present {
            if !wanted.contains_key(path) && !entry.missing {
                summary.deletes += 1;
                actions.push(Action{method:"DELETE".into(),path:path.clone(),body:json!({"apiVersion":"v1","kind":"DeleteOptions","preconditions":{"uid":entry.uid,"resourceVersion":entry.resource_version},"propagationPolicy":"Background"})});
            }
        }
        Ok(PreparedChange {
            operation: operation.into(),
            revision: context.revision.clone(),
            artifact_sha256: context.artifact_sha256.clone(),
            target_sha256: self.execution_fingerprint()?,
            summary,
            payload: json!({"actions":actions,"creates":creates}),
            preconditions: observed.preconditions.clone(),
        })
    }
    fn execute_change(
        &mut self,
        context: &Context,
        change: &PreparedChange,
    ) -> Result<Observation> {
        require(
            change.revision == context.revision
                && change.artifact_sha256 == context.artifact_sha256
                && change.target_sha256 == self.execution_fingerprint()?,
            "Stale Kubernetes change",
        )?;
        let observed = self.observe(context)?;
        require(
            observed.preconditions == change.preconditions,
            "Kubernetes change preconditions are stale",
        )?;
        let actions: Vec<Action> = serde_json::from_value(change.payload["actions"].clone())?;
        let mut updated = inventory(&observed.inventory)?;
        for action in actions {
            let result = self.mutate(&action.method, &action.path, &action.body)?;
            if action.method == "DELETE" {
                if self.get(&action.path)?.is_none() {
                    updated.remove(&action.path);
                }
                continue;
            }
            let name = text(&result, "/metadata/name")?;
            let path = action
                .path
                .split('?')
                .next()
                .ok_or_else(|| Error::Invalid("Invalid action path".into()))?;
            let path = if action.method == "POST" {
                format!("{path}/{name}")
            } else {
                path.into()
            };
            updated.insert(
                path.clone(),
                Entry {
                    path,
                    uid: text(&result, "/metadata/uid")?.into(),
                    resource_version: text(&result, "/metadata/resourceVersion")?.into(),
                    kind: text(&result, "/kind")?.into(),
                    name: name.into(),
                    missing: false,
                },
            );
        }
        let mut after = context.clone();
        after.inventory = serde_json::to_value(updated)?;
        after.interrupted = Some(change.clone());
        self.observe(&after)
    }
    fn observe_removal(&mut self, context: &Context) -> Result<Observation> {
        let mut empty = context.clone();
        empty.artifact["items"] = json!([]);
        let mut result = self.observe(&empty)?;
        // Missing historical objects have no delete action or retained inventory.
        // Excluding them keeps the saved preconditions stable on the next read.
        if let Some(preconditions) = result.preconditions.as_object_mut() {
            preconditions.retain(|_, value| !value.is_null());
        }
        result.exists = result.inventory.as_object().is_some_and(|m| !m.is_empty());
        Ok(result)
    }
    fn prepare_removal(
        &mut self,
        context: &Context,
        observed: &Observation,
        operation: &str,
    ) -> Result<PreparedChange> {
        let mut empty = context.clone();
        empty.artifact["items"] = json!([]);
        self.prepare_change(&empty, observed, operation)
    }
    fn execute_removal(
        &mut self,
        context: &Context,
        change: &PreparedChange,
    ) -> Result<Observation> {
        let actions: Vec<Action> = serde_json::from_value(change.payload["actions"].clone())?;
        require(
            actions.iter().all(|a| a.method == "DELETE"),
            "Expected a deletion plan",
        )?;
        let mut empty = context.clone();
        empty.artifact["items"] = json!([]);
        let observed = self.observe_removal(context)?;
        require(
            observed.preconditions == change.preconditions,
            "Kubernetes removal preconditions are stale",
        )?;
        // execute_change checks the same inventory snapshot, with absent objects
        // already excluded just as they were during removal planning.
        empty.inventory = observed.inventory;
        empty.interrupted = None;
        let mut result = self.execute_change(&empty, change)?;
        result.exists = result.inventory.as_object().is_some_and(|m| !m.is_empty());
        Ok(result)
    }
    fn remove(&mut self, context: &Context, _observed: &Observation) -> Result<Observation> {
        let observed = self.observe_removal(context)?;
        let plan = self.prepare_removal(context, &observed, "remove")?;
        self.execute_removal(context, &plan)
    }
}

#![allow(clippy::unwrap_used)]
#![cfg(feature = "kubernetes")]
use nickel_plane::{
    adapters::kubernetes::{Api, Kubernetes},
    backend::*,
    model::Unit,
    targets::KubernetesTarget,
    Error, Result,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
#[derive(Default)]
struct FakeApi {
    objects: BTreeMap<String, Value>,
    writes: usize,
    conflict: bool,
    interrupt: bool,
    delay_delete: bool,
}
impl Api for FakeApi {
    fn request(&mut self, method: &str, path: &str, body: Option<&Value>) -> Result<(u16, Value)> {
        let route = path.split('?').next().unwrap_or(path);
        let body = body.cloned().unwrap_or(Value::Null);
        if method == "GET" {
            return Ok(self
                .objects
                .get(route)
                .map(|v| (200, v.clone()))
                .unwrap_or((404, json!({"message":"private diagnostic"}))));
        }
        if self.conflict {
            return Ok((409, json!({"message":"private diagnostic"})));
        }
        if path.contains("dryRun=All") {
            return Ok((200, body));
        }
        if method == "DELETE" {
            let Some(old) = self.objects.get(route) else {
                return Ok((404, Value::Null));
            };
            if old["metadata"]["uid"] != body["preconditions"]["uid"] {
                return Ok((409, Value::Null));
            }
            if !self.delay_delete {
                self.objects.remove(route);
            }
            self.writes += 1;
            return Ok((200, json!({"status":"Success"})));
        }
        let destination = if method == "POST" {
            format!(
                "{route}/{}",
                body["metadata"]["name"]
                    .as_str()
                    .ok_or_else(|| Error::Invalid("name".into()))?
            )
        } else {
            route.into()
        };
        if method == "POST" && self.objects.contains_key(&destination) {
            return Ok((409, Value::Null));
        }
        if let Some(old) = self.objects.get(&destination) {
            if body["metadata"]["resourceVersion"] != old["metadata"]["resourceVersion"] {
                return Ok((409, Value::Null));
            }
        }
        let mut updated = body;
        self.writes += 1;
        if updated["metadata"].get("uid").is_none() {
            updated["metadata"]["uid"] = json!(format!("uid-{}", self.writes));
        }
        updated["metadata"]["resourceVersion"] = json!(self.writes.to_string());
        self.objects.insert(destination, updated.clone());
        if self.interrupt {
            self.interrupt = false;
            return Err(Error::Backend("interrupted response".into()));
        }
        Ok((200, updated))
    }
}
fn target() -> KubernetesTarget {
    KubernetesTarget {
        server: "https://cluster.example".into(),
        namespace: "test".into(),
        ca_file: None,
        token_file: None,
        allow_http_loopback: false,
    }
}
fn context(work: std::path::PathBuf) -> Result<Context> {
    let unit: Unit = serde_json::from_value(
        json!({"backend":"kubernetes","target":"cluster","configuration":{}}),
    )?;
    Ok(Context {
        owner: "owner".into(),
        instance: "instance".into(),
        unit_id: "app".into(),
        revision: "revision".into(),
        unit,
        artifact: json!({"apiVersion":"v1","kind":"List","items":[{"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"settings","namespace":"test"},"data":{"value":"one"}}]}),
        artifact_sha256: "digest".into(),
        inventory: Value::Null,
        interrupted: None,
        work,
        engine_version: None,
        provider_lock: None,
    })
}
#[test]
fn creates_owned_objects_and_refuses_replacement_and_stale_changes() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut c = context(temp.path().into())?;
    let mut k = Kubernetes {
        api: FakeApi::default(),
        target: target(),
    };
    let before = k.observe(&c)?;
    let plan = k.prepare_change(&c, &before, "operation")?;
    assert_eq!(k.api.writes, 0);
    let observed = k.execute_change(&c, &plan)?;
    assert!(observed.exists && !observed.drifted);
    assert_eq!(observed.readiness, Readiness::Ready);
    c.inventory = observed.inventory;
    let path = "/api/v1/namespaces/test/configmaps/settings";
    c.artifact["items"][0]["data"]["value"] = json!("two");
    c.artifact_sha256 = "next".into();
    let before = k.observe(&c)?;
    let plan = k.prepare_change(&c, &before, "second")?;
    k.api
        .objects
        .get_mut(path)
        .ok_or_else(|| Error::Invalid("missing object".into()))?["metadata"]["resourceVersion"] =
        json!("changed");
    assert!(k.execute_change(&c, &plan).is_err());
    assert_eq!(k.api.writes, 1);
    k.api
        .objects
        .get_mut(path)
        .ok_or_else(|| Error::Invalid("missing object".into()))?["metadata"]["uid"] =
        json!("foreign");
    assert!(k.observe(&c).is_err());
    Ok(())
}
#[test]
fn recovers_interrupted_create_only_with_matching_operation_intent() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut c = context(temp.path().into())?;
    let mut k = Kubernetes {
        api: FakeApi {
            interrupt: true,
            ..Default::default()
        },
        target: target(),
    };
    let before = k.observe(&c)?;
    let plan = k.prepare_change(&c, &before, "intended")?;
    assert!(k.execute_change(&c, &plan).is_err());
    assert!(k.observe(&c).is_err());
    c.interrupted = Some(plan);
    let observed = k.observe(&c)?;
    assert!(observed.exists);
    assert_eq!(k.api.writes, 1);
    Ok(())
}
#[test]
fn conflicts_scope_and_readiness_are_explicit() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut c = context(temp.path().into())?;
    let mut k = Kubernetes {
        api: FakeApi {
            conflict: true,
            ..Default::default()
        },
        target: target(),
    };
    let before = k.observe(&c)?;
    let error = k
        .prepare_change(&c, &before, "plan")
        .err()
        .ok_or_else(|| Error::Invalid("conflict accepted".into()))?;
    assert!(!error.to_string().contains("private diagnostic"));
    c.artifact["items"][0]["metadata"]["namespace"] = json!("outside");
    assert!(k.observe(&c).is_err());
    Ok(())
}

#[test]
fn watch_expiry_reconnects_and_pruning_waits_for_owned_deletion() -> Result<()> {
    struct Watch {
        calls: usize,
    }
    impl Api for Watch {
        fn request(&mut self, method: &str, path: &str, _: Option<&Value>) -> Result<(u16, Value)> {
            assert_eq!(method, "GET");
            assert!(path.contains("watch=true"));
            assert!(path.contains("resourceVersion=12"));
            self.calls += 1;
            Ok(if self.calls == 1 {
                (410, Value::Null)
            } else {
                (200, json!([{ "type":"MODIFIED"}]))
            })
        }
    }
    let mut watch = Kubernetes {
        api: Watch { calls: 0 },
        target: target(),
    };
    let inv = json!({"/api/v1/namespaces/test/configmaps/settings":{"path":"/api/v1/namespaces/test/configmaps/settings","uid":"u","resourceVersion":"12","kind":"ConfigMap","name":"settings"}});
    assert!(watch.poll_events(&inv)?);
    assert!(watch.poll_events(&inv)?);
    assert_eq!(watch.api.calls, 2);
    let temp = tempfile::tempdir()?;
    let mut c = context(temp.path().into())?;
    let mut k = Kubernetes {
        api: FakeApi::default(),
        target: target(),
    };
    let o = k.observe(&c)?;
    let p = k.prepare_change(&c, &o, "create")?;
    c.inventory = k.execute_change(&c, &p)?.inventory;
    c.artifact["items"] = json!([]);
    let o = k.observe(&c)?;
    let p = k.prepare_change(&c, &o, "prune")?;
    assert_eq!(p.summary.deletes, 1);
    let o = k.execute_change(&c, &p)?;
    assert_eq!(o.inventory, json!({}));
    assert_eq!(k.api.writes, 2);
    Ok(())
}
#[test]
fn local_http_watch_transport_parses_events_and_never_follows_redirects() -> Result<()> {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };
    let server = TcpListener::bind("127.0.0.1:0")?;
    let address = server.local_addr()?;
    let handle = thread::spawn(move || -> std::io::Result<()> {
        for response in ["HTTP/1.1 200 OK\r\nContent-Length: 39\r\nConnection: close\r\n\r\n{\"type\":\"MODIFIED\"}\n{\"type\":\"DELETED\"}\n", "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/secret\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"] {
   let(mut stream,_)=server.accept()?;stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;let mut buf=[0u8;8192];let n=stream.read(&mut buf)?;assert!(String::from_utf8_lossy(&buf[..n]).contains("watch=true"));stream.write_all(response.as_bytes())?;
  }
        Ok(())
    });
    let mut t = target();
    t.server = format!("http://{address}");
    t.allow_http_loopback = true;
    let mut k = Kubernetes::connect(t)?;
    let first = k
        .api
        .request("GET", "/api/v1/namespaces/test/configmaps?watch=true", None)?;
    assert_eq!(first.0, 200);
    assert_eq!(first.1.as_array().map(Vec::len), Some(2));
    assert!(matches!(
        k.api
            .request("GET", "/api/v1/namespaces/test/configmaps?watch=true", None),
        Ok((302, _)) | Err(_)
    ));
    handle
        .join()
        .map_err(|_| Error::Backend("fixture thread".into()))??;
    Ok(())
}
#[test]
fn array_drift_and_workload_readiness_require_observed_state() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut c = context(temp.path().into())?;
    c.artifact["items"][0] = json!({"apiVersion":"networking.k8s.io/v1","kind":"NetworkPolicy","metadata":{"name":"deny","namespace":"test"},"spec":{"podSelector":{},"ingress":[],"egress":[],"policyTypes":["Ingress","Egress"]}});
    let mut k = Kubernetes {
        api: FakeApi::default(),
        target: target(),
    };
    let o = k.observe(&c)?;
    let p = k.prepare_change(&c, &o, "np")?;
    c.inventory = k.execute_change(&c, &p)?.inventory;
    k.api
        .objects
        .get_mut("/apis/networking.k8s.io/v1/namespaces/test/networkpolicies/deny")
        .ok_or_else(|| Error::Invalid("policy".into()))?["spec"]["ingress"] = json!([{}]);
    assert!(k.observe(&c)?.drifted);
    c.inventory = Value::Null;
    c.artifact["items"][0] = json!({"apiVersion":"apps/v1","kind":"Deployment","metadata":{"name":"app","namespace":"test"},"spec":{"replicas":1}});
    let o = k.observe(&c)?;
    let p = k.prepare_change(&c, &o, "deployment")?;
    let o = k.execute_change(&c, &p)?;
    c.inventory = o.inventory;
    assert_eq!(o.readiness, Readiness::NotReady);
    let obj = k
        .api
        .objects
        .get_mut("/apis/apps/v1/namespaces/test/deployments/app")
        .ok_or_else(|| Error::Invalid("deployment".into()))?;
    obj["metadata"]["generation"] = json!(2);
    obj["status"] =
        json!({"observedGeneration":1,"readyReplicas":1,"updatedReplicas":1,"availableReplicas":1});
    assert_eq!(k.observe(&c)?.readiness, Readiness::NotReady);
    k.api
        .objects
        .get_mut("/apis/apps/v1/namespaces/test/deployments/app")
        .ok_or_else(|| Error::Invalid("deployment".into()))?["status"]["observedGeneration"] =
        json!(2);
    assert_eq!(k.observe(&c)?.readiness, Readiness::Ready);
    Ok(())
}

#[test]
fn interrupted_create_followed_by_removal_tracks_finalizers() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut c = context(temp.path().into())?;
    let mut k = Kubernetes {
        api: FakeApi {
            interrupt: true,
            delay_delete: true,
            ..Default::default()
        },
        target: target(),
    };
    let o = k.observe(&c)?;
    let p = k.prepare_change(&c, &o, "in-flight")?;
    assert!(k.execute_change(&c, &p).is_err());
    c.interrupted = Some(p);
    let recovered = k.observe(&c)?;
    let pending = k.remove(&c, &recovered)?;
    assert!(
        pending.exists,
        "Finalizers must retain recovered inventory until disappearance"
    );
    assert!(pending.inventory.as_object().is_some_and(|m| !m.is_empty()));
    c.inventory = pending.inventory;
    k.api.objects.clear();
    let o = k.observe(&c)?;
    assert!(!k.remove(&c, &o)?.exists);
    Ok(())
}
#[test]
fn loopback_http_creation_update_and_delete_use_preconditioned_requests() -> Result<()> {
    use std::{
        io::{BufRead, Read, Write},
        net::TcpListener,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        thread,
        time::Duration,
    };
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;
    let stop = Arc::new(AtomicBool::new(false));
    let done = stop.clone();
    let thread = thread::spawn(move || -> Result<()> {
        let mut api = FakeApi::default();
        while !done.load(Ordering::SeqCst) {
            let (mut stream, _) = match listener.accept() {
                Ok(v) => v,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            stream.set_read_timeout(Some(Duration::from_secs(5)))?;
            let mut reader = std::io::BufReader::new(stream.try_clone()?);
            let mut line = String::new();
            reader.read_line(&mut line)?;
            let mut words = line.split_whitespace();
            let method = words.next().unwrap_or("").to_string();
            let path = words.next().unwrap_or("").to_string();
            let mut length = 0usize;
            loop {
                line.clear();
                reader.read_line(&mut line)?;
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        length = value
                            .trim()
                            .parse()
                            .map_err(|_| Error::Invalid("length".into()))?;
                    }
                }
            }
            let mut bytes = vec![0; length];
            reader.read_exact(&mut bytes)?;
            let body = if bytes.is_empty() {
                None
            } else {
                Some(serde_json::from_slice::<Value>(&bytes)?)
            };
            eprintln!("fixture request {method} {path}");
            let (code, value) = api.request(&method, &path, body.as_ref())?;
            let data = serde_json::to_vec(&value)?;
            write!(stream,"HTTP/1.0 {code} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",data.len())?;
            stream.write_all(&data)?;
        }
        Ok(())
    });
    let result = (|| -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut c = context(temp.path().into())?;
        let mut t = target();
        t.server = format!("http://{address}");
        t.allow_http_loopback = true;
        let mut k = Kubernetes::connect(t)?;
        for operation in ["first", "second"] {
            c.artifact["items"][0]["data"]["value"] = json!(operation);
            c.artifact_sha256 = operation.into();
            let o = k.observe(&c)?;
            let p = k.prepare_change(&c, &o, operation)?;
            let o = k.execute_change(&c, &p)?;
            assert!(o.exists && !o.drifted);
            c.inventory = o.inventory;
        }
        let o = k.observe(&c)?;
        assert!(!k.remove(&c, &o)?.exists);
        Ok(())
    })();
    stop.store(true, Ordering::SeqCst);
    thread
        .join()
        .map_err(|_| Error::Backend("fixture panic".into()))??;
    result
}

#[test]
fn recovers_superseded_creation_before_pruning() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut c = context(temp.path().into())?;
    let mut k = Kubernetes {
        api: FakeApi {
            interrupt: true,
            ..Default::default()
        },
        target: target(),
    };
    let o = k.observe(&c)?;
    let p = k.prepare_change(&c, &o, "lost")?;
    assert!(k.execute_change(&c, &p).is_err());
    c.interrupted = Some(p);
    c.revision = "next".into();
    c.artifact_sha256 = "next".into();
    c.artifact["items"][0]["metadata"]["name"] = json!("replacement-name");
    let o = k.observe(&c)?;
    assert!(k
        .api
        .objects
        .contains_key("/api/v1/namespaces/test/configmaps/settings"));
    assert!(
        o.inventory
            .get("/api/v1/namespaces/test/configmaps/settings")
            .is_some(),
        "Superseded creation must be recovered"
    );
    let p = k.prepare_change(&c, &o, "next")?;
    assert_eq!(
        p.summary.deletes, 1,
        "Superseded creation must remain eligible for explicit pruning"
    );
    Ok(())
}
struct SharedApi(std::sync::Arc<std::sync::Mutex<FakeApi>>);
impl Api for SharedApi {
    fn request(&mut self, m: &str, p: &str, b: Option<&Value>) -> Result<(u16, Value)> {
        self.0.lock().unwrap().request(m, p, b)
    }
}
struct ReviewValidate;
impl nickel_plane::composition::Validator for ReviewValidate {
    fn validate(
        &self,
        _: &Unit,
        v: &Value,
        b: &nickel_plane::composition::CompiledUnit,
    ) -> Result<nickel_plane::composition::CompiledUnit> {
        let mut b = b.clone();
        b.artifact = v.clone();
        Ok(b)
    }
}
#[test]
fn missing_uid_history_survives_denied_replacement_and_revision_change() -> Result<()> {
    use nickel_plane::{
        composition::{Compiled, CompiledUnit},
        model::Instance,
        reconcile::{reconcile, Options, Registry},
        store::Store,
    };
    let temp = tempfile::tempdir()?;
    let c = context(temp.path().into())?;
    let instance: Instance = serde_json::from_value(
        json!({"apiVersion":"nickel-plane/v1","id":"demo","composition":{"name":"test","version":"1"},"policy":{"mode":"manage","allowReplace":false},"units":{"first":{"backend":"kubernetes","target":"cluster","configuration":c.artifact}}}),
    )?;
    let mut c = Compiled {
        version: 1,
        revision: String::new(),
        instance,
        inputs: Value::Null,
        units: BTreeMap::from([(
            "first".into(),
            CompiledUnit {
                artifact: c.artifact,
                receipt: Value::Null,
                engine_version: None,
                provider_lock: None,
            },
        )]),
    };
    c.revision = c.digest()?;
    let mut s = Store::open(&temp.path().join("store"))?;
    s.submit(&c)?;
    let remote = std::sync::Arc::new(std::sync::Mutex::new(FakeApi::default()));
    let mut backends: Registry = BTreeMap::from([(
        "cluster".into(),
        Box::new(Kubernetes {
            api: SharedApi(remote.clone()),
            target: target(),
        }) as Box<dyn Backend>,
    )]);
    reconcile(
        &mut s,
        &c,
        &ReviewValidate,
        &mut backends,
        &Options::default(),
    )?;
    assert_eq!(remote.lock().unwrap().writes, 1);
    remote.lock().unwrap().objects.clear();
    reconcile(
        &mut s,
        &c,
        &ReviewValidate,
        &mut backends,
        &Options::default(),
    )?;
    assert!(s
        .unit("demo", "first")?
        .unwrap()
        .failure
        .unwrap()
        .contains("replacement"));
    c.instance.display_name = Some("Harmless display change".into());
    c.revision = c.digest()?;
    s.submit(&c)?;
    reconcile(
        &mut s,
        &c,
        &ReviewValidate,
        &mut backends,
        &Options::default(),
    )?;
    assert_eq!(
        remote.lock().unwrap().writes,
        1,
        "Forbidden replacement must not execute"
    );
    assert!(!s.unit("demo", "first")?.unwrap().applied);
    c.instance.policy.allow_replace = true;
    c.revision = c.digest()?;
    s.submit(&c)?;
    remote.lock().unwrap().interrupt = true;
    reconcile(
        &mut s,
        &c,
        &ReviewValidate,
        &mut backends,
        &Options::default(),
    )?;
    assert_eq!(remote.lock().unwrap().writes, 2);
    reconcile(
        &mut s,
        &c,
        &ReviewValidate,
        &mut backends,
        &Options {
            now: 1000,
            ..Default::default()
        },
    )?;
    assert!(s.unit("demo", "first")?.unwrap().applied);
    assert_eq!(
        remote.lock().unwrap().writes,
        2,
        "Interrupted authorized replacement must not replay"
    );
    Ok(())
}
#[test]
fn optional_empty_lists_follow_kubernetes_serialization() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mut c = context(temp.path().into())?;
    c.artifact["items"][0] = json!({"apiVersion":"networking.k8s.io/v1","kind":"NetworkPolicy","metadata":{"name":"deny","namespace":"test"},"spec":{"podSelector":{},"ingress":[],"egress":[],"policyTypes":["Ingress","Egress"]}});
    let mut k = Kubernetes {
        api: FakeApi::default(),
        target: target(),
    };
    let o = k.observe(&c)?;
    let p = k.prepare_change(&c, &o, "np")?;
    c.inventory = k.execute_change(&c, &p)?.inventory;
    let spec = k
        .api
        .objects
        .get_mut("/apis/networking.k8s.io/v1/namespaces/test/networkpolicies/deny")
        .unwrap()["spec"]
        .as_object_mut()
        .unwrap();
    spec.remove("ingress");
    spec.remove("egress");
    assert!(
        !k.observe(&c)?.drifted,
        "Omitted empty optional lists are equivalent"
    );
    Ok(())
}

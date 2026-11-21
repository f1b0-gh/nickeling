#![allow(clippy::unwrap_used)]
#![cfg(feature = "kubernetes")]
use nickel_plane::{
    adapters::kubernetes::{Api, Kubernetes},
    backend::Backend,
    targets::KubernetesTarget,
    Error, Result,
};
use serde_json::json;
use std::{
    net::{TcpListener, TcpStream},
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn server(directory: &Path) -> Result<(Server, String)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    drop(listener);
    std::fs::write(directory.join("health"), b"HTTP/1.0 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\n\r\n{\"ok\":true}")?;
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tls");
    let child = Command::new("openssl")
        .args([
            "s_server",
            "-quiet",
            "-no_cache",
            "-HTTP",
            "-accept",
            &address.to_string(),
            "-cert",
        ])
        .arg(fixtures.join("server.pem"))
        .arg("-key")
        .arg(fixtures.join("server-test-key.pem"))
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let mut server = Server(child);
    let started = Instant::now();
    loop {
        if TcpStream::connect(address).is_ok() {
            break;
        }
        if server.0.try_wait()?.is_some() || started.elapsed() > Duration::from_secs(5) {
            return Err(Error::Backend("Synthetic TLS fixture did not start".into()));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok((server, format!("https://{address}")))
}

#[test]
fn rotated_ca_replaces_actual_tls_trust_only_after_reconnection() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (_server, address) = server(temp.path())?;
    let ca = temp.path().join("active-ca.pem");
    let proof = temp.path().join("previous-ca.pem");
    std::fs::write(&ca, include_bytes!("fixtures/tls/old-ca.pem"))?;
    std::fs::write(&proof, include_bytes!("fixtures/tls/new-ca.pem"))?;
    let mut target = KubernetesTarget {
        server: address,
        namespace: "test".into(),
        ca_file: Some(ca.clone()),
        previous_ca_file: Some(proof.clone()),
        token_file: None,
        allow_http_loopback: false,
    };
    let mut old = Kubernetes::connect(target.clone())?;
    let identity = old.target_identity()?;
    let execution = old.execution_fingerprint()?;
    // The proof file contains the server's issuer, but must never be TLS roots.
    assert!(matches!(
        old.api.request("GET", "/health", None),
        Err(Error::Backend(_))
    ));
    std::fs::write(&ca, include_bytes!("fixtures/tls/new-ca.pem"))?;
    assert!(old.execution_fingerprint().is_err());
    let error = old.api.request("GET", "/health", None).unwrap_err();
    assert!(error.to_string().contains("reconnect"));
    std::fs::write(&proof, include_bytes!("fixtures/tls/old-ca.pem"))?;
    target.previous_ca_file = Some(proof);
    let mut new = Kubernetes::connect(target)?;
    assert_eq!(new.target_identity()?, identity);
    assert_ne!(new.execution_fingerprint()?, execution);
    assert_eq!(
        new.api.request("GET", "/health", None)?,
        (200, json!({"ok":true}))
    );
    Ok(())
}

#[test]
fn connection_material_keeps_private_file_symlink_and_size_refusals() -> Result<()> {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let temp = tempfile::tempdir()?;
    let ca = temp.path().join("ca.pem");
    let token = temp.path().join("token");
    std::fs::write(&ca, include_bytes!("fixtures/tls/new-ca.pem"))?;
    std::fs::write(&token, "synthetic-test-token")?;
    std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600))?;
    let target = KubernetesTarget {
        server: "https://cluster.example".into(),
        namespace: "test".into(),
        ca_file: Some(ca.clone()),
        previous_ca_file: None,
        token_file: Some(token.clone()),
        allow_http_loopback: false,
    };
    let client = Kubernetes::connect(target.clone())?;
    std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o644))?;
    assert!(client.execution_fingerprint().is_err());
    std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600))?;
    std::fs::write(&token, vec![b'x'; 64 * 1024 + 1])?;
    assert!(client
        .execution_fingerprint()
        .unwrap_err()
        .to_string()
        .contains("too large"));
    std::fs::write(&token, "synthetic-test-token")?;
    std::fs::write(&ca, vec![b'x'; 1024 * 1024 + 1])?;
    assert!(client
        .execution_fingerprint()
        .unwrap_err()
        .to_string()
        .contains("too large"));
    std::fs::write(&ca, include_bytes!("fixtures/tls/new-ca.pem"))?;
    let real_ca = temp.path().join("real-ca.pem");
    std::fs::rename(&ca, &real_ca)?;
    symlink(&real_ca, &ca)?;
    assert!(client.execution_fingerprint().is_err());
    assert!(Kubernetes::connect(target).is_err());
    Ok(())
}

#[test]
fn token_rotation_refuses_cached_requests_and_reconnects_with_new_authorization() -> Result<()> {
    use std::{
        io::{Read, Write},
        os::unix::fs::PermissionsExt,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        },
        thread::{self, JoinHandle},
    };
    struct Guard {
        stop: Arc<AtomicBool>,
        worker: Option<JoinHandle<std::io::Result<()>>>,
    }
    impl Guard {
        fn finish(mut self) -> Result<()> {
            self.stop.store(true, Ordering::SeqCst);
            self.worker
                .take()
                .unwrap()
                .join()
                .map_err(|_| Error::Backend("Authentication fixture thread failed".into()))??;
            Ok(())
        }
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;
    let headers = Arc::new(Mutex::new(Vec::new()));
    let received = headers.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let done = stop.clone();
    let worker = thread::spawn(move || -> std::io::Result<()> {
        let started = Instant::now();
        while !done.load(Ordering::SeqCst) && started.elapsed() < Duration::from_secs(10) {
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(error) => return Err(error),
            };
            stream.set_read_timeout(Some(Duration::from_secs(2)))?;
            stream.set_write_timeout(Some(Duration::from_secs(2)))?;
            let started = Instant::now();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                if request.len() >= 16 * 1024 || started.elapsed() >= Duration::from_secs(2) {
                    return Err(std::io::Error::other(
                        "Authentication fixture request limit",
                    ));
                }
                let mut byte = [0];
                stream.read_exact(&mut byte)?;
                request.push(byte[0]);
            }
            let request = String::from_utf8(request)
                .map_err(|_| std::io::Error::other("Invalid fixture request"))?;
            let authorization = request.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("authorization")
                    .then(|| value.trim().to_string())
            });
            received.lock().unwrap().push(authorization);
            stream.write_all(
                b"HTTP/1.0 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
            )?;
        }
        Ok(())
    });
    let guard = Guard {
        stop,
        worker: Some(worker),
    };
    let temp = tempfile::tempdir()?;
    let token = temp.path().join("private-token");
    std::fs::write(&token, "synthetic-token-A")?;
    std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600))?;
    let target = KubernetesTarget {
        server: format!("http://{address}"),
        namespace: "test".into(),
        ca_file: None,
        previous_ca_file: None,
        token_file: Some(token.clone()),
        allow_http_loopback: true,
    };
    let mut old = Kubernetes::connect(target.clone())?;
    assert_eq!(old.api.request("GET", "/health", None)?.0, 200);
    assert_eq!(
        *headers.lock().unwrap(),
        vec![Some("Bearer synthetic-token-A".into())]
    );
    std::fs::write(&token, "synthetic-token-B")?;
    assert!(old
        .api
        .request("GET", "/health", None)
        .unwrap_err()
        .to_string()
        .contains("reconnect"));
    assert_eq!(
        headers.lock().unwrap().len(),
        1,
        "The stale request must not reach the server"
    );
    let mut new = Kubernetes::connect(target)?;
    assert_eq!(new.api.request("GET", "/health", None)?.0, 200);
    assert_eq!(
        *headers.lock().unwrap(),
        vec![
            Some("Bearer synthetic-token-A".into()),
            Some("Bearer synthetic-token-B".into())
        ]
    );
    guard.finish()
}

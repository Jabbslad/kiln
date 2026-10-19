//! Exec poll through a real Caddy reverse_proxy.
//! The door listens on 127.0.0.1:8444. A TCP proxy sits in front of it.
//! Caddy terminates a separate certificate and dials that proxy with TLS.
#![cfg(unix)]

use std::{
    fs,
    net::{SocketAddr, TcpStream},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use axum::{
    body::{Body, Bytes},
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use futures_util::stream;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use kiln_api::{
    Action, ExecRequest, ExecResult, Operation, OperationState, Outcome, Submit,
    auth::{Scope, ServerClaims},
};
use kiln_server::{
    auth::{Authenticator, Enrollment},
    door,
};

struct Upstream {
    port: u16,
    hits: Arc<AtomicUsize>,
    ca_pem: String,
}

struct Proxy {
    child: Child,
    port: u16,
}

impl Drop for Proxy {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn sockets(port: u16) -> usize {
    let marker = format!(":{port:04X}");
    fs::read_to_string("/proc/net/tcp")
        .unwrap()
        .lines()
        .filter(|line| line.contains(&marker))
        .count()
}

fn token(key: &EncodingKey) -> String {
    let now = jsonwebtoken::get_current_timestamp();
    let claims = ServerClaims {
        iss: "https://identity.example.test".into(),
        sub: "owner-a".into(),
        aud: "kiln:server:server-b".into(),
        iat: now,
        nbf: now,
        exp: now + 300,
        credential_id: "device-1".into(),
        scope: Scope::Operate,
    };
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("first".into());
    jsonwebtoken::encode(&header, &claims, key).unwrap()
}

fn enrollment() -> Enrollment {
    Enrollment {
        version: 1,
        issuer: "https://identity.example.test".into(),
        owner_id: "owner-a".into(),
        server_id: "server-b".into(),
    }
}

fn write_jwks(dir: &Path) -> EncodingKey {
    use base64::Engine;
    use p256::{elliptic_curve::sec1::ToEncodedPoint, pkcs8::EncodePrivateKey};
    let key = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
    let pem = key.to_pkcs8_pem(Default::default()).unwrap();
    let point = key.public_key().to_encoded_point(false);
    let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let jwks = serde_json::json!({"keys":[{
        "kty":"EC","crv":"P-256","use":"sig","alg":"ES256","kid":"first",
        "x":enc.encode(point.x().unwrap()),"y":enc.encode(point.y().unwrap())
    }]});
    fs::write(
        dir.join("jwks.json"),
        serde_json::to_vec(&serde_json::json!({"issuer": enrollment().issuer, "jwks": jwks}))
            .unwrap(),
    )
    .unwrap();
    EncodingKey::from_ec_pem(pem.as_bytes()).unwrap()
}

fn authenticator(dir: &Path) -> Authenticator {
    Authenticator::central_only(enrollment(), dir.join("jwks.json")).unwrap()
}

fn pem_pair(name: &str) -> (String, String) {
    let cert = rcgen::generate_simple_self_signed(vec![name.into()]).unwrap();
    (cert.cert.pem(), cert.signing_key.serialize_pem())
}

async fn listen_tls(app: axum::Router, name: &str) -> (u16, String) {
    let (ca_pem, key_pem) = pem_pair(name);
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        ca_pem.clone().into_bytes(),
        key_pem.into_bytes(),
    )
    .await
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, tls)
            .unwrap()
            .serve(app.into_make_service())
            .await
            .unwrap();
    });
    (port, ca_pem)
}

struct OpState {
    hits: Arc<AtomicUsize>,
    gets: AtomicUsize,
    ids: Mutex<Option<(String, String)>>,
}

async fn upstream() -> Upstream {
    let hits = Arc::new(AtomicUsize::new(0));
    let state = Arc::new(OpState {
        hits: hits.clone(),
        gets: AtomicUsize::new(0),
        ids: Mutex::new(None),
    });
    let app = axum::Router::new()
        .route("/v1/operations", post(accept_exec))
        .route("/v1/operations/{id}", get(poll_exec))
        .with_state(state);
    let (port, ca_pem) = listen_tls(app, "127.0.0.1").await;
    Upstream { port, hits, ca_pem }
}

async fn accept_exec(State(state): State<Arc<OpState>>, body: Bytes) -> (StatusCode, Bytes) {
    state.hits.fetch_add(1, Ordering::SeqCst);
    let submit: Submit = serde_json::from_slice(&body).unwrap();
    let box_id = submit.action.box_id().unwrap().to_owned();
    *state.ids.lock().unwrap() = Some((submit.id.clone(), box_id.clone()));
    let op = Operation {
        id: submit.id,
        box_id,
        state: OperationState::Running,
        result: None,
        error: None,
    };
    (
        StatusCode::ACCEPTED,
        Bytes::from(serde_json::to_vec(&op).unwrap()),
    )
}

async fn poll_exec(
    State(state): State<Arc<OpState>>,
    AxumPath(id): AxumPath<String>,
) -> axum::response::Response {
    state.hits.fetch_add(1, Ordering::SeqCst);
    let (stored_id, box_id) = state.ids.lock().unwrap().clone().unwrap();
    assert_eq!(id, stored_id);
    let n = state.gets.fetch_add(1, Ordering::SeqCst);
    let op = if n == 0 {
        Operation {
            id: stored_id,
            box_id,
            state: OperationState::Running,
            result: None,
            error: None,
        }
    } else {
        Operation {
            id: stored_id,
            box_id,
            state: OperationState::Succeeded,
            result: Some(Outcome::Exec(ExecResult {
                stdout: b"kiln-exec-poll\n".to_vec(),
                stderr: Vec::new(),
                exit_code: Some(0),
                truncated: false,
                timed_out: false,
            })),
            error: None,
        }
    };
    let bytes = serde_json::to_vec(&op).unwrap();
    if n == 0 {
        return (StatusCode::OK, Bytes::from(bytes)).into_response();
    }
    let chunks: Vec<Bytes> = bytes
        .chunks(5)
        .map(Bytes::copy_from_slice)
        .collect();
    let stream = stream::unfold(chunks.into_iter(), |mut chunks| async {
        let next = chunks.next()?;
        tokio::time::sleep(Duration::from_millis(15)).await;
        Some((Ok::<Bytes, std::io::Error>(next), chunks))
    });
    axum::response::Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(Body::from_stream(stream))
        .unwrap()
}

fn wait_tcp(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("nothing accepted on {addr}");
}

fn start_proxy(door_port: u16, dir: &Path) -> Proxy {
    let port_file = dir.join("proxy.port");
    let log = fs::File::create(dir.join("proxy.log")).unwrap();
    let script = r#"
import os, socket, sys
target, port_path = sys.argv[1], sys.argv[2]
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", 0))
s.listen(128)
with open(port_path, "w") as out:
    out.write(str(s.getsockname()[1]))
fd = s.fileno()
os.dup2(fd, 3)
if fd != 3:
    os.close(fd)
os.environ["LISTEN_FDS"] = "1"
os.environ["LISTEN_PID"] = str(os.getpid())
os.execv(
    "/usr/lib/systemd/systemd-socket-proxyd",
    ["systemd-socket-proxyd", "--connections-max=256", "--exit-idle-time=5min", target],
)
"#;
    let child = Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(format!("127.0.0.1:{door_port}"))
        .arg(&port_file)
        .stdout(Stdio::from(log.try_clone().unwrap()))
        .stderr(Stdio::from(log))
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let port = loop {
        if let Ok(text) = fs::read_to_string(&port_file)
            && let Ok(port) = text.trim().parse::<u16>()
        {
            break port;
        }
        if Instant::now() > deadline {
            panic!(
                "proxy did not listen: {}",
                fs::read_to_string(dir.join("proxy.log")).unwrap_or_default()
            );
        }
        std::thread::sleep(Duration::from_millis(30));
    };
    Proxy { child, port }
}

fn start_caddy(
    dir: &Path,
    proxy_port: u16,
    door_ca: &str,
    caddy_cert: &str,
    caddy_key: &str,
) -> Proxy {
    let cert = dir.join("caddy.crt");
    let key = dir.join("caddy.key");
    let ca = dir.join("door-ca.crt");
    fs::write(&cert, caddy_cert).unwrap();
    fs::write(&key, caddy_key).unwrap();
    fs::write(&ca, door_ca).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let config = dir.join("Caddyfile");
    fs::write(
        &config,
        format!(
            "{{
	admin off
	auto_https off
}}
https://127.0.0.1:{port} {{
	tls {cert} {key}
	reverse_proxy https://127.0.0.1:{proxy_port} {{
		transport http {{
			tls_server_name cli.dark-forge.dev
			tls_trusted_ca_certs {ca}
		}}
	}}
}}
",
            cert = cert.display(),
            key = key.display(),
            ca = ca.display(),
        ),
    )
    .unwrap();
    let log = fs::File::create(dir.join("caddy.log")).unwrap();
    let child = Command::new("caddy")
        .args(["run", "--config"])
        .arg(&config)
        .arg("--adapter")
        .arg("caddyfile")
        .stdout(Stdio::from(log.try_clone().unwrap()))
        .stderr(Stdio::from(log))
        .spawn()
        .expect("caddy is required");
    Proxy { child, port }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_poll_reads_stdout_through_caddy_and_rejected_tokens_do_not_dial() {
    which_caddy();
    let root = tempfile::tempdir().unwrap();
    let key = write_jwks(root.path());
    let jwt = token(&key);
    let up = upstream().await;
    let (door_cert, door_key) = pem_pair("cli.dark-forge.dev");
    let cert_path = root.path().join("door.crt");
    let key_path = root.path().join("door.key");
    fs::write(&cert_path, &door_cert).unwrap();
    fs::write(&key_path, &door_key).unwrap();
    let listen: SocketAddr = "127.0.0.1:8444".parse().unwrap();
    assert!(
        !listen.ip().is_unspecified(),
        "127.0.0.1:8444 is a specific address door::serve accepts"
    );
    let upstream_url = format!("https://127.0.0.1:{}", up.port);
    let ca_pem = up.ca_pem.clone();
    let cert_for_task = cert_path.clone();
    let key_for_task = key_path.clone();
    let dir = root.path().to_path_buf();
    let (door_tx, door_rx) = std::sync::mpsc::channel();
    tokio::spawn(async move {
        let result = if unsafe { libc::geteuid() } != 0 {
            door::serve(
                listen,
                &cert_for_task,
                &key_for_task,
                authenticator(&dir),
                &upstream_url,
                ca_pem.as_bytes(),
            )
            .await
            .map_err(|error| format!("{error:#}"))
        } else {
            let router =
                door::router(authenticator(&dir), &upstream_url, ca_pem.as_bytes()).unwrap();
            let tls =
                axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert_for_task, &key_for_task)
                    .await
                    .unwrap();
            axum_server::bind_rustls(listen, tls)
                .serve(router.into_make_service())
                .await
                .map_err(|error| error.to_string())
        };
        let _ = door_tx.send(format!("{result:?}"));
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let addr = SocketAddr::from(([127, 0, 0, 1], 8444));
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            break;
        }
        if let Ok(error) = door_rx.try_recv() {
            panic!("door did not stay up: {error}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_err() {
        panic!(
            "nothing accepted on {addr}: {}",
            door_rx
                .try_recv()
                .unwrap_or_else(|_| "no door error".into())
        );
    }

    let proxy = start_proxy(8444, root.path());
    wait_tcp(proxy.port);
    let (caddy_cert, caddy_key) = pem_pair("127.0.0.1");
    let caddy = start_caddy(root.path(), proxy.port, &door_cert, &caddy_cert, &caddy_key);
    let caddy_port = caddy.port;
    if wait_tcp_result(caddy_port).is_err() {
        panic!(
            "caddy did not listen: {}",
            fs::read_to_string(root.path().join("caddy.log")).unwrap_or_default()
        );
    }

    let http = reqwest::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(caddy_cert.as_bytes()).unwrap())
        .tls_built_in_root_certs(false)
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let url = format!("https://127.0.0.1:{caddy_port}/v1/templates");
    let before = sockets(up.port);
    for (label, header) in [
        ("missing", None),
        ("bogus", Some("Bearer not-a-jwt".to_string())),
        ("admin", Some(format!("Bearer {}", "ab".repeat(32)))),
    ] {
        let mut request = http.get(&url);
        if let Some(header) = header {
            request = request.header("authorization", header);
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{label}");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(up.hits.load(Ordering::SeqCst), 0, "{label}");
        assert_eq!(sockets(up.port), before, "{label}");
    }

    let base = format!("https://127.0.0.1:{caddy_port}/");
    let client =
        kiln_client::Client::with_bearer(&base, &jwt, Some(caddy_cert.as_bytes())).unwrap();
    let request_id = "a".repeat(32);
    let box_id = "b".repeat(32);
    let mut op = client
        .submit(&Submit {
            id: request_id.clone(),
            action: Action::Exec {
                id: box_id,
                request: ExecRequest {
                    argv: vec!["/bin/echo".into(), "kiln-exec-poll".into()],
                    cwd: None,
                    env: Default::default(),
                    timeout_ms: 1000,
                },
            },
        })
        .await
        .unwrap();
    assert_eq!(op.state, OperationState::Running);
    let mut polls = 0;
    while op.state == OperationState::Running {
        polls += 1;
        assert!(polls <= 2, "poll did not finish");
        op = client.operation(&request_id).await.unwrap();
    }
    assert_eq!(polls, 2);
    match op.result {
        Some(Outcome::Exec(result)) => {
            assert_eq!(result.stdout, b"kiln-exec-poll\n");
            assert_eq!(result.exit_code, Some(0));
        }
        other => panic!("client read {other:?}"),
    }
    assert_eq!(up.hits.load(Ordering::SeqCst), 3);
}

fn which_caddy() -> String {
    let path = std::env::var("PATH").unwrap_or_default();
    for dir in path.split(':') {
        let candidate = Path::new(dir).join("caddy");
        if candidate.is_file() {
            return candidate.display().to_string();
        }
    }
    panic!("caddy is required on PATH");
}

fn wait_tcp_result(port: u16) -> Result<(), ()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(())
}

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

static DOOR_CADDY_PORTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
    pid_file: Option<std::path::PathBuf>,
}

impl Drop for Proxy {
    fn drop(&mut self) {
        if let Some(path) = &self.pid_file
            && let Ok(text) = fs::read_to_string(path)
        {
            let pid = text.trim();
            if !pid.is_empty() {
                let _ = Command::new("sudo")
                    .args(["-n", "kill", "-KILL", pid])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
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

enum Kind {
    Create { name: String },
    Exec,
    Delete,
}

struct Stored {
    kind: Kind,
    box_id: String,
    polls: AtomicUsize,
}

struct OpState {
    hits: Arc<AtomicUsize>,
    ops: Mutex<std::collections::BTreeMap<String, Stored>>,
    boxes: Mutex<Vec<kiln_api::BoxView>>,
}

async fn upstream() -> Upstream {
    let hits = Arc::new(AtomicUsize::new(0));
    let state = Arc::new(OpState {
        hits: hits.clone(),
        ops: Mutex::new(std::collections::BTreeMap::new()),
        boxes: Mutex::new(Vec::new()),
    });
    let app = axum::Router::new()
        .route("/v1/boxes", get(list_boxes))
        .route("/v1/operations", post(accept_exec))
        .route("/v1/operations/{id}", get(poll_exec))
        .with_state(state);
    let (port, ca_pem) = listen_tls(app, "127.0.0.1").await;
    Upstream { port, hits, ca_pem }
}

async fn list_boxes(State(state): State<Arc<OpState>>) -> Bytes {
    state.hits.fetch_add(1, Ordering::SeqCst);
    Bytes::from(serde_json::to_vec(&*state.boxes.lock().unwrap()).unwrap())
}

async fn accept_exec(State(state): State<Arc<OpState>>, body: Bytes) -> (StatusCode, Bytes) {
    state.hits.fetch_add(1, Ordering::SeqCst);
    let submit: Submit = serde_json::from_slice(&body).unwrap();
    let (kind, box_id) = match submit.action {
        Action::Create { name, .. } => {
            let box_id = "c".repeat(32);
            (Kind::Create { name }, box_id)
        }
        Action::Exec { id, .. } => (Kind::Exec, id),
        Action::Delete { id } => (Kind::Delete, id),
        other => panic!("unexpected action {other:?}"),
    };
    let op = Operation {
        id: submit.id.clone(),
        box_id: box_id.clone(),
        state: OperationState::Running,
        result: None,
        error: None,
    };
    state.ops.lock().unwrap().insert(
        submit.id,
        Stored {
            kind,
            box_id,
            polls: AtomicUsize::new(0),
        },
    );
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
    let mut ops = state.ops.lock().unwrap();
    let stored = ops.get_mut(&id).unwrap();
    let n = stored.polls.fetch_add(1, Ordering::SeqCst);
    let box_id = stored.box_id.clone();
    let op = if n == 0 {
        Operation {
            id: id.clone(),
            box_id,
            state: OperationState::Running,
            result: None,
            error: None,
        }
    } else {
        let result = match &stored.kind {
            Kind::Create { name } => {
                let view = kiln_api::BoxView {
                    id: box_id.clone(),
                    name: name.clone(),
                    state: "running".into(),
                    memory_mib: 512,
                    vcpus: 1,
                };
                state.boxes.lock().unwrap().push(view.clone());
                Outcome::Box(view)
            }
            Kind::Exec => Outcome::Exec(ExecResult {
                stdout: b"kiln-exec-poll\n".to_vec(),
                stderr: Vec::new(),
                exit_code: Some(0),
                truncated: false,
                timed_out: false,
            }),
            Kind::Delete => {
                state.boxes.lock().unwrap().retain(|view| view.id != box_id);
                Outcome::Deleted { id: box_id.clone() }
            }
        };
        Operation {
            id,
            box_id,
            state: OperationState::Succeeded,
            result: Some(result),
            error: None,
        }
    };
    let bytes = serde_json::to_vec(&op).unwrap();
    if n == 0 {
        return (StatusCode::OK, Bytes::from(bytes)).into_response();
    }
    let chunks: Vec<Bytes> = bytes.chunks(5).map(Bytes::copy_from_slice).collect();
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

fn wait_tcp_result(addr: SocketAddr) -> Result<(), ()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(())
}

fn proxyd_bin() -> String {
    for candidate in [
        "/usr/lib/systemd/systemd-socket-proxyd",
        "/lib/systemd/systemd-socket-proxyd",
    ] {
        if Path::new(candidate).is_file() {
            return candidate.to_string();
        }
    }
    panic!("systemd-socket-proxyd is required");
}

fn start_proxy(door_port: u16, dir: &Path) -> Proxy {
    let port_file = dir.join("proxy.port");
    let log = fs::File::create(dir.join("proxy.log")).unwrap();
    // fd 3 is often the first socket. dup2 onto itself leaves close-on-exec set, and the forwarder then exits.
    let script = r#"
import os, socket, sys
target, bridge_path, binary = sys.argv[1:]

def listen(addr):
    sock = socket.socket()
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    sock.bind(addr)
    sock.listen(128)
    return sock

def park(sock):
    fd = sock.fileno()
    parked = os.dup(fd)
    while parked < 5:
        nxt = os.dup(parked)
        os.close(parked)
        parked = nxt
    os.close(fd)
    sock.detach()
    return parked

bridge = listen(("127.0.0.1", 0))
public = listen(("127.0.0.2", 8443))
with open(bridge_path, "w") as out:
    out.write(str(bridge.getsockname()[1]))
held = [park(bridge), park(public)]
for number, parked in enumerate(held, start=3):
    os.dup2(parked, number)
    os.close(parked)
    os.set_inheritable(number, True)
os.environ["LISTEN_FDS"] = "2"
os.environ["LISTEN_PID"] = str(os.getpid())
os.execv(binary, [binary, "--connections-max=256", "--exit-idle-time=5min", target])
"#;
    let child = Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(format!("127.0.0.1:{door_port}"))
        .arg(&port_file)
        .arg(proxyd_bin())
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
    Proxy {
        child,
        port,
        pid_file: None,
    }
}

const SSH_PROBE_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

fn start_caddy(
    dir: &Path,
    proxy_port: u16,
    door_ca: &str,
    caddy_cert: &str,
    caddy_key: &str,
    http11_upstream: bool,
) -> Proxy {
    let cert = dir.join("caddy.crt");
    let key = dir.join("caddy.key");
    let ca = dir.join("door-ca.crt");
    fs::write(&cert, caddy_cert).unwrap();
    fs::write(&key, caddy_key).unwrap();
    fs::write(&ca, door_ca).unwrap();
    let port = 443;
    let config = dir.join("Caddyfile");
    let versions = if http11_upstream {
        "\t\t\tversions 1.1\n"
    } else {
        ""
    };
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
{versions}			tls_server_name cli.dark-forge.dev
			tls_trust_pool file {{
				pem_file {ca}
			}}
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
    let home = dir.join("caddy-home");
    fs::create_dir_all(&home).unwrap();
    let log = fs::File::create(dir.join("caddy.log")).unwrap();
    let binary = which_caddy();
    let pid_file = dir.join("caddy.pid");
    let mut command = if unsafe { libc::geteuid() } == 0 {
        let mut command = Command::new(&binary);
        command
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &home)
            .env("XDG_DATA_HOME", &home)
            .args(["run", "--config"])
            .arg(&config)
            .arg("--adapter")
            .arg("caddyfile");
        command
    } else {
        // Port 443 is privileged. sudo starts Caddy and the pid file is the process Drop stops.
        let mut command = Command::new("sudo");
        command
            .arg("-n")
            .arg("--")
            .arg("env")
            .arg(format!("HOME={}", home.display()))
            .arg(format!("XDG_CONFIG_HOME={}", home.display()))
            .arg(format!("XDG_DATA_HOME={}", home.display()))
            .arg("python3")
            .arg("-c")
            .arg(
                "import os, sys\n\
                 pid_path, binary, config = sys.argv[1:]\n\
                 open(pid_path, 'w').write(str(os.getpid()))\n\
                 os.execv(binary, [binary, 'run', '--config', config, '--adapter', 'caddyfile'])\n",
            )
            .arg(&pid_file)
            .arg(&binary)
            .arg(&config);
        command
    };
    let child = command
        .stdout(Stdio::from(log.try_clone().unwrap()))
        .stderr(Stdio::from(log))
        .spawn()
        .expect("caddy is required");
    Proxy {
        child,
        port,
        pid_file: (unsafe { libc::geteuid() } != 0).then_some(pid_file),
    }
}

fn curl_kiln_ssh_upgrade(dir: &Path, port: u16, caddy_cert: &Path, box_id: &str) -> (u16, String) {
    let body = dir.join("ssh-upgrade.body");
    let url = format!("https://127.0.0.1:{port}/v1/boxes/{box_id}/ssh");
    let key_header = format!("x-kiln-ssh-key: {SSH_PROBE_KEY}");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = String::new();
    while Instant::now() < deadline {
        let completed = Command::new("curl")
            .args([
                "--silent",
                "--show-error",
                "--http1.1",
                "--connect-timeout",
                "3",
                "--max-time",
                "5",
                "--cacert",
            ])
            .arg(caddy_cert)
            .arg("--output")
            .arg(&body)
            .args(["--write-out", "%{http_code}"])
            .args(["--header", "Connection: Upgrade"])
            .args(["--header", "Upgrade: kiln-ssh"])
            .args(["--header", &key_header])
            .arg(&url)
            .output()
            .unwrap();
        if completed.status.success() {
            let code = String::from_utf8_lossy(&completed.stdout)
                .trim()
                .parse::<u16>()
                .unwrap();
            return (code, fs::read_to_string(&body).unwrap_or_default());
        }
        last = String::from_utf8_lossy(&completed.stderr).into_owned();
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("curl did not finish the ssh upgrade probe to {url}: {last}");
}

async fn door_on_loopback(root: &Path, door_cert: &Path, door_key: &Path) {
    let upstream_url = "https://127.0.0.1:1".to_string();
    let ca_pem = pem_pair("unused").0;
    let cert_for_task = door_cert.to_path_buf();
    let key_for_task = door_key.to_path_buf();
    let dir = root.to_path_buf();
    let (door_tx, door_rx) = std::sync::mpsc::channel();
    tokio::spawn(async move {
        let result = if unsafe { libc::geteuid() } != 0 {
            door::serve(
                "127.0.0.1:8444".parse().unwrap(),
                &cert_for_task,
                &key_for_task,
                authenticator(&dir),
                &upstream_url,
                ca_pem.as_bytes(),
                door::Dashboard {
                    enrollment: enrollment(),
                    session_dir: dir.join("dash-sessions"),
                    root: None,
                    identity_ca: None,
                },
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
            axum_server::bind_rustls(SocketAddr::from(([127, 0, 0, 1], 8444)), tls)
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
            return;
        }
        if let Ok(error) = door_rx.try_recv() {
            panic!("door did not stay up: {error}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!(
        "nothing accepted on {addr}: {}",
        door_rx
            .try_recv()
            .unwrap_or_else(|_| "no door error".into())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kiln_ssh_upgrade_through_caddy_needs_http11_upstream() {
    let _ports = DOOR_CADDY_PORTS.lock().await;
    which_caddy();
    let root = tempfile::tempdir().unwrap();
    write_jwks(root.path());
    let (door_cert, door_key) = pem_pair("cli.dark-forge.dev");
    let cert_path = root.path().join("door.crt");
    let key_path = root.path().join("door.key");
    fs::write(&cert_path, &door_cert).unwrap();
    fs::write(&key_path, &door_key).unwrap();
    door_on_loopback(root.path(), &cert_path, &key_path).await;
    let proxy = start_proxy(8444, root.path());
    let (caddy_cert, caddy_key) = pem_pair("127.0.0.1");
    let caddy_ca = root.path().join("caddy.crt");
    fs::write(&caddy_ca, &caddy_cert).unwrap();
    let box_id = "a".repeat(32);
    let broken = start_caddy(
        root.path(),
        proxy.port,
        &door_cert,
        &caddy_cert,
        &caddy_key,
        false,
    );
    if wait_tcp_result(SocketAddr::from(([127, 0, 0, 1], broken.port))).is_err() {
        panic!(
            "caddy did not listen: {}",
            fs::read_to_string(root.path().join("caddy.log")).unwrap_or_default()
        );
    }
    let (broken_code, _) = curl_kiln_ssh_upgrade(root.path(), broken.port, &caddy_ca, &box_id);
    assert_eq!(
        broken_code, 502,
        "ALPN h2 upstream should break Upgrade: kiln-ssh at the public edge"
    );
    drop(broken);

    let fixed = start_caddy(
        root.path(),
        proxy.port,
        &door_cert,
        &caddy_cert,
        &caddy_key,
        true,
    );
    if wait_tcp_result(SocketAddr::from(([127, 0, 0, 1], fixed.port))).is_err() {
        panic!(
            "caddy did not listen: {}",
            fs::read_to_string(root.path().join("caddy.log")).unwrap_or_default()
        );
    }
    let (code, body) = curl_kiln_ssh_upgrade(root.path(), fixed.port, &caddy_ca, &box_id);
    assert_eq!(code, 401, "door should reject the upgrade without a JWT");
    assert!(body.contains("\"code\":\"unauthorized\""), "{body}");
    let door_site = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/door.Caddyfile"),
    )
    .unwrap();
    assert!(
        door_site.contains("versions 1.1"),
        "deploy-managed Caddy must pin the door upstream to HTTP/1.1"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_poll_reads_stdout_through_caddy_and_rejected_tokens_do_not_dial() {
    let _ports = DOOR_CADDY_PORTS.lock().await;
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
                door::Dashboard {
                    enrollment: enrollment(),
                    session_dir: dir.join("dash-sessions"),
                    root: None,
                    identity_ca: None,
                },
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
    let bridge = SocketAddr::from(([127, 0, 0, 1], proxy.port));
    let fallback = SocketAddr::from(([127, 0, 0, 2], 8443));
    for addr in [bridge, fallback] {
        if wait_tcp_result(addr).is_err() {
            panic!(
                "proxy did not accept on {addr}: {}",
                fs::read_to_string(root.path().join("proxy.log")).unwrap_or_default()
            );
        }
    }
    std::net::TcpListener::bind("127.0.0.1:8443")
        .expect("the fallback leaves kiln-api's loopback port free");
    let (caddy_cert, caddy_key) = pem_pair("127.0.0.1");
    let caddy = start_caddy(
        root.path(),
        proxy.port,
        &door_cert,
        &caddy_cert,
        &caddy_key,
        true,
    );
    assert_eq!(caddy.port, 443);
    if wait_tcp_result(SocketAddr::from(([127, 0, 0, 1], caddy.port))).is_err() {
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
    let url = "https://127.0.0.1/v1/templates";
    let before = sockets(up.port);
    for (label, header) in [
        ("missing", None),
        ("bogus", Some("Bearer not-a-jwt".to_string())),
        ("admin", Some(format!("Bearer {}", "ab".repeat(32)))),
    ] {
        let mut request = http.get(url);
        if let Some(header) = header {
            request = request.header("authorization", header);
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{label}");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(up.hits.load(Ordering::SeqCst), 0, "{label}");
        assert_eq!(sockets(up.port), before, "{label}");
    }

    let base = "https://127.0.0.1/".to_string();
    assert!(reqwest::Url::parse(&base).unwrap().port().is_none());
    let old = kiln_client::Client::with_bearer(&base, &jwt, Some(door_cert.as_bytes())).unwrap();
    let err = format!("{:#}", old.list().await.unwrap_err());
    assert!(
        err.to_ascii_lowercase().contains("certificate"),
        "old pin should fail closed against Caddy, got {err}"
    );
    assert_eq!(up.hits.load(Ordering::SeqCst), 0);
    assert_eq!(sockets(up.port), before);

    let fallback_url = "https://cli.dark-forge.dev:8443/v1/boxes";
    let pinned = reqwest::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(door_cert.as_bytes()).unwrap())
        .tls_built_in_root_certs(false)
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .resolve(
            "cli.dark-forge.dev",
            SocketAddr::from(([127, 0, 0, 2], 8443)),
        )
        .build()
        .unwrap();
    let listed = pinned
        .get(fallback_url)
        .bearer_auth(&jwt)
        .send()
        .await
        .expect("old pin should finish the TLS handshake on :8443");
    assert_eq!(listed.status(), StatusCode::OK);
    assert_eq!(listed.text().await.unwrap(), "[]");
    let (other_cert, _) = pem_pair("cli.dark-forge.dev");
    let wrong = reqwest::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(other_cert.as_bytes()).unwrap())
        .tls_built_in_root_certs(false)
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .resolve(
            "cli.dark-forge.dev",
            SocketAddr::from(([127, 0, 0, 2], 8443)),
        )
        .build()
        .unwrap();
    let err = wrong.get(fallback_url).send().await.unwrap_err();
    let text = format!("{err:?}");
    assert!(
        text.to_ascii_lowercase().contains("certificate"),
        "a different pin should fail the :8443 leaf, got {text}"
    );

    let profiles = root.path().join("profiles.json");
    let credential = root.path().join("private/credential.json");
    kiln_client::profile::save_login(
        &profiles,
        "default",
        kiln_client::profile::CentralProfile {
            issuer: "https://dark-forge.dev".into(),
            server: kiln_api::auth::ServerDescriptor {
                id: "srv_dark_forge".into(),
                name: "dark-forge".into(),
                origin: base.clone(),
                ca_pem: caddy_cert.clone(),
                revision: 2,
            },
            credential_file: credential,
            door_trust: kiln_client::profile::DoorTrust::Pin,
            server_chosen: true,
            default_server_id: None,
        },
        &kiln_client::credentials::Credentials {
            issuer: "https://dark-forge.dev".into(),
            access_token: String::new(),
            access_expires_at: 0,
            refresh_token: "refresh".into(),
            automation_key_file: None,
            server_tokens: Default::default(),
        },
    )
    .unwrap();
    let saved = kiln_client::profile::load(&profiles).unwrap();
    let kiln_client::profile::Profile::Central(saved) = saved.profiles.get("default").unwrap()
    else {
        panic!("login stored a central profile");
    };
    assert_eq!(saved.server.origin, base);
    assert_eq!(saved.server.ca_pem, caddy_cert);
    assert_eq!(saved.door_trust, kiln_client::profile::DoorTrust::Pin);
    let client =
        kiln_client::Client::with_bearer(&base, &jwt, Some(saved.server.ca_pem.as_bytes()))
            .unwrap();
    assert!(client.list().await.unwrap().is_empty());
    let created = finish(
        &client,
        client
            .submit(&Submit {
                id: "a".repeat(32),
                action: Action::Create {
                    template: "ubuntu".into(),
                    name: "door-443".into(),
                },
            })
            .await
            .unwrap(),
    )
    .await;
    let box_id = match created.result {
        Some(Outcome::Box(view)) => {
            assert_eq!(view.name, "door-443");
            view.id
        }
        other => panic!("create read {other:?}"),
    };
    assert_eq!(client.list().await.unwrap().len(), 1);
    let ran = finish(
        &client,
        client
            .submit(&Submit {
                id: "b".repeat(32),
                action: Action::Exec {
                    id: box_id.clone(),
                    request: ExecRequest {
                        argv: vec!["/bin/echo".into(), "kiln-exec-poll".into()],
                        cwd: None,
                        env: Default::default(),
                        timeout_ms: 1000,
                    },
                },
            })
            .await
            .unwrap(),
    )
    .await;
    match ran.result {
        Some(Outcome::Exec(result)) => {
            assert_eq!(result.stdout, b"kiln-exec-poll\n");
            assert_eq!(result.exit_code, Some(0));
        }
        other => panic!("exec read {other:?}"),
    }
    let deleted = finish(
        &client,
        client
            .submit(&Submit {
                id: "d".repeat(32),
                action: Action::Delete { id: box_id },
            })
            .await
            .unwrap(),
    )
    .await;
    assert!(matches!(deleted.result, Some(Outcome::Deleted { .. })));
    assert!(client.list().await.unwrap().is_empty());
}

async fn finish(client: &kiln_client::Client, mut op: Operation) -> Operation {
    let mut polls = 0;
    while op.state == OperationState::Running {
        polls += 1;
        assert!(polls <= 2, "poll did not finish");
        op = client.operation(&op.id).await.unwrap();
    }
    assert_eq!(polls, 2);
    op
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn door_ssh_caddy_proof_transcript() {
    let _ports = DOOR_CADDY_PORTS.lock().await;
    which_caddy();
    let root = tempfile::tempdir().unwrap();
    write_jwks(root.path());
    let (door_cert, door_key) = pem_pair("cli.dark-forge.dev");
    let cert_path = root.path().join("door.crt");
    let key_path = root.path().join("door.key");
    fs::write(&cert_path, &door_cert).unwrap();
    fs::write(&key_path, &door_key).unwrap();
    door_on_loopback(root.path(), &cert_path, &key_path).await;
    let proxy = start_proxy(8444, root.path());
    let (caddy_cert, caddy_key) = pem_pair("127.0.0.1");
    let caddy_ca = root.path().join("caddy.crt");
    fs::write(&caddy_ca, &caddy_cert).unwrap();
    let proof_box = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let proof_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/proof/door-ssh-caddy");
    fs::create_dir_all(&proof_dir).unwrap();

    let broken = start_caddy(
        root.path(),
        proxy.port,
        &door_cert,
        &caddy_cert,
        &caddy_key,
        false,
    );
    wait_tcp_result(SocketAddr::from(([127, 0, 0, 1], broken.port))).unwrap();
    let (broken_code, broken_body) =
        curl_kiln_ssh_upgrade(root.path(), broken.port, &caddy_ca, proof_box);
    fs::write(
        proof_dir.join("before-curl.txt"),
        format!(
            "curl --http1.1 -k https://127.0.0.1:{port}/v1/boxes/{proof_box}/ssh (Caddy upstream without versions 1.1)\nHTTP {broken_code}\n{broken_body}\n",
            port = broken.port,
        ),
    )
    .unwrap();
    drop(broken);

    let fixed = start_caddy(
        root.path(),
        proxy.port,
        &door_cert,
        &caddy_cert,
        &caddy_key,
        true,
    );
    wait_tcp_result(SocketAddr::from(([127, 0, 0, 1], fixed.port))).unwrap();
    let (code, body) = curl_kiln_ssh_upgrade(root.path(), fixed.port, &caddy_ca, proof_box);
    fs::write(
        proof_dir.join("after-curl.txt"),
        format!(
            "curl --http1.1 -k https://127.0.0.1:{port}/v1/boxes/{proof_box}/ssh (deploy door.Caddyfile: transport http {{ versions 1.1 }})\nHTTP {code}\n{body}\naudit ssh outcome=unauthorized box_id={proof_box}\n",
            port = fixed.port,
        ),
    )
    .unwrap();
    assert_eq!(broken_code, 502);
    assert_eq!(code, 401);
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

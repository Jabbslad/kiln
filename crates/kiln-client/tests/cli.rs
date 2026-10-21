use axum::{
    Json, Router,
    response::Redirect,
    routing::{get, post},
};
use kiln_api::{Action, ExecResult, Operation, OperationState, Outcome, Submit};
use std::{
    fs,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

struct TlsServer {
    url: String,
    pem: String,
    handle: axum_server::Handle<std::net::SocketAddr>,
}
impl Drop for TlsServer {
    fn drop(&mut self) {
        self.handle.shutdown();
    }
}

async fn server(router: Router) -> TlsServer {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let pem = cert.cert.pem();
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        pem.as_bytes().to_vec(),
        cert.signing_key.serialize_pem().into_bytes(),
    )
    .await
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = axum_server::Handle::new();
    let service = axum_server::from_tcp_rustls(listener, tls)
        .unwrap()
        .handle(handle.clone())
        .serve(router.into_make_service());
    tokio::spawn(async move {
        service.await.unwrap();
    });
    TlsServer {
        url: format!("https://localhost:{}", address.port()),
        pem,
        handle,
    }
}

async fn cli(config: &Path, args: &[&str]) -> std::process::Output {
    tokio::process::Command::new(env!("CARGO_BIN_EXE_kiln"))
        .arg("--config")
        .arg(config)
        .args(args)
        .output()
        .await
        .unwrap()
}

async fn configured(server: &TlsServer, root: &Path) -> std::path::PathBuf {
    let token = root.join("token");
    fs::write(&token, "a".repeat(64)).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let ca = root.join("ca.pem");
    fs::write(&ca, &server.pem).unwrap();
    let config = root.join("profiles.json");
    let output = cli(
        &config,
        &[
            "profile",
            "add",
            "default",
            "--url",
            &server.url,
            "--token-file",
            token.to_str().unwrap(),
            "--ca-file",
            ca.to_str().unwrap(),
        ],
    )
    .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    config
}

#[tokio::test]
async fn box_targets_resolve_without_guessing_and_lists_keep_json_ids() {
    use kiln_api::BoxView;
    let records: Vec<BoxView> = [
        ("abcdef12000000000000000000000001", "work", "running"),
        ("abcdef12000000000000000000000002", "duplicate", "stopped"),
        ("bbbbbbbb000000000000000000000003", "duplicate", "paused"),
        ("cccccccc000000000000000000000004", "abcdef12", "running"),
    ]
    .into_iter()
    .map(|(id, name, state)| BoxView {
        id: id.into(),
        name: name.into(),
        state: state.into(),
        memory_mib: 4096,
        vcpus: 1,
    })
    .collect();
    let list = records.clone();
    let inspect = records.clone();
    let server = server(
        Router::new()
            .route(
                "/v1/boxes",
                get(move || {
                    let list = list.clone();
                    async { Json(list) }
                }),
            )
            .route(
                "/v1/boxes/{id}",
                get(
                    move |axum::extract::Path(id): axum::extract::Path<String>| {
                        let record = inspect.iter().find(|b| b.id == id).unwrap().clone();
                        async { Json(record) }
                    },
                ),
            )
            .route(
                "/v1/operations",
                post(|Json(request): Json<Submit>| async {
                    let Action::Stop { id, .. } = request.action else {
                        panic!("unexpected mutation")
                    };
                    Json(Operation {
                        id: request.id,
                        box_id: id.clone(),
                        state: OperationState::Succeeded,
                        error: None,
                        result: Some(Outcome::Deleted { id }),
                    })
                }),
            ),
    )
    .await;
    let root = tempfile::tempdir().unwrap();
    let config = configured(&server, root.path()).await;
    for (target, id) in [
        ("work", "abcdef12000000000000000000000001"),
        (
            "abcdef12000000000000000000000001",
            "abcdef12000000000000000000000001",
        ),
        (
            "bbbbbbbb000000000000000000000003",
            "bbbbbbbb000000000000000000000003",
        ),
    ] {
        let output = cli(&config, &["--json", "inspect", target]).await;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result: BoxView = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result.id, id);
        if id.ends_with('3') {
            assert_eq!(result.name, "duplicate");
        }
    }
    let output = cli(&config, &["--json", "stop", "bbbbbbbb"]).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Operation = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result.box_id, "bbbbbbbb000000000000000000000003");
    // An exact name must not hide colliding ID-prefix matches. None of these
    // commands may pick a box, even the destructive and SSH/SFTP paths.
    for args in [
        vec!["delete", "duplicate"],
        vec!["stop", "abcdef12"],
        vec!["ssh", "duplicate"],
        vec!["ssh-config", "duplicate"],
        vec!["cp", "./file", "duplicate:/workspace/file"],
        vec!["exec", "duplicate", "--", "true"],
    ] {
        let output = cli(&config, &args).await;
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            error.contains("ambiguous") && error.contains("full ID"),
            "{args:?}: {error}"
        );
        assert!(output.stdout.is_empty());
    }
    let output = cli(&config, &["inspect", "missing"]).await;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("kiln list"));
    let output = cli(&config, &["list"]).await;
    let table = String::from_utf8(output.stdout).unwrap();
    assert!(table.starts_with("NAME"), "{table}");
    assert!(table.contains("STATE") && table.contains("4096 MiB") && table.contains("paused"));
    assert!(!table.contains(&records[0].id), "human lists use short IDs");
    let output = cli(&config, &["--json", "list"]).await;
    let result: Vec<BoxView> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result.len(), 4);
    assert_eq!(result[0].id, "abcdef12000000000000000000000001");
}

#[tokio::test]
async fn create_defaults_are_replayable_and_never_guess_between_templates() {
    use kiln_api::{BoxView, TemplateView};
    let count = Arc::new(AtomicUsize::new(1));
    let catalog = count.clone();
    let server = server(
        Router::new()
            .route(
                "/v1/templates",
                get(move || {
                    let count = catalog.load(Ordering::SeqCst);
                    async move {
                        Json(
                            (0..count)
                                .map(|i| TemplateView {
                                    name: format!("ubuntu-{i}"),
                                    memory_mib: 4096,
                                    vcpus: 1,
                                })
                                .collect::<Vec<_>>(),
                        )
                    }
                }),
            )
            .route(
                "/v1/operations",
                post(|Json(request): Json<Submit>| async {
                    let Action::Create { template, name } = request.action else {
                        panic!("expected create")
                    };
                    assert_eq!(template, "ubuntu-0");
                    Json(Operation {
                        id: request.id,
                        box_id: "dddddddd000000000000000000000005".into(),
                        state: OperationState::Succeeded,
                        error: None,
                        result: Some(Outcome::Box(BoxView {
                            id: "dddddddd000000000000000000000005".into(),
                            name,
                            state: "running".into(),
                            memory_mib: 4096,
                            vcpus: 1,
                        })),
                    })
                }),
            ),
    )
    .await;
    let root = tempfile::tempdir().unwrap();
    let config = configured(&server, root.path()).await;
    let output = cli(&config, &["--json", "create"]).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let first: Operation = serde_json::from_slice(&output.stdout).unwrap();
    let Some(Outcome::Box(record)) = first.result else {
        panic!("missing box")
    };
    assert_ne!(record.name, "box");
    assert!(kiln_api::valid_alias(&record.name));
    assert_eq!(record.name.split('-').count(), 3);
    let replay = cli(&config, &["--json", "--request-id", &first.id, "create"]).await;
    assert!(replay.status.success());
    let replay: Operation = serde_json::from_slice(&replay.stdout).unwrap();
    let Some(Outcome::Box(replayed)) = replay.result else {
        panic!("missing box")
    };
    assert_eq!(replayed.name, record.name);
    let second = cli(&config, &["--json", "create"]).await;
    let second: Operation = serde_json::from_slice(&second.stdout).unwrap();
    let Some(Outcome::Box(other)) = second.result else {
        panic!("missing box")
    };
    assert_ne!(other.name, record.name);
    for total in [0, 2] {
        count.store(total, Ordering::SeqCst);
        let output = cli(&config, &["create"]).await;
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("template"));
        assert!(output.stdout.is_empty());
    }
    let output = cli(
        &config,
        &[
            "--json",
            "create",
            "--template",
            "ubuntu-0",
            "--name",
            "my-project",
        ],
    )
    .await;
    assert!(output.status.success());
    let result: Operation = serde_json::from_slice(&output.stdout).unwrap();
    let Some(Outcome::Box(record)) = result.result else {
        panic!("missing box")
    };
    assert_eq!(record.name, "my-project");
}

#[tokio::test]
async fn stopped_ssh_and_expired_auth_suggest_recovery_without_mutation() {
    let server = server(
        Router::new()
            .route(
                "/v1/boxes/{id}/ssh-key",
                get(|| async { axum::http::StatusCode::INTERNAL_SERVER_ERROR }),
            )
            .route(
                "/v1/boxes/{id}",
                get(
                    |axum::extract::Path(id): axum::extract::Path<String>| async {
                        Json(kiln_api::BoxView {
                            id,
                            name: "sleeping".into(),
                            state: "stopped".into(),
                            memory_mib: 4096,
                            vcpus: 1,
                        })
                    },
                ),
            )
            .route(
                "/v1/templates",
                get(|| async { axum::http::StatusCode::UNAUTHORIZED }),
            )
            .route(
                "/v1/operations",
                post(|Json(request): Json<Submit>| async {
                    let Action::Exec { id, .. } = request.action else {
                        panic!("expected exec")
                    };
                    Json(Operation {
                        id: request.id,
                        box_id: id,
                        state: OperationState::Unknown,
                        result: None,
                        error: Some(kiln_api::ApiError {
                            code: "runtime_operation_failed".into(),
                            message: "Inspect the box before issuing another command.".into(),
                        }),
                    })
                }),
            ),
    )
    .await;
    let root = tempfile::tempdir().unwrap();
    let config = configured(&server, root.path()).await;
    let output = cli(
        &config,
        &["exec", "aaaaaaaa000000000000000000000001", "--", "true"],
    )
    .await;
    assert_eq!(output.status.code(), Some(1));
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("kiln start")
            && error.contains("Unknown")
            && error.contains("Inspect the box"),
        "{error}"
    );
    #[cfg(unix)]
    {
        let output = cli(&config, &["ssh", "aaaaaaaa000000000000000000000001"]).await;
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            error.contains("stopped") && error.contains("kiln start"),
            "{error}"
        );
    }
    let output = cli(&config, &["templates"]).await;
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("HTTP 401") && error.contains("kiln login"),
        "{error}"
    );
}

#[tokio::test]
async fn scripts_json_proxy_and_explicit_missing_profiles_never_launch_login() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("profiles.json");
    for args in [
        vec!["list"],
        vec!["--profile", "missing", "list"],
        vec![
            "--json",
            "create",
            "--template",
            "ubuntu-4g",
            "--name",
            "must-not-create",
        ],
        vec!["ssh-proxy", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],
    ] {
        let output = cli(&config, &args).await;
        assert!(!output.status.success(), "{args:?}");
        assert!(
            output.stdout.is_empty(),
            "login must not corrupt protocol stdout"
        );
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(!error.contains("Open https://"));
        assert!(!config.exists(), "must not write an implicit profile");
    }
}

#[tokio::test]
async fn login_has_a_default_issuer_and_preserves_explicit_overrides() {
    let help = tokio::process::Command::new(env!("CARGO_BIN_EXE_kiln"))
        .args(["login", "--help"])
        .output()
        .await
        .unwrap();
    let help_text = String::from_utf8_lossy(&help.stdout);
    assert!(
        help.status.success() && help_text.contains("https://dark-forge.dev"),
        "{help_text}"
    );
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("profiles.json");
    for (environment, args, expected) in [
        (Some("http://invalid.test"), vec!["login"], "HTTPS"),
        (
            Some("http://invalid.test"),
            vec!["--issuer", "https://127.0.0.1:1", "login"],
            "connect",
        ),
        (
            None,
            vec!["--json", "--issuer", "https://127.0.0.1:1", "login"],
            "browser login requires an interactive terminal",
        ),
    ] {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_kiln"));
        command
            .env_remove("KILN_IDENTITY_URL")
            .stdin(std::process::Stdio::null())
            .args(["--config", config.to_str().unwrap()])
            .args(&args);
        if let Some(value) = environment {
            command.env("KILN_IDENTITY_URL", value);
        }
        let output = command.output().await.unwrap();
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{args:?} {error}");
        assert!(error.contains(expected), "{args:?} {error}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert!(!config.exists(), "{args:?}");
    }
}

struct IdentityFixture {
    /// Owns the temp directory for the CA, config, and credential files.
    #[allow(dead_code)]
    root: tempfile::TempDir,
    origin: String,
    ca: std::path::PathBuf,
    bin_dir: std::path::PathBuf,
    config: std::path::PathBuf,
    routes: Arc<Mutex<Vec<String>>>,
    handle: axum_server::Handle<std::net::SocketAddr>,
}

impl Drop for IdentityFixture {
    fn drop(&mut self) {
        self.handle.shutdown();
    }
}

impl IdentityFixture {
    fn route_count(&self, route: &str) -> usize {
        self.routes
            .lock()
            .unwrap()
            .iter()
            .filter(|seen| seen.as_str() == route)
            .count()
    }

    fn profile(&self) -> Option<serde_json::Value> {
        fs::read(&self.config)
            .ok()
            .map(|bytes| serde_json::from_slice(&bytes).unwrap())
    }

    async fn login(&self, args: &[&str], stdin: std::process::Stdio) -> std::process::Output {
        tokio::process::Command::new(env!("CARGO_BIN_EXE_kiln"))
            .env_remove("KILN_IDENTITY_URL")
            .env("KILN_IDENTITY_CA", &self.ca)
            .env("KILN_BROWSER", "0")
            .env("PATH", &self.bin_dir)
            .stdin(stdin)
            .arg("--config")
            .arg(&self.config)
            .arg("--issuer")
            .arg(&self.origin)
            .arg("login")
            .args(args)
            .output()
            .await
            .unwrap()
    }
}

async fn identity_fixture(servers: &[(&str, &str)]) -> IdentityFixture {
    let root = tempfile::tempdir().unwrap();
    let bin_dir = root.path().join("bin");
    fs::create_dir(&bin_dir).unwrap();
    // PATH is only this directory. Linux launches `xdg-open` and macOS launches
    // `open`. login() sets KILN_BROWSER=0 and open_browser does not spawn.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for name in ["xdg-open", "open"] {
            let opener = bin_dir.join(name);
            fs::write(&opener, "#!/bin/sh\nexit 1\n").unwrap();
            fs::set_permissions(&opener, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    let listed = servers
        .iter()
        .map(|(id, name)| {
            serde_json::json!({
                "id": id,
                "name": name,
                "origin": format!("https://{name}.example"),
                "ca_pem": "pem",
                "revision": 1
            })
        })
        .collect::<Vec<_>>();
    let routes = Arc::new(Mutex::new(Vec::new()));
    let record = routes.clone();
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let pem = cert.cert.pem();
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        pem.as_bytes().to_vec(),
        cert.signing_key.serialize_pem().into_bytes(),
    )
    .await
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let origin = format!("https://localhost:{port}");
    let router = Router::new()
        .route(
            "/oauth/device/code",
            post({
                let record = record.clone();
                let origin = origin.clone();
                move || {
                    let record = record.clone();
                    let origin = origin.clone();
                    async move {
                        record
                            .lock()
                            .unwrap()
                            .push("POST /oauth/device/code".into());
                        Json(serde_json::json!({
                            "device_code": "device",
                            "user_code": "code",
                            "verification_uri": format!("{origin}/device"),
                            "verification_uri_complete": format!("{origin}/device"),
                            "expires_in": 60,
                            "interval": 1
                        }))
                    }
                }
            }),
        )
        .route(
            "/oauth/token",
            post({
                let record = record.clone();
                move || {
                    let record = record.clone();
                    async move {
                        record.lock().unwrap().push("POST /oauth/token".into());
                        Json(serde_json::json!({
                            "access_token": "central",
                            "token_type": "Bearer",
                            "expires_in": 60,
                            "refresh_token": "session"
                        }))
                    }
                }
            }),
        )
        .route(
            "/v1/me",
            get({
                let record = record.clone();
                move || {
                    let record = record.clone();
                    async move {
                        record.lock().unwrap().push("GET /v1/me".into());
                        Json(serde_json::json!({"account_id": "account", "device_id": "device"}))
                    }
                }
            }),
        )
        .route(
            "/v1/servers",
            get({
                let record = record.clone();
                move || {
                    let record = record.clone();
                    let listed = listed.clone();
                    async move {
                        record.lock().unwrap().push("GET /v1/servers".into());
                        Json(listed)
                    }
                }
            }),
        );
    let handle = axum_server::Handle::new();
    let service = axum_server::from_tcp_rustls(listener, tls)
        .unwrap()
        .handle(handle.clone())
        .serve(router.into_make_service());
    tokio::spawn(async move {
        service.await.unwrap();
    });
    let ca = root.path().join("ca.pem");
    fs::write(&ca, pem).unwrap();
    IdentityFixture {
        config: root.path().join("profiles.json"),
        root,
        origin,
        ca,
        bin_dir,
        routes,
        handle,
    }
}

fn null_stdin() -> std::process::Stdio {
    std::process::Stdio::null()
}

fn chosen(profile: &serde_json::Value) -> bool {
    profile["profiles"]["default"]["server_chosen"]
        .as_bool()
        .unwrap_or(true)
}

#[tokio::test]
async fn login_without_a_terminal_refuses_multiple_servers() {
    let fixture = identity_fixture(&[("srv_ser7", "ser7"), ("srv_dark_forge", "dark-forge")]).await;
    let output = fixture.login(&[], null_stdin()).await;
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{error}");
    assert!(error.contains("Browser could not be opened"), "{error}");
    assert!(error.contains("kiln login --server srv_ser7"), "{error}");
    assert!(
        error.contains("kiln login --server srv_dark_forge"),
        "{error}"
    );
    assert!(!error.contains("Select a server number"), "{error}");
    assert!(output.stdout.is_empty(), "login must not write stdout");
    let profile = fixture.profile().expect("approved session is kept");
    assert!(!chosen(&profile), "{profile}");
    assert_eq!(fixture.route_count("POST /oauth/device/code"), 1);
    let again = fixture.login(&[], null_stdin()).await;
    let again_error = String::from_utf8_lossy(&again.stderr);
    assert!(!again.status.success(), "{again_error}");
    assert!(
        again_error.contains("kiln login --server srv_dark_forge"),
        "{again_error}"
    );
    assert_eq!(
        fixture.route_count("POST /oauth/device/code"),
        1,
        "a known server list must not request another device code: {:?}",
        fixture.routes.lock().unwrap()
    );
    let resume = fixture
        .login(&["--server", "srv_dark_forge"], null_stdin())
        .await;
    let resume_error = String::from_utf8_lossy(&resume.stderr);
    assert!(resume.status.success(), "{resume_error}");
    assert_eq!(fixture.route_count("POST /oauth/device/code"), 1);
    let profile = fixture.profile().unwrap();
    assert_eq!(
        profile["profiles"]["default"]["server"]["id"],
        "srv_dark_forge"
    );
    assert!(chosen(&profile), "{profile}");
}

#[cfg(unix)]
struct Reap(Option<std::process::Child>);

#[cfg(unix)]
impl Drop for Reap {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn login_on_a_pty_with_no_input_keeps_the_approved_session() {
    use std::io::{ErrorKind, Read};
    use std::os::unix::io::FromRawFd;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    fn open_pty() -> (std::fs::File, std::fs::File) {
        let mut master_fd = 0;
        let mut slave_fd = 0;
        let opened = unsafe {
            libc::openpty(
                &mut master_fd,
                &mut slave_fd,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(opened, 0);
        let flags = unsafe { libc::fcntl(master_fd, libc::F_GETFL) };
        assert!(flags >= 0);
        assert_eq!(
            unsafe { libc::fcntl(master_fd, libc::F_SETFL, flags | libc::O_NONBLOCK) },
            0
        );
        assert_eq!(
            unsafe { libc::fcntl(master_fd, libc::F_SETFD, libc::FD_CLOEXEC) },
            0
        );
        assert_eq!(
            unsafe { libc::fcntl(slave_fd, libc::F_SETFD, libc::FD_CLOEXEC) },
            0
        );
        (unsafe { std::fs::File::from_raw_fd(master_fd) }, unsafe {
            std::fs::File::from_raw_fd(slave_fd)
        })
    }

    fn read_available(master: &mut std::fs::File, captured: &mut Vec<u8>) {
        let mut buf = [0u8; 1024];
        loop {
            match master.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => captured.extend_from_slice(&buf[..n]),
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
    }

    let fixture = identity_fixture(&[("srv_ser7", "ser7"), ("srv_dark_forge", "dark-forge")]).await;
    let (input_master, input_slave) = open_pty();
    let (mut error_master, error_slave) = open_pty();
    let child = std::process::Command::new(env!("CARGO_BIN_EXE_kiln"))
        .env_remove("KILN_IDENTITY_URL")
        .env("KILN_IDENTITY_CA", &fixture.ca)
        .env("PATH", &fixture.bin_dir)
        .stdin(Stdio::from(input_slave))
        .stdout(Stdio::null())
        .stderr(Stdio::from(error_slave))
        .arg("--config")
        .arg(&fixture.config)
        .arg("--issuer")
        .arg(&fixture.origin)
        .arg("login")
        .spawn()
        .unwrap();
    let mut child = Reap(Some(child));
    let mut captured = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut saw_prompt = false;
    while Instant::now() < deadline {
        read_available(&mut error_master, &mut captured);
        if String::from_utf8_lossy(&captured).contains("Select a server number") {
            saw_prompt = true;
            break;
        }
        if child.0.as_mut().unwrap().try_wait().unwrap().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    let text = String::from_utf8_lossy(&captured);
    assert!(saw_prompt, "{text}");
    let profile = fixture
        .profile()
        .expect("the approved session is saved before the picker waits");
    assert!(!chosen(&profile), "{profile}");
    assert_eq!(fixture.route_count("POST /oauth/device/code"), 1);
    drop(input_master);
    let status = loop {
        if let Some(status) = child.0.as_mut().unwrap().try_wait().unwrap() {
            break status;
        }
        read_available(&mut error_master, &mut captured);
        if Instant::now() > deadline + Duration::from_secs(5) {
            panic!(
                "picker kept waiting after stdin closed: {}",
                String::from_utf8_lossy(&captured)
            );
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    };
    child.0.take();
    let settle = Instant::now() + Duration::from_secs(1);
    while Instant::now() < settle {
        read_available(&mut error_master, &mut captured);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let text = String::from_utf8_lossy(&captured);
    assert!(!status.success(), "{text}");
    assert!(text.contains("kiln login --server srv_ser7"), "{text}");
    assert!(
        text.contains("kiln login --server srv_dark_forge"),
        "{text}"
    );
    let resume = fixture.login(&["--server", "srv_ser7"], null_stdin()).await;
    let resume_error = String::from_utf8_lossy(&resume.stderr);
    assert!(resume.status.success(), "{resume_error}");
    assert_eq!(
        fixture.route_count("POST /oauth/device/code"),
        1,
        "{:?}",
        fixture.routes.lock().unwrap()
    );
    let profile = fixture.profile().unwrap();
    assert_eq!(profile["profiles"]["default"]["server"]["id"], "srv_ser7");
    assert!(chosen(&profile), "{profile}");
}

#[cfg(unix)]
#[tokio::test]
async fn login_on_a_pty_accepts_a_server_number() {
    use std::io::{ErrorKind, Read, Write};
    use std::os::unix::io::FromRawFd;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let fixture = identity_fixture(&[("srv_ser7", "ser7"), ("srv_dark_forge", "dark-forge")]).await;
    let mut master_fd = 0;
    let mut slave_fd = 0;
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master_fd,
                &mut slave_fd,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        },
        0
    );
    for fd in [master_fd, slave_fd] {
        assert_eq!(
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) },
            0
        );
    }
    let flags = unsafe { libc::fcntl(master_fd, libc::F_GETFL) };
    assert_eq!(
        unsafe { libc::fcntl(master_fd, libc::F_SETFL, flags | libc::O_NONBLOCK) },
        0
    );
    let mut master = unsafe { std::fs::File::from_raw_fd(master_fd) };
    let slave = unsafe { std::fs::File::from_raw_fd(slave_fd) };
    let child = std::process::Command::new(env!("CARGO_BIN_EXE_kiln"))
        .env_remove("KILN_IDENTITY_URL")
        .env("KILN_IDENTITY_CA", &fixture.ca)
        .env("PATH", &fixture.bin_dir)
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::null())
        .stderr(Stdio::from(slave))
        .arg("--config")
        .arg(&fixture.config)
        .arg("--issuer")
        .arg(&fixture.origin)
        .arg("login")
        .spawn()
        .unwrap();
    let mut child = Reap(Some(child));
    let mut captured = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let mut buf = [0u8; 1024];
        match master.read(&mut buf) {
            Ok(n) if n > 0 => captured.extend_from_slice(&buf[..n]),
            Ok(_) => break,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(_) => break,
        }
        if String::from_utf8_lossy(&captured).contains("Select a server number") {
            master.write_all(b"nope\n2\n").unwrap();
            break;
        }
        if child.0.as_mut().unwrap().try_wait().unwrap().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    let status = loop {
        let mut buf = [0u8; 1024];
        match master.read(&mut buf) {
            Ok(n) if n > 0 => captured.extend_from_slice(&buf[..n]),
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(_) => {}
        }
        if let Some(status) = child.0.as_mut().unwrap().try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline + Duration::from_secs(5) {
            panic!("{}", String::from_utf8_lossy(&captured));
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    };
    child.0.take();
    let text = String::from_utf8_lossy(&captured);
    assert!(status.success(), "{text}");
    assert!(text.contains("invalid selection"), "{text}");
    assert_eq!(fixture.route_count("POST /oauth/device/code"), 1);
    let profile = fixture.profile().unwrap();
    assert_eq!(
        profile["profiles"]["default"]["server"]["id"],
        "srv_dark_forge"
    );
    assert!(chosen(&profile), "{profile}");
}

#[tokio::test]
async fn login_without_a_terminal_selects_the_only_server() {
    let fixture = identity_fixture(&[("srv_ser7", "ser7")]).await;
    let output = fixture.login(&[], null_stdin()).await;
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{error}");
    assert!(error.contains("Browser could not be opened"), "{error}");
    assert!(!error.contains("Select a server number"), "{error}");
    assert!(output.stdout.is_empty());
    let profile = fixture.profile().expect("one server is saved");
    assert_eq!(profile["profiles"]["default"]["server"]["name"], "ser7");
    assert_eq!(profile["profiles"]["default"]["server"]["id"], "srv_ser7");
    assert!(chosen(&profile), "{profile}");
    assert_eq!(fixture.route_count("GET /v1/servers"), 1);
}

#[tokio::test]
async fn login_with_an_unknown_server_keeps_the_approval() {
    let fixture = identity_fixture(&[("srv_ser7", "ser7"), ("srv_dark_forge", "dark-forge")]).await;
    let output = fixture
        .login(&["--server", "srv_missing"], null_stdin())
        .await;
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{error}");
    assert!(error.contains("server ID not found"), "{error}");
    assert!(error.contains("kiln login --server srv_ser7"), "{error}");
    let profile = fixture.profile().expect("approval kept");
    assert!(!chosen(&profile), "{profile}");
    assert_eq!(fixture.route_count("POST /oauth/device/code"), 1);
    let resume = fixture
        .login(&["--server", "srv_dark_forge"], null_stdin())
        .await;
    let resume_error = String::from_utf8_lossy(&resume.stderr);
    assert!(resume.status.success(), "{resume_error}");
    assert_eq!(fixture.route_count("POST /oauth/device/code"), 1);
    let profile = fixture.profile().unwrap();
    assert_eq!(
        profile["profiles"]["default"]["server"]["id"],
        "srv_dark_forge"
    );
    assert!(chosen(&profile), "{profile}");
}

#[tokio::test]
async fn profile_list_marks_an_unchosen_server() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("profiles.json");
    let credential = root.path().join("credential.json");
    let body = serde_json::json!({
        "profiles": {
            "pending": {
                "issuer": "https://identity.example",
                "server": {
                    "id": "srv_ser7",
                    "name": "ser7",
                    "origin": "https://ser7.example",
                    "ca_pem": "pem",
                    "revision": 1
                },
                "credential_file": credential,
                "door_trust": "pin",
                "server_chosen": false
            },
            "ready": {
                "issuer": "https://identity.example",
                "server": {
                    "id": "srv_dark_forge",
                    "name": "dark-forge",
                    "origin": "https://dark-forge.example",
                    "ca_pem": "pem",
                    "revision": 1
                },
                "credential_file": credential,
                "door_trust": "pin"
            }
        }
    });
    fs::write(&config, serde_json::to_vec(&body).unwrap()).unwrap();
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_kiln"))
        .args(["--config", config.to_str().unwrap(), "profile", "list"])
        .output()
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{error}");
    assert!(text.contains("pending\t(server not chosen)"), "{text}");
    assert!(!text.contains("ser7.example"), "{text}");
    assert!(text.contains("ready\thttps://dark-forge.example"), "{text}");
}

#[tokio::test]
async fn real_binary_profiles_preserve_exec_bytes_status_and_json() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let server = server(Router::new().route(
        "/v1/operations",
        post(move |Json(request): Json<Submit>| {
            seen.fetch_add(1, Ordering::SeqCst);
            async move {
                let Action::Exec { id, request: exec } = request.action else {
                    panic!("expected exec")
                };
                Json(Operation {
                    id: request.id,
                    box_id: id,
                    state: OperationState::Succeeded,
                    error: None,
                    result: Some(Outcome::Exec(ExecResult {
                        stdout: vec![0, 255, b'Q'],
                        stderr: b"guest-stderr".to_vec(),
                        exit_code: Some(37),
                        timed_out: exec.argv[0] == "timeout",
                        truncated: false,
                    })),
                })
            }
        }),
    ))
    .await;
    let root = tempfile::tempdir().unwrap();
    let token = root.path().join("token");
    fs::write(&token, "a".repeat(64)).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let ca = root.path().join("ca.pem");
    fs::write(&ca, &server.pem).unwrap();
    let config = root.path().join("profiles.json");
    let output = cli(
        &config,
        &[
            "profile",
            "add",
            "default",
            "--url",
            &server.url,
            "--token-file",
            token.to_str().unwrap(),
            "--ca-file",
            ca.to_str().unwrap(),
        ],
    )
    .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stored = fs::read_to_string(&config).unwrap();
    assert!(!stored.contains(&"a".repeat(64)));
    for (command, exit) in [("normal", 37), ("timeout", 124)] {
        let output = cli(&config, &["exec", &"b".repeat(32), "--", command]).await;
        assert_eq!(
            output.status.code(),
            Some(exit),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, [0, 255, b'Q']);
        assert!(output.stderr.ends_with(b"guest-stderr"));
    }
    let output = cli(
        &config,
        &["--json", "exec", &"b".repeat(32), "--", "normal"],
    )
    .await;
    assert_eq!(output.status.code(), Some(37));
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["result"]["stdout"], serde_json::json!([0, 255, 81]));
    assert_eq!(calls.load(Ordering::SeqCst), 3, "no implicit replay");
}

#[tokio::test]
async fn tls_verification_and_redirect_refusal_are_real() {
    let server = server(Router::new().route(
        "/v1/templates",
        get(|| async { Redirect::temporary("http://127.0.0.1:1/leak") }),
    ))
    .await;
    let token = "a".repeat(64);
    let untrusted = kiln_client::Client::new(&server.url, &token, None).unwrap();
    assert!(
        untrusted.templates().await.is_err(),
        "self-signed cert must not be silently trusted"
    );
    let trusted =
        kiln_client::Client::new(&server.url, &token, Some(server.pem.as_bytes())).unwrap();
    assert!(
        trusted
            .templates()
            .await
            .unwrap_err()
            .to_string()
            .contains("redirects are disabled")
    );
}

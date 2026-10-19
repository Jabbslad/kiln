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
        Arc,
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
        vec!["--issuer", "https://identity.example.test", "login"],
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
    let root = tempfile::tempdir().unwrap();
    for (environment, explicit, expected) in [
        (None, None, "browser login requires an interactive terminal"),
        (Some("http://invalid.test"), None, "HTTPS"),
        (
            Some("http://invalid.test"),
            Some("https://override.example.test"),
            "browser login requires an interactive terminal",
        ),
    ] {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_kiln"));
        command.env_remove("KILN_IDENTITY_URL").args([
            "--config",
            root.path().join("profiles.json").to_str().unwrap(),
        ]);
        if let Some(value) = environment {
            command.env("KILN_IDENTITY_URL", value);
        }
        if let Some(value) = explicit {
            command.args(["--issuer", value]);
        }
        let output = command.arg("login").output().await.unwrap();
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success());
        assert!(error.contains(expected), "{error}");
        assert!(output.stdout.is_empty());
        assert!(!root.path().join("profiles.json").exists());
    }
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

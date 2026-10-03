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

//! Driven by scripts/prove-create-wait-cli.sh: real `kiln` binary, mock HTTPS host.

use axum::{
    Json, Router,
    extract::{Path, RawQuery},
    http::StatusCode,
    routing::{get, post},
};
use kiln_api::{Action, BoxView, Operation, OperationState, Outcome, Submit, TemplateView};
use std::{
    fs,
    path::Path as FsPath,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

struct Counters {
    create_posts: AtomicUsize,
    create_posts_with_wait: AtomicUsize,
    operation_gets: AtomicUsize,
}

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

async fn start_mock(counters: Arc<Counters>) -> TlsServer {
    let pending: Arc<Mutex<Option<Operation>>> = Arc::new(Mutex::new(None));
    let pending_post = pending.clone();
    let poll_counters = counters.clone();
    let router = Router::new()
        .route(
            "/v1/templates",
            get(|| async {
                Json(vec![TemplateView {
                    name: "demo".into(),
                    memory_mib: 512,
                    vcpus: 1,
                }])
            }),
        )
        .route(
            "/v1/operations",
            post(
                |RawQuery(query): RawQuery, Json(submit): Json<Submit>| async move {
                    let wait = query.as_deref().is_some_and(|q| q.contains("wait=true"));
                    counters.create_posts.fetch_add(1, Ordering::SeqCst);
                    if wait {
                        counters
                            .create_posts_with_wait
                            .fetch_add(1, Ordering::SeqCst);
                    }
                    let op = Operation {
                        id: submit.id.clone(),
                        box_id: "bbbbbbbb000000000000000000000002".into(),
                        state: if wait {
                            OperationState::Succeeded
                        } else {
                            OperationState::Running
                        },
                        error: None,
                        result: if wait {
                            Some(Outcome::Box(BoxView {
                                id: "bbbbbbbb000000000000000000000002".into(),
                                name: match submit.action {
                                    Action::Create { name, .. } => name,
                                    _ => "box".into(),
                                },
                                state: "running".into(),
                                memory_mib: 512,
                                vcpus: 1,
                                owner: None,
                            }))
                        } else {
                            None
                        },
                    };
                    if wait {
                        (StatusCode::OK, Json(op))
                    } else {
                        *pending_post.lock().unwrap() = Some(op.clone());
                        (StatusCode::ACCEPTED, Json(op))
                    }
                },
            ),
        )
        .route(
            "/v1/operations/{id}",
            get({
                let gets = poll_counters.clone();
                move |Path(id): Path<String>,
                      state: axum::extract::State<Arc<Mutex<Option<Operation>>>>| async move {
                    gets.operation_gets.fetch_add(1, Ordering::SeqCst);
                    let mut guard = state.lock().unwrap();
                    let op = guard.as_mut().expect("missing pending op");
                    assert_eq!(op.id, id);
                    if op.state == OperationState::Running {
                        op.state = OperationState::Succeeded;
                        op.result = Some(Outcome::Box(BoxView {
                            id: op.box_id.clone(),
                            name: "proof-box".into(),
                            state: "running".into(),
                            memory_mib: 512,
                            vcpus: 1,
                            owner: None,
                        }));
                    }
                    Json(op.clone())
                }
            })
            .with_state(pending.clone()),
        );

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

async fn kiln(config: &FsPath, args: &[&str]) -> std::process::Output {
    tokio::process::Command::new(env!("CARGO_BIN_EXE_kiln"))
        .arg("--config")
        .arg(config)
        .args(args)
        .output()
        .await
        .unwrap()
}

async fn profile(config: &FsPath, server: &TlsServer, root: &FsPath) {
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
    let output = kiln(
        config,
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
}

fn emit(label: &str, output: &std::process::Output) {
    println!("=== {label} ===");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stderr.is_empty() {
        print!("{stderr}");
        if !stderr.ends_with('\n') {
            println!();
        }
    }
    print!("{stdout}");
    if !stdout.is_empty() && !stdout.ends_with('\n') {
        println!();
    }
    println!("exit={}", output.status);
    println!();
}

#[tokio::test]
async fn create_wait_cli_before_after_proof() {
    let counters = Arc::new(Counters {
        create_posts: AtomicUsize::new(0),
        create_posts_with_wait: AtomicUsize::new(0),
        operation_gets: AtomicUsize::new(0),
    });
    let server = start_mock(counters.clone()).await;
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("profiles.json");
    profile(&config, &server, root.path()).await;

    let before = kiln(
        &config,
        &[
            "--json",
            "--request-id",
            "aaaaaaaa000000000000000000000001",
            "create",
            "--template",
            "demo",
            "--name",
            "proof-before",
        ],
    )
    .await;
    emit("before: kiln create --json (202 + client poll)", &before);
    assert!(before.status.success(), "before create must succeed");

    let after = kiln(
        &config,
        &[
            "--json",
            "--request-id",
            "cccccccc000000000000000000000003",
            "create",
            "--wait",
            "--template",
            "demo",
            "--name",
            "proof-after",
        ],
    )
    .await;
    emit(
        "after: kiln create --wait --json (200, no client poll)",
        &after,
    );
    assert!(after.status.success(), "after create --wait must succeed");

    let posts = counters.create_posts.load(Ordering::SeqCst);
    let waits = counters.create_posts_with_wait.load(Ordering::SeqCst);
    let gets = counters.operation_gets.load(Ordering::SeqCst);
    println!("mock: create_posts={posts} create_posts_with_wait={waits} operation_gets={gets}");
    assert_eq!(posts, 2);
    assert_eq!(waits, 1);
    assert!(
        gets >= 1,
        "client poll should GET the operation at least once"
    );
}

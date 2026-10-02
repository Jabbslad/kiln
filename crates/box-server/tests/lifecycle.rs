//! Opt-in real KVM, TLS and host-process restart test. No simulated VM backend.
use box_api::{Action, ExecRequest, Operation, OperationState, Outcome, Submit, new_id};
use box_client::Client;
use box_runtime::runtime::Runtime;
use box_server::{gateway, host::Config};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};

struct Fixture {
    root: Option<tempfile::TempDir>,
    host: Option<Child>,
    tls: Option<axum_server::Handle<std::net::SocketAddr>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(host) = self.host.as_mut() {
            let _ = host.kill();
            let _ = host.wait();
        }
        if let Some(handle) = &self.tls {
            handle.shutdown();
        }
        let root = self.root.take().unwrap();
        if fs::read_dir(root.path().join("runtime/boxes"))
            .is_ok_and(|mut entries| entries.next().is_some())
        {
            eprintln!(
                "Unfinished VM state retained for cleanup: {}",
                root.keep().display()
            );
        }
    }
}

impl Fixture {
    fn start(&mut self) {
        let root = self.root.as_ref().unwrap().path();
        self.host = Some(
            Command::new(
                std::env::var_os("BOXD_TEST_HOST")
                    .unwrap_or_else(|| env!("CARGO_BIN_EXE_boxd-host").into()),
            )
            .arg("--config")
            .arg(root.join("host.json"))
            .stdout(Stdio::null())
            .stderr(
                fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(root.join("host.log"))
                    .unwrap(),
            )
            .spawn()
            .unwrap(),
        );
    }
}

async fn ready(client: &Client) {
    for _ in 0..100 {
        if client.templates().await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("host did not become ready");
}

async fn finished(client: &Client, mut op: Operation) -> Operation {
    for _ in 0..1200 {
        if op.state != OperationState::Running {
            assert_eq!(op.state, OperationState::Succeeded, "{op:?}");
            return op;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        op = client.operation(&op.id).await.unwrap();
    }
    panic!("operation {} did not finish", op.id);
}

async fn action(client: &Client, action: Action) -> Operation {
    finished(
        client,
        client
            .submit(&Submit {
                id: new_id(),
                action,
            })
            .await
            .unwrap(),
    )
    .await
}

fn exec(id: &str, command: &str) -> Action {
    Action::Exec {
        id: id.into(),
        request: ExecRequest {
            argv: vec!["/bin/sh".into(), "-c".into(), command.into()],
            cwd: Some("/root".into()),
            env: Default::default(),
            timeout_ms: 10000,
        },
    }
}

fn output(op: Operation) -> Vec<u8> {
    let Some(Outcome::Exec(result)) = op.result else {
        panic!("missing exec result")
    };
    assert_eq!(result.exit_code, Some(0));
    assert!(!result.timed_out);
    result.stdout
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a real image, Firecracker/KVM and optionally approved isolated-host setup"]
async fn https_lifecycle_and_crashed_host_never_replay_exec() {
    let image = PathBuf::from(std::env::var("BOXD_TEST_IMAGE").expect("BOXD_TEST_IMAGE required"));
    let isolation = std::env::var_os("BOXD_TEST_ISOLATION_CONFIG").map(PathBuf::from);
    let root = match std::env::var_os("BOXD_TEST_STATE_PARENT") {
        Some(parent) => tempfile::tempdir_in(parent).unwrap(),
        None => tempfile::tempdir().unwrap(),
    };
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut fixture = Fixture {
        root: Some(root),
        host: None,
        tls: None,
    };
    let root = fixture.root.as_ref().unwrap().path().to_owned();
    let runtime =
        Runtime::open_with_isolation(&root.join("runtime"), isolation.as_deref()).unwrap();
    let template = runtime.build_template(&image, 256, 1).await.unwrap();
    let config = Config {
        runtime_dir: root.join("runtime"),
        journal_dir: root.join("journal"),
        socket: root.join("host.sock"),
        isolation_config: isolation.clone(),
        allow_unsafe_development: isolation.is_none(),
        templates: [("test".into(), template.id.clone())].into(),
    };
    fs::write(root.join("host.json"), serde_json::to_vec(&config).unwrap()).unwrap();
    fixture.start();
    let token = format!("{}{}", new_id(), new_id());
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
    let url = format!(
        "https://localhost:{}",
        listener.local_addr().unwrap().port()
    );
    let handle = axum_server::Handle::new();
    let server = axum_server::from_tcp_rustls(listener, tls)
        .unwrap()
        .handle(handle.clone())
        .serve(
            gateway::router(&config.socket, &token)
                .unwrap()
                .into_make_service(),
        );
    fixture.tls = Some(handle);
    tokio::spawn(async move {
        server.await.unwrap();
    });
    let client = Client::new(&url, &token, Some(pem.as_bytes())).unwrap();
    ready(&client).await;
    let request = Submit {
        id: new_id(),
        action: Action::Create {
            template: "test".into(),
            name: "remote-test".into(),
        },
    };
    let (first, retry) = tokio::join!(client.submit(&request), client.submit(&request));
    let first = first.unwrap();
    let retry = retry.unwrap();
    assert_eq!(first.box_id, retry.box_id);
    // Drop the originating HTTP client while the accepted operation is running.
    drop(client);
    let client = Client::new(&url, &token, Some(pem.as_bytes())).unwrap();
    let created = finished(&client, first).await;
    let id = created.box_id;
    assert_eq!(client.list().await.unwrap().len(), 1);
    assert_eq!(
        output(
            action(
                &client,
                exec(&id, "printf retained > /root/remote-data; printf hello")
            )
            .await
        ),
        b"hello"
    );
    for (action_value, expected) in [
        (Action::Pause { id: id.clone() }, "paused"),
        (Action::Resume { id: id.clone() }, "running"),
        (
            Action::Stop {
                id: id.clone(),
                force: false,
            },
            "stopped",
        ),
        (Action::Start { id: id.clone() }, "running"),
    ] {
        action(&client, action_value).await;
        assert_eq!(client.inspect(&id).await.unwrap().state, expected);
    }
    assert_eq!(
        output(action(&client, exec(&id, "cat /root/remote-data")).await),
        b"retained"
    );
    let side_effect = Submit {
        id: new_id(),
        action: exec(&id, "printf x >> /root/once; sleep 2"),
    };
    let running = client.submit(&side_effect).await.unwrap();
    assert_eq!(running.state, OperationState::Running);
    tokio::time::sleep(Duration::from_millis(300)).await;
    fixture.host.as_mut().unwrap().kill().unwrap();
    fixture.host.as_mut().unwrap().wait().unwrap();
    fixture.start();
    ready(&client).await;
    assert_eq!(
        client.operation(&running.id).await.unwrap().state,
        OperationState::Unknown
    );
    assert_eq!(
        client.submit(&side_effect).await.unwrap().state,
        OperationState::Unknown
    );
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        output(action(&client, exec(&id, "cat /root/once")).await),
        b"x",
        "side effect must run once, not zero or twice"
    );
    action(&client, Action::Delete { id: id.clone() }).await;
    assert!(client.list().await.unwrap().is_empty());
    assert_eq!(
        client.submit(&request).await.unwrap().box_id,
        id,
        "retry after deletion must not resurrect a box"
    );
    assert!(client.list().await.unwrap().is_empty());
    runtime.delete_snapshot(&template.id).unwrap();
    eprintln!(
        "PASS: HTTPS create/retry/disconnect, exec, pause/resume, persistent stop/start, host-crash unknown/no-replay, delete/tombstone"
    );
}

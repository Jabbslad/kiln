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
    let host_key = if std::env::var_os("BOXD_TEST_CLIENT").is_some() {
        Some(client.ssh_host_key(&id).await.unwrap())
    } else {
        None
    };
    if let Some(binary) = std::env::var_os("BOXD_TEST_CLIENT") {
        let token_path = root.join("admin.token");
        fs::write(&token_path, &token).unwrap();
        fs::set_permissions(&token_path, fs::Permissions::from_mode(0o600)).unwrap();
        let ca_path = root.join("ca.crt");
        fs::write(&ca_path, &pem).unwrap();
        let profile = root.join("profiles.json");
        let output = tokio::process::Command::new(&binary)
            .arg("--config")
            .arg(&profile)
            .args(["profile", "add", "default", "--url", &url, "--token-file"])
            .arg(&token_path)
            .arg("--ca-file")
            .arg(&ca_path)
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = tokio::process::Command::new(&binary)
            .arg("--config")
            .arg(&profile)
            .args(["ssh", &id, "--", "printf ssh-connected; exit 37"])
            .output()
            .await
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(37),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"ssh-connected");
        let source = root.join("upload.bin");
        let destination = root.join("download.bin");
        let bytes: Vec<u8> = (0..200_123).map(|n| ((n * 37 + 13) % 256) as u8).collect();
        fs::write(&source, &bytes).unwrap();
        let remote = format!("{id}:/workspace/binary.dat");
        for (from, to) in [
            (source.to_str().unwrap(), remote.as_str()),
            (remote.as_str(), destination.to_str().unwrap()),
        ] {
            let output = tokio::process::Command::new(&binary)
                .arg("--config")
                .arg(&profile)
                .args(["cp", from, to])
                .output()
                .await
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert_eq!(fs::read(&destination).unwrap(), bytes);
        let ssh_config = root.join("ssh-config");
        let output = tokio::process::Command::new(&binary)
            .arg("--config")
            .arg(&profile)
            .args(["ssh-config", &id])
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        fs::write(&ssh_config, output.stdout).unwrap();
        let alias = format!("boxd-default-{id}");
        // Editors open direct-tcpip channels to their guest-local server.
        output_of_forwarding_probe(&client, &id, &ssh_config, &alias).await;
        let output = tokio::process::Command::new("python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../scripts/test-ssh-pty.py"
            ))
            .arg(&ssh_config)
            .arg(&alias)
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        eprintln!("{}", String::from_utf8_lossy(&output.stdout));
        let known_hosts = root.join(format!("ssh/default/known_hosts_{alias}"));
        let original = fs::read(&known_hosts).unwrap();
        fs::write(&known_hosts, format!("{alias} ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n")).unwrap();
        let output = tokio::process::Command::new("ssh")
            .arg("-F")
            .arg(&ssh_config)
            .args([&alias, "true"])
            .output()
            .await
            .unwrap();
        assert_eq!(output.status.code(), Some(255));
        assert!(String::from_utf8_lossy(&output.stderr).contains("Host key verification failed"));
        fs::write(known_hosts, original).unwrap();
        eprintln!("PASS: real HTTPS SSH command/exit status and 200123-byte SFTP roundtrip");
    }
    if std::env::var_os("BOXD_TEST_NETWORK").is_some() {
        assert_eq!(output(action(&client, exec(&id, "python3 -c 'import urllib.request; print(urllib.request.urlopen(\"http://8.8.8.8:8080/probe\", timeout=5).read().decode(), end=\"\")'")).await), b"boxd-egress\n");
    }
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
    if let Some(key) = &host_key {
        assert_eq!(&client.ssh_host_key(&id).await.unwrap(), key);
    }
    if std::env::var_os("BOXD_TEST_NETWORK").is_some() {
        assert_eq!(output(action(&client, exec(&id, "python3 -c 'import urllib.request; print(urllib.request.urlopen(\"http://8.8.8.8:8080/probe\", timeout=5).read().decode(), end=\"\")'")).await), b"boxd-egress\n");
        let second = action(
            &client,
            Action::Create {
                template: "test".into(),
                name: "second-clone".into(),
            },
        )
        .await
        .box_id;
        if let Some(key) = &host_key {
            assert_ne!(&client.ssh_host_key(&second).await.unwrap(), key);
        }
        assert_eq!(output(action(&client, exec(&second, "python3 -c 'import urllib.request; print(urllib.request.urlopen(\"http://8.8.8.8:8080/probe\", timeout=5).read().decode(), end=\"\")'")).await), b"boxd-egress\n");
        action(&client, Action::Delete { id: second }).await;
        eprintln!(
            "PASS: persistent SSH host key, distinct clone keys, egress after restart and on another network slot"
        );
    }
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

async fn output_of_forwarding_probe(
    client: &Client,
    id: &str,
    config: &std::path::Path,
    alias: &str,
) {
    use tokio::io::AsyncWriteExt;
    output(action(client, exec(id, "mkdir -p /tmp/editor-probe; printf editor-forwarded > /tmp/editor-probe/probe; nohup python3 -m http.server 8081 --bind 127.0.0.1 --directory /tmp/editor-probe >/tmp/editor-probe.log 2>&1 </dev/null &")).await);
    let mut ssh = tokio::process::Command::new("ssh")
        .arg("-F")
        .arg(config)
        .args(["-W", "127.0.0.1:8081", alias])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    ssh.stdin
        .as_mut()
        .unwrap()
        .write_all(b"GET /probe HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let output = tokio::time::timeout(Duration::from_secs(15), ssh.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.ends_with(b"editor-forwarded"));
    let denied = tokio::process::Command::new("ssh")
        .arg("-F")
        .arg(config)
        .args(["-W", "192.168.50.1:80", alias])
        .output()
        .await
        .unwrap();
    assert!(!denied.status.success());
    assert!(String::from_utf8_lossy(&denied.stderr).contains("administratively prohibited"));
    eprintln!("PASS: editor loopback forwarding and non-loopback forwarding rejection");
}

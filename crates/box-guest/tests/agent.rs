use box_guest::{Agent, FakeInitializer};
use box_protocol::{ErrorCode, ExecRequest, InitializeRequest, Request, Response};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

fn exec(argv: &[&str]) -> Request {
    Request::Exec(ExecRequest {
        argv: argv.iter().map(|s| s.to_string()).collect(),
        cwd: None,
        env: BTreeMap::new(),
        timeout_ms: 2_000,
    })
}

#[tokio::test]
async fn bootstrap_never_executes_workloads_and_service_requires_handoff() {
    let init = Arc::new(FakeInitializer::default());
    let agent = Agent::new(init.clone(), 2);
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("initialized");
    assert!(Agent::from_boot_marker(init.clone(), 2, &marker).is_err());
    let request = InitializeRequest {
        hostname: "box-17".into(),
        machine_id: std::fs::read_to_string("/etc/machine-id")
            .unwrap()
            .trim()
            .into(),
        entropy: vec![19; 64],
    };
    assert!(matches!(
        box_guest::bootstrap::handle(&agent, Request::Initialize(request.clone()), &marker).await,
        Response::Initialized { .. }
    ));
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap(),
        format!("{}\n", request.machine_id)
    );
    assert!(matches!(
        box_guest::bootstrap::handle(&agent, exec(&["/bin/true"]), &marker).await,
        Response::Error(e) if e.code == ErrorCode::NotInitialized
    ));
    let service = Agent::from_boot_marker(init.clone(), 2, &marker).unwrap();
    assert!(
        matches!(service.handle(exec(&["/bin/sh", "-c", "exit 17"])).await,
        Response::Exec(result) if result.exit_code == Some(17))
    );
    assert!(matches!(service.handle(Request::Initialize(request)).await,
        Response::Error(e) if e.code == ErrorCode::AlreadyInitialized));
    assert_eq!(init.calls(), 1);
    std::fs::write(&marker, b"not-an-identity\n").unwrap();
    assert!(Agent::from_boot_marker(init, 2, &marker).is_err());
}

#[test]
fn handoff_rejects_a_valid_but_foreign_machine_identity() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("initialized");
    let current = std::fs::read_to_string("/etc/machine-id").unwrap();
    let foreign = if current.starts_with('1') {
        "2".repeat(32)
    } else {
        "1".repeat(32)
    };
    std::fs::write(&marker, foreign).unwrap();
    assert!(Agent::from_boot_marker(Arc::new(FakeInitializer::default()), 1, &marker).is_err());
}

#[tokio::test]
async fn failed_handoff_never_advertises_a_ready_guest() {
    let agent = Agent::new(Arc::new(FakeInitializer::default()), 1);
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("absent/marker");
    let request = Request::Initialize(InitializeRequest {
        hostname: "box-3".into(),
        machine_id: "123456789abcdef0123456789abcdef0".into(),
        entropy: vec![3; 32],
    });
    assert!(
        matches!(box_guest::bootstrap::handle(&agent, request, &marker).await,
        Response::Error(e) if e.code == ErrorCode::InitializationFailed)
    );
    assert!(matches!(
        box_guest::bootstrap::handle(&agent, Request::Hello { version: 1 }, &marker).await,
        Response::Hello {
            initialized: false,
            ..
        }
    ));
    assert!(!marker.exists());
}

#[tokio::test]
async fn initialization_is_a_one_way_barrier() {
    let init = Arc::new(FakeInitializer::default());
    let agent = Agent::new(init.clone(), 2);
    assert!(
        matches!(agent.handle(exec(&["true"])).await, Response::Error(e) if e.code == ErrorCode::NotInitialized)
    );
    let request = InitializeRequest {
        hostname: "box-1".into(),
        machine_id: "0123456789abcdef0123456789abcdef".into(),
        entropy: vec![7; 32],
    };
    assert!(matches!(
        agent.handle(Request::Initialize(request.clone())).await,
        Response::Initialized { .. }
    ));
    assert!(
        matches!(agent.handle(Request::Initialize(request)).await, Response::Error(e) if e.code == ErrorCode::AlreadyInitialized)
    );
    assert_eq!(init.calls(), 1);
}

#[tokio::test]
async fn exec_separates_and_caps_output() {
    let agent = Agent::initialized_for_test(2);
    let script = "printf out; printf err >&2; head -c 70000 /dev/zero";
    let Response::Exec(result) = agent.handle(exec(&["sh", "-c", script])).await else {
        panic!()
    };
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(&result.stdout[..3], b"out");
    assert_eq!(result.stderr, b"err");
    assert_eq!(result.stdout.len(), 65_536);
    assert!(result.truncated);
    assert!(!result.timed_out);
}

#[tokio::test]
async fn timeout_kills_process_group_and_reaps_it() {
    let agent = Agent::initialized_for_test(1);
    let request = Request::Exec(ExecRequest {
        argv: vec!["sh".into(), "-c".into(), "sleep 30 & wait".into()],
        cwd: None,
        env: BTreeMap::new(),
        timeout_ms: 20,
    });
    let Response::Exec(result) =
        tokio::time::timeout(Duration::from_secs(2), agent.handle(request))
            .await
            .unwrap()
    else {
        panic!()
    };
    assert!(result.timed_out);
    assert_eq!(result.exit_code, None);
}

#[tokio::test]
async fn disconnect_cancels_and_reaps_execution() {
    let agent = Agent::initialized_for_test(1);
    let (cancel, cancelled) = tokio::sync::oneshot::channel();
    let request = Request::Exec(ExecRequest {
        argv: vec!["sh".into(), "-c".into(), "sleep 30 & wait".into()],
        cwd: None,
        env: BTreeMap::new(),
        timeout_ms: 30_000,
    });
    cancel.send(()).unwrap();
    let response = tokio::time::timeout(
        Duration::from_secs(2),
        agent.handle_with_cancellation(request, cancelled),
    )
    .await
    .unwrap();
    assert!(matches!(response, Response::Error(e) if e.code == ErrorCode::ExecutionFailed));
}

#[tokio::test]
async fn exited_shell_with_inherited_pipes_still_obeys_deadline() {
    let agent = Agent::initialized_for_test(1);
    let request = Request::Exec(ExecRequest {
        argv: vec!["sh".into(), "-c".into(), "sleep 3 & exit 7".into()],
        cwd: None,
        env: BTreeMap::new(),
        timeout_ms: 30,
    });
    let response = tokio::time::timeout(Duration::from_secs(1), agent.handle(request)).await;
    assert!(
        response.is_ok(),
        "inherited pipes must not disable the command deadline"
    );
}

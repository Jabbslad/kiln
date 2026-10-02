use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use box_api::{Action, Submit, new_id};
use box_server::host::{Config, Host};
use serde_json::{Value, json};
use tower::ServiceExt;

fn config(root: &std::path::Path) -> Config {
    Config {
        runtime_dir: root.join("runtime"),
        journal_dir: root.join("journal"),
        socket: root.join("host.sock"),
        isolation_config: None,
        allow_unsafe_development: true,
        templates: Default::default(),
    }
}

#[tokio::test]
async fn private_service_requires_explicit_development_and_exclusive_ownership() {
    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    config.allow_unsafe_development = false;
    assert!(Host::open(&config).is_err());
    config.allow_unsafe_development = true;
    let host = Host::open(&config).unwrap();
    assert!(
        Host::open(&config).is_err(),
        "two services must not recover each other's live requests"
    );
    drop(host);
    assert!(Host::open(&config).is_ok());
}

#[tokio::test]
async fn only_catalog_resources_and_owned_boxes_are_accessible() {
    let root = tempfile::tempdir().unwrap();
    let host = Host::open(&config(root.path())).unwrap();
    let router = host.router();
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/templates")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Value>(&to_bytes(response.into_body(), 1024).await.unwrap())
            .unwrap(),
        json!([])
    );
    for action in [
        Action::Create {
            template: "missing".into(),
            name: "ok".into(),
        },
        Action::Delete { id: new_id() },
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::post("/v1/operations")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&Submit {
                            id: new_id(),
                            action,
                        })
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let bytes = to_bytes(response.into_body(), 1024).await.unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains(root.path().to_str().unwrap()));
    }
    for suffix in ["ssh", "ssh-key"] {
        let response = router.clone().oneshot(
            Request::get(format!("/v1/boxes/{}/{suffix}", new_id()))
                .header("connection", "upgrade")
                .header("upgrade", box_api::SSH_UPGRADE)
                .header(box_api::SSH_KEY_HEADER, "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
                .body(Body::empty()).unwrap(),
        ).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    let response = router
        .oneshot(
            Request::post("/v1/operations")
                .header("content-type", "application/json")
                .body(Body::from(vec![b' '; box_api::MAX_REQUEST_BYTES + 1]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn concurrent_http_retries_conflict_and_survive_catalog_changes() {
    use std::{fs, os::unix::fs::PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    drop(Host::open(&config).unwrap());
    let template = new_id();
    let directory = config.runtime_dir.join("snapshots").join(&template);
    fs::create_dir(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    // Deliberately incompatible metadata: rejection happens before a VM can be
    // launched even on a machine that has working KVM/Firecracker installed.
    fs::write(directory.join("snapshot.json"), serde_json::to_vec(&json!({
        "schema_version":1,"id":template,"source_box":new_id(),"template":true,
        "host":"incompatible-test-host","memory_mib":384,"vcpus":2,"hashes":{},
        "image":{"schema_version":1,"architecture":"x86_64","kernel_path":"/private/canary",
        "kernel_sha256":"unused","rootfs_path":"/private/canary","rootfs_sha256":"unused","agent_protocol_version":1}
    })).unwrap()).unwrap();
    config.templates.insert("small".into(), template);
    let host = Host::open(&config).unwrap();
    let router = host.router();
    let response = router
        .clone()
        .oneshot(Request::get("/v1/templates").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&to_bytes(response.into_body(), 1024).await.unwrap())
            .unwrap(),
        json!([{"name":"small","memory_mib":384,"vcpus":2}])
    );
    let request = Submit {
        id: new_id(),
        action: Action::Create {
            template: "small".into(),
            name: "first".into(),
        },
    };
    let send = |request: &Submit| {
        Request::post("/v1/operations")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(request).unwrap()))
            .unwrap()
    };
    let (first, retry) = tokio::join!(
        router.clone().oneshot(send(&request)),
        router.clone().oneshot(send(&request))
    );
    let first = first.unwrap();
    let retry = retry.unwrap();
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    assert_eq!(retry.status(), StatusCode::OK);
    let first: Value =
        serde_json::from_slice(&to_bytes(first.into_body(), 4096).await.unwrap()).unwrap();
    let retry: Value =
        serde_json::from_slice(&to_bytes(retry.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(first["box_id"], retry["box_id"]);
    let changed = Submit {
        id: request.id.clone(),
        action: Action::Create {
            template: "small".into(),
            name: "changed".into(),
        },
    };
    assert_eq!(
        router
            .clone()
            .oneshot(send(&changed))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    let mut terminal = false;
    for _ in 0..100 {
        let response = router
            .clone()
            .oneshot(
                Request::get(format!("/v1/operations/{}", request.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        if value["state"] != "running" {
            assert_eq!(value["state"], "failed");
            terminal = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(terminal);
    drop(router);
    drop(host);
    config.templates.clear();
    let router = Host::open(&config).unwrap().router();
    let response = router.oneshot(send(&request)).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "retries must work even when the template alias was removed"
    );
    let value: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(value["box_id"], first["box_id"]);
    assert!(!value.to_string().contains("/private/canary"));
}

#[tokio::test]
async fn socket_does_not_replace_regular_files_or_allow_world_access() {
    use std::{fs, os::unix::fs::PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("host.sock");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(box_server::host::bind_socket(&socket).await.is_err());
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(&socket, "retain").unwrap();
    assert!(box_server::host::bind_socket(&socket).await.is_err());
    assert_eq!(fs::read_to_string(&socket).unwrap(), "retain");
    fs::remove_file(&socket).unwrap();
    let listener = box_server::host::bind_socket(&socket).await.unwrap();
    assert_eq!(
        fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
        0o660
    );
    assert!(box_server::host::bind_socket(&socket).await.is_err());
    drop(listener);
    assert!(box_server::host::bind_socket(&socket).await.is_ok());
}

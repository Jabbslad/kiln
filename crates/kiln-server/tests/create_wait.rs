use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use kiln_api::{Action, Submit, new_id};
use kiln_server::host::{Config, Host};
use tower::ServiceExt;

fn config(root: &std::path::Path) -> Config {
    Config {
        runtime_dir: root.join("runtime"),
        journal_dir: root.join("journal"),
        socket: root.join("host.sock"),
        isolation_config: None,
        allow_unsafe_development: true,
        templates: Default::default(),
        tenant_owner_id: String::new(),
        member_box_quota: kiln_api::auth::DEFAULT_MEMBER_BOX_QUOTA,
    }
}

#[tokio::test]
async fn create_wait_rejects_unknown_query_parameters() {
    let root = tempfile::tempdir().unwrap();
    let host = Host::open(&config(root.path())).unwrap();
    let router = host.router();
    let body = serde_json::to_vec(&Submit {
        id: new_id(),
        action: Action::Create {
            template: "missing".into(),
            name: "box".into(),
        },
    })
    .unwrap();
    let response = router
        .oneshot(
            Request::post("/v1/operations?preview=1")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_wait_returns_failed_create_without_extra_sleep() {
    use std::{fs, os::unix::fs::PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    drop(Host::open(&config).unwrap());
    let template = new_id();
    let directory = config.runtime_dir.join("snapshots").join(&template);
    fs::create_dir(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        directory.join("snapshot.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema_version":1,"id":template,"source_box":new_id(),"template":true,
            "host":"incompatible-test-host","memory_mib":384,"vcpus":2,"hashes":{},
            "image":{"schema_version":1,"architecture":"x86_64","kernel_path":"/private/canary",
            "kernel_sha256":"unused","rootfs_path":"/private/canary","rootfs_sha256":"unused","agent_protocol_version":1}
        }))
        .unwrap(),
    )
    .unwrap();
    config.templates.insert("small".into(), template);
    let host = Host::open(&config).unwrap();
    let router = host.router();
    let request = Submit {
        id: new_id(),
        action: Action::Create {
            template: "small".into(),
            name: "first".into(),
        },
    };
    let response = router
        .oneshot(
            Request::post("/v1/operations?wait=true&wait_seconds=30")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let value: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(value["state"], "failed");
}

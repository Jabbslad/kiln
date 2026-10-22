use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use kiln_api::{
    Action, Submit,
    auth::{CALLER_ROLE_HEADER, CALLER_SUB_HEADER, CALLER_TENANT_HEADER},
    new_id,
};
use kiln_runtime::runtime::BoxRecord;
use kiln_server::host::{Config, Host};
use std::os::unix::fs::PermissionsExt;
use tower::ServiceExt;

fn config(root: &std::path::Path) -> Config {
    Config {
        runtime_dir: root.join("runtime"),
        journal_dir: root.join("journal"),
        socket: root.join("host.sock"),
        isolation_config: None,
        allow_unsafe_development: true,
        templates: Default::default(),
        tenant_owner_id: "acct-owner".into(),
        member_box_quota: 4,
    }
}

fn write_box(root: &std::path::Path, record: &BoxRecord) {
    let box_dir = root.join("runtime/boxes").join(&record.id);
    let run_dir = box_dir.join(&record.run);
    std::fs::create_dir_all(&run_dir).unwrap();
    std::fs::set_permissions(&box_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::set_permissions(&run_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(
        box_dir.join("box.json"),
        serde_json::to_vec(record).unwrap(),
    )
    .unwrap();
}

fn sample_record(id: &str, name: &str, owner: &str) -> BoxRecord {
    BoxRecord {
        schema_version: 1,
        id: id.into(),
        name: name.into(),
        state: "stopped".into(),
        run: "0123456789abcdef0123456789abcde0".into(),
        process: None,
        image: kiln_runtime::image::Manifest {
            schema_version: 1,
            architecture: "x86_64".into(),
            kernel_path: "/kernel".into(),
            kernel_sha256: "ab".repeat(32),
            rootfs_path: "/rootfs".into(),
            rootfs_sha256: "cd".repeat(32),
            agent_protocol_version: 1,
            boot_mode: kiln_runtime::image::BootMode::Init,
        },
        host: "fixture".into(),
        memory_mib: 256,
        vcpus: 1,
        source: None,
        operation: None,
        last_error: None,
        disk_copy: String::new(),
        disk_layer: None,
        jail: None,
        network: None,
        last_launch: None,
        owner: owner.into(),
    }
}

fn member_headers() -> [(String, String); 3] {
    [
        (CALLER_SUB_HEADER.into(), "acct-member".into()),
        (CALLER_TENANT_HEADER.into(), "ten_1".into()),
        (CALLER_ROLE_HEADER.into(), "member".into()),
    ]
}

#[tokio::test]
async fn member_list_and_get_hide_other_accounts_boxes() {
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let owner_box = "0123456789abcdef0123456789abcdef";
    let member_box = "0123456789abcdef0123456789abcde1";
    write_box(
        root.path(),
        &sample_record(owner_box, "owner-box", "acct-owner"),
    );
    write_box(
        root.path(),
        &sample_record(member_box, "member-box", "acct-member"),
    );
    for path in ["runtime", "runtime/boxes"] {
        std::fs::set_permissions(
            root.path().join(path),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
    }
    let cfg = config(root.path());
    let host = Host::open(&cfg).unwrap();
    let db = rusqlite::Connection::open(cfg.journal_dir.join("operations.sqlite")).unwrap();
    for (id, name) in [(owner_box, "owner-box"), (member_box, "member-box")] {
        db.execute(
            "INSERT INTO operations(id,fingerprint,box_id,is_create,running,response,created_at)
             VALUES (?1,zeroblob(32),?2,1,0,?3,1)",
            rusqlite::params![
                format!("op-{id}"),
                id,
                format!(
                    r#"{{"id":"op-{id}","box_id":"{id}","state":"succeeded","result":{{"type":"box","id":"{id}","name":"{name}","state":"stopped","memory_mib":256,"vcpus":1}},"error":null}}"#
                ),
            ],
        )
        .unwrap();
    }
    let router = host.router();
    let mut list = Request::builder().uri("/v1/boxes");
    for (name, value) in member_headers() {
        list = list.header(name, value);
    }
    let response = router
        .clone()
        .oneshot(list.body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Vec<serde_json::Value> =
        serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap();
    assert_eq!(body.len(), 1);
    assert_eq!(body[0]["name"], "member-box");

    for target in [owner_box, "owner-box"] {
        let mut inspect = Request::builder().uri(format!("/v1/boxes/{target}"));
        for (name, value) in member_headers() {
            inspect = inspect.header(name, value);
        }
        let response = router
            .clone()
            .oneshot(inspect.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

/// Another member's box may exist in the journal before `box.json` is written.
/// Members must not operate on it during that window.
#[tokio::test]
async fn member_cannot_operate_on_another_accounts_inflight_create() {
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    for path in ["runtime", "journal"] {
        std::fs::create_dir_all(root.path().join(path)).unwrap();
        std::fs::set_permissions(
            root.path().join(path),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
    }
    let cfg = config(root.path());
    let host = Host::open(&cfg).unwrap();
    let inflight = "0123456789abcdef0123456789abcde2";
    let db = rusqlite::Connection::open(cfg.journal_dir.join("operations.sqlite")).unwrap();
    db.execute(
        "INSERT INTO operations(id,fingerprint,box_id,is_create,running,response,created_at,create_owner)
         VALUES (?1,zeroblob(32),?2,1,1,?3,1,?4)",
        rusqlite::params![
            "op-inflight",
            inflight,
            format!(
                r#"{{"id":"op-inflight","box_id":"{inflight}","state":"running","result":null,"error":null}}"#
            ),
            "acct-other",
        ],
    )
    .unwrap();
    let router = host.router();
    let submit = Submit {
        id: new_id(),
        action: Action::Delete { id: inflight.into() },
    };
    let mut request = Request::builder()
        .method("POST")
        .uri("/v1/operations")
        .header("content-type", "application/json");
    for (name, value) in member_headers() {
        request = request.header(name, value);
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::from(serde_json::to_vec(&submit).unwrap())).unwrap())
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "member must not delete another account's in-flight create"
    );
}

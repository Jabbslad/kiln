//! Host API checkpoint create/restore. No guest is booted here: capture and
//! restore need KVM, and the ignored lifecycle test drives that path.
use std::{fs, os::unix::fs::PermissionsExt, time::UNIX_EPOCH};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use kiln_api::{Action, Submit, new_id};
use kiln_server::host::{Config, Host};
use serde_json::{Value, json};
use tower::ServiceExt;

const ACK: &str = "restore rewinds memory and disk, not external systems; pass --acknowledge-external-state-replay";

fn config(root: &std::path::Path) -> Config {
    Config {
        runtime_dir: root.join("runtime"),
        journal_dir: root.join("journal"),
        socket: root.join("host.sock"),
        isolation_config: None,
        allow_unsafe_development: true,
        jail_pool: 0,
        templates: Default::default(),
    }
}

fn snapshot_file(directory: &std::path::Path, id: &str, source_box: &str, template: bool) {
    fs::create_dir_all(directory).unwrap();
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        directory.join("snapshot.json"),
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "id": id,
            "source_box": source_box,
            "template": template,
            "host": "not-this-host",
            "memory_mib": 256,
            "vcpus": 1,
            "hashes": {},
            "isolated": false,
            "image": {
                "schema_version": 1,
                "architecture": "x86_64",
                "kernel_path": "/private/canary",
                "kernel_sha256": "unused",
                "rootfs_path": "/private/canary",
                "rootfs_sha256": "unused",
                "agent_protocol_version": 1
            }
        }))
        .unwrap(),
    )
    .unwrap();
}

async fn body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap()
}

fn post(request: &Submit) -> Request<Body> {
    Request::post("/v1/operations")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(request).unwrap()))
        .unwrap()
}

async fn wait_finished(router: &axum::Router, id: &str) -> Value {
    for _ in 0..100 {
        let response = router
            .clone()
            .oneshot(
                Request::get(format!("/v1/operations/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let value = body(response).await;
        if value["state"] != "running" {
            return value;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("operation {id} did not finish");
}

#[tokio::test]
async fn checkpoint_create_lists_restore_replay_and_ownership() {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = config(root.path());
    drop(Host::open(&config).unwrap());
    let template = new_id();
    snapshot_file(
        &config.runtime_dir.join("snapshots").join(&template),
        &template,
        &new_id(),
        true,
    );
    config.templates.insert("small".into(), template.clone());
    let host = Host::open(&config).unwrap();
    let router = host.router();

    let create = Submit {
        id: new_id(),
        action: Action::Create {
            template: "small".into(),
            name: "owned".into(),
        },
    };
    let created = router.clone().oneshot(post(&create)).await.unwrap();
    assert_eq!(created.status(), StatusCode::ACCEPTED);
    let owned = body(created).await["box_id"].as_str().unwrap().to_owned();
    assert_eq!(wait_finished(&router, &create.id).await["state"], "failed");

    let checkpoint_id = new_id();
    let directory = config.runtime_dir.join("snapshots").join(&checkpoint_id);
    snapshot_file(&directory, &checkpoint_id, &owned, false);
    fs::write(directory.join("disk.ext4"), b"ckpt").unwrap();
    let modified = fs::metadata(directory.join("snapshot.json"))
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let dashboard = router
        .clone()
        .oneshot(
            Request::get("/api/v1/dashboard")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(dashboard.status(), StatusCode::OK);
    let dashboard = body(dashboard).await;
    let checkpoints = dashboard["checkpoints"].as_array().unwrap();
    assert_eq!(checkpoints.len(), 1, "{dashboard}");
    let listed = &checkpoints[0];
    assert_eq!(listed["id"], checkpoint_id);
    assert_eq!(listed["box_id"], owned);
    assert_eq!(listed["name"], checkpoint_id);
    assert_eq!(listed["size_bytes"], 4);
    assert_eq!(listed["compatible"], false);
    assert_eq!(listed["created_at"], stamp(modified));
    assert!(
        checkpoints
            .iter()
            .all(|checkpoint| checkpoint["id"] != template)
    );

    let unacked = Submit {
        id: new_id(),
        action: Action::Restore {
            id: owned.clone(),
            checkpoint: checkpoint_id.clone(),
            acknowledge_external_state_replay: false,
        },
    };
    let rejected = router.clone().oneshot(post(&unacked)).await.unwrap();
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    let rejected = body(rejected).await;
    assert_eq!(rejected["message"], ACK);
    assert_eq!(
        router
            .clone()
            .oneshot(
                Request::get(format!("/v1/operations/{}", unacked.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND,
        "a rejected restore must not consume the request id"
    );

    let omitted = router
        .clone()
        .oneshot(
            Request::post("/v1/operations")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "id": new_id(),
                        "action": {
                            "type": "restore",
                            "id": owned,
                            "checkpoint": checkpoint_id,
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(omitted.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body(omitted).await["message"], ACK);

    let foreign = new_id();
    for action in [
        Action::Checkpoint {
            id: foreign.clone(),
        },
        Action::Restore {
            id: foreign,
            checkpoint: checkpoint_id.clone(),
            acknowledge_external_state_replay: true,
        },
    ] {
        let response = router
            .clone()
            .oneshot(post(&Submit {
                id: new_id(),
                action,
            }))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    let capture = Submit {
        id: new_id(),
        action: Action::Checkpoint { id: owned.clone() },
    };
    let first = router.clone().oneshot(post(&capture)).await.unwrap();
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    let first = body(first).await;
    let replay = router.clone().oneshot(post(&capture)).await.unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(body(replay).await["id"], first["id"]);
    let finished = wait_finished(&router, &capture.id).await;
    assert_eq!(finished["state"], "failed");
    assert_eq!(finished["box_id"], owned);
    let again = router.clone().oneshot(post(&capture)).await.unwrap();
    assert_eq!(again.status(), StatusCode::OK);
    assert_eq!(body(again).await, finished);
    let mut stored = non_template_checkpoints(&config.runtime_dir);
    stored.sort();
    assert_eq!(stored, vec![checkpoint_id.clone()]);

    let changed = Submit {
        id: capture.id.clone(),
        action: Action::Restore {
            id: owned.clone(),
            checkpoint: checkpoint_id.clone(),
            acknowledge_external_state_replay: true,
        },
    };
    assert_eq!(
        router
            .clone()
            .oneshot(post(&changed))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );

    let restore = Submit {
        id: new_id(),
        action: Action::Restore {
            id: owned,
            checkpoint: checkpoint_id,
            acknowledge_external_state_replay: true,
        },
    };
    let accepted = router.clone().oneshot(post(&restore)).await.unwrap();
    assert_eq!(accepted.status(), StatusCode::ACCEPTED);
    let accepted = body(accepted).await;
    assert_ne!(accepted["id"], first["id"]);
    // No guest is running, so the runtime refuses the restore. The
    // acknowledgement itself was accepted. The KVM lifecycle test covers a
    // restore that finishes succeeded.
    assert_eq!(wait_finished(&router, &restore.id).await["state"], "failed");
}

fn non_template_checkpoints(runtime: &std::path::Path) -> Vec<String> {
    let mut ids = Vec::new();
    for entry in fs::read_dir(runtime.join("snapshots")).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path().join("snapshot.json");
        if !path.is_file() {
            continue;
        }
        let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        if value["template"] != true {
            ids.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    ids
}

fn stamp(secs: u64) -> String {
    let secs = i64::try_from(secs).unwrap_or(i64::MAX);
    let days = secs.div_euclid(86_400);
    let tod = u32::try_from(secs.rem_euclid(86_400)).unwrap_or(0);
    let (year, month, day) = civil_date(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60,
    )
}

fn civil_date(days_since_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = z.div_euclid(146_097);
    let doe = u64::try_from(z - era * 146_097).unwrap_or(0);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let year = i64::try_from(yoe).unwrap_or(0) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year + i64::from(month <= 2);
    (
        i32::try_from(year).unwrap_or(i32::MAX),
        u32::try_from(month).unwrap_or(0),
        u32::try_from(day).unwrap_or(0),
    )
}

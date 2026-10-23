//! Real JWT gateway -> private Unix host boundary, using disposable metadata only.
//! No VM, production service, or identity network request is needed.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use base64::Engine;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use kiln_api::{Action, Role, Scope, ServerClaims, Submit, new_id};
use kiln_server::{
    auth::{Authenticator, Enrollment},
    gateway,
    host::{Config, Host, bind_socket},
};
use p256::{elliptic_curve::sec1::ToEncodedPoint, pkcs8::EncodePrivateKey};
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt};
use tower::ServiceExt;

// Stopped-box inspection resolves Firecracker for process discovery. Run these
// metadata-only cases in a child with a private PATH, without changing the
// environment of concurrent tests or depending on an installed VMM.
fn run_with_fixture_firecracker(test: &str) -> bool {
    if std::env::var("KILN_ISOLATION_FIXTURE_TEST").as_deref() == Ok(test) {
        return false;
    }
    let bin = tempfile::tempdir().unwrap();
    let firecracker = bin.path().join("firecracker");
    // Discovery needs a file; preflight must fail and no launch may succeed.
    fs::write(&firecracker, "#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(&firecracker, fs::Permissions::from_mode(0o755)).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .env("PATH", bin.path())
        .env("KILN_ISOLATION_FIXTURE_TEST", test)
        .args(["--exact", test, "--nocapture"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{test}:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

struct Fixture {
    root: tempfile::TempDir,
    router: Router,
    host: Router,
    server: tokio::task::JoinHandle<()>,
    key: EncodingKey,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    fn seed_box(&self, account: Option<&str>, op: &Value) {
        let id = op["box_id"].as_str().unwrap();
        let dir = self.root.path().join("runtime/boxes").join(id);
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(
            dir.join("box.json"),
            serde_json::to_vec(&json!({
                "schema_version":1, "id":id, "name":format!("box-{id}"), "owner":account,
                "state":"stopped", "run":new_id(), "process":null, "host":"fixture",
                "memory_mib":256, "vcpus":1, "source":null, "operation":null, "last_error":null,
                "disk_copy":"copy", "image":{"schema_version":1,"architecture":"x86_64",
                "kernel_path":"/missing","kernel_sha256":"unused","rootfs_path":"/missing",
                "rootfs_sha256":"unused","agent_protocol_version":1}
            }))
            .unwrap(),
        )
        .unwrap();
    }

    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let mut cfg = Config {
            runtime_dir: root.path().join("runtime"),
            journal_dir: root.path().join("journal"),
            socket: root.path().join("host.sock"),
            isolation_config: None,
            allow_unsafe_development: true,
            templates: Default::default(),
        };
        drop(Host::open(&cfg).unwrap());
        let template = new_id();
        let dir = cfg.runtime_dir.join("snapshots").join(&template);
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        // Incompatible metadata rejects launch even on a fully configured host.
        fs::write(dir.join("snapshot.json"), serde_json::to_vec(&json!({
            "schema_version":1,"id":template,"source_box":new_id(),"template":true,
            "host":"incompatible-test-host","memory_mib":256,"vcpus":1,"hashes":{},
            "image":{"schema_version":1,"architecture":"x86_64","kernel_path":"/missing",
            "kernel_sha256":"unused","rootfs_path":"/missing","rootfs_sha256":"unused","agent_protocol_version":1}
        })).unwrap()).unwrap();
        cfg.templates.insert("small".into(), template);
        let host = Host::open(&cfg).unwrap().router();
        let listener = bind_socket(&cfg.socket).await.unwrap();
        let service = host.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, service).await.unwrap();
        });
        let key = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let pem = key.to_pkcs8_pem(Default::default()).unwrap();
        let point = key.public_key().to_encoded_point(false);
        let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let cache = root.path().join("jwks.json");
        fs::write(
            &cache,
            serde_json::to_vec(&json!({
                "issuer":"https://identity.example.test", "jwks":{"keys":[{
                "kty":"EC","crv":"P-256","use":"sig","alg":"ES256","kid":"fixture",
                "x":enc.encode(point.x().unwrap()),"y":enc.encode(point.y().unwrap())}]}
            }))
            .unwrap(),
        )
        .unwrap();
        let auth = Authenticator::central_only(
            Enrollment {
                version: 1,
                issuer: "https://identity.example.test".into(),
                owner_id: "owner".into(),
                server_id: "server".into(),
                tenant_id: Some("tenant-a".into()),
            },
            cache,
        )
        .unwrap();
        let router = gateway::router_with_auth(&cfg.socket, auth).unwrap();
        Self {
            root,
            router,
            host,
            server,
            key: EncodingKey::from_ec_pem(pem.as_bytes()).unwrap(),
        }
    }

    fn token(&self, account: &str, tenant: &str) -> String {
        let now = jsonwebtoken::get_current_timestamp();
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("fixture".into());
        jsonwebtoken::encode(
            &header,
            &ServerClaims {
                iss: "https://identity.example.test".into(),
                sub: account.into(),
                aud: "kiln:server:server".into(),
                tenant_id: Some(tenant.into()),
                iat: now,
                nbf: now,
                exp: now + 300,
                credential_id: format!("key-{account}"),
                scope: Scope::Operate,
                role: Some(if account == "owner" {
                    Role::Owner
                } else {
                    Role::Member
                }),
            },
            &self.key,
        )
        .unwrap()
    }

    async fn request(
        &self,
        account: &str,
        method: &str,
        path: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let response = self.router.clone().oneshot(Request::builder().method(method).uri(path)
            .header("authorization", format!("Bearer {}", self.token(account, "tenant-a")))
            // Untrusted clients cannot override the verified caller at the host.
            .header(kiln_api::OPERATION_ACTOR_HEADER, "owner")
            .header("x-kiln-account-role", "owner")
            .header("connection", "upgrade")
            .header("upgrade", kiln_api::SSH_UPGRADE)
            .header(kiln_api::SSH_KEY_HEADER, "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
            .header("content-type", "application/json")
            .body(if body.is_null() { Body::empty() } else { Body::from(serde_json::to_vec(&body).unwrap()) }).unwrap())
            .await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), kiln_api::MAX_RESPONSE_BYTES)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    async fn create(&self, account: &str, wait: bool) -> (Submit, Value) {
        let submit = Submit {
            id: new_id(),
            action: Action::Create {
                template: "small".into(),
                name: format!("{account}-box"),
            },
        };
        let path = if wait {
            "/v1/operations?wait=true&wait_seconds=1"
        } else {
            "/v1/operations"
        };
        let (status, mut operation) = self
            .request(
                account,
                "POST",
                path,
                serde_json::to_value(&submit).unwrap(),
            )
            .await;
        assert_eq!(
            status,
            if wait {
                StatusCode::OK
            } else {
                StatusCode::ACCEPTED
            }
        );
        for _ in 0..100 {
            if operation["state"] != "running" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            let (status, polled) = self
                .request(
                    account,
                    "GET",
                    &format!("/v1/operations/{}", submit.id),
                    Value::Null,
                )
                .await;
            assert_eq!(status, StatusCode::OK);
            operation = polled;
        }
        assert_eq!(
            operation["state"], "failed",
            "fixture must never reach a VM launch"
        );
        (submit, operation)
    }
}

#[tokio::test]
#[ignore = "requires host preflight: accessible KVM, cgroup v2 and Firecracker/jailer 1.17; launches no VM"]
async fn creator_is_persisted_and_members_only_list_their_own_boxes() {
    let f = Fixture::new().await;
    assert!(kiln_runtime::host::check().development_ready);
    let dir = fs::read_dir(f.root.path().join("runtime/snapshots"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let path = dir.join("snapshot.json");
    let mut snapshot: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    snapshot["host"] = kiln_runtime::host::fingerprint().unwrap().into();
    // Missing hashes fail AFTER reservation, but always BEFORE boot.
    fs::write(&path, serde_json::to_vec(&snapshot).unwrap()).unwrap();
    let (_, a) = f.create("member-a", false).await;
    let (_, b) = f.create("member-b", true).await;
    for (account, op) in [("member-a", &a), ("member-b", &b)] {
        let id = op["box_id"].as_str().unwrap();
        let saved: Value = serde_json::from_slice(
            &fs::read(
                f.root
                    .path()
                    .join("runtime/boxes")
                    .join(id)
                    .join("box.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            saved["owner"], account,
            "ownership must survive even a failed launch"
        );
        let (status, boxes) = f.request(account, "GET", "/v1/boxes", Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(boxes.as_array().unwrap().len(), 1);
        assert_eq!(boxes[0]["id"], id);
        assert_eq!(boxes[0]["owner"], account);
    }
    let (status, boxes) = f.request("owner", "GET", "/v1/boxes", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(boxes.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn inventories_hide_other_members_and_legacy_owner_boxes() {
    if run_with_fixture_firecracker("inventories_hide_other_members_and_legacy_owner_boxes") {
        return;
    }
    let f = Fixture::new().await;
    let (_, a) = f.create("member-a", false).await;
    let (_, b) = f.create("member-b", true).await;
    let (_, legacy) = f.create("owner", false).await;
    f.seed_box(Some("member-a"), &a);
    f.seed_box(Some("member-b"), &b);
    f.seed_box(None, &legacy);
    for (account, own) in [("member-a", &a), ("member-b", &b)] {
        for path in ["/v1/boxes", "/api/v1/dashboard"] {
            let (status, body) = f.request(account, "GET", path, Value::Null).await;
            assert_eq!(status, StatusCode::OK);
            let boxes = if path == "/v1/boxes" {
                &body
            } else {
                &body["boxes"]
            };
            assert_eq!(boxes.as_array().unwrap().len(), 1, "{account} {path}");
            assert_eq!(boxes[0]["id"], own["box_id"]);
        }
        let path = format!("/v1/boxes/{}", legacy["box_id"].as_str().unwrap());
        assert_eq!(
            f.request(account, "GET", &path, Value::Null).await.0,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        f.request("owner", "GET", "/v1/boxes", Value::Null)
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn cross_member_operations_reads_and_replays_are_hidden() {
    if run_with_fixture_firecracker("cross_member_operations_reads_and_replays_are_hidden") {
        return;
    }
    let f = Fixture::new().await;
    let (submit, op) = f.create("member-a", false).await;
    // Acceptance persists the actor even when preflight failed before box.json.
    let operation_path = format!("/v1/operations/{}", submit.id);
    for account in ["member-a", "member-b"] {
        let expected = if account == "member-a" {
            StatusCode::OK
        } else {
            StatusCode::NOT_FOUND
        };
        assert_eq!(
            f.request(account, "GET", &operation_path, Value::Null)
                .await
                .0,
            expected
        );
        for path in ["/v1/operations", "/v1/operations?wait=true&wait_seconds=1"] {
            assert_eq!(
                f.request(
                    account,
                    "POST",
                    path,
                    serde_json::to_value(&submit).unwrap()
                )
                .await
                .0,
                expected
            );
        }
    }
    f.seed_box(Some("member-a"), &op);
    let box_id = op["box_id"].as_str().unwrap();
    for account in ["member-b", "member-a", "owner"] {
        let expected = if account == "member-b" {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::OK
        };
        for path in [
            format!("/v1/boxes/{box_id}"),
            format!("/v1/operations/{}", submit.id),
        ] {
            assert_eq!(
                f.request(account, "GET", &path, Value::Null).await.0,
                expected,
                "{account} {path}"
            );
        }
        assert_eq!(
            f.request(
                account,
                "POST",
                "/v1/operations",
                serde_json::to_value(&submit).unwrap()
            )
            .await
            .0,
            expected
        );
    }
    let changed = Submit {
        id: submit.id.clone(),
        action: Action::Delete { id: box_id.into() },
    };
    assert_eq!(
        f.request(
            "member-b",
            "POST",
            "/v1/operations",
            serde_json::to_value(&changed).unwrap()
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.request(
            "member-a",
            "POST",
            "/v1/operations",
            serde_json::to_value(&changed).unwrap()
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    for action in [
        json!({"type":"stop","id":box_id,"force":true}),
        json!({"type":"start","id":box_id}),
        json!({"type":"pause","id":box_id}),
        json!({"type":"resume","id":box_id}),
        json!({"type":"delete","id":box_id}),
        json!({"type":"checkpoint","id":box_id}),
        json!({"type":"restore","id":box_id,"checkpoint":new_id(),"acknowledge_external_state_replay":true}),
        json!({"type":"exec","id":box_id,"request":{"argv":["true"],"timeout_ms":1000}}),
    ] {
        let request = json!({"id":new_id(),"action":action});
        let (status, body) = f
            .request("member-b", "POST", "/v1/operations", request.clone())
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{request}: {body}");
    }
    for suffix in ["ssh-key", "ssh"] {
        assert_eq!(
            f.request(
                "member-b",
                "GET",
                &format!("/v1/boxes/{box_id}/{suffix}"),
                Value::Null
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
    // A successful own-box deletion is replayable after its metadata is gone;
    // another member cannot read or replay that operation afterward either.
    let delete = json!({"id":new_id(),"action":{"type":"delete","id":box_id}});
    let (status, _) = f
        .request("member-a", "POST", "/v1/operations", delete.clone())
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let path = format!("/v1/operations/{}", delete["id"].as_str().unwrap());
    let mut done = Value::Null;
    for _ in 0..100 {
        done = f.request("member-a", "GET", &path, Value::Null).await.1;
        if done["state"] != "running" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(done["state"], "succeeded");
    assert_eq!(
        f.request("member-a", "POST", "/v1/operations", delete.clone())
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        f.request("member-b", "POST", "/v1/operations", delete)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.request("member-b", "GET", &path, Value::Null).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn wrong_tenant_and_partial_host_identity_fail_closed() {
    let f = Fixture::new().await;
    let response = f
        .router
        .clone()
        .oneshot(
            Request::get("/v1/boxes")
                .header(
                    "authorization",
                    format!("Bearer {}", f.token("member-a", "tenant-b")),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    for (name, value) in [
        (kiln_api::OPERATION_ACTOR_HEADER, "member-a"),
        ("x-kiln-account-role", "member"),
    ] {
        let response = f
            .host
            .clone()
            .oneshot(
                Request::get("/v1/boxes")
                    .header(name, value)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "partial trusted identity must not become local admin"
        );
    }
    let (submit, _) = f.create("member-a", false).await;
    let db = rusqlite::Connection::open(f.root.path().join("journal/operations.sqlite")).unwrap();
    db.execute("UPDATE operations SET actor=NULL WHERE id=?1", [&submit.id])
        .unwrap();
    for account in ["member-a", "owner"] {
        let expected = if account == "owner" {
            StatusCode::OK
        } else {
            StatusCode::NOT_FOUND
        };
        assert_eq!(
            f.request(
                account,
                "GET",
                &format!("/v1/operations/{}", submit.id),
                Value::Null
            )
            .await
            .0,
            expected
        );
        assert_eq!(
            f.request(
                account,
                "POST",
                "/v1/operations",
                serde_json::to_value(&submit).unwrap()
            )
            .await
            .0,
            expected
        );
    }
}

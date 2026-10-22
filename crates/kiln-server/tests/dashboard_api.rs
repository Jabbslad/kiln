use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use kiln_api::{
    ApiError, BoxView, ExecResult, Operation, OperationState, Outcome,
    auth::{MemberRole, Scope, ServerClaims},
};
use kiln_runtime::{
    image::{BootMode, Manifest},
    runtime::BoxRecord,
};
use kiln_server::{
    auth::{Authenticator, Enrollment},
    gateway,
    host::{Config, Host, bind_socket},
};
use serde_json::{Value, json};
use tower::ServiceExt;

const WORKSPACE: &str = "server-b";
const STDOUT: &str = "secret-argv-7183 secret-stdout-7183";
const STDERR: &str = "secret-stderr-7183";
const KERNEL: &str = "/private/canary-kernel";
const ROOTFS: &str = "/private/canary-rootfs";
const PATH_ERROR: &str = "/private/canary-error";
const PATH_DETAIL: &str = "/var/lib/kiln/op-path-canary";
const CORRUPT_ROW: &str = "CORRUPT-ROW-NOT-JSON";
const SAFE_ERROR: &str = "interrupted launch stopped; disk retained for explicit start or restore";

fn hid(n: u64) -> String {
    format!("{n:032x}")
}

fn manifest() -> Manifest {
    Manifest {
        schema_version: 1,
        architecture: "x86_64".into(),
        kernel_path: KERNEL.into(),
        kernel_sha256: "ab".repeat(32),
        rootfs_path: ROOTFS.into(),
        rootfs_sha256: "cd".repeat(32),
        agent_protocol_version: 1,
        boot_mode: BootMode::Init,
    }
}

fn write_box(runtime: &Path, id: &str, record: &BoxRecord, disk: Option<&[u8]>) {
    let directory = runtime.join("boxes").join(id).join(&record.run);
    fs::create_dir_all(&directory).unwrap();
    fs::set_permissions(
        runtime.join("boxes").join(id),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        runtime.join("boxes").join(id).join("box.json"),
        serde_json::to_vec(record).unwrap(),
    )
    .unwrap();
    if let Some(bytes) = disk {
        fs::write(directory.join("disk.ext4"), bytes).unwrap();
    }
}

fn snapshot(runtime: &Path, id: &str, memory_mib: u32, vcpus: u8, disk: Option<&[u8]>) {
    let directory = runtime.join("snapshots").join(id);
    fs::create_dir_all(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        directory.join("snapshot.json"),
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "id": id,
            "source_box": hid(9),
            "template": true,
            "host": "fixture-host",
            "memory_mib": memory_mib,
            "vcpus": vcpus,
            "hashes": {},
            "image": {
                "schema_version": 1,
                "architecture": "x86_64",
                "kernel_path": "/private/canary-template",
                "kernel_sha256": "unused",
                "rootfs_path": "/private/canary-template-root",
                "rootfs_sha256": "unused",
                "agent_protocol_version": 1
            }
        }))
        .unwrap(),
    )
    .unwrap();
    if let Some(bytes) = disk {
        fs::write(directory.join("disk.ext4"), bytes).unwrap();
    }
}

fn insert(
    db: &rusqlite::Connection,
    id: &str,
    box_id: &str,
    create: bool,
    at: i64,
    operation: &Operation,
) {
    db.execute(
        "INSERT INTO operations(id, fingerprint, box_id, is_create, running, response, created_at)
         VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6)",
        rusqlite::params![
            id,
            vec![0u8; 32],
            box_id,
            i64::from(create),
            serde_json::to_string(operation).unwrap(),
            at,
        ],
    )
    .unwrap();
}

fn operation(
    id: &str,
    box_id: &str,
    state: OperationState,
    result: Option<Outcome>,
    message: Option<&str>,
) -> Operation {
    Operation {
        id: id.into(),
        box_id: box_id.into(),
        state,
        result,
        error: message.map(|message| ApiError {
            code: "runtime_operation_failed".into(),
            message: message.into(),
        }),
    }
}

struct Fixture {
    socket: std::path::PathBuf,
    root: tempfile::TempDir,
    ready: String,
    bare: String,
    alpha: String,
    beta: String,
    source: String,
    create_alpha: String,
    create_beta: String,
    failed: String,
    exec: String,
    unknown: String,
    oldest: String,
    path_op: String,
    corrupt: String,
}

async fn fixture() -> (Fixture, Arc<AtomicUsize>) {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let runtime = root.path().join("runtime");
    let journal = root.path().join("journal");
    let mut config = Config {
        runtime_dir: runtime.clone(),
        journal_dir: journal.clone(),
        socket: root.path().join("host.sock"),
        isolation_config: None,
        allow_unsafe_development: true,
        templates: Default::default(),
    };
    drop(Host::open(&config).unwrap());
    let ready = hid(3);
    let bare = hid(4);
    snapshot(&runtime, &ready, 384, 2, Some(b"disks"));
    snapshot(&runtime, &bare, 256, 1, None);
    config.templates.insert("ready".into(), ready.clone());
    config.templates.insert("bare".into(), bare.clone());
    let alpha = hid(0xa1);
    let beta = hid(0xb1);
    let alpha_run = hid(5);
    let beta_run = hid(6);
    let source = hid(7);
    write_box(
        &runtime,
        &alpha,
        &BoxRecord {
            schema_version: 1,
            id: alpha.clone(),
            name: "alpha".into(),
            state: "starting".into(),
            run: alpha_run,
            process: None,
            image: manifest(),
            host: "fixture-host".into(),
            memory_mib: 256,
            vcpus: 1,
            source: None,
            operation: None,
            last_error: Some(PATH_ERROR.into()),
            disk_copy: "Copy".into(),
            disk_layer: None,
            jail: None,
            network: None,
            last_launch: None,
        },
        None,
    );
    write_box(
        &runtime,
        &beta,
        &BoxRecord {
            schema_version: 1,
            id: beta.clone(),
            name: "beta".into(),
            state: "running".into(),
            run: beta_run,
            process: None,
            image: manifest(),
            host: "fixture-host".into(),
            memory_mib: 512,
            vcpus: 2,
            source: Some(source.clone()),
            operation: Some("start".into()),
            last_error: Some(SAFE_ERROR.into()),
            disk_copy: "Copy".into(),
            disk_layer: None,
            jail: None,
            network: None,
            last_launch: Some(kiln_runtime::launch_timing::LaunchTiming {
                method: "start".into(),
                cache: "uncontrolled".into(),
                disk_copy: "Copy".into(),
                storage_ms: None,
                vmm_ms: Some(1200.),
                restore_ms: None,
                agent_ms: Some(400.),
                init_ms: Some(300.),
                first_command_ms: None,
                total_ms: Some(2500.),
            }),
        },
        Some(b"boxdisk!!"),
    );
    let host = Host::open(&config).unwrap();
    let db = rusqlite::Connection::open(journal.join("operations.sqlite")).unwrap();
    db.busy_timeout(std::time::Duration::from_secs(2)).unwrap();
    let create_alpha = hid(201);
    let create_beta = hid(202);
    let failed = hid(203);
    let exec = hid(204);
    let unknown = hid(205);
    let oldest = hid(1);
    let path_op = hid(206);
    let corrupt = hid(207);
    insert(
        &db,
        &create_alpha,
        &alpha,
        true,
        1_700_000_100,
        &operation(&create_alpha, &alpha, OperationState::Succeeded, None, None),
    );
    insert(
        &db,
        &create_beta,
        &beta,
        true,
        1_700_000_000,
        &operation(
            &create_beta,
            &beta,
            OperationState::Succeeded,
            Some(Outcome::Box(BoxView {
                id: beta.clone(),
                name: "beta".into(),
                state: "running".into(),
                memory_mib: 512,
                vcpus: 2,
            })),
            None,
        ),
    );
    insert(
        &db,
        &failed,
        &beta,
        false,
        1_700_000_200,
        &operation(
            &failed,
            &beta,
            OperationState::Failed,
            None,
            Some("template was not prepared"),
        ),
    );
    insert(
        &db,
        &exec,
        &beta,
        false,
        1_700_000_300,
        &operation(
            &exec,
            &beta,
            OperationState::Succeeded,
            Some(Outcome::Exec(ExecResult {
                stdout: STDOUT.as_bytes().to_vec(),
                stderr: STDERR.as_bytes().to_vec(),
                exit_code: Some(7),
                truncated: false,
                timed_out: false,
            })),
            Some("guest command failed"),
        ),
    );
    insert(
        &db,
        &unknown,
        &beta,
        false,
        1_700_000_400,
        &operation(
            &unknown,
            &beta,
            OperationState::Unknown,
            None,
            Some("unknown-row-marker"),
        ),
    );
    insert(
        &db,
        &oldest,
        &beta,
        false,
        1,
        &operation(
            &oldest,
            &beta,
            OperationState::Succeeded,
            None,
            Some("oldest-omitted"),
        ),
    );
    insert(
        &db,
        &path_op,
        &beta,
        false,
        1_700_000_500,
        &operation(
            &path_op,
            &beta,
            OperationState::Failed,
            None,
            Some(PATH_DETAIL),
        ),
    );
    db.execute(
        "INSERT INTO operations(id, fingerprint, box_id, is_create, running, response, created_at)
         VALUES (?1, ?2, ?3, 0, 0, ?4, ?5)",
        rusqlite::params![corrupt, vec![0u8; 32], beta, CORRUPT_ROW, 1_700_000_600_i64],
    )
    .unwrap();
    for at in 2_i64..=96 {
        let id = hid(300 + u64::try_from(at).unwrap());
        insert(
            &db,
            &id,
            &beta,
            false,
            at,
            &operation(&id, &beta, OperationState::Succeeded, None, None),
        );
    }
    drop(db);
    let hits = Arc::new(AtomicUsize::new(0));
    let seen = hits.clone();
    let router = host.router().layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let seen = seen.clone();
            async move {
                seen.fetch_add(1, Ordering::SeqCst);
                next.run(request).await
            }
        },
    ));
    let listener = bind_socket(&config.socket).await.unwrap();
    let socket = config.socket.clone();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (
        Fixture {
            socket,
            root,
            ready,
            bare,
            alpha,
            beta,
            source,
            create_alpha,
            create_beta,
            failed,
            exec,
            unknown,
            oldest,
            path_op,
            corrupt,
        },
        hits,
    )
}

fn enrollment(cache: &Path) -> (Authenticator, EncodingKey) {
    use base64::Engine;
    use p256::{elliptic_curve::sec1::ToEncodedPoint, pkcs8::EncodePrivateKey};
    let key = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
    let pem = key.to_pkcs8_pem(Default::default()).unwrap();
    let point = key.public_key().to_encoded_point(false);
    let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let jwks = json!({"keys":[{
        "kty":"EC","crv":"P-256","use":"sig","alg":"ES256","kid":"first",
        "x":enc.encode(point.x().unwrap()),"y":enc.encode(point.y().unwrap())
    }]});
    let enrollment = Enrollment {
        version: 1,
        issuer: "https://identity.example.test".into(),
        owner_id: "owner-a".into(),
        server_id: WORKSPACE.into(),
    };
    fs::write(
        cache,
        serde_json::to_vec(&json!({"issuer": enrollment.issuer, "jwks": jwks})).unwrap(),
    )
    .unwrap();
    let auth =
        Authenticator::new(&"b".repeat(64), Some((enrollment, cache.to_path_buf()))).unwrap();
    (auth, EncodingKey::from_ec_pem(pem.as_bytes()).unwrap())
}

fn read_jwt(key: &EncodingKey) -> String {
    let now = jsonwebtoken::get_current_timestamp();
    let claims = ServerClaims {
        iss: "https://identity.example.test".into(),
        sub: "owner-a".into(),
        aud: format!("kiln:server:{WORKSPACE}"),
        tenant_id: Some("ten_test".into()),
        iat: now,
        nbf: now,
        exp: now + 300,
        credential_id: "device-1".into(),
        scope: Scope::Read,
        role: Some(MemberRole::Owner),
    };
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("first".into());
    jsonwebtoken::encode(&header, &claims, key).unwrap()
}

async fn get(router: &axum::Router, token: Option<&str>, uri: &str) -> axum::response::Response {
    let mut request = Request::builder().uri(uri);
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

fn assert_document(raw: &str, value: &Value, fixture: &Fixture) {
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["workspace"], WORKSPACE);
    assert_eq!(value["host"]["name"], WORKSPACE);
    assert_eq!(value["host"]["state"], "online");
    assert!(value["host"]["metrics"]["cpu_percent"].as_f64().is_some());
    assert!(value["host"]["metrics"]["memory_bytes"].as_u64().unwrap() > 0);
    assert!(value["host"]["metrics"]["disk_bytes"].as_u64().unwrap() > 0);
    assert!(
        value["host"]["metrics"]["disk_total_bytes"]
            .as_u64()
            .unwrap()
            > 0
    );
    let observed = value["observed_at"].as_str().unwrap();
    assert_eq!(observed.len(), 20, "{observed}");
    assert!(observed.ends_with('Z'), "{observed}");
    assert_eq!(observed.as_bytes()[10], b'T', "{observed}");
    assert_eq!(observed.as_bytes()[4], b'-', "{observed}");
    assert_eq!(observed.as_bytes()[13], b':', "{observed}");
    assert_ne!(observed, "2023-11-14T22:13:20Z");
    assert_eq!(value["checkpoints"], json!([]));
    assert_eq!(value["boxes"].as_array().unwrap().len(), 2);

    let alpha = &value["boxes"][0];
    assert_eq!(alpha["id"], fixture.alpha);
    assert_eq!(alpha["name"], "alpha");
    assert_eq!(alpha["state"], "unknown");
    assert_eq!(alpha["image"], "");
    assert!(alpha["resources"].is_null());
    assert!(alpha["metrics"]["cpu_percent"].is_null());
    assert!(alpha["timing"].is_null());
    assert!(alpha["last_error"].is_null());
    assert!(alpha["last_operation"].is_null());
    assert!(alpha["restored_from"].is_null());
    assert_eq!(alpha["created_at"], "2023-11-14T22:15:00Z");

    let beta = &value["boxes"][1];
    assert_eq!(beta["id"], fixture.beta);
    assert_eq!(beta["name"], "beta");
    assert_eq!(beta["state"], "running");
    assert_eq!(beta["image"], "");
    assert_eq!(beta["resources"]["vcpus"], 2);
    assert_eq!(beta["resources"]["memory_bytes"], 512 * 1024 * 1024);
    assert_eq!(beta["resources"]["disk_bytes"], b"boxdisk!!".len());
    assert!(beta["metrics"]["disk_bytes"].as_u64().unwrap() >= b"boxdisk!!".len() as u64);
    assert!(beta["metrics"]["memory_bytes"].is_null());
    assert_eq!(beta["timing"]["total_ms"].as_f64(), Some(2500.));
    assert_eq!(beta["last_error"], SAFE_ERROR);
    assert_eq!(beta["last_operation"], "start");
    assert_eq!(beta["restored_from"], fixture.source);
    assert_eq!(beta["created_at"], "2023-11-14T22:13:20Z");

    assert_eq!(value["templates"].as_array().unwrap().len(), 1);
    let template = &value["templates"][0];
    assert_eq!(template["id"], fixture.ready);
    assert_eq!(template["name"], "ready");
    assert_eq!(template["version"], "");
    assert_eq!(template["description"], "");
    assert_eq!(template["compatible"], true);
    assert_eq!(template["development_only"], true);
    assert_eq!(template["resources"]["vcpus"], 2);
    assert_eq!(template["resources"]["memory_bytes"], 384 * 1024 * 1024);
    assert_eq!(template["resources"]["disk_bytes"], b"disks".len());
    assert!(!raw.contains(&fixture.bare));

    let operations = value["operations"].as_array().unwrap();
    assert_eq!(operations.len(), 98);
    let by_id = |id: &str| operations.iter().find(|row| row["id"] == id).unwrap();
    let create = by_id(&fixture.create_beta);
    assert_eq!(create["kind"], "create");
    assert_eq!(create["state"], "completed");
    assert_eq!(create["box_name"], "beta");
    assert_eq!(create["detail"], "");
    assert_eq!(create["steps"], json!([]));
    assert_eq!(create["created_at"], "2023-11-14T22:13:20Z");
    let failed = by_id(&fixture.failed);
    assert_eq!(failed["kind"], "unknown");
    assert_eq!(failed["state"], "failed");
    assert_eq!(failed["detail"], "template was not prepared");
    let exec = by_id(&fixture.exec);
    assert_eq!(exec["state"], "completed");
    assert_eq!(exec["detail"], "guest command failed");
    assert_eq!(exec["kind"], "unknown");
    assert!(
        operations
            .iter()
            .any(|row| row["id"] == fixture.create_alpha)
    );
    assert!(operations.iter().all(|row| row["id"] != fixture.unknown));
    assert!(operations.iter().all(|row| row["id"] != fixture.oldest));
    assert!(operations.iter().all(|row| row["id"] != fixture.corrupt));
    let path_op = by_id(&fixture.path_op);
    assert_eq!(path_op["detail"], "");
    assert_eq!(path_op["state"], "failed");
    for secret in [
        STDOUT,
        STDERR,
        "secret-argv-7183",
        "secret-stdout-7183",
        KERNEL,
        ROOTFS,
        PATH_ERROR,
        PATH_DETAIL,
        CORRUPT_ROW,
        "unknown-row-marker",
        "oldest-omitted",
        "/private/canary-template",
        "argv",
    ] {
        assert!(!raw.contains(secret), "{secret}");
    }
    assert!(!raw.contains(fixture.root.path().to_str().unwrap()));
}

#[tokio::test]
async fn read_scope_and_admin_token_receive_schema_version_one() {
    let (fixture, hits) = fixture().await;
    let (auth, key) = enrollment(&fixture.root.path().join("jwks.json"));
    let router = gateway::router_with_workspace(&fixture.socket, auth, WORKSPACE.into()).unwrap();
    for (label, token) in [("missing", None), ("bogus", Some("not-a-jwt"))] {
        let response = get(&router, token, "/api/v1/dashboard").await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{label}");
        assert_eq!(hits.load(Ordering::SeqCst), 0, "{label}");
    }
    let response = get(&router, Some(&"b".repeat(64)), "/api/v1/dashboard?x=1").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(hits.load(Ordering::SeqCst), 0);

    for token in ["b".repeat(64), read_jwt(&key)] {
        let response = get(&router, Some(&token), "/api/v1/dashboard").await;
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(content_type.contains("application/json"), "{content_type}");
        assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
        let bytes = to_bytes(response.into_body(), kiln_api::MAX_RESPONSE_BYTES)
            .await
            .unwrap();
        let raw = String::from_utf8(bytes.to_vec()).unwrap();
        let value: Value = serde_json::from_str(&raw).unwrap();
        assert_document(&raw, &value, &fixture);
    }
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn unreachable_host_is_offline() {
    let root = tempfile::tempdir().unwrap();
    let token = "c".repeat(64);
    let router = gateway::router_with_workspace(
        &root.path().join("missing.sock"),
        Authenticator::new(&token, None).unwrap(),
        WORKSPACE.into(),
    )
    .unwrap();
    let response = get(&router, None, "/api/v1/dashboard").await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = get(&router, Some(&token), "/api/v1/dashboard").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("application/json")
    );
    assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    let raw = String::from_utf8(bytes.to_vec()).unwrap();
    let value: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["workspace"], WORKSPACE);
    assert_eq!(value["host"]["name"], WORKSPACE);
    assert_eq!(value["host"]["state"], "offline");
    assert!(value["host"]["metrics"]["cpu_percent"].is_null());
    assert_eq!(value["boxes"], json!([]));
    assert_eq!(value["templates"], json!([]));
    assert_eq!(value["checkpoints"], json!([]));
    assert_eq!(value["operations"], json!([]));
    assert!(!raw.contains(root.path().to_str().unwrap()));
}

#[tokio::test]
async fn dashboard_schema_accepts_enrolled_and_unenrolled_servers() {
    let (fixture, hits) = fixture().await;
    let (auth, key) = enrollment(&fixture.root.path().join("schema-jwks.json"));
    let enrolled = gateway::router_with_workspace(&fixture.socket, auth, WORKSPACE.into()).unwrap();
    let enrolled_body =
        document_body(get(&enrolled, Some(&read_jwt(&key)), "/api/v1/dashboard").await).await;
    assert_eq!(enrolled_body["workspace"], WORKSPACE);
    assert_eq!(enrolled_body["host"]["name"], WORKSPACE);
    assert_eq!(enrolled_body["host"]["state"], "online");

    let open_token = "d".repeat(64);
    let open = gateway::router_with_auth(
        &fixture.socket,
        Authenticator::new(&open_token, None).unwrap(),
    )
    .unwrap();
    for (label, token) in [("missing", None), ("bogus", Some("not-a-jwt"))] {
        let response = get(&open, token, "/api/v1/dashboard").await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{label}");
    }
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "rejected tokens dialed the host"
    );
    let online = document_body(get(&open, Some(&open_token), "/api/v1/dashboard").await).await;
    assert_eq!(online["workspace"], "local");
    assert_eq!(online["host"]["name"], "local");
    assert_eq!(online["host"]["state"], "online");
    assert!(!online["boxes"].as_array().unwrap().is_empty());

    let root = tempfile::tempdir().unwrap();
    let closed_token = "e".repeat(64);
    let closed = gateway::router_with_auth(
        &root.path().join("missing.sock"),
        Authenticator::new(&closed_token, None).unwrap(),
    )
    .unwrap();
    let denied = get(&closed, Some("not-a-jwt"), "/api/v1/dashboard").await;
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let offline = document_body(get(&closed, Some(&closed_token), "/api/v1/dashboard").await).await;
    assert_eq!(offline["workspace"], "local");
    assert_eq!(offline["host"]["name"], "local");
    assert_eq!(offline["host"]["state"], "offline");
    assert_eq!(offline["boxes"], json!([]));
    assert_eq!(hits.load(Ordering::SeqCst), 2);

    parse_with_dashboard_schema(&json!({
        "enrolled": enrolled_body,
        "unenrolled_online": online,
        "unenrolled_offline": offline,
    }));
}

async fn document_body(response: axum::response::Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), kiln_api::MAX_RESPONSE_BYTES)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn parse_with_dashboard_schema(cases: &Value) {
    let web = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web");
    if !web.join("node_modules/zod/package.json").exists() {
        let install = std::process::Command::new("npm")
            .args(["ci"])
            .current_dir(&web)
            .status()
            .expect("npm ci");
        assert!(install.success(), "npm ci failed");
    }
    let mut child = std::process::Command::new("node")
        .args([
            "--experimental-strip-types",
            "--input-type=module",
            "-e",
            r#"import { dashboardSchema } from './src/data.ts'
const chunks = []
for await (const chunk of process.stdin) chunks.push(chunk)
const cases = JSON.parse(Buffer.concat(chunks).toString())
for (const name of ['enrolled', 'unenrolled_online', 'unenrolled_offline']) {
  dashboardSchema.parse(cases[name])
}"#,
        ])
        .current_dir(&web)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("node");
    std::io::Write::write_all(
        child.stdin.as_mut().unwrap(),
        &serde_json::to_vec(cases).unwrap(),
    )
    .unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "dashboardSchema rejected gateway JSON\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

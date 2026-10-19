use axum::{
    Router,
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use kiln_identity::{Config, Store};
use p256::pkcs8::EncodePrivateKey;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    net::SocketAddr,
    sync::atomic::{AtomicI64, Ordering},
};
use tower::ServiceExt;

static CLOCK: AtomicI64 = AtomicI64::new(1_700_000_000);
fn now() -> i64 {
    CLOCK.load(Ordering::SeqCst)
}

const ORIGIN: &str = "https://identity.example.test";
const COOKIE: &str = "__Host-kiln_session=browser; __Host-kiln_csrf=csrf";

struct Fixture {
    root: tempfile::TempDir,
    app: Router,
    store: Store,
}

impl Fixture {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let key = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let key_path = root.path().join("signing.pem");
        std::fs::write(
            &key_path,
            key.to_pkcs8_pem(Default::default()).unwrap().as_bytes(),
        )
        .unwrap();
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let store = Store::open(&root.path().join("identity.db")).unwrap();
        let cfg = Config {
            public_origin: ORIGIN.into(),
            github_client_id: "fixture-github".into(),
            github_secret_file: key_path.clone(),
            google_client_id: "fixture-google".into(),
            google_secret_file: key_path.clone(),
            signing_key_file: key_path,
            signing_key_id: "fixture-key".into(),
            retiring_jwks_file: None,
            trusted_proxy_loopback: false,
            now,
        };
        let app = kiln_identity::router(cfg, store.clone()).unwrap();
        Self { root, app, store }
    }

    fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.root.path().join("identity.db")).unwrap()
    }
}

async fn send(
    app: &Router,
    path: &str,
    ip: &str,
    origin: Option<&str>,
    cookie: Option<&str>,
    body: &str,
) -> (StatusCode, Vec<u8>) {
    let mut req = Request::builder()
        .method("POST")
        .uri(path)
        .extension(ConnectInfo(
            format!("{ip}:1234").parse::<SocketAddr>().unwrap(),
        ))
        .header("content-type", "application/x-www-form-urlencoded");
    if let Some(origin) = origin {
        req = req.header("origin", origin);
    }
    if let Some(cookie) = cookie {
        req = req.header("cookie", cookie);
    }
    let response = app
        .clone()
        .oneshot(req.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 65_536).await.unwrap();
    (status, bytes.to_vec())
}

fn error_of(bytes: &[u8]) -> String {
    serde_json::from_slice::<Value>(bytes).unwrap()["error"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn approval_body(code: &str) -> String {
    format!("csrf=csrf&user_code={code}&decision=approve")
}

#[tokio::test]
async fn unauthenticated_approvals_hit_the_ip_limit() {
    let f = Fixture::new();
    let body = approval_body("ABCD23456789");
    for attempt in 1..=5 {
        let (status, bytes) = send(
            &f.app,
            "/device/approve",
            "192.0.2.10",
            Some(ORIGIN),
            Some(COOKIE),
            &body,
        )
        .await;
        assert_eq!(
            (status, error_of(&bytes)),
            (StatusCode::UNAUTHORIZED, "login_required".to_owned()),
            "attempt {attempt}"
        );
    }
    let (status, bytes) = send(
        &f.app,
        "/device/approve",
        "192.0.2.10",
        Some(ORIGIN),
        Some(COOKIE),
        &body,
    )
    .await;
    assert_eq!(
        (status, error_of(&bytes)),
        (StatusCode::TOO_MANY_REQUESTS, "rate_limited".to_owned())
    );
    let (status, bytes) = send(
        &f.app,
        "/device/approve",
        "192.0.2.11",
        Some(ORIGIN),
        Some(COOKIE),
        &body,
    )
    .await;
    assert_eq!(
        (status, error_of(&bytes)),
        (StatusCode::UNAUTHORIZED, "login_required".to_owned())
    );
}

#[tokio::test]
async fn bad_token_grants_hit_the_ip_limit() {
    let f = Fixture::new();
    let body = "grant_type=urn:ietf:params:oauth:grant-type:device_code&device_code=missing";
    for attempt in 1..=30 {
        let (status, bytes) = send(&f.app, "/oauth/token", "192.0.2.20", None, None, body).await;
        assert_eq!(
            (status, error_of(&bytes)),
            (StatusCode::BAD_REQUEST, "invalid_grant".to_owned()),
            "attempt {attempt}"
        );
    }
    let (status, bytes) = send(&f.app, "/oauth/token", "192.0.2.20", None, None, body).await;
    assert_eq!(
        (status, error_of(&bytes)),
        (StatusCode::TOO_MANY_REQUESTS, "rate_limited".to_owned())
    );
    let (status, bytes) = send(&f.app, "/oauth/token", "192.0.2.21", None, None, body).await;
    assert_eq!(
        (status, error_of(&bytes)),
        (StatusCode::BAD_REQUEST, "invalid_grant".to_owned())
    );
}

#[tokio::test]
async fn pending_device_polls_are_not_rate_limited_at_the_login_cadence() {
    let f = Fixture::new();
    let grant = f
        .store
        .create_device("laptop", "192.0.2.30", now())
        .unwrap();
    let body = format!(
        "grant_type=urn:ietf:params:oauth:grant-type:device_code&device_code={}",
        grant.device_code
    );
    for attempt in 1..=12 {
        let (status, bytes) = send(&f.app, "/oauth/token", "192.0.2.30", None, None, &body).await;
        assert_ne!(status, StatusCode::TOO_MANY_REQUESTS, "attempt {attempt}");
        assert_eq!(
            (status, error_of(&bytes)),
            (StatusCode::BAD_REQUEST, "slow_down".to_owned()),
            "attempt {attempt}"
        );
    }
}

#[tokio::test]
async fn a_live_session_can_still_approve_and_receive_a_token() {
    let f = Fixture::new();
    let owner = f
        .store
        .seed_account("github", "42", "tester", now())
        .unwrap();
    let grant = f
        .store
        .create_device("laptop", "192.0.2.40", now())
        .unwrap();
    f.db()
        .execute(
            "INSERT INTO browser_sessions(hash,csrf_hash,account_id,expires_at) VALUES(?1,?2,?3,?4)",
            rusqlite::params![
                Sha256::digest(b"browser").to_vec(),
                Sha256::digest(b"csrf").to_vec(),
                owner,
                now() + 43_200,
            ],
        )
        .unwrap();
    let (status, _) = send(
        &f.app,
        "/device/approve",
        "192.0.2.41",
        Some(ORIGIN),
        Some(COOKIE),
        &approval_body(&grant.user_code),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    CLOCK.fetch_add(5, Ordering::SeqCst);
    let (status, bytes) = send(
        &f.app,
        "/oauth/token",
        "192.0.2.42",
        None,
        None,
        &format!(
            "grant_type=urn:ietf:params:oauth:grant-type:device_code&device_code={}",
            grant.device_code
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(body["access_token"].is_string());
}

#[tokio::test]
async fn sqlite_errors_on_token_and_approval_are_server_errors() {
    let f = Fixture::new();
    f.db().execute("DROP TABLE pending_devices", []).unwrap();
    let (status, bytes) = send(
        &f.app,
        "/oauth/token",
        "192.0.2.50",
        None,
        None,
        "grant_type=urn:ietf:params:oauth:grant-type:device_code&device_code=missing",
    )
    .await;
    assert_eq!(
        (status, error_of(&bytes)),
        (StatusCode::INTERNAL_SERVER_ERROR, "server_error".to_owned())
    );
    f.db().execute("DROP TABLE refresh_tokens", []).unwrap();
    let (status, bytes) = send(
        &f.app,
        "/oauth/token",
        "192.0.2.51",
        None,
        None,
        "grant_type=refresh_token&refresh_token=missing",
    )
    .await;
    assert_eq!(
        (status, error_of(&bytes)),
        (StatusCode::INTERNAL_SERVER_ERROR, "server_error".to_owned())
    );
    f.db().execute("DROP TABLE browser_sessions", []).unwrap();
    let (status, bytes) = send(
        &f.app,
        "/device/approve",
        "192.0.2.52",
        Some(ORIGIN),
        Some(COOKIE),
        &approval_body("ABCD23456789"),
    )
    .await;
    assert_eq!(
        (status, error_of(&bytes)),
        (StatusCode::INTERNAL_SERVER_ERROR, "server_error".to_owned())
    );
}

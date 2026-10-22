use axum::{
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use kiln_identity::{Store, query_signin_audit, read_signin_audit, signin_audit_path};
use p256::pkcs8::EncodePrivateKey;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    net::SocketAddr,
    path::Path,
    sync::atomic::{AtomicI64, Ordering},
};
use tower::ServiceExt;

const WIDE_SINCE: &str = "1970-01-01T00:00:00.000Z";
const WIDE_UNTIL: &str = "9999-12-31T23:59:59.999Z";
const KNOWN_CODE: &str = "known-oauth-code-ZZZZ";
const KNOWN_TOKEN: &str = "known-login-token-ZZZZ";
const KNOWN_DEVICE: &str = "known-device-code-ZZZZ";
const KNOWN_VERIFIER: &str = "known-verifier-known-verifier-known-verifier-ZZ";
const KNOWN_REDIRECT: &str = "https://audit-fixture.example/callback";
const KNOWN_QUERY: &str =
    "code=known-oauth-code-ZZZZ&state=known-state-ZZZZ&url=https://audit-fixture.example/callback";

static CLOCK: AtomicI64 = AtomicI64::new(1_700_000_000);

fn now() -> i64 {
    CLOCK.load(Ordering::SeqCst)
}

struct Fixture {
    root: tempfile::TempDir,
    app: axum::Router,
    store: Store,
}

impl Fixture {
    fn open() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let key = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let key_path = root.path().join("signing.pem");
        std::fs::write(&key_path, key.to_pkcs8_pem(Default::default()).unwrap()).unwrap();
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let store = Store::open(&root.path().join("identity.db")).unwrap();
        let cfg = kiln_identity::Config {
            public_origin: "https://identity.example.test".into(),
            github_client_id: "fixture-github".into(),
            github_secret_file: key_path.clone(),
            google_client_id: "fixture-google".into(),
            google_secret_file: key_path.clone(),
            signing_key_file: key_path,
            signing_key_id: "fixture-key".into(),
            retiring_jwks_file: None,
            trusted_proxy_loopback: true,
            github_authorize_url: kiln_identity::GITHUB_AUTHORIZE_URL.into(),
            github_token_url: kiln_identity::GITHUB_TOKEN_URL.into(),
            github_user_url: kiln_identity::GITHUB_USER_URL.into(),
            provider_ca_pem: None,
            now,
        };
        let app = kiln_identity::router(cfg, store.clone()).unwrap();
        Self { root, app, store }
    }

    fn database(&self) -> std::path::PathBuf {
        self.root.path().join("identity.db")
    }

    fn flush(&self) {
        self.store.flush_signin_audit();
    }
}

#[allow(clippy::too_many_arguments)]
async fn send(
    app: &axum::Router,
    method: &str,
    path: &str,
    peer: &str,
    token: Option<&str>,
    cookie: Option<&str>,
    user_agent: Option<&str>,
    forwarded: Option<&str>,
    content_type: Option<&str>,
    body: &str,
) -> (StatusCode, String) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .extension(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
    if let Some(token) = token {
        req = req.header("authorization", format!("Bearer {token}"));
    }
    if let Some(cookie) = cookie {
        req = req.header("cookie", cookie);
    }
    if let Some(user_agent) = user_agent {
        req = req.header("user-agent", user_agent);
    }
    if let Some(forwarded) = forwarded {
        req = req.header("x-forwarded-for", forwarded);
    }
    if let Some(content_type) = content_type {
        req = req.header("content-type", content_type);
    }
    let response = app
        .clone()
        .oneshot(req.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 65_536).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn sha256_hex(raw: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for byte in Sha256::digest(raw.as_bytes()) {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

fn sha256_bytes(raw: &str) -> Vec<u8> {
    Sha256::digest(raw.as_bytes()).to_vec()
}

fn pkce(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn stored_text(database: &Path) -> (Vec<String>, String) {
    let path = signin_audit_path(database);
    let conn =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let mut stmt = conn
        .prepare("SELECT * FROM signin_audit ORDER BY id")
        .unwrap();
    let names: Vec<String> = stmt
        .column_names()
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    let mut query = stmt.query([]).unwrap();
    let mut text = String::new();
    while let Some(row) = query.next().unwrap() {
        for index in 0..names.len() {
            let value = match row.get_ref(index).unwrap() {
                rusqlite::types::ValueRef::Text(bytes) => {
                    String::from_utf8_lossy(bytes).into_owned()
                }
                rusqlite::types::ValueRef::Integer(value) => value.to_string(),
                rusqlite::types::ValueRef::Null => String::new(),
                other => format!("{other:?}"),
            };
            text.push_str(&value);
            text.push('\n');
        }
    }
    drop(query);
    drop(stmt);
    drop(conn);
    let mut bytes = std::fs::read(&path).unwrap_or_default();
    for suffix in ["-wal", "-shm"] {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        if let Ok(more) = std::fs::read(std::path::PathBuf::from(name)) {
            bytes.extend(more);
        }
    }
    text.push_str(&String::from_utf8_lossy(&bytes));
    (names, text)
}

fn assert_absent(haystack: &str, needles: &[String]) {
    for needle in needles {
        assert!(
            !needle.is_empty() && !haystack.contains(needle),
            "{needle} appeared in a stored audit record"
        );
    }
}

#[tokio::test]
async fn failed_sign_in_writes_one_record_without_the_query() {
    CLOCK.store(1_700_000_000, Ordering::SeqCst);
    let fixture = Fixture::open();
    let (status, body) = send(
        &fixture.app,
        "GET",
        &format!("/oauth/github/callback?{KNOWN_QUERY}"),
        "127.0.0.1:9",
        None,
        Some("__Host-kiln_session=known-session-cookie-ZZZZ"),
        Some("KilnAudit"),
        Some("203.0.113.46"),
        None,
        "",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body, "{\"error\":\"invalid_state\"}");
    fixture.flush();
    let rows = read_signin_audit(&fixture.database()).unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].event, "sign_in");
    assert_eq!(rows[0].outcome, "failure");
    assert_eq!(rows[0].reason, "invalid_state");
    assert_eq!(rows[0].client_ip, "203.0.113.46");
    assert_eq!(rows[0].user_agent, "KilnAudit");
    assert_eq!(
        rows[0].session_id_hash,
        sha256_hex("known-session-cookie-ZZZZ")
    );
    assert!(rows[0].token_id_hash.is_empty());
    let (_, text) = stored_text(&fixture.database());
    assert_absent(
        &text,
        &[
            KNOWN_CODE.into(),
            KNOWN_QUERY.into(),
            "known-state-ZZZZ".into(),
            KNOWN_REDIRECT.into(),
            "known-session-cookie-ZZZZ".into(),
            "/oauth/github/callback".into(),
        ],
    );
}

#[tokio::test]
async fn refresh_writes_one_record_without_raw_tokens() {
    CLOCK.store(1_700_000_100, Ordering::SeqCst);
    let fixture = Fixture::open();
    let account = fixture
        .store
        .seed_account("github", "42", "reader", now())
        .unwrap();
    let grant = fixture
        .store
        .create_device("laptop", "192.0.2.1", now())
        .unwrap();
    fixture
        .store
        .approve_code(&grant.user_code, &account, true, now())
        .unwrap();
    CLOCK.fetch_add(6, Ordering::SeqCst);
    let (status, body) = send(
        &fixture.app,
        "POST",
        "/oauth/token",
        "192.0.2.10:9",
        None,
        None,
        Some("KilnAudit"),
        None,
        Some("application/x-www-form-urlencoded"),
        &format!(
            "grant_type=urn:ietf:params:oauth:grant-type:device_code&device_code={}",
            grant.device_code
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let issued: Value = serde_json::from_str(&body).unwrap();
    let access = issued["access_token"].as_str().unwrap().to_owned();
    let refresh = issued["refresh_token"].as_str().unwrap().to_owned();
    fixture.flush();
    let before = read_signin_audit(&fixture.database()).unwrap().len();
    let (status, body) = send(
        &fixture.app,
        "POST",
        "/oauth/token",
        "192.0.2.10:9",
        None,
        None,
        Some("KilnAudit"),
        None,
        Some("application/x-www-form-urlencoded"),
        &format!("grant_type=refresh_token&refresh_token={refresh}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let refreshed: Value = serde_json::from_str(&body).unwrap();
    let new_access = refreshed["access_token"].as_str().unwrap().to_owned();
    fixture.flush();
    let rows = read_signin_audit(&fixture.database()).unwrap();
    let refresh_rows: Vec<_> = rows
        .iter()
        .filter(|row| row.event == "token_refresh")
        .collect();
    assert_eq!(refresh_rows.len(), 1);
    assert_eq!(refresh_rows[0].outcome, "success");
    assert_eq!(refresh_rows[0].account_id, account);
    assert_eq!(refresh_rows[0].token_id_hash, sha256_hex(&new_access));
    assert!(rows.len() > before);
    let (_, text) = stored_text(&fixture.database());
    assert_absent(
        &text,
        &[
            access,
            refresh.clone(),
            new_access,
            grant.device_code,
            grant.user_code,
        ],
    );
    let _ = refresh;
}

#[tokio::test]
async fn audit_store_rejects_updates_and_deletes_and_survives_prune() {
    CLOCK.store(1_700_000_200, Ordering::SeqCst);
    let fixture = Fixture::open();
    let (status, _) = send(
        &fixture.app,
        "POST",
        "/oauth/token",
        "192.0.2.10:9",
        None,
        None,
        None,
        None,
        Some("application/x-www-form-urlencoded"),
        "grant_type=password",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    fixture.flush();
    let path = signin_audit_path(&fixture.database());
    let conn = rusqlite::Connection::open(&path).unwrap();
    let updated = conn.execute("UPDATE signin_audit SET reason='rewritten'", []);
    assert!(updated.is_err(), "update must abort");
    let deleted = conn.execute("DELETE FROM signin_audit", []);
    assert!(deleted.is_err(), "delete must abort");
    drop(conn);
    fixture.store.prune(now() + 10_000_000).unwrap();
    let Fixture { root, store, .. } = fixture;
    let database = root.path().join("identity.db");
    assert_eq!(read_signin_audit(&database).unwrap().len(), 1);
    drop(store);
    let reopened = Store::open(&database).unwrap();
    reopened.flush_signin_audit();
    assert_eq!(read_signin_audit(&database).unwrap().len(), 1);
}

#[tokio::test]
async fn doctor_and_admin_return_the_record_for_account_and_time() {
    CLOCK.store(1_700_000_300, Ordering::SeqCst);
    let fixture = Fixture::open();
    let account = fixture
        .store
        .seed_account("github", "7", "reader", now())
        .unwrap();
    let other = fixture
        .store
        .seed_account("github", "8", "other", now())
        .unwrap();
    let grant = fixture
        .store
        .create_device("laptop", "192.0.2.1", now())
        .unwrap();
    fixture
        .store
        .approve_code(&grant.user_code, &account, true, now())
        .unwrap();
    CLOCK.fetch_add(6, Ordering::SeqCst);
    let (status, body) = send(
        &fixture.app,
        "POST",
        "/oauth/token",
        "192.0.2.10:9",
        None,
        None,
        None,
        None,
        Some("application/x-www-form-urlencoded"),
        &format!(
            "grant_type=urn:ietf:params:oauth:grant-type:device_code&device_code={}",
            grant.device_code
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    fixture.flush();
    let found = query_signin_audit(&fixture.database(), &account, WIDE_SINCE, WIDE_UNTIL).unwrap();
    assert!(
        found
            .iter()
            .any(|row| row.event == "token_issue" && row.outcome == "success")
    );
    assert!(
        query_signin_audit(&fixture.database(), &other, WIDE_SINCE, WIDE_UNTIL)
            .unwrap()
            .is_empty()
    );
    assert!(
        query_signin_audit(
            &fixture.database(),
            &account,
            "9999-01-01T00:00:00.000Z",
            WIDE_UNTIL
        )
        .unwrap()
        .is_empty()
    );
    assert!(query_signin_audit(&fixture.database(), "", WIDE_SINCE, WIDE_UNTIL).is_err());
    let bin = env!("CARGO_BIN_EXE_kiln-identity");
    for command in ["doctor", "admin"] {
        let output = std::process::Command::new(bin)
            .args([
                command,
                "--database",
                fixture.database().to_str().unwrap(),
                "--account",
                &account,
                "--since",
                WIDE_SINCE,
                "--until",
                WIDE_UNTIL,
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{command} {output:?}");
        let rows: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
        assert!(rows.iter().any(|row| row["account_id"] == account));
        assert!(!String::from_utf8_lossy(&output.stdout).contains(&grant.device_code));
    }
    let bad = std::process::Command::new(bin)
        .args([
            "doctor",
            "--database",
            fixture.database().to_str().unwrap(),
            "--account",
            &account,
            "--since",
            "not-a-timestamp",
            "--until",
            WIDE_UNTIL,
        ])
        .output()
        .unwrap();
    assert!(!bad.status.success());
}

#[tokio::test]
async fn stored_record_never_contains_url_code_or_raw_token() {
    CLOCK.store(1_700_000_400, Ordering::SeqCst);
    let fixture = Fixture::open();
    let account = fixture
        .store
        .seed_account("github", "77", "reader", now())
        .unwrap();
    let (server, _directory) = fixture
        .store
        .create_server(&account, "app", "https://192.0.2.9", "test CA", now())
        .unwrap();
    let db = rusqlite::Connection::open(fixture.database()).unwrap();
    db.execute(
        "UPDATE servers SET redirect_uri=?1, login_secret_hash=?2 WHERE id=?3",
        rusqlite::params![KNOWN_REDIRECT, sha256_bytes(KNOWN_TOKEN), server],
    )
    .unwrap();
    db.execute(
        "INSERT INTO server_codes(hash,server_id,account_id,redirect_uri,challenge,expires_at) VALUES(?1,?2,?3,?4,?5,?6)",
        rusqlite::params![
            sha256_bytes(KNOWN_CODE),
            server,
            account,
            KNOWN_REDIRECT,
            pkce(KNOWN_VERIFIER),
            now() + 60
        ],
    )
    .unwrap();
    drop(db);
    let (status, body) = send(
        &fixture.app,
        "POST",
        &format!("/oauth/server/token?{KNOWN_QUERY}"),
        "127.0.0.1:9",
        Some(KNOWN_TOKEN),
        None,
        Some("KilnAudit"),
        Some("203.0.113.46"),
        Some("application/json"),
        &serde_json::json!({
            "code": KNOWN_CODE,
            "code_verifier": KNOWN_VERIFIER,
            "redirect_uri": KNOWN_REDIRECT,
        })
        .to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let exchange: Value = serde_json::from_str(&body).unwrap();
    let access = exchange["access_token"].as_str().unwrap().to_owned();
    let reader = exchange["reader"].as_str().unwrap().to_owned();
    let (device_status, device_body) = send(
        &fixture.app,
        "POST",
        &format!("/oauth/token?device_code={KNOWN_DEVICE}&code={KNOWN_CODE}"),
        "127.0.0.1:9",
        Some(KNOWN_TOKEN),
        None,
        Some("KilnAudit"),
        Some("203.0.113.46"),
        Some("application/x-www-form-urlencoded"),
        &format!(
            "grant_type=urn:ietf:params:oauth:grant-type:device_code&device_code={KNOWN_DEVICE}"
        ),
    )
    .await;
    assert_eq!(device_status, StatusCode::BAD_REQUEST, "{device_body}");
    fixture.flush();
    let (names, text) = stored_text(&fixture.database());
    assert_eq!(
        names,
        [
            "id",
            "ts",
            "account_id",
            "tenant_id",
            "client_ip",
            "user_agent",
            "client_id",
            "session_id_hash",
            "token_id_hash",
            "auth_method",
            "event",
            "outcome",
            "reason",
        ]
    );
    assert!(text.lines().filter(|line| !line.is_empty()).count() > names.len());
    assert_absent(
        &text,
        &[
            KNOWN_CODE.into(),
            KNOWN_DEVICE.into(),
            KNOWN_TOKEN.into(),
            KNOWN_VERIFIER.into(),
            KNOWN_REDIRECT.into(),
            KNOWN_QUERY.into(),
            "audit-fixture.example".into(),
            access,
            reader,
            format!("/oauth/server/token?{KNOWN_QUERY}"),
        ],
    );
    let rows = read_signin_audit(&fixture.database()).unwrap();
    assert!(
        rows.iter()
            .any(|row| row.event == "oauth_code_exchange" && row.outcome == "success")
    );
}

#[tokio::test]
async fn broken_audit_journal_leaves_token_responses_unchanged() {
    CLOCK.store(1_700_000_500, Ordering::SeqCst);
    let healthy = Fixture::open();
    let broken_root = tempfile::tempdir().unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(broken_root.path(), std::fs::Permissions::from_mode(0o700))
            .unwrap();
    }
    let broken_db = broken_root.path().join("identity.db");
    std::fs::create_dir(signin_audit_path(&broken_db)).unwrap();
    let broken_store = Store::open(&broken_db).unwrap();
    let key = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
    let key_path = broken_root.path().join("signing.pem");
    std::fs::write(&key_path, key.to_pkcs8_pem(Default::default()).unwrap()).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let cfg = kiln_identity::Config {
        public_origin: "https://identity.example.test".into(),
        github_client_id: "fixture-github".into(),
        github_secret_file: key_path.clone(),
        google_client_id: "fixture-google".into(),
        google_secret_file: key_path.clone(),
        signing_key_file: key_path,
        signing_key_id: "fixture-key".into(),
        retiring_jwks_file: None,
        trusted_proxy_loopback: false,
        github_authorize_url: kiln_identity::GITHUB_AUTHORIZE_URL.into(),
        github_token_url: kiln_identity::GITHUB_TOKEN_URL.into(),
        github_user_url: kiln_identity::GITHUB_USER_URL.into(),
        provider_ca_pem: None,
        now,
    };
    let broken_app = kiln_identity::router(cfg, broken_store.clone()).unwrap();
    broken_store.flush_signin_audit();
    assert!(
        broken_store.signin_audit_failures() >= 1,
        "a directory journal must be counted"
    );
    for (path, body, code) in [
        (
            "/oauth/token",
            "grant_type=password",
            "unsupported_grant_type",
        ),
        (
            "/oauth/token",
            "grant_type=urn:ietf:params:oauth:grant-type:device_code&device_code=missing-device",
            "invalid_grant",
        ),
    ] {
        let left = send(
            &healthy.app,
            "POST",
            path,
            "192.0.2.10:9",
            None,
            None,
            None,
            None,
            Some("application/x-www-form-urlencoded"),
            body,
        )
        .await;
        let right = send(
            &broken_app,
            "POST",
            path,
            "192.0.2.10:9",
            None,
            None,
            None,
            None,
            Some("application/x-www-form-urlencoded"),
            body,
        )
        .await;
        assert_eq!(left.0, right.0);
        assert_eq!(left.1, right.1);
        assert_eq!(left.1, format!("{{\"error\":\"{code}\"}}"));
    }
}

#[tokio::test]
async fn slow_down_is_not_recorded() {
    CLOCK.store(1_700_000_600, Ordering::SeqCst);
    let fixture = Fixture::open();
    let account = fixture
        .store
        .seed_account("github", "9", "reader", now())
        .unwrap();
    let grant = fixture
        .store
        .create_device("laptop", "192.0.2.1", now())
        .unwrap();
    fixture
        .store
        .approve_code(&grant.user_code, &account, true, now())
        .unwrap();
    let (status, body) = send(
        &fixture.app,
        "POST",
        "/oauth/token",
        "192.0.2.10:9",
        None,
        None,
        None,
        None,
        Some("application/x-www-form-urlencoded"),
        &format!(
            "grant_type=urn:ietf:params:oauth:grant-type:device_code&device_code={}",
            grant.device_code
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body, "{\"error\":\"slow_down\"}");
    fixture.flush();
    assert!(read_signin_audit(&fixture.database()).unwrap().is_empty());
}

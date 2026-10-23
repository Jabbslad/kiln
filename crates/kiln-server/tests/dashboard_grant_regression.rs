//! Hermetic regression for v0.4.81 dashboard sign-in: legacy server enrollment
//! (no `tenant_id` field) must accept real identity owner JWTs that carry the v3
//! tenant UUID after schema migration.

use std::net::SocketAddr;

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{HeaderMap, Request, StatusCode},
};
use kiln_api::auth::APP_OAUTH_REDIRECT;
use kiln_identity::{Config, Store};
use kiln_server::{
    app_listener::{self, BIND},
    auth::{Authenticator, Enrollment},
};
use p256::pkcs8::EncodePrivateKey;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

const ISSUER: &str = "https://identity.example.test";
const OWNER: &str = "acct_owner";
const SERVER: &str = "srv_one";
const LOGIN_SECRET: &str = "fixture-login-secret-for-dashboard-grant-test";
const SESSION_RAW: &str = "owner-browser-session-material";

fn now() -> i64 {
    jsonwebtoken::get_current_timestamp() as i64
}

fn legacy_v2(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let db = rusqlite::Connection::open(path).unwrap();
    db.execute_batch(
        "CREATE TABLE accounts(id TEXT PRIMARY KEY,created_at INTEGER NOT NULL,revoked_at INTEGER);
         CREATE TABLE servers(id TEXT PRIMARY KEY,owner_id TEXT NOT NULL,name TEXT NOT NULL,origin TEXT NOT NULL,ca_pem TEXT NOT NULL,revision INTEGER NOT NULL,active INTEGER NOT NULL,directory_hash BLOB NOT NULL,created_at INTEGER NOT NULL,redirect_uri TEXT,login_secret_hash BLOB);
         INSERT INTO accounts(id,created_at) VALUES('acct_owner', 1);
         INSERT INTO servers(id,owner_id,name,origin,ca_pem,revision,active,directory_hash,created_at) VALUES('srv_one','acct_owner','one','https://192.0.2.9/','ca',1,1,X'11',1);",
    )
    .unwrap();
    db.pragma_update(None, "user_version", 2).unwrap();
    drop(db);
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

fn hash_secret(raw: &str) -> Vec<u8> {
    Sha256::digest(raw.as_bytes()).to_vec()
}

struct IdentityFixture {
    app: Router,
    db_path: std::path::PathBuf,
}

fn identity_fixture(db_path: &std::path::Path) -> IdentityFixture {
    use std::os::unix::fs::PermissionsExt;
    let root = db_path.parent().unwrap();
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).unwrap();
    legacy_v2(db_path);
    drop(Store::open(db_path).unwrap());
    let db = rusqlite::Connection::open(db_path).unwrap();
    db.execute(
        "UPDATE servers SET redirect_uri=?1, login_secret_hash=?2 WHERE id=?3",
        rusqlite::params![APP_OAUTH_REDIRECT, hash_secret(LOGIN_SECRET), SERVER],
    )
    .unwrap();
    let tenant: String = db
        .query_row(
            "SELECT tenant_id FROM servers WHERE id=?1",
            [SERVER],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!tenant.is_empty());
    assert_eq!(
        db.query_row::<i64, _, _>(
            "SELECT count(*) FROM tenant_members WHERE tenant_id=?1 AND role='owner'",
            [&tenant],
            |row| row.get(0),
        )
        .unwrap(),
        1
    );
    for idx in 0..3 {
        db.execute(
            "INSERT INTO app_readers(hash,server_id,account_id,expires_at) VALUES(?1,?2,?3,?4)",
            rusqlite::params![
                Sha256::digest(format!("reader-{idx}").as_bytes()).to_vec(),
                SERVER,
                OWNER,
                now() + 86_400
            ],
        )
        .unwrap();
    }
    drop(db);

    let key = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
    let key_path = root.join("signing.pem");
    std::fs::write(
        &key_path,
        key.to_pkcs8_pem(p256::pkcs8::LineEnding::LF)
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let store = Store::open(db_path).unwrap();
    let cfg = Config {
        public_origin: ISSUER.into(),
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
    let app = kiln_identity::router(cfg, store).unwrap();
    IdentityFixture {
        app,
        db_path: db_path.to_path_buf(),
    }
}

async fn jwks_from_identity(app: &Router) -> serde_json::Value {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/.well-known/jwks.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 65_536).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn serve_identity(app: Router) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    port
}

fn remember_session(db_path: &std::path::Path, account: &str, raw: &str) {
    let db = rusqlite::Connection::open(db_path).unwrap();
    db.execute(
        "INSERT INTO browser_sessions(hash,csrf_hash,account_id,expires_at) VALUES(?1,?2,?3,?4)",
        rusqlite::params![
            Sha256::digest(raw.as_bytes()).to_vec(),
            Sha256::digest(b"csrf").to_vec(),
            account,
            now() + 43_200
        ],
    )
    .unwrap();
}

fn legacy_authenticator(root: &std::path::Path, jwks: &serde_json::Value) -> Authenticator {
    let cache = root.join("jwks.json");
    std::fs::write(
        &cache,
        serde_json::to_vec(&serde_json::json!({"issuer": ISSUER, "jwks": jwks})).unwrap(),
    )
    .unwrap();
    Authenticator::central_only(
        Enrollment {
            version: 1,
            issuer: ISSUER.into(),
            owner_id: OWNER.into(),
            server_id: SERVER.into(),
            tenant_id: None,
        },
        cache,
    )
    .unwrap()
}

async fn listen_tls(app: Router) -> (u16, String) {
    let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
    let ca_pem = cert.cert.pem();
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        ca_pem.clone().into_bytes(),
        cert.signing_key.serialize_pem().into_bytes(),
    )
    .await
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, tls)
            .unwrap()
            .serve(app.into_make_service())
            .await
            .unwrap();
    });
    (port, ca_pem)
}

async fn send_dashboard(router: &Router, request: Request<Body>) -> axum::response::Response {
    let (mut parts, body) = request.into_parts();
    parts.extensions.insert(ConnectInfo(BIND));
    router
        .clone()
        .oneshot(Request::from_parts(parts, body))
        .await
        .unwrap()
}

async fn read_response(response: axum::response::Response) -> (StatusCode, HeaderMap, String) {
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 262_144).await.unwrap();
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

fn attempt_cookie(headers: &HeaderMap) -> String {
    let raw = headers
        .get_all("set-cookie")
        .iter()
        .find_map(|value| {
            let text = value.to_str().unwrap();
            text.starts_with("__Host-kiln_app_attempt=")
                .then(|| text.split(';').next().unwrap().to_owned())
        })
        .expect("sign-in attempt cookie");
    assert!(raw.starts_with("__Host-kiln_app_attempt="));
    raw
}

#[tokio::test]
async fn legacy_enrollment_accepts_v3_identity_owner_dashboard_grant() {
    let root = tempfile::tempdir().unwrap();
    let fixture = identity_fixture(&root.path().join("identity.db"));
    let jwks = jwks_from_identity(&fixture.app).await;
    remember_session(&fixture.db_path, OWNER, SESSION_RAW);
    let identity_port = serve_identity(fixture.app).await;

    let upstream = Router::new().route(
        "/api/v1/dashboard",
        axum::routing::get(|| async { axum::Json(serde_json::json!({"schema_version": 1})) }),
    );
    let (upstream_port, ca_pem) = listen_tls(upstream).await;

    let auth = legacy_authenticator(root.path(), &jwks);
    let dashboard = app_listener::router_with(
        auth,
        &format!("https://127.0.0.1:{upstream_port}"),
        ca_pem.as_bytes(),
        None,
        app_listener::SignIn {
            authorize_origin: ISSUER.into(),
            loopback: format!("http://127.0.0.1:{identity_port}"),
            login_secret: LOGIN_SECRET.into(),
            server_id: SERVER.into(),
        },
    )
    .unwrap();

    let (status, headers, _) = read_response(
        send_dashboard(
            &dashboard,
            Request::builder()
                .uri("/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let attempt = attempt_cookie(&headers);
    let location = headers.get("location").unwrap().to_str().unwrap();
    let authorize = url::Url::parse(location).unwrap();
    assert_eq!(authorize.path(), "/oauth/server");

    let authorize_url = format!(
        "http://127.0.0.1:{identity_port}{}{}",
        authorize.path(),
        authorize
            .query()
            .map(|query| format!("?{query}"))
            .unwrap_or_default()
    );
    let response = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(authorize_url)
        .header("cookie", format!("__Host-kiln_session={SESSION_RAW}"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "{}",
        response.text().await.unwrap()
    );
    let callback = response
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(callback.starts_with(APP_OAUTH_REDIRECT));
    let callback_url = url::Url::parse(&callback).unwrap();
    let code = callback_url
        .query_pairs()
        .find(|(key, _)| key == "code")
        .unwrap()
        .1
        .into_owned();
    let state = callback_url
        .query_pairs()
        .find(|(key, _)| key == "state")
        .unwrap()
        .1
        .into_owned();

    let callback_request = Request::builder()
        .uri(format!("/oauth/callback?code={code}&state={state}"))
        .header("cookie", attempt)
        .body(Body::empty())
        .unwrap();
    let (status, headers, body) =
        read_response(send_dashboard(&dashboard, callback_request).await).await;
    assert_eq!(
        status,
        StatusCode::SEE_OTHER,
        "expected dashboard session grant, got: {body}"
    );
    assert!(
        !body.contains("Identity did not grant a dashboard session"),
        "{body}"
    );
    assert_eq!(headers.get("location").unwrap(), "/boxes");
    let session = headers
        .get_all("set-cookie")
        .iter()
        .find_map(|value| {
            let text = value.to_str().unwrap();
            text.starts_with("__Host-kiln_app=").then(|| {
                text.split(';')
                    .next()
                    .unwrap()
                    .split_once('=')
                    .unwrap()
                    .1
                    .to_owned()
            })
        })
        .expect("dashboard session cookie");
    assert!(!session.is_empty());

    let dashboard_request = Request::builder()
        .uri("/api/v1/dashboard")
        .header("cookie", format!("__Host-kiln_app={session}"))
        .header("authorization", "Bearer placeholder")
        .body(Body::empty())
        .unwrap();
    let (status, _, body) =
        read_response(send_dashboard(&dashboard, dashboard_request).await).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap()["schema_version"],
        1
    );
}

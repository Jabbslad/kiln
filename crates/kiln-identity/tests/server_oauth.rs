use axum::{
    Router,
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{HeaderMap, Request, StatusCode},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use kiln_api::auth::{APP_OAUTH_REDIRECT, Scope, ServerClaims};
use kiln_identity::{Config, Store};
use p256::pkcs8::EncodePrivateKey;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    net::SocketAddr,
    sync::atomic::{AtomicI64, Ordering},
};
use tower::ServiceExt;

static CLOCK: AtomicI64 = AtomicI64::new(1_000);
fn now() -> i64 {
    CLOCK.load(Ordering::SeqCst)
}

const VERIFIER: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER_VERIFIER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

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
        let app = kiln_identity::router(cfg, store.clone()).unwrap();
        Self { root, app, store }
    }

    fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.root.path().join("identity.db")).unwrap()
    }

    async fn raw(
        &self,
        method: &str,
        path: &str,
        peer: &str,
        token: Option<&str>,
        cookie: Option<&str>,
        body: Option<&str>,
    ) -> (StatusCode, HeaderMap, String) {
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
        let body = if let Some(body) = body {
            req = req.header("content-type", "application/json");
            Body::from(body.to_owned())
        } else {
            Body::empty()
        };
        let response = self
            .app
            .clone()
            .oneshot(req.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = to_bytes(response.into_body(), 131072).await.unwrap();
        (
            status,
            headers,
            String::from_utf8_lossy(&bytes).into_owned(),
        )
    }
}

fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn server_path(redirect: &str, verifier: &str, state: &str) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("redirect_uri", redirect);
    query.append_pair("state", state);
    query.append_pair("code_challenge", &challenge(verifier));
    query.append_pair("code_challenge_method", "S256");
    format!("/oauth/server?{}", query.finish())
}

fn claims(jwt: &str) -> ServerClaims {
    let payload = jwt.split('.').nth(1).unwrap();
    let bytes = URL_SAFE_NO_PAD.decode(payload).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn remember(f: &Fixture, account: &str, raw: &str) {
    f.db()
        .execute(
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

fn code_count(f: &Fixture) -> i64 {
    f.db()
        .query_row("SELECT count(*) FROM server_codes", [], |row| row.get(0))
        .unwrap()
}

fn private_dir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

fn legacy_servers(path: &std::path::Path, version: i64) {
    use std::os::unix::fs::PermissionsExt;
    let db = rusqlite::Connection::open(path).unwrap();
    db.execute_batch(
        "CREATE TABLE accounts(id TEXT PRIMARY KEY,created_at INTEGER NOT NULL,revoked_at INTEGER);
         CREATE TABLE servers(id TEXT PRIMARY KEY,owner_id TEXT NOT NULL,name TEXT NOT NULL,origin TEXT NOT NULL,ca_pem TEXT NOT NULL,revision INTEGER NOT NULL,active INTEGER NOT NULL,directory_hash BLOB NOT NULL,created_at INTEGER NOT NULL);
         INSERT INTO accounts(id,created_at) VALUES('acct_old', 1);
         INSERT INTO servers(id,owner_id,name,origin,ca_pem,revision,active,directory_hash,created_at) VALUES('srv_old','acct_old','old','https://192.0.2.8/','ca',1,1,X'11',1);",
    )
    .unwrap();
    db.pragma_update(None, "user_version", version).unwrap();
    drop(db);
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn version_one_migrates_to_two_and_leaves_the_redirect_empty() {
    let dir = private_dir();
    let path = dir.path().join("identity.db");
    legacy_servers(&path, 1);
    drop(Store::open(&path).unwrap());
    let db = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = db
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 2);
    let redirect: Option<String> = db
        .query_row(
            "SELECT redirect_uri FROM servers WHERE id='srv_old'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(redirect.is_none());
    let secret: Option<Vec<u8>> = db
        .query_row(
            "SELECT login_secret_hash FROM servers WHERE id='srv_old'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(secret.is_none());
    drop(db);
    Store::open(&path).unwrap();

    let broken = dir.path().join("broken.db");
    legacy_servers(&broken, 2);
    match Store::open(&broken) {
        Ok(_) => panic!("a version 2 file without the new columns opened"),
        Err(error) => assert!(
            error.to_string().contains("restore a compatible backup"),
            "{error}"
        ),
    }
}

#[tokio::test]
async fn server_oauth_is_single_use_and_split_from_the_directory_token() {
    CLOCK.store(1_000, Ordering::SeqCst);
    let f = Fixture::new();
    let version: i64 = f
        .db()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 2);
    let owner = f
        .store
        .seed_account("github", "101", "owner", now())
        .unwrap();
    let other = f
        .store
        .seed_account("google", "202", "other", now())
        .unwrap();
    let (server, directory) = f
        .store
        .create_server(&owner, "one", "https://192.0.2.9", "test CA", now())
        .unwrap();
    let enrolled = server_path(APP_OAUTH_REDIRECT, VERIFIER, "state-one");
    let (status, _, body) = f
        .raw("GET", &enrolled, "192.0.2.10:9", None, None, None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(json_error(&body), "invalid_redirect");
    assert_eq!(code_count(&f), 0);

    let wrong = json!({"redirect_uri":"https://evil.example/oauth/callback"}).to_string();
    let (status, _, body) = f
        .raw(
            "POST",
            &format!("/v1/servers/{server}/app-login"),
            "192.0.2.11:9",
            Some(&directory),
            None,
            Some(&wrong),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(json_error(&body), "invalid_redirect");

    let install = json!({"redirect_uri": APP_OAUTH_REDIRECT}).to_string();
    let (status, _, body) = f
        .raw(
            "POST",
            &format!("/v1/servers/{server}/app-login"),
            "192.0.2.11:9",
            Some(&directory),
            None,
            Some(&install),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let login = json(&body)["login_token"].as_str().unwrap().to_owned();
    assert!(!login.is_empty());
    let (status, _, body) = f
        .raw(
            "POST",
            &format!("/v1/servers/{server}/app-login"),
            "192.0.2.11:9",
            Some(&directory),
            None,
            Some(&install),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    let (status, _, body) = f
        .raw(
            "POST",
            &format!("/v1/servers/{server}/app-login"),
            "192.0.2.11:9",
            Some(&login),
            None,
            Some(&install),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");

    let wrong_uri = server_path("https://evil.example/oauth/callback", VERIFIER, "state-one");
    let (status, _, body) = f
        .raw("GET", &wrong_uri, "192.0.2.12:9", None, None, None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(json_error(&body), "invalid_redirect");
    assert_eq!(code_count(&f), 0);

    remember(&f, &owner, "owner-session");
    let (status, headers, body) = f
        .raw(
            "GET",
            &enrolled,
            "192.0.2.13:9",
            None,
            Some("__Host-kiln_session=owner-session"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert!(!body.contains("Continue with GitHub"), "{body}");
    assert!(!body.contains("Continue with Google"), "{body}");
    let location = headers.get("location").unwrap().to_str().unwrap();
    let redirected = url::Url::parse(location).unwrap();
    assert_eq!(
        redirected.as_str().split('?').next().unwrap(),
        APP_OAUTH_REDIRECT
    );
    let code = query(&redirected, "code");
    assert_eq!(query(&redirected, "state"), "state-one");
    assert!(!code.is_empty());

    let before = code_count(&f);
    remember(&f, &other, "other-session");
    let (status, headers, body) = f
        .raw(
            "GET",
            &server_path(APP_OAUTH_REDIRECT, VERIFIER, "state-other"),
            "192.0.2.14:9",
            None,
            Some("__Host-kiln_session=other-session"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("This account cannot continue"), "{body}");
    assert!(headers.get("location").is_none(), "{headers:?}");
    assert!(!body.contains("code="), "{body}");
    assert_eq!(code_count(&f), before);

    let (status, headers, body) = f
        .raw(
            "GET",
            &server_path(APP_OAUTH_REDIRECT, VERIFIER, "state-new"),
            "192.0.2.15:9",
            None,
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains(APP_OAUTH_REDIRECT), "{body}");
    assert!(body.contains("href=/oauth/github"), "{body}");
    assert!(body.contains("href=/oauth/google"), "{body}");
    assert!(
        !body.contains("https://app.dark-forge.dev/oauth/github"),
        "{body}"
    );
    assert!(headers.get("location").is_none());
    let authorizations: i64 = f
        .db()
        .query_row("SELECT count(*) FROM server_authorizations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(authorizations, 1);

    let (status, headers, body) = f
        .raw("GET", "/oauth/github", "192.0.2.16:9", None, None, None)
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let authorize = url::Url::parse(headers.get("location").unwrap().to_str().unwrap()).unwrap();
    assert_eq!(
        query(&authorize, "redirect_uri"),
        "https://identity.example.test/oauth/github/callback"
    );
    assert!(!authorize.as_str().contains("app.dark-forge.dev"));
    let (status, _, body) = f
        .raw("GET", "/oauth/apple", "192.0.2.17:9", None, None, None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(json_error(&body), "not_found");

    let exchange = |code: &str, verifier: &str, redirect: &str| {
        json!({
            "code": code,
            "code_verifier": verifier,
            "redirect_uri": redirect,
        })
        .to_string()
    };
    let (status, _, body) = f
        .raw(
            "POST",
            "/oauth/server/token",
            "192.0.2.18:9",
            Some(&directory),
            None,
            Some(&exchange(&code, VERIFIER, APP_OAUTH_REDIRECT)),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    let (status, _, body) = f
        .raw(
            "POST",
            "/oauth/server/token",
            "192.0.2.18:9",
            Some(&login),
            None,
            Some(&exchange(&code, VERIFIER, APP_OAUTH_REDIRECT)),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let granted = json(&body);
    assert_eq!(granted["token_type"], "Bearer");
    assert_eq!(granted["expires_in"], 300);
    let jwt = granted["access_token"].as_str().unwrap();
    let reader = granted["reader"].as_str().unwrap().to_owned();
    let decoded = claims(jwt);
    assert_eq!(decoded.iss, "https://identity.example.test");
    assert_eq!(decoded.sub, owner);
    assert_eq!(decoded.aud, format!("kiln:server:{server}"));
    assert_eq!(decoded.scope, Scope::Read);
    assert_eq!(decoded.credential_id, format!("app_{server}"));
    assert_eq!(decoded.exp - decoded.iat, 300);
    let (status, _, body) = f
        .raw(
            "POST",
            "/oauth/server/token",
            "192.0.2.18:9",
            Some(&login),
            None,
            Some(&exchange(&code, VERIFIER, APP_OAUTH_REDIRECT)),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(json_error(&body), "invalid_grant");
    let (status, _, body) = f
        .raw(
            "POST",
            "/oauth/server/token",
            "192.0.2.18:9",
            None,
            None,
            Some(&json!({"reader": reader}).to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        claims(json(&body)["access_token"].as_str().unwrap()).scope,
        Scope::Read
    );

    let pkce = owner_code(&f, &enrolled).await;
    let (status, _, body) = f
        .raw(
            "POST",
            "/oauth/server/token",
            "192.0.2.18:9",
            Some(&login),
            None,
            Some(&exchange(&pkce, OTHER_VERIFIER, APP_OAUTH_REDIRECT)),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(json_error(&body), "invalid_grant");
    let (status, _, body) = f
        .raw(
            "POST",
            "/oauth/server/token",
            "192.0.2.18:9",
            Some(&login),
            None,
            Some(&exchange(&pkce, VERIFIER, APP_OAUTH_REDIRECT)),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(json_error(&body), "invalid_grant");

    let mismatched = owner_code(&f, &enrolled).await;
    let (status, _, body) = f
        .raw(
            "POST",
            "/oauth/server/token",
            "192.0.2.18:9",
            Some(&login),
            None,
            Some(&exchange(
                &mismatched,
                VERIFIER,
                "https://app.dark-forge.dev/oauth/other",
            )),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(json_error(&body), "invalid_grant");

    let fresh = owner_code(&f, &enrolled).await;
    let stale = owner_code(&f, &enrolled).await;
    let issued = now();
    CLOCK.store(issued + 59, Ordering::SeqCst);
    let (status, _, body) = f
        .raw(
            "POST",
            "/oauth/server/token",
            "192.0.2.18:9",
            Some(&login),
            None,
            Some(&exchange(&fresh, VERIFIER, APP_OAUTH_REDIRECT)),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    CLOCK.store(issued + 60, Ordering::SeqCst);
    let (status, _, body) = f
        .raw(
            "POST",
            "/oauth/server/token",
            "192.0.2.18:9",
            Some(&login),
            None,
            Some(&exchange(&stale, VERIFIER, APP_OAUTH_REDIRECT)),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(json_error(&body), "expired_token");

    let ca = rcgen::generate_simple_self_signed(vec!["localhost".into()])
        .unwrap()
        .cert
        .pem();
    let update = json!({
        "name": "renamed",
        "origin": "https://192.0.2.20/",
        "ca_pem": ca,
    })
    .to_string();
    let (status, _, body) = f
        .raw(
            "PUT",
            &format!("/v1/servers/{server}"),
            "192.0.2.19:9",
            Some(&login),
            None,
            Some(&update),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    let origin: String = f
        .db()
        .query_row("SELECT origin FROM servers WHERE id=?1", [&server], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(origin, "https://192.0.2.9");
    let (status, _, body) = f
        .raw(
            "PUT",
            &format!("/v1/servers/{server}"),
            "192.0.2.19:9",
            Some(&directory),
            None,
            Some(&update),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let origin: String = f
        .db()
        .query_row("SELECT origin FROM servers WHERE id=?1", [&server], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(origin, "https://192.0.2.20/");

    let status_body = json!({"server_id": server}).to_string();
    let (status, _, body) = f
        .raw(
            "POST",
            "/v1/server-status",
            "192.0.2.21:9",
            Some(&login),
            None,
            Some(&status_body),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        json(&body),
        json!({"account_active": true, "server_active": true})
    );
    let (status, _, _) = f
        .raw(
            "POST",
            "/v1/server-status",
            "192.0.2.21:9",
            Some(&directory),
            None,
            Some(&status_body),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = f
        .raw(
            "POST",
            "/v1/server-status",
            "192.0.2.21:9",
            Some("wrong-login"),
            None,
            Some(&status_body),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    f.db()
        .execute(
            "UPDATE accounts SET revoked_at=?1 WHERE id=?2",
            rusqlite::params![now(), owner],
        )
        .unwrap();
    let (status, _, body) = f
        .raw(
            "POST",
            "/v1/server-status",
            "192.0.2.21:9",
            Some(&login),
            None,
            Some(&status_body),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        json(&body),
        json!({"account_active": false, "server_active": true})
    );
    f.db()
        .execute("UPDATE accounts SET revoked_at=NULL WHERE id=?1", [&owner])
        .unwrap();
    let (status, _, _) = f
        .raw(
            "DELETE",
            &format!("/v1/servers/{server}"),
            "192.0.2.21:9",
            Some(&directory),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, body) = f
        .raw(
            "POST",
            "/v1/server-status",
            "192.0.2.21:9",
            Some(&login),
            None,
            Some(&status_body),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        json(&body),
        json!({"account_active": true, "server_active": false})
    );

    f.db()
        .execute("UPDATE servers SET active=1 WHERE id=?1", [&server])
        .unwrap();
    for index in 0..11 {
        let (status, _, body) = f
            .raw("GET", &enrolled, "192.0.2.30:9", None, None, None)
            .await;
        assert_eq!(
            status,
            if index < 10 {
                StatusCode::OK
            } else {
                StatusCode::TOO_MANY_REQUESTS
            },
            "{index} {body}"
        );
    }
}

async fn owner_code(f: &Fixture, path: &str) -> String {
    let (status, headers, body) = f
        .raw(
            "GET",
            path,
            "192.0.2.13:9",
            None,
            Some("__Host-kiln_session=owner-session"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    query(
        &url::Url::parse(headers.get("location").unwrap().to_str().unwrap()).unwrap(),
        "code",
    )
}

fn json(body: &str) -> Value {
    serde_json::from_str(body).unwrap()
}

fn json_error(body: &str) -> String {
    json(body)["error"].as_str().unwrap().to_owned()
}

fn query(url: &url::Url, name: &str) -> String {
    url.query_pairs()
        .find(|(key, _)| key == name)
        .unwrap()
        .1
        .into_owned()
}

#[cfg(feature = "lab-provider")]
mod lab {
    use super::*;
    use axum::{
        Json,
        routing::{get, post},
    };

    fn lab_now() -> i64 {
        1_700_000_000
    }

    #[tokio::test]
    async fn provider_callback_returns_a_code_for_the_enrolled_owner() {
        use std::os::unix::fs::PermissionsExt;
        let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
        let cert_pem = cert.cert.pem();
        let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
            cert_pem.clone().into_bytes(),
            cert.signing_key.serialize_pem().into_bytes(),
        )
        .await
        .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let app = Router::new()
            .route(
                "/token",
                post(|| async {
                    Json(json!({"access_token":"fixture-access","token_type":"Bearer"}))
                }),
            )
            .route(
                "/user",
                get(|| async { Json(json!({"id":101,"login":"owner"})) }),
            );
        tokio::spawn(async move {
            axum_server::from_tcp_rustls(listener, tls)
                .unwrap()
                .serve(app.into_make_service())
                .await
                .unwrap();
        });

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
        let owner = store
            .seed_account("github", "101", "owner", lab_now())
            .unwrap();
        let (server, directory) = store
            .create_server(&owner, "one", "https://192.0.2.9", "test CA", lab_now())
            .unwrap();
        let cfg = Config {
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
            github_token_url: format!("https://127.0.0.1:{port}/token"),
            github_user_url: format!("https://127.0.0.1:{port}/user"),
            provider_ca_pem: Some(cert_pem.into_bytes()),
            now: lab_now,
        };
        let app = kiln_identity::router(cfg, store.clone()).unwrap();
        let fixture = Fixture { root, app, store };
        let install = json!({"redirect_uri": APP_OAUTH_REDIRECT}).to_string();
        let (status, _, body) = fixture
            .raw(
                "POST",
                &format!("/v1/servers/{server}/app-login"),
                "192.0.2.40:9",
                Some(&directory),
                None,
                Some(&install),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, headers, body) = fixture
            .raw(
                "GET",
                &server_path(APP_OAUTH_REDIRECT, VERIFIER, "state-lab"),
                "192.0.2.41:9",
                None,
                None,
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("href=/oauth/github"), "{body}");
        let session = headers
            .get_all("set-cookie")
            .iter()
            .find_map(|value| {
                value
                    .to_str()
                    .unwrap()
                    .strip_prefix("__Host-kiln_session=")
                    .map(|rest| rest.split(';').next().unwrap().to_owned())
            })
            .unwrap();
        let cookie = format!("__Host-kiln_session={session}");
        let (status, headers, body) = fixture
            .raw(
                "GET",
                "/oauth/github",
                "192.0.2.41:9",
                None,
                Some(&cookie),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
        let authorize =
            url::Url::parse(headers.get("location").unwrap().to_str().unwrap()).unwrap();
        let state = query(&authorize, "state");
        let (status, headers, body) = fixture
            .raw(
                "GET",
                &format!("/oauth/github/callback?code=fixture-code&state={state}"),
                "192.0.2.41:9",
                None,
                Some(&cookie),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
        let location = url::Url::parse(headers.get("location").unwrap().to_str().unwrap()).unwrap();
        assert_eq!(
            location.as_str().split('?').next().unwrap(),
            APP_OAUTH_REDIRECT
        );
        assert_eq!(query(&location, "state"), "state-lab");
        assert!(!query(&location, "code").is_empty());
        assert!(!body.contains("Continue with GitHub"), "{body}");
    }
}

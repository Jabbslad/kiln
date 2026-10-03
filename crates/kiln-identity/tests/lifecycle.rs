use axum::{
    Router,
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use kiln_identity::{Config, Store};
use p256::pkcs8::EncodePrivateKey;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    net::SocketAddr,
    sync::atomic::{AtomicI64, Ordering},
};
use tower::ServiceExt;

static CLOCK: AtomicI64 = AtomicI64::new(1000);
fn now() -> i64 {
    CLOCK.load(Ordering::SeqCst)
}

struct Fixture {
    root: tempfile::TempDir,
    app: Router,
    store: Store,
    cfg: Config,
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
            now,
        };
        let app = kiln_identity::router(cfg.clone(), store.clone()).unwrap();
        Self {
            root,
            app,
            store,
            cfg,
        }
    }
    fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.root.path().join("identity.db")).unwrap()
    }
    async fn call(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Value,
    ) -> (StatusCode, Value) {
        call(&self.app, method, path, token, body).await
    }
    async fn device(&self, owner: &str) -> Value {
        let code = self
            .store
            .create_device("laptop", "192.0.2.1", now())
            .unwrap();
        self.store
            .approve_code(&code.user_code, owner, true, now())
            .unwrap();
        CLOCK.fetch_add(5, Ordering::SeqCst);
        let (status, tokens) = self.call("POST", "/oauth/token", None, json!({"grant_type":"urn:ietf:params:oauth:grant-type:device_code", "device_code":code.device_code})).await;
        assert_eq!(status, StatusCode::OK, "{tokens}");
        tokens
    }
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let form = path == "/oauth/token" || path == "/oauth/device/code";
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .extension(ConnectInfo("192.0.2.1:1234".parse::<SocketAddr>().unwrap()));
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    let body = if method == "GET" || method == "DELETE" {
        String::new()
    } else if form {
        req = req.header("Content-Type", "application/x-www-form-urlencoded");
        let mut encoder = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in body.as_object().unwrap() {
            encoder.append_pair(k, v.as_str().unwrap());
        }
        encoder.finish()
    } else {
        req = req.header("Content-Type", "application/json");
        body.to_string()
    };
    let response = app
        .clone()
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 131072).await.unwrap();
    (
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        },
    )
}

// One injected-clock scenario avoids globally racing the deterministic clock.
#[tokio::test]
async fn grants_revocation_ownership_and_expiry_boundaries() {
    let f = Fixture::new();
    for index in 0..11 {
        let response = f
            .app
            .clone()
            .oneshot(
                Request::get("/oauth/github")
                    .extension(ConnectInfo(
                        "192.0.2.88:1234".parse::<SocketAddr>().unwrap(),
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if index < 10 {
                StatusCode::SEE_OTHER
            } else {
                StatusCode::TOO_MANY_REQUESTS
            }
        );
    }
    let response = f
        .app
        .clone()
        .oneshot(
            Request::post("/oauth/token")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("grant_type={}", "x".repeat(17000))))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let owner = f
        .store
        .seed_account("github", "101", "owner", now())
        .unwrap();
    let other = f
        .store
        .seed_account("google", "101", "owner", now())
        .unwrap();
    let code = f.store.create_device("laptop", "192.0.2.2", now()).unwrap();
    let poll = json!({"grant_type":"urn:ietf:params:oauth:grant-type:device_code", "device_code":code.device_code});
    assert_eq!(
        f.call("POST", "/oauth/token", None, poll.clone()).await.1["error"],
        "slow_down"
    );
    CLOCK.fetch_add(10, Ordering::SeqCst);
    assert_eq!(
        f.call("POST", "/oauth/token", None, poll.clone()).await.1["error"],
        "authorization_pending"
    );
    f.store
        .approve_code(&code.user_code, &owner, false, now())
        .unwrap();
    CLOCK.fetch_add(10, Ordering::SeqCst);
    assert_eq!(
        f.call("POST", "/oauth/token", None, poll).await.1["error"],
        "access_denied"
    );

    let tokens = f.device(&owner).await;
    let access = tokens["access_token"].as_str().unwrap();
    let refresh = json!({"grant_type":"refresh_token", "refresh_token":tokens["refresh_token"]});
    let rotated = f.call("POST", "/oauth/token", None, refresh.clone()).await;
    assert_eq!(rotated.0, StatusCode::OK);
    assert_ne!(tokens["refresh_token"], rotated.1["refresh_token"]);
    assert_eq!(
        f.call("POST", "/oauth/token", None, refresh).await.1["error"],
        "invalid_grant"
    );
    assert_eq!(
        f.call("GET", "/v1/me", Some(access), Value::Null).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.call(
            "GET",
            "/v1/me",
            rotated.1["access_token"].as_str(),
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.call(
            "POST",
            "/oauth/token",
            None,
            json!({"grant_type":"refresh_token", "refresh_token":rotated.1["refresh_token"]})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );

    let tokens = f.device(&owner).await;
    let access = tokens["access_token"].as_str().unwrap();
    let (sid, directory) = f
        .store
        .create_server(&owner, "one", "https://192.0.2.9", "test CA", now())
        .unwrap();
    let (foreign, _) = f
        .store
        .create_server(&other, "other", "https://192.0.2.10", "test CA", now())
        .unwrap();
    let grant = f
        .call("POST", "/v1/access", Some(access), json!({"server_id":sid}))
        .await;
    assert_eq!(grant.0, StatusCode::OK);
    let jwks = f
        .call("GET", "/.well-known/jwks.json", None, Value::Null)
        .await
        .1;
    let set: jsonwebtoken::jwk::JwkSet = serde_json::from_value(jwks).unwrap();
    let key = jsonwebtoken::DecodingKey::from_jwk(&set.keys[0]).unwrap();
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::ES256);
    validation.validate_exp = false; // injected historical clock; independently check exact bounds below
    validation.set_audience(&[format!("kiln:server:{sid}")]);
    validation.set_issuer(&["https://identity.example.test"]);
    let claims = jsonwebtoken::decode::<kiln_api::auth::ServerClaims>(
        grant.1["access_token"].as_str().unwrap(),
        &key,
        &validation,
    )
    .unwrap()
    .claims;
    assert_eq!(claims.sub, owner);
    assert_eq!(claims.iat, now() as u64);
    assert_eq!(claims.exp - claims.iat, 300);
    assert_eq!(
        f.call(
            "POST",
            "/v1/access",
            Some(access),
            json!({"server_id":foreign})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let read = f
        .call(
            "POST",
            "/v1/keys",
            Some(access),
            json!({"server_id":sid,"scope":"read","expires_in":17}),
        )
        .await;
    assert_eq!(read.0, StatusCode::OK);
    let raw_key = read.1["key"].as_str().unwrap();
    assert_eq!(
        f.call("GET", "/v1/me", Some(raw_key), Value::Null).await.0,
        StatusCode::UNAUTHORIZED
    );
    let exchange = f
        .call("POST", "/v1/key-token", Some(raw_key), json!({}))
        .await;
    assert_eq!(exchange.0, StatusCode::OK);
    let claims = jsonwebtoken::decode::<kiln_api::auth::ServerClaims>(
        exchange.1["token"]["access_token"].as_str().unwrap(),
        &key,
        &validation,
    )
    .unwrap()
    .claims;
    assert_eq!(claims.scope, kiln_api::auth::Scope::Read);
    CLOCK.fetch_add(16, Ordering::SeqCst);
    assert_eq!(
        f.call("POST", "/v1/key-token", Some(raw_key), json!({}))
            .await
            .0,
        StatusCode::OK
    );
    CLOCK.fetch_add(1, Ordering::SeqCst);
    assert_eq!(
        f.call("POST", "/v1/key-token", Some(raw_key), json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.call("GET", "/v1/servers", Some(access), Value::Null)
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        f.call(
            "POST",
            "/v1/access",
            Some(&directory),
            json!({"server_id":sid})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );

    // Revocation after browser approval but before device polling must issue nothing.
    let pending = f.store.create_device("late", "192.0.2.3", now()).unwrap();
    f.store
        .approve_code(&pending.user_code, &other, true, now())
        .unwrap();
    f.db()
        .execute(
            "UPDATE accounts SET revoked_at=?1 WHERE id=?2",
            rusqlite::params![now(), other],
        )
        .unwrap();
    CLOCK.fetch_add(5, Ordering::SeqCst);
    assert_eq!(f.call("POST", "/oauth/token", None, json!({"grant_type":"urn:ietf:params:oauth:grant-type:device_code", "device_code":pending.device_code})).await.0, StatusCode::BAD_REQUEST);

    concurrent_replay(&f, &owner).await;
    registration_scenario(&f, &owner).await;

    // Idle expiry is exclusive and persists across reopen.
    let idle = f.device(&owner).await;
    CLOCK.fetch_add(30 * 86400, Ordering::SeqCst);
    assert_eq!(
        f.call(
            "POST",
            "/oauth/token",
            None,
            json!({"grant_type":"refresh_token", "refresh_token":idle["refresh_token"]})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let db = f.db();
    let count: i64 = db
        .query_row(
            "SELECT count(*) FROM refresh_tokens WHERE hash=?1",
            [Sha256::digest(idle["refresh_token"].as_str().unwrap().as_bytes()).to_vec()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

async fn registration_scenario(f: &Fixture, owner: &str) {
    let ca = rcgen::generate_simple_self_signed(vec!["localhost".into()])
        .unwrap()
        .cert
        .pem();
    let body = json!({"name":"<host>","origin":"https://127.0.0.1:9","ca_pem":ca});
    let (status, reg) = f
        .call("POST", "/v1/registrations", None, body.clone())
        .await;
    assert_eq!(status, StatusCode::OK);
    let secret = reg["registration_code"].as_str().unwrap();
    let code = reg["user_code"].as_str().unwrap();
    assert_ne!(secret, code);
    let now = now();
    f.db().execute("INSERT INTO browser_sessions(hash,csrf_hash,account_id,expires_at) VALUES(?1,?2,?3,?4)", rusqlite::params![Sha256::digest(b"browser").to_vec(),Sha256::digest(b"csrf").to_vec(),owner,now+43200]).unwrap();
    let approval = |csrf: &str, origin: &str| {
        Request::builder()
            .method("POST")
            .uri("/device/approve")
            .extension(ConnectInfo("192.0.2.2:1234".parse::<SocketAddr>().unwrap()))
            .header("Origin", origin)
            .header(
                "Cookie",
                "__Host-kiln_session=browser; __Host-kiln_csrf=csrf",
            )
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(Body::from(format!(
                "csrf={csrf}&user_code={code}&decision=approve"
            )))
            .unwrap()
    };
    for (csrf, origin) in [
        ("wrong", "https://identity.example.test"),
        ("csrf", "https://evil.example"),
    ] {
        assert_eq!(
            f.app
                .clone()
                .oneshot(approval(csrf, origin))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        f.app
            .clone()
            .oneshot(approval("csrf", "https://identity.example.test"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        f.app
            .clone()
            .oneshot(approval("csrf", "https://identity.example.test"))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    CLOCK.fetch_add(5, Ordering::SeqCst);
    let (status, registered) = f
        .call(
            "POST",
            "/v1/registrations/poll",
            None,
            json!({"registration_code":secret}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{registered}");
    let sid = registered["server_id"].as_str().unwrap();
    assert!(!f.store.servers(owner).unwrap().iter().any(|s| s.id == sid));
    let directory = registered["directory_token"].as_str().unwrap();
    assert_eq!(
        f.call(
            "POST",
            "/v1/registrations/activate",
            Some(directory),
            json!({})
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        f.call(
            "POST",
            "/v1/registrations/activate",
            Some(directory),
            json!({})
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert!(f.store.servers(owner).unwrap().iter().any(|s| s.id == sid));
    assert_eq!(
        f.call(
            "DELETE",
            &format!("/v1/servers/{sid}"),
            Some(directory),
            Value::Null
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        f.call(
            "POST",
            "/v1/registrations/activate",
            Some(directory),
            json!({})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    for _ in 0..9 {
        assert_eq!(
            f.call("POST", "/v1/registrations", None, body.clone())
                .await
                .0,
            StatusCode::OK
        );
    }
    assert_eq!(
        f.call("POST", "/v1/registrations", None, body).await.0,
        StatusCode::TOO_MANY_REQUESTS
    );
}

async fn concurrent_replay(f: &Fixture, owner: &str) {
    let tokens = f.device(owner).await;
    let other = kiln_identity::router(
        f.cfg.clone(),
        Store::open(&f.root.path().join("identity.db")).unwrap(),
    )
    .unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let tasks = [f.app.clone(), other]
        .into_iter()
        .map(|app| {
            let barrier = barrier.clone();
            let body =
                json!({"grant_type":"refresh_token","refresh_token":tokens["refresh_token"]});
            tokio::task::spawn_blocking(move || {
                barrier.wait();
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(call(&app, "POST", "/oauth/token", None, body))
            })
        })
        .collect::<Vec<_>>();
    let mut results = Vec::new();
    for task in tasks {
        results.push(task.await.unwrap());
    }
    assert_eq!(
        results.iter().filter(|(s, _)| *s == StatusCode::OK).count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|(s, _)| *s == StatusCode::BAD_REQUEST)
            .count(),
        1
    );
    let (_, issued) = results.iter().find(|(s, _)| *s == StatusCode::OK).unwrap();
    assert_eq!(
        f.call(
            "GET",
            "/v1/me",
            issued["access_token"].as_str(),
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}

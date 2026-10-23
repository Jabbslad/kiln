use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use kiln_api::auth::RevocationSet;
use kiln_identity::{Config, Store};
use p256::pkcs8::EncodePrivateKey;
use rusqlite::params;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;
use tower::ServiceExt;

struct Fixture {
    _root: tempfile::TempDir,
    app: Router,
    db_path: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
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
        let db_path = root.path().join("identity.db");
        let store = Store::open(&db_path).unwrap();
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
            now: || 10_000,
        };
        let app = kiln_identity::router(cfg, store).unwrap();
        Self {
            _root: root,
            app,
            db_path,
        }
    }

    fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(&self.db_path).unwrap()
    }

    async fn post(&self, path: &str, token: &str, body: Value) -> (StatusCode, Value) {
        let request = Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 65_536).await.unwrap();
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, value)
    }
}

#[tokio::test]
async fn tenant_wide_revocation_set_lists_removed_devices_and_keys() {
    let f = Fixture::new();
    let db = f.db();
    db.execute_batch(
        "INSERT INTO accounts(id,created_at) VALUES('owner-a',1),('member-b',1);
         INSERT INTO tenants(id,owner_account_id,created_at) VALUES('ten_test','owner-a',1);
         INSERT INTO tenant_members(tenant_id,account_id,role) VALUES('ten_test','owner-a','owner'),('ten_test','member-b','member');
         INSERT INTO servers(id,owner_id,tenant_id,name,origin,ca_pem,revision,active,directory_hash,created_at,login_secret_hash)
         VALUES('srv_a','owner-a','ten_test','lab','https://host/','pem',1,1,x'00',1,x'aa'),('srv_b','owner-a','ten_test','lab2','https://host2/','pem',1,1,x'01',1,x'aa');
         INSERT INTO devices(id,account_id,name,created_at,last_used,revoked_at)
         VALUES('dev_live','owner-a','laptop',1,1,NULL),('dev_revoked','member-b','other',1,1,999);
         INSERT INTO automation_keys(id,owner_id,server_id,hash,scope,expires_at,revoked_at)
         VALUES('key_live','owner-a','srv_a',x'01','operate',999999,NULL),('key_revoked','owner-a','srv_b',x'02','operate',999999,999);",
    )
    .unwrap();
    let login = "login-secret";
    db.execute(
        "UPDATE servers SET login_secret_hash=?1 WHERE id='srv_a'",
        params![Sha256::digest(login.as_bytes()).to_vec()],
    )
    .unwrap();
    let (status, body) = f
        .post("/v1/revocations", login, json!({"tenant_id":"ten_test"}))
        .await;
    assert_eq!(status, StatusCode::OK);
    let set: RevocationSet = serde_json::from_value(body).unwrap();
    assert!(set.credentials.contains(&"dev_revoked".to_string()));
    assert!(set.credentials.contains(&"key_revoked".to_string()));
    assert!(!set.credentials.contains(&"dev_live".to_string()));
}

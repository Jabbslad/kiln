use axum::{
    Router,
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use kiln_api::auth::{MemberRole, Scope, ServerClaims};
use kiln_identity::{Config, Store};
use p256::pkcs8::EncodePrivateKey;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use tower::ServiceExt;

struct Fixture {
    app: Router,
    store: Store,
    root: tempfile::TempDir,
}

impl Fixture {
    fn new(now: fn() -> i64) -> Self {
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
        Self { app, store, root }
    }

    fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.root.path().join("identity.db")).unwrap()
    }

    async fn json(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .extension(ConnectInfo("192.0.2.50:9".parse::<SocketAddr>().unwrap()));
        if let Some(token) = token {
            req = req.header("authorization", format!("Bearer {token}"));
        }
        let body = if let Some(body) = body {
            req = req.header("content-type", "application/json");
            Body::from(body.to_string())
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
        let bytes = to_bytes(response.into_body(), 131072).await.unwrap();
        let value = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, value)
    }
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

fn central_token(
    db: &rusqlite::Connection,
    account: &str,
    device: &str,
    raw: &str,
    refresh_seed: &[u8],
    now: i64,
) {
    db.execute(
        "INSERT INTO devices(id,account_id,name,created_at,last_used) VALUES(?1,?2,'laptop',?3,?3)",
        rusqlite::params![device, account, now],
    )
    .unwrap();
    let family = format!("fam-{device}");
    db.execute(
        "INSERT INTO refresh_tokens(hash,family_id,device_id,created_at,last_used,absolute_expiry) VALUES(?1,?2,?3,?4,?4,?5)",
        rusqlite::params![
            Sha256::digest(refresh_seed).to_vec(),
            family,
            device,
            now,
            now + 86400
        ],
    )
    .unwrap();
    db.execute(
        "INSERT INTO central_tokens(hash,device_id,family_id,expires_at) VALUES(?1,?2,?3,?4)",
        rusqlite::params![
            Sha256::digest(raw.as_bytes()).to_vec(),
            device,
            format!("fam-{device}"),
            now + 3600
        ],
    )
    .unwrap();
}

fn claims(jwt: &str) -> ServerClaims {
    let payload = jwt.split('.').nth(1).unwrap();
    let bytes = URL_SAFE_NO_PAD.decode(payload).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
fn version_two_migrates_to_three_and_groups_servers_by_owner_tenant() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.path().join("identity.db");
    legacy_v2(&path);
    drop(Store::open(&path).unwrap());
    let db = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = db
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 3);
    let tenant: String = db
        .query_row(
            "SELECT tenant_id FROM servers WHERE id='srv_one'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let owner_row: (String, String) = db
        .query_row(
            "SELECT account_id,role FROM tenant_members WHERE tenant_id=?1",
            [&tenant],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(owner_row, ("acct_owner".to_owned(), "owner".to_owned()));
    let default: String = db
        .query_row(
            "SELECT default_server_id FROM tenants WHERE id=?1",
            [&tenant],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(default, "srv_one");
}

#[tokio::test]
async fn tenant_membership_is_shared_and_tokens_carry_tenant_id() {
    fn now() -> i64 {
        8_000
    }
    let f = Fixture::new(now);
    let owner = f
        .store
        .seed_account("github", "801", "owner", now())
        .unwrap();
    let other = f
        .store
        .seed_account("github", "802", "other", now())
        .unwrap();
    let (server_a, directory_a) = f
        .store
        .create_server(&owner, "a", "https://192.0.2.81", "ca", now())
        .unwrap();
    let (server_b, _directory_b) = f
        .store
        .create_server(&owner, "b", "https://192.0.2.82", "ca", now())
        .unwrap();
    let tenant: String = f
        .db()
        .query_row(
            "SELECT tenant_id FROM servers WHERE id=?1",
            [&server_a],
            |row| row.get(0),
        )
        .unwrap();
    let same: String = f
        .db()
        .query_row(
            "SELECT tenant_id FROM servers WHERE id=?1",
            [&server_b],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tenant, same);

    let central = "c".repeat(43);
    central_token(
        &f.db(),
        &owner,
        "dev_owner",
        &central,
        b"refresh-owner",
        now(),
    );

    let (status, _) = f
        .json(
            "POST",
            &format!("/v1/tenants/{tenant}/members"),
            Some(&central),
            Some(json!({"name":"automation"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = f
        .json(
            "GET",
            &format!("/v1/tenants/{tenant}/members"),
            Some(&directory_a),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["tenant_id"], tenant);
    let members = body["members"].as_array().unwrap();
    assert_eq!(members.len(), 2);
    let member = members
        .iter()
        .find(|m| m["role"] == "member")
        .expect("member row");
    let member_id = member["account_id"].as_str().unwrap();

    let (status, key_body) = f
        .json(
            "POST",
            "/v1/keys",
            Some(&central),
            Some(json!({
                "server_id": server_a,
                "scope": "operate",
                "account": "automation",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let key_secret = key_body["key"].as_str().unwrap().to_owned();

    let (status, token_body) = f
        .json("POST", "/v1/key-token", Some(&key_secret), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let jwt = token_body["token"]["access_token"].as_str().unwrap();
    let decoded = claims(jwt);
    assert_eq!(decoded.sub, member_id);
    assert_eq!(decoded.tenant_id, tenant);
    assert_eq!(decoded.role, MemberRole::Member);
    assert_eq!(decoded.scope, Scope::Operate);

    let other_central = "d".repeat(43);
    central_token(
        &f.db(),
        &other,
        "dev_other",
        &other_central,
        b"refresh-other",
        now(),
    );
    let (status, _) = f
        .json(
            "POST",
            &format!("/v1/tenants/{tenant}/members"),
            Some(&other_central),
            Some(json!({"name":"blocked"})),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = f
        .json(
            "DELETE",
            &format!("/v1/tenants/{tenant}/members/automation"),
            Some(&central),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

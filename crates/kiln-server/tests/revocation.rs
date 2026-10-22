use axum::{Json, Router, routing::post};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use kiln_api::auth::{RevocationQuery, RevocationSet, Role, Scope, ServerClaims};
use kiln_server::auth::{Authenticator, Enrollment};

#[path = "common.rs"]
mod common;

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Mutex;

fn fixture() -> (EncodingKey, serde_json::Value) {
    use base64::Engine;
    use p256::{elliptic_curve::sec1::ToEncodedPoint, pkcs8::EncodePrivateKey};
    let key = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
    let pem = key.to_pkcs8_pem(Default::default()).unwrap();
    let point = key.public_key().to_encoded_point(false);
    let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    (
        EncodingKey::from_ec_pem(pem.as_bytes()).unwrap(),
        serde_json::json!({"keys":[{
            "kty":"EC","crv":"P-256","use":"sig","alg":"ES256","kid":"first",
            "x":enc.encode(point.x().unwrap()),"y":enc.encode(point.y().unwrap())
        }]}),
    )
}

fn jwt(key: &EncodingKey, enrollment: &Enrollment, credential_id: &str) -> String {
    let now = jsonwebtoken::get_current_timestamp();
    let claims = ServerClaims {
        iss: enrollment.issuer.clone(),
        sub: enrollment.owner_id.clone(),
        aud: format!("kiln:server:{}", enrollment.server_id),
        tenant_id: enrollment.tenant_id.clone(),
        iat: now,
        nbf: now,
        exp: now + 300,
        credential_id: credential_id.into(),
        scope: Scope::Operate,
        role: Some(Role::Owner),
    };
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("first".into());
    jsonwebtoken::encode(&header, &claims, key).unwrap()
}

#[tokio::test]
async fn tenant_revocation_feed_rejects_within_one_refresh_cycle() {
    let revocations = Arc::new(Mutex::new(RevocationSet {
        accounts: vec![],
        credentials: vec![],
    }));
    let fetches = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/v1/revocations",
        post({
            let revocations = revocations.clone();
            let fetches = fetches.clone();
            move |Json(body): Json<RevocationQuery>| {
                fetches.fetch_add(1, Ordering::SeqCst);
                assert_eq!(body.tenant_id, common::FIXTURE_TENANT_ID);
                async move {
                    let set = revocations.lock().await.clone();
                    Json(set)
                }
            }
        }),
    );
    let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        cert.cert.pem().into_bytes(),
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
    let enrollment = Enrollment {
        version: 1,
        issuer: format!("https://127.0.0.1:{port}"),
        owner_id: "owner-a".into(),
        server_id: "server-b".into(),
        tenant_id: common::fixture_tenant_id(),
    };
    let http = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .https_only(true)
        .build()
        .unwrap();
    let watch =
        kiln_server::revocation::RevocationWatch::new(enrollment, "login-secret".into(), http);
    watch
        .allow("owner-a", "device-live", Role::Owner)
        .unwrap();
    *revocations.lock().await = RevocationSet {
        accounts: vec![],
        credentials: vec!["device-live".into()],
    };
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(300)).await;
        if watch
            .allow("owner-a", "device-live", Role::Owner)
            .is_err()
        {
            return;
        }
    }
    panic!("revoked credential was still accepted after refresh window");
}

#[tokio::test]
async fn member_tokens_fail_closed_when_revocation_feed_is_stale() {
    let watch = kiln_server::revocation::RevocationWatch::new(
        Enrollment {
            version: 1,
            issuer: "https://identity.example.test".into(),
            owner_id: "owner-a".into(),
            server_id: "server-b".into(),
            tenant_id: common::fixture_tenant_id(),
        },
        "login-secret".into(),
        reqwest::Client::builder().https_only(true).build().unwrap(),
    );
    assert!(
        watch
            .allow("member-b", "device-m", Role::Member)
            .is_err()
    );
}

#[tokio::test]
async fn authenticator_checks_the_tenant_revocation_feed() {
    let (key, jwks) = fixture();
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("keys.json");
    let enrollment = Enrollment {
        version: 1,
        issuer: "https://identity.example.test".into(),
        owner_id: "owner-a".into(),
        server_id: "server-b".into(),
        tenant_id: common::fixture_tenant_id(),
    };
    std::fs::write(
        &cache,
        serde_json::to_vec(&serde_json::json!({"issuer":enrollment.issuer,"jwks":jwks})).unwrap(),
    )
    .unwrap();
    let auth = Authenticator::central_only(enrollment.clone(), cache)
        .unwrap()
        .with_login_secret("login-secret".into())
        .unwrap();
    let live = jwt(&key, &enrollment, "device-live");
    auth.authenticate_central(&live).await.unwrap();
}

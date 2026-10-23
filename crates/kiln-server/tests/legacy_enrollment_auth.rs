//! Legacy `/etc/kiln/identity.json` without `tenant_id` must accept owner app tokens
//! that carry the v3 tenant UUID (as `issue_app` stamps) and pre-v3 owner tokens that
//! omit `tenant_id`, and still refuse other tenants and non-owner subjects.

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use kiln_api::auth::{Role, Scope, ServerClaims};
use kiln_server::auth::{Authenticator, Enrollment};

fn fixture() -> (Authenticator, EncodingKey, Enrollment) {
    use base64::Engine;
    use p256::{elliptic_curve::sec1::ToEncodedPoint, pkcs8::EncodePrivateKey};
    let root = tempfile::tempdir().unwrap();
    let key = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
    let encoded = key.to_pkcs8_pem(p256::pkcs8::LineEnding::LF).unwrap();
    let point = key.public_key().to_encoded_point(false);
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let jwks = serde_json::json!({"keys":[{
        "kty":"EC","crv":"P-256","use":"sig","alg":"ES256","kid":"fixture-key",
        "x":b64.encode(point.x().unwrap()),"y":b64.encode(point.y().unwrap())
    }]});
    let enrollment = Enrollment {
        version: 1,
        issuer: "https://identity.example.test".into(),
        owner_id: "acct_owner".into(),
        server_id: "srv_one".into(),
        tenant_id: None,
    };
    let cache = root.path().join("jwks.json");
    std::fs::write(
        &cache,
        serde_json::to_vec(&serde_json::json!({
            "issuer": enrollment.issuer,
            "jwks": jwks
        }))
        .unwrap(),
    )
    .unwrap();
    let auth = Authenticator::central_only(enrollment.clone(), cache).unwrap();
    (
        auth,
        EncodingKey::from_ec_pem(encoded.as_bytes()).unwrap(),
        enrollment,
    )
}

fn issue_app_token(
    key: &EncodingKey,
    enrollment: &Enrollment,
    sub: &str,
    tenant_id: Option<&str>,
    role: Role,
    scope: Scope,
) -> String {
    let now = jsonwebtoken::get_current_timestamp();
    let claims = ServerClaims {
        iss: enrollment.issuer.clone(),
        sub: sub.into(),
        aud: format!("kiln:server:{}", enrollment.server_id),
        tenant_id: tenant_id.map(str::to_owned),
        iat: now,
        nbf: now,
        exp: now + 300,
        credential_id: format!("app_{}", enrollment.server_id),
        scope,
        role: Some(role),
    };
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("fixture-key".into());
    jsonwebtoken::encode(&header, &claims, key).unwrap()
}

#[tokio::test]
async fn legacy_enrollment_accepts_owner_app_token_with_v3_tenant_id() {
    let (auth, key, enrollment) = fixture();
    let token = issue_app_token(
        &key,
        &enrollment,
        "acct_owner",
        Some("ten_production_uuid"),
        Role::Owner,
        Scope::Operate,
    );
    let grant = auth.authenticate_central(&token).await.unwrap();
    assert_eq!(grant.caller.sub, "acct_owner");
    assert_eq!(grant.caller.tenant_id, "ten_production_uuid");
    assert_eq!(grant.caller.role, Role::Owner);
    assert!(matches!(grant.scope, Scope::Operate));
}

#[tokio::test]
async fn legacy_enrollment_refuses_another_servers_owner_token_before_bind() {
    let (auth, key, enrollment) = fixture();
    let now = jsonwebtoken::get_current_timestamp();
    let claims = ServerClaims {
        iss: enrollment.issuer.clone(),
        sub: "acct_owner".into(),
        aud: "kiln:server:srv_other".into(),
        tenant_id: Some("ten_other".into()),
        iat: now,
        nbf: now,
        exp: now + 300,
        credential_id: "app_srv_other".into(),
        scope: Scope::Operate,
        role: Some(Role::Owner),
    };
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("fixture-key".into());
    let token = jsonwebtoken::encode(&header, &claims, &key).unwrap();
    assert!(auth.authenticate_central(&token).await.is_err());
}

#[tokio::test]
async fn legacy_enrollment_refuses_wrong_tenant_after_owner_bind() {
    let (auth, key, enrollment) = fixture();
    let good = issue_app_token(
        &key,
        &enrollment,
        "acct_owner",
        Some("ten_bound"),
        Role::Owner,
        Scope::Operate,
    );
    auth.authenticate_central(&good).await.unwrap();
    let bad = issue_app_token(
        &key,
        &enrollment,
        "acct_owner",
        Some("ten_other"),
        Role::Owner,
        Scope::Operate,
    );
    assert!(auth.authenticate_central(&bad).await.is_err());
}

#[tokio::test]
async fn legacy_enrollment_refuses_member_token_for_another_tenant() {
    let (auth, key, enrollment) = fixture();
    let token = issue_app_token(
        &key,
        &enrollment,
        "acct_member",
        Some("ten_other_tenant"),
        Role::Member,
        Scope::Read,
    );
    assert!(auth.authenticate_central(&token).await.is_err());
}

#[tokio::test]
async fn legacy_enrollment_accepts_pre_v3_owner_app_token_without_tenant_id() {
    let (auth, key, enrollment) = fixture();
    let token = issue_app_token(
        &key,
        &enrollment,
        "acct_owner",
        None,
        Role::Owner,
        Scope::Operate,
    );
    let grant = auth.authenticate_central(&token).await.unwrap();
    assert_eq!(grant.caller.sub, "acct_owner");
    assert_eq!(grant.caller.tenant_id, "acct_owner");
    assert_eq!(grant.caller.role, Role::Owner);
    assert!(matches!(grant.scope, Scope::Operate));
}

#[tokio::test]
async fn legacy_enrollment_refuses_non_owner_token_without_tenant_id() {
    let (auth, key, enrollment) = fixture();
    let token = issue_app_token(
        &key,
        &enrollment,
        "acct_other",
        None,
        Role::Owner,
        Scope::Operate,
    );
    assert!(auth.authenticate_central(&token).await.is_err());
}

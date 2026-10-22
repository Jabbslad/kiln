use jsonwebtoken::{Algorithm, EncodingKey, Header};
use kiln_api::{Role, Scope, ServerClaims};
use kiln_server::auth::{Enrollment, verify};

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

fn claims(enrollment: &Enrollment, sub: &str, role: Role, scope: Scope, now: u64) -> ServerClaims {
    ServerClaims {
        iss: enrollment.issuer.clone(),
        sub: sub.into(),
        aud: format!("kiln:server:{}", enrollment.server_id),
        tenant_id: Some(
            enrollment
                .bound_tenant_id()
                .unwrap_or(enrollment.owner_id.as_str())
                .into(),
        ),
        iat: now,
        nbf: now,
        exp: now + 300,
        credential_id: "device-c".into(),
        scope,
        role: Some(role),
    }
}

#[test]
fn legacy_enrollment_binds_owner_app_tenant_from_the_first_valid_token() {
    let (key, jwks) = fixture();
    let enrollment = Enrollment {
        version: 1,
        issuer: "https://identity.example.test".into(),
        owner_id: "owner-a".into(),
        server_id: "server-b".into(),
        tenant_id: None,
    };
    let now = jsonwebtoken::get_current_timestamp();
    let token_claims = ServerClaims {
        iss: enrollment.issuer.clone(),
        sub: "owner-a".into(),
        aud: format!("kiln:server:{}", enrollment.server_id),
        tenant_id: Some("ten_v3".into()),
        iat: now,
        nbf: now,
        exp: now + 300,
        credential_id: "app_server-b".into(),
        scope: Scope::Operate,
        role: Some(Role::Owner),
    };
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("first".into());
    let token = jsonwebtoken::encode(&header, &token_claims, &key).unwrap();
    let grant = verify(&token, &enrollment, &jwks).unwrap();
    assert_eq!(grant.caller.tenant_id, "ten_v3");
    assert_eq!(grant.caller.role, Role::Owner);
}

#[test]
fn legacy_enrollment_without_tenant_id_starts_unbound() {
    let json = r#"{"version":1,"issuer":"https://identity.example.test","owner_id":"owner-a","server_id":"server-b"}"#;
    let enrollment: Enrollment = serde_json::from_str(json).unwrap();
    enrollment.validate().unwrap();
    assert!(enrollment.bound_tenant_id().is_none());
}

#[test]
fn issuer_tenant_audience_time_and_scope_are_enforced() {
    let (key, jwks) = fixture();
    let enrollment = Enrollment {
        version: 1,
        issuer: "https://identity.example.test".into(),
        owner_id: "owner-a".into(),
        server_id: "server-b".into(),
        tenant_id: Some("tenant-t".into()),
    };
    let now = jsonwebtoken::get_current_timestamp();
    let token_claims = claims(&enrollment, "owner-a", Role::Owner, Scope::Read, now);
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("first".into());
    let encode = |c: &ServerClaims| jsonwebtoken::encode(&header, c, &key).unwrap();
    let grant = verify(&encode(&token_claims), &enrollment, &jwks).unwrap();
    assert_eq!(grant.scope, Scope::Read);
    assert_eq!(grant.caller.sub, "owner-a");
    assert_eq!(grant.caller.tenant_id, "tenant-t");
    assert_eq!(grant.caller.role, Role::Owner);
    for field in [
        "issuer", "tenant", "server", "expired", "future", "lifetime",
    ] {
        let mut bad = token_claims.clone();
        match field {
            "issuer" => bad.iss.push_str("/other"),
            "tenant" => bad.tenant_id = Some("tenant-other".into()),
            "server" => bad.aud = "kiln:server:server-e".into(),
            "expired" => {
                bad.iat = now - 400;
                bad.nbf = now - 400;
                bad.exp = now - 100;
            }
            "future" => {
                bad.iat = now + 100;
                bad.nbf = now + 100;
                bad.exp = now + 400;
            }
            "lifetime" => bad.exp = now + 301,
            _ => unreachable!(),
        }
        assert!(
            verify(&encode(&bad), &enrollment, &jwks).is_err(),
            "{field}"
        );
    }
    header.jku = Some("https://attacker.example.test/keys".into());
    assert!(
        verify(
            &jsonwebtoken::encode(&header, &token_claims, &key).unwrap(),
            &enrollment,
            &jwks
        )
        .is_err()
    );
}

#[test]
fn tenant_member_is_accepted_when_tenant_id_matches() {
    let (key, jwks) = fixture();
    let enrollment = Enrollment {
        version: 1,
        issuer: "https://identity.example.test".into(),
        owner_id: "owner-a".into(),
        server_id: "server-b".into(),
        tenant_id: Some("tenant-t".into()),
    };
    let now = jsonwebtoken::get_current_timestamp();
    let token_claims = claims(&enrollment, "member-z", Role::Member, Scope::Read, now);
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("first".into());
    let token = jsonwebtoken::encode(&header, &token_claims, &key).unwrap();
    let grant = verify(&token, &enrollment, &jwks).unwrap();
    assert_eq!(grant.caller.sub, "member-z");
    assert_eq!(grant.caller.role, Role::Member);
    assert_eq!(grant.caller.tenant_id, "tenant-t");
    let mut other_tenant = token_claims.clone();
    other_tenant.tenant_id = Some("tenant-other".into());
    assert!(
        verify(
            &jsonwebtoken::encode(&header, &other_tenant, &key).unwrap(),
            &enrollment,
            &jwks
        )
        .is_err()
    );
}

#[tokio::test]
async fn central_caller_returns_the_authenticated_sub() {
    let (key, jwks) = fixture();
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("keys.json");
    let enrollment = Enrollment {
        version: 1,
        issuer: "https://identity.example.test".into(),
        owner_id: "owner-a".into(),
        server_id: "server-b".into(),
        tenant_id: Some("tenant-t".into()),
    };
    std::fs::write(
        &cache,
        serde_json::to_vec(&serde_json::json!({"issuer":enrollment.issuer,"jwks":jwks})).unwrap(),
    )
    .unwrap();
    let auth = kiln_server::auth::Authenticator::central_only(enrollment.clone(), cache).unwrap();
    let now = jsonwebtoken::get_current_timestamp();
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("first".into());
    let token = jsonwebtoken::encode(
        &header,
        &claims(&enrollment, "member-z", Role::Member, Scope::Read, now),
        &key,
    )
    .unwrap();
    assert_eq!(auth.central_caller(&token).await, "member-z");
}

#[tokio::test]
async fn read_scope_cannot_reach_operations_or_ssh() {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let (key, jwks) = fixture();
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("keys.json");
    let enrollment = Enrollment {
        version: 1,
        issuer: "https://identity.example.test".into(),
        owner_id: "owner-a".into(),
        server_id: "server-b".into(),
        tenant_id: None,
    };
    std::fs::write(
        &cache,
        serde_json::to_vec(&serde_json::json!({"issuer":enrollment.issuer,"jwks":jwks})).unwrap(),
    )
    .unwrap();
    let auth =
        kiln_server::auth::Authenticator::new(&"a".repeat(64), Some((enrollment.clone(), cache)))
            .unwrap();
    let router =
        kiln_server::gateway::router_with_auth(&root.path().join("missing.sock"), auth).unwrap();
    let now = jsonwebtoken::get_current_timestamp();
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("first".into());
    let token = jsonwebtoken::encode(
        &header,
        &claims(&enrollment, "owner-a", Role::Owner, Scope::Read, now),
        &key,
    )
    .unwrap();
    for (method, path, status) in [
        ("GET", "/v1/templates", StatusCode::BAD_GATEWAY),
        ("GET", "/v1/boxes", StatusCode::BAD_GATEWAY),
        ("GET", "/v1/boxes/abc", StatusCode::BAD_GATEWAY),
        ("GET", "/v1/operations/abc", StatusCode::FORBIDDEN),
        ("POST", "/v1/operations", StatusCode::FORBIDDEN),
        ("GET", "/v1/boxes/abc/ssh-key", StatusCode::FORBIDDEN),
        ("GET", "/v1/boxes/abc/ssh", StatusCode::FORBIDDEN),
    ] {
        let r = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("Authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), status, "{method} {path}");
    }
}

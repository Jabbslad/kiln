use jsonwebtoken::{Algorithm, EncodingKey, Header};
use kiln_api::auth::{Scope, ServerClaims};
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

#[test]
fn issuer_owner_audience_time_and_scope_are_enforced() {
    let (key, jwks) = fixture();
    let enrollment = Enrollment {
        version: 1,
        issuer: "https://identity.example.test".into(),
        owner_id: "owner-a".into(),
        server_id: "server-b".into(),
    };
    let now = jsonwebtoken::get_current_timestamp();
    let claims = ServerClaims {
        iss: enrollment.issuer.clone(),
        sub: enrollment.owner_id.clone(),
        aud: "kiln:server:server-b".into(),
        iat: now,
        nbf: now,
        exp: now + 300,
        credential_id: "device-c".into(),
        scope: Scope::Read,
    };
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("first".into());
    let encode = |c: &ServerClaims| jsonwebtoken::encode(&header, c, &key).unwrap();
    assert_eq!(
        verify(&encode(&claims), &enrollment, &jwks).unwrap(),
        Scope::Read
    );
    for field in ["issuer", "owner", "server", "expired", "future", "lifetime"] {
        let mut bad = claims.clone();
        match field {
            "issuer" => bad.iss.push_str("/other"),
            "owner" => bad.sub = "owner-d".into(),
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
            &jsonwebtoken::encode(&header, &claims, &key).unwrap(),
            &enrollment,
            &jwks
        )
        .is_err()
    );
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
    let c = ServerClaims {
        iss: enrollment.issuer,
        sub: enrollment.owner_id,
        aud: "kiln:server:server-b".into(),
        iat: now,
        nbf: now,
        exp: now + 300,
        credential_id: "device-c".into(),
        scope: Scope::Read,
    };
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("first".into());
    let token = jsonwebtoken::encode(&header, &c, &key).unwrap();
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

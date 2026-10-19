use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use kiln_api::auth::{Scope, ServerClaims};
use kiln_server::{
    auth::{Authenticator, Enrollment},
    door,
};
use tower::ServiceExt;

fn sockets(port: u16) -> usize {
    let marker = format!(":{port:04X}");
    std::fs::read_to_string("/proc/net/tcp")
        .unwrap()
        .lines()
        .filter(|line| line.contains(&marker))
        .count()
}

fn token(key: &EncodingKey, sub: &str, audience: &str) -> String {
    let now = jsonwebtoken::get_current_timestamp();
    let claims = ServerClaims {
        iss: "https://identity.example.test".into(),
        sub: sub.into(),
        aud: audience.into(),
        iat: now,
        nbf: now,
        exp: now + 300,
        credential_id: "device-1".into(),
        scope: Scope::Operate,
    };
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("first".into());
    jsonwebtoken::encode(&header, &claims, key).unwrap()
}

struct Upstream {
    port: u16,
    hits: Arc<AtomicUsize>,
    ca_pem: String,
}

async fn upstream() -> Upstream {
    let hits = Arc::new(AtomicUsize::new(0));
    let seen = hits.clone();
    let app = axum::Router::new().route(
        "/v1/templates",
        axum::routing::get(move || {
            let seen = seen.clone();
            async move {
                seen.fetch_add(1, Ordering::SeqCst);
                axum::Json(serde_json::json!([]))
            }
        }),
    );
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
    Upstream { port, hits, ca_pem }
}

fn auth(dir: &std::path::Path) -> (Authenticator, EncodingKey) {
    use base64::Engine;
    use p256::{elliptic_curve::sec1::ToEncodedPoint, pkcs8::EncodePrivateKey};
    let key = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
    let pem = key.to_pkcs8_pem(Default::default()).unwrap();
    let point = key.public_key().to_encoded_point(false);
    let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let jwks = serde_json::json!({"keys":[{
        "kty":"EC","crv":"P-256","use":"sig","alg":"ES256","kid":"first",
        "x":enc.encode(point.x().unwrap()),"y":enc.encode(point.y().unwrap())
    }]});
    let enrollment = Enrollment {
        version: 1,
        issuer: "https://identity.example.test".into(),
        owner_id: "owner-a".into(),
        server_id: "server-b".into(),
    };
    let cache = dir.join("jwks.json");
    std::fs::write(
        &cache,
        serde_json::to_vec(&serde_json::json!({"issuer": enrollment.issuer, "jwks": jwks}))
            .unwrap(),
    )
    .unwrap();
    (
        Authenticator::central_only(enrollment, cache).unwrap(),
        EncodingKey::from_ec_pem(pem.as_bytes()).unwrap(),
    )
}

#[tokio::test]
async fn unauthenticated_and_wrong_account_requests_do_not_reach_the_api() {
    let root = tempfile::tempdir().unwrap();
    let up = upstream().await;
    let (auth, key) = auth(root.path());
    let router = door::router(
        auth,
        &format!("https://127.0.0.1:{}", up.port),
        up.ca_pem.as_bytes(),
    )
    .unwrap();
    let before = sockets(up.port);
    for (label, header) in [
        ("missing", None),
        ("admin", Some(format!("Bearer {}", "ab".repeat(32)))),
        (
            "wrong-account",
            Some(format!(
                "Bearer {}",
                token(&key, "owner-b", "kiln:server:server-b")
            )),
        ),
        (
            "wrong-server",
            Some(format!(
                "Bearer {}",
                token(&key, "owner-a", "kiln:server:server-c")
            )),
        ),
    ] {
        let mut request = Request::builder().uri("/v1/templates");
        if let Some(header) = header {
            request = request.header("authorization", header);
        }
        let response = router
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{label}");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(up.hits.load(Ordering::SeqCst), 0, "{label}");
        assert_eq!(sockets(up.port), before, "{label}");
    }
    let response = router
        .oneshot(
            Request::builder()
                .uri("/v1/templates")
                .header(
                    "authorization",
                    format!("Bearer {}", token(&key, "owner-a", "kiln:server:server-b")),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(up.hits.load(Ordering::SeqCst), 1);
}

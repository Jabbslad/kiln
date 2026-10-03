#![allow(dead_code)]

#[path = "../src/providers.rs"]
mod providers;

use axum::{
    Json, Router,
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use p256::{SecretKey, elliptic_curve::sec1::ToEncodedPoint, pkcs8::EncodePrivateKey};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tower::ServiceExt;

static CLOCK: AtomicI64 = AtomicI64::new(1_000);
fn now() -> i64 {
    CLOCK.load(Ordering::SeqCst)
}

#[derive(Clone, Copy)]
enum Fault {
    None,
    Signature,
    Issuer,
    Audience,
    Nonce,
    AtHash,
}

struct Fixture {
    base: String,
    client: reqwest::Client,
    fault: Arc<Mutex<Fault>>,
}

impl Fixture {
    async fn new() -> Self {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let cert_pem = cert.cert.pem();
        let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
            cert_pem.clone().into_bytes(),
            cert.signing_key.serialize_pem().into_bytes(),
        )
        .await
        .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let key = SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let wrong_key = SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let point = key.public_key().to_encoded_point(false);
        let x = URL_SAFE_NO_PAD.encode(point.x().unwrap());
        let y = URL_SAFE_NO_PAD.encode(point.y().unwrap());
        let pem = key.to_pkcs8_pem(Default::default()).unwrap().to_string();
        let wrong_pem = wrong_key
            .to_pkcs8_pem(Default::default())
            .unwrap()
            .to_string();
        let base = format!("https://localhost:{}", addr.port());
        let fault = Arc::new(Mutex::new(Fault::None));
        let state = fault.clone();
        let issuer = base.clone();
        let token = post(move || {
            let fault = *state.lock().unwrap();
            let issuer = issuer.clone();
            let pem = pem.clone();
            let wrong_pem = wrong_pem.clone();
            async move {
                let access = "fixture-access-token";
                let digest = Sha256::digest(access.as_bytes());
                let mut claims = json!({
                    "iss": issuer,
                    "sub": "101",
                    "aud": "fixture-google",
                    "exp": SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() + 300,
                    "iat": SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
                    "nonce": "fixture-nonce",
                    "at_hash": URL_SAFE_NO_PAD.encode(&digest[..digest.len()/2]),
                    "name": "Fixture User"
                });
                match fault {
                    Fault::Issuer => claims["iss"] = json!("https://attacker.example"),
                    Fault::Audience => claims["aud"] = json!("other-client"),
                    Fault::Nonce => claims["nonce"] = json!("other-nonce"),
                    Fault::AtHash => claims["at_hash"] = json!("invalid"),
                    _ => {}
                }
                let mut header = Header::new(Algorithm::ES256);
                header.kid = Some("fixture-key".into());
                let signing = if matches!(fault, Fault::Signature) {
                    &wrong_pem
                } else {
                    &pem
                };
                let id_token = encode(
                    &header,
                    &claims,
                    &EncodingKey::from_ec_pem(signing.as_bytes()).unwrap(),
                )
                .unwrap();
                Json(json!({"access_token":access,"token_type":"Bearer","id_token":id_token}))
            }
        });
        let discovery_base = base.clone();
        let discovery = get(move || async move {
            Json(json!({
                "issuer": discovery_base,
                "authorization_endpoint": format!("{discovery_base}/authorize"),
                "token_endpoint": format!("{discovery_base}/token"),
                "jwks_uri": format!("{discovery_base}/jwks"),
                "response_types_supported":["code"], "subject_types_supported":["public"],
                "id_token_signing_alg_values_supported":["ES256"]
            }))
        });
        let jwks = get(move || async move {
            Json(json!({"keys":[{
                "kty":"EC","use":"sig","crv":"P-256","kid":"fixture-key","alg":"ES256","x":x,"y":y
            }]}))
        });
        let app = Router::new()
            .route("/.well-known/openid-configuration", discovery)
            .route("/jwks", jwks)
            .route("/token", token)
            .route(
                "/github/token",
                post(|| async { Json(json!({"access_token":"gh-token","token_type":"bearer"})) }),
            )
            .route(
                "/github/fail",
                post(|| async {
                    (
                        axum::http::StatusCode::UNAUTHORIZED,
                        Json(json!({"error":"bad_verification_code"})),
                    )
                }),
            )
            .route(
                "/github/user",
                get(|| async {
                    Json(json!({"id":101,"login":"mutable-name","email":"same@example.test"}))
                }),
            );
        tokio::spawn(
            axum_server::from_tcp_rustls(listener, tls)
                .unwrap()
                .serve(app.into_make_service()),
        );
        let root = reqwest::Certificate::from_pem(cert_pem.as_bytes()).unwrap();
        let client = reqwest::Client::builder()
            .add_root_certificate(root)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .unwrap();
        Self {
            base,
            client,
            fault,
        }
    }

    async fn google(&self) -> anyhow::Result<providers::ProviderIdentity> {
        providers::test_google_finish(
            &self.client,
            "fixture-google",
            "secret",
            "https://identity.example/callback",
            "code",
            "fixture-nonce",
            "verifier",
            &self.base,
        )
        .await
    }
}

#[tokio::test]
async fn github_uses_numeric_immutable_subject_and_rejects_failed_exchange() {
    let f = Fixture::new().await;
    let identity = providers::test_github_finish(
        &f.client,
        "client",
        "secret",
        "code",
        "https://identity.example/callback",
        "verifier",
        &format!("{}/github/token", f.base),
        &format!("{}/github/user", f.base),
    )
    .await
    .unwrap();
    assert_eq!(identity.provider, providers::Provider::Github);
    assert_eq!(identity.subject, "101");
    assert_eq!(identity.display_name, "mutable-name");
    assert!(
        providers::test_github_finish(
            &f.client,
            "client",
            "secret",
            "bad",
            "https://identity.example/callback",
            "verifier",
            &format!("{}/github/fail", f.base),
            &format!("{}/github/user", f.base)
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn google_verifies_signature_issuer_audience_nonce_and_access_token_hash() {
    let f = Fixture::new().await;
    let identity = f.google().await.unwrap();
    assert_eq!(identity.provider, providers::Provider::Google);
    assert_eq!(identity.subject, "101");
    for fault in [
        Fault::Signature,
        Fault::Issuer,
        Fault::Audience,
        Fault::Nonce,
        Fault::AtHash,
    ] {
        *f.fault.lock().unwrap() = fault;
        assert!(
            f.google().await.is_err(),
            "accepted invalid {}",
            match fault {
                Fault::Signature => "signature",
                Fault::Issuer => "issuer",
                Fault::Audience => "audience",
                Fault::Nonce => "nonce",
                Fault::AtHash => "at_hash",
                Fault::None => unreachable!(),
            }
        );
    }
}

#[tokio::test]
async fn equal_subjects_from_distinct_providers_remain_namespaced() {
    let f = Fixture::new().await;
    let google = f.google().await.unwrap();
    let github = providers::test_github_finish(
        &f.client,
        "client",
        "secret",
        "code",
        "https://identity.example/callback",
        "verifier",
        &format!("{}/github/token", f.base),
        &format!("{}/github/user", f.base),
    )
    .await
    .unwrap();
    assert_eq!(google.subject, github.subject);
    assert_ne!(google.provider, github.provider);
}

#[tokio::test]
async fn callback_rejects_missing_wrong_reused_cross_provider_cookie_and_expired_state() {
    use axum::{body::Body, http::Request};
    use p256::pkcs8::EncodePrivateKey;
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let key = SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
    let path = dir.path().join("key");
    std::fs::write(
        &path,
        key.to_pkcs8_pem(Default::default()).unwrap().as_bytes(),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let app = kiln_identity::router(
        kiln_identity::Config {
            public_origin: "https://identity.example.test".into(),
            github_client_id: "github".into(),
            github_secret_file: path.clone(),
            google_client_id: "google".into(),
            google_secret_file: path.clone(),
            signing_key_file: path,
            signing_key_id: "test".into(),
            retiring_jwks_file: None,
            trusted_proxy_loopback: false,
            now,
        },
        kiln_identity::Store::open_memory().unwrap(),
    )
    .unwrap();
    async fn begin(app: &axum::Router) -> (String, String) {
        let r = app
            .clone()
            .oneshot(
                Request::get("/oauth/github")
                    .extension(axum::extract::ConnectInfo(
                        "192.0.2.1:1234".parse::<std::net::SocketAddr>().unwrap(),
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let state = url::Url::parse(r.headers()["location"].to_str().unwrap())
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1
            .into_owned();
        let cookie = r
            .headers()
            .get_all("set-cookie")
            .iter()
            .find_map(|v| {
                let s = v.to_str().unwrap();
                s.strip_prefix("__Host-kiln_session=")
                    .map(|x| x.split(';').next().unwrap().to_owned())
            })
            .unwrap();
        (state, cookie)
    }
    async fn callback(
        app: &axum::Router,
        path: String,
        cookie: Option<&str>,
    ) -> axum::http::StatusCode {
        let mut r = Request::get(path);
        if let Some(c) = cookie {
            r = r.header("cookie", format!("__Host-kiln_session={c}"));
        }
        app.clone()
            .oneshot(r.body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }
    let (state, cookie) = begin(&app).await;
    assert_eq!(
        callback(&app, "/oauth/github/callback?code=x".into(), Some(&cookie)).await,
        axum::http::StatusCode::BAD_REQUEST
    );
    assert_eq!(
        callback(
            &app,
            "/oauth/github/callback?code=x&state=wrong".into(),
            Some(&cookie)
        )
        .await,
        axum::http::StatusCode::BAD_REQUEST
    );
    assert_eq!(
        callback(
            &app,
            format!("/oauth/github/callback?code=x&state={state}"),
            Some("changed")
        )
        .await,
        axum::http::StatusCode::BAD_REQUEST
    );
    assert_eq!(
        callback(
            &app,
            format!("/oauth/google/callback?code=x&state={state}"),
            Some(&cookie)
        )
        .await,
        axum::http::StatusCode::BAD_REQUEST
    );
    assert_eq!(
        callback(
            &app,
            format!("/oauth/github/callback?code=x&state={state}"),
            Some(&cookie)
        )
        .await,
        axum::http::StatusCode::BAD_REQUEST
    );
    let (expired, cookie) = begin(&app).await;
    CLOCK.store(1_600, Ordering::SeqCst);
    assert_eq!(
        callback(
            &app,
            format!("/oauth/github/callback?code=x&state={expired}"),
            Some(&cookie)
        )
        .await,
        axum::http::StatusCode::BAD_REQUEST
    );
}

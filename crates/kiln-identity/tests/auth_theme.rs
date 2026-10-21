use axum::{
    Router,
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use kiln_identity::{Config, Store};
use p256::pkcs8::EncodePrivateKey;
use std::net::SocketAddr;
use tower::ServiceExt;

fn fixture() -> Router {
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
        now: kiln_identity::unix_now,
    };
    kiln_identity::router(cfg, store).unwrap()
}

async fn get(app: &Router, path: &str) -> (StatusCode, Vec<u8>) {
    let response = app
        .clone()
        .oneshot(
            Request::get(path)
                .extension(ConnectInfo("192.0.2.1:1234".parse::<SocketAddr>().unwrap()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 262144).await.unwrap();
    (status, bytes.to_vec())
}

fn assert_console_tokens(css: &str) {
    for token in [
        "#0e0e0e",
        "#202020",
        "#78e772",
        "'Geist Mono'",
        "url(/static/fonts/geist-latin-400-normal.woff2)",
        "url(/static/fonts/geist-mono-latin-400-normal.woff2)",
    ] {
        assert!(css.contains(token), "auth.css missing {token}");
    }
    assert!(
        !css.contains("fonts.googleapis"),
        "auth.css must not load third-party fonts"
    );
}

#[tokio::test]
async fn login_and_device_pages_use_console_theme_assets() {
    let app = fixture();
    for path in ["/login", "/device"] {
        let (status, html) = get(&app, path).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        let html = String::from_utf8_lossy(&html);
        assert!(html.contains("/static/auth.css"), "{path} stylesheet");
        assert!(html.contains("theme-color"), "{path} theme-color");
        assert!(
            html.contains("Light the way in.") || html.contains("Enter your code."),
            "{path} copy"
        );
    }
    let (status, css) = get(&app, "/static/auth.css").await;
    assert_eq!(status, StatusCode::OK);
    assert_console_tokens(&String::from_utf8_lossy(&css));
    for font in [
        "/static/fonts/geist-latin-400-normal.woff2",
        "/static/fonts/geist-mono-latin-400-normal.woff2",
    ] {
        let (status, body) = get(&app, font).await;
        assert_eq!(status, StatusCode::OK, "{font}");
        assert!(body.len() > 1000, "{font} payload");
    }
}

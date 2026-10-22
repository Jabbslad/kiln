use std::{
    path::Path,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    body::{Body, Bytes, to_bytes},
    extract::ConnectInfo,
    http::{HeaderMap, Request, StatusCode},
    response::Response,
};
use futures_util::stream;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use kiln_api::{Action, Submit, auth::MemberRole, auth::Scope, auth::ServerClaims, new_id};
use kiln_server::{
    app_listener::{self, BIND},
    auth::{Authenticator, Enrollment},
    door,
    host::{Config, Host, bind_socket},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

const CSP: &str = "default-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'";
const HTML: &str = "<!doctype html><html><body>kiln</body></html>";

struct Upstream {
    port: u16,
    hits: Arc<AtomicUsize>,
    ca_pem: String,
}

fn archive(root: &Path) {
    std::fs::create_dir_all(root.join("assets")).unwrap();
    std::fs::write(root.join("index.html"), HTML).unwrap();
    std::fs::write(root.join("assets/app.js"), b"console.log(1)").unwrap();
}

fn pem() -> String {
    rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()])
        .unwrap()
        .cert
        .pem()
}

fn auth(dir: &Path) -> (Authenticator, EncodingKey) {
    use base64::Engine;
    use p256::{elliptic_curve::sec1::ToEncodedPoint, pkcs8::EncodePrivateKey};
    let key = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
    let encoded = key.to_pkcs8_pem(Default::default()).unwrap();
    let point = key.public_key().to_encoded_point(false);
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let jwks = serde_json::json!({"keys":[{
        "kty":"EC","crv":"P-256","use":"sig","alg":"ES256","kid":"first",
        "x":b64.encode(point.x().unwrap()),"y":b64.encode(point.y().unwrap())
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
        EncodingKey::from_ec_pem(encoded.as_bytes()).unwrap(),
    )
}

fn token(key: &EncodingKey, sub: &str, scope: Scope) -> String {
    token_with(key, sub, scope, "device-1")
}

fn tenant_token(key: &EncodingKey, sub: &str, scope: Scope, tenant_id: &str, role: MemberRole) -> String {
    let now = jsonwebtoken::get_current_timestamp();
    let claims = ServerClaims {
        iss: "https://identity.example.test".into(),
        sub: sub.into(),
        aud: "kiln:server:server-b".into(),
        iat: now,
        nbf: now,
        exp: now + 300,
        credential_id: "device-member".into(),
        scope,
        tenant_id: Some(tenant_id.into()),
        role: Some(role),
    };
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("first".into());
    jsonwebtoken::encode(&header, &claims, key).unwrap()
}

fn token_with(key: &EncodingKey, sub: &str, scope: Scope, credential_id: &str) -> String {
    let now = jsonwebtoken::get_current_timestamp();
    let role = (sub == "owner-a").then_some(MemberRole::Owner);
    let claims = ServerClaims {
        iss: "https://identity.example.test".into(),
        sub: sub.into(),
        aud: "kiln:server:server-b".into(),
        tenant_id: Some("tenant-a".into()),
        iat: now,
        nbf: now,
        exp: now + 300,
        credential_id: credential_id.into(),
        scope,
        role,
    };
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("first".into());
    jsonwebtoken::encode(&header, &claims, key).unwrap()
}

async fn listen_tls(app: axum::Router) -> (u16, String) {
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
    (port, ca_pem)
}

async fn upstream_json() -> Upstream {
    let hits = Arc::new(AtomicUsize::new(0));
    let seen_dashboard = hits.clone();
    let seen_other = hits.clone();
    let app = axum::Router::new()
        .route(
            "/api/v1/dashboard",
            axum::routing::get(move || {
                let seen_dashboard = seen_dashboard.clone();
                async move {
                    seen_dashboard.fetch_add(1, Ordering::SeqCst);
                    axum::Json(serde_json::json!({"schema_version": 1}))
                }
            }),
        )
        .fallback(axum::routing::any(move || {
            let seen_other = seen_other.clone();
            async move {
                seen_other.fetch_add(1, Ordering::SeqCst);
                StatusCode::OK
            }
        }));
    let (port, ca_pem) = listen_tls(app).await;
    Upstream { port, hits, ca_pem }
}

async fn upstream_huge() -> Upstream {
    let hits = Arc::new(AtomicUsize::new(0));
    let seen = hits.clone();
    let app = axum::Router::new().route(
        "/api/v1/dashboard",
        axum::routing::get(move || {
            let seen = seen.clone();
            async move {
                seen.fetch_add(1, Ordering::SeqCst);
                let chunks = vec![
                    Ok::<Bytes, std::io::Error>(Bytes::from(vec![b'y'; 1024])),
                    Ok(Bytes::from(vec![b'y'; kiln_api::MAX_RESPONSE_BYTES])),
                ];
                Body::from_stream(stream::iter(chunks))
            }
        }),
    );
    let (port, ca_pem) = listen_tls(app).await;
    Upstream { port, hits, ca_pem }
}

fn open(root: &Path, up: &Upstream) -> (axum::Router, EncodingKey) {
    let (auth, key) = auth(root);
    let router = app_listener::router(
        auth,
        &format!("https://127.0.0.1:{}", up.port),
        up.ca_pem.as_bytes(),
        Some(root),
    )
    .unwrap();
    (router, key)
}

async fn send(router: &axum::Router, peer: &str, request: Request<Body>) -> Response {
    let (mut parts, body) = request.into_parts();
    parts
        .extensions
        .insert(ConnectInfo(peer.parse::<std::net::SocketAddr>().unwrap()));
    router
        .clone()
        .oneshot(Request::from_parts(parts, body))
        .await
        .unwrap()
}

fn get(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

fn bearer(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

async fn text(response: Response) -> (StatusCode, HeaderMap, String) {
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 16 * 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

fn assert_security(headers: &HeaderMap) {
    assert_eq!(headers.get("content-security-policy").unwrap(), CSP);
    assert_eq!(headers.get("referrer-policy").unwrap(), "strict-origin");
    assert_eq!(headers.get("x-content-type-options").unwrap(), "nosniff");
}

#[test]
fn bind_refuses_other_addresses() {
    let root = require_bind_message(BIND, 0);
    assert!(root.contains("unprivileged"), "{root}");
    for listen in [
        "0.0.0.0:8093",
        "172.18.0.1:8093",
        "127.0.0.1:8094",
        "[::1]:8093",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_kiln-api"))
            .args([
                "app",
                "--listen",
                listen,
                "--upstream-ca",
                "/no/such/ca.crt",
            ])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{listen}");
        let err = String::from_utf8_lossy(&output.stderr);
        assert!(
            err.contains("app listener must bind 127.0.0.1:8093"),
            "{listen}: {err}"
        );
        assert!(!err.to_lowercase().contains("listening"), "{listen}: {err}");
    }
}

fn require_bind_message(listen: std::net::SocketAddr, euid: u32) -> String {
    app_listener::require_bind(listen, euid)
        .unwrap_err()
        .to_string()
}

#[test]
fn upstream_must_stay_on_loopback_https() {
    let root = tempfile::tempdir().unwrap();
    let ca = pem();
    for upstream in [
        "http://127.0.0.1:8443",
        "https://172.18.0.1:8443",
        "https://0.0.0.0:8443",
        "https://127.0.0.1:8443/v1",
        "https://example.com:8443",
    ] {
        let (auth, _) = auth(root.path());
        let result = app_listener::router(auth, upstream, ca.as_bytes(), None);
        assert!(result.is_err(), "{upstream}");
    }
}

#[test]
fn shipped_units_bind_loopback_and_the_bridge() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let app = std::fs::read_to_string(root.join("deploy/kiln-app.service")).unwrap();
    assert!(app.contains("127.0.0.1:8093"));
    assert!(app.contains("NoNewPrivileges=yes"));
    assert!(app.contains("ProtectSystem=strict"));
    assert!(app.contains("StateDirectory=kiln-api\n"));
    assert!(app.contains("StateDirectoryMode=0700\n"));
    assert!(!app.contains("ReadWritePaths="));
    assert!(!app.contains("0.0.0.0"));
    assert!(!app.contains("172.18.0.1"));
    assert!(!app.contains("--tls-cert"));
    let socket = std::fs::read_to_string(root.join("deploy/kiln-app-proxy.socket")).unwrap();
    assert!(socket.contains("ListenStream=172.18.0.1:8093"));
    assert!(socket.contains("FreeBind=yes"));
    assert!(socket.contains("IPAddressAllow=172.18.0.3/32"));
    assert!(!socket.contains("0.0.0.0"));
    let proxy = std::fs::read_to_string(root.join("deploy/kiln-app-proxy.service")).unwrap();
    assert!(proxy.contains("127.0.0.1:8093"));
    assert!(proxy.contains("IPAddressAllow=localhost"));
    let door_unit = std::fs::read_to_string(root.join("deploy/kiln-door.service")).unwrap();
    assert!(!door_unit.contains("dashboard-root"), "{door_unit}");
    assert!(!door_unit.contains("session-dir"), "{door_unit}");
}

#[tokio::test]
async fn missing_archive_serves_no_html() {
    let root = tempfile::tempdir().unwrap();
    let up = upstream_json().await;
    let (auth, _) = auth(root.path());
    let missing = root.path().join("absent");
    let router = app_listener::router(
        auth,
        &format!("https://127.0.0.1:{}", up.port),
        up.ca_pem.as_bytes(),
        Some(&missing),
    )
    .unwrap();
    let (status, _, body) = text(send(&router, "127.0.0.1:9", get("/boxes")).await).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("\"code\":\"not_found\"") || body.contains("\"code\": \"not_found\""));
    assert!(!body.contains("<html"));
    let (status, _, body) =
        text(send(&router, "127.0.0.1:9", get("/api/v1/dashboard")).await).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["code"], "unauthorized");
    assert_eq!(up.hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn deep_link_returns_index_html() {
    let root = tempfile::tempdir().unwrap();
    archive(root.path());
    std::os::unix::fs::symlink(
        root.path().join("index.html"),
        root.path().join("link.html"),
    )
    .unwrap();
    let up = upstream_json().await;
    let (router, _) = open(root.path(), &up);
    let (status, headers, body) = text(send(&router, "127.0.0.1:9", get("/boxes")).await).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    assert_eq!(body, HTML);
    let (status, _, body) = text(send(&router, "127.0.0.1:9", get("/link.html")).await).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!body.contains("kiln"));
    let (status, _, body) = text(send(&router, "127.0.0.1:9", get("/missing.js")).await).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!body.contains("<html"));
}

#[tokio::test]
async fn v1_paths_return_json_not_the_dashboard() {
    let root = tempfile::tempdir().unwrap();
    archive(root.path());
    let up = upstream_json().await;
    let (router, _) = open(root.path(), &up);
    let (status, headers, body) =
        text(send(&router, "127.0.0.1:9", get("/v1/templates")).await).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        headers
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("application/json"),
        "{headers:?}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["code"], "not_found");
    assert!(!body.contains("<html"));
    assert_ne!(body, HTML);
    assert_eq!(up.hits.load(Ordering::SeqCst), 0);
    let (status, headers, body) = text(send(&router, "127.0.0.1:9", get("/boxes")).await).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    assert_eq!(body, HTML);
}

#[tokio::test]
async fn archive_escapes_stay_inside_the_root() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("archive");
    archive(&root);
    let marker = "outside-marker-not-in-archive";
    std::fs::write(parent.path().join("secret.txt"), marker).unwrap();
    std::os::unix::fs::symlink(
        parent.path().join("secret.txt"),
        root.join("link-to-secret.txt"),
    )
    .unwrap();
    std::os::unix::fs::symlink(parent.path(), root.join("linkdir")).unwrap();
    let up = upstream_json().await;
    let (router, _) = open(&root, &up);
    for uri in ["/../secret.txt", "/%2e%2e/secret.txt"] {
        let (status, _, body) = text(send(&router, "127.0.0.1:9", get(uri)).await).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {body}");
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["code"], "invalid_request", "{uri}");
        assert!(!body.contains(marker), "{uri}");
    }
    for uri in ["/link-to-secret.txt", "/linkdir/secret.txt"] {
        let (status, headers, body) = text(send(&router, "127.0.0.1:9", get(uri)).await).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
        assert!(
            headers
                .get("content-type")
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("application/json"),
            "{uri}: {headers:?}"
        );
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["code"], "not_found", "{uri}");
        assert!(!body.contains(marker), "{uri}");
        assert!(!body.contains("<html"), "{uri}");
    }
}

#[tokio::test]
async fn missing_api_path_is_json() {
    let root = tempfile::tempdir().unwrap();
    archive(root.path());
    let up = upstream_json().await;
    let (router, _) = open(root.path(), &up);
    let (status, _, body) = text(send(&router, "127.0.0.1:9", get("/api/v1/missing")).await).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["code"], "not_found");
    assert!(!body.contains("<html"));
    assert_eq!(up.hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn browser_authorization_is_ignored_and_the_session_cookie_is_proxied() {
    let root = tempfile::tempdir().unwrap();
    archive(root.path());
    let up = upstream_json().await;
    let (auth, key) = auth(root.path());
    let read = token(&key, "owner-a", Scope::Read);
    let (router, cookie) = app_listener::router_with_read_session(
        auth,
        &format!("https://127.0.0.1:{}", up.port),
        up.ca_pem.as_bytes(),
        Some(root.path()),
        &read,
    )
    .unwrap();
    for scope in [Scope::Read, Scope::Operate] {
        let (status, _, body) = text(
            send(
                &router,
                "127.0.0.1:9",
                bearer("/api/v1/dashboard", &token(&key, "owner-a", scope)),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["code"], "unauthorized");
    }
    assert_eq!(up.hits.load(Ordering::SeqCst), 0);
    let request = Request::builder()
        .uri("/api/v1/dashboard")
        .header("cookie", format!("__Host-kiln_app={cookie}"))
        .header("authorization", "Bearer from-the-browser")
        .body(Body::empty())
        .unwrap();
    let (status, headers, body) = text(send(&router, "127.0.0.1:9", request).await).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get("cache-control").unwrap(), "no-store");
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["schema_version"], 1);
    assert_eq!(up.hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn administrator_token_is_not_forwarded() {
    let root = tempfile::tempdir().unwrap();
    let up = upstream_json().await;
    let (router, _) = open(root.path(), &up);
    let (status, _, body) = text(
        send(
            &router,
            "127.0.0.1:9",
            bearer("/api/v1/dashboard", &"ab".repeat(32)),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["code"], "unauthorized");
    assert_eq!(up.hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn response_headers_and_cache() {
    let root = tempfile::tempdir().unwrap();
    archive(root.path());
    let up = upstream_json().await;
    let (router, key) = open(root.path(), &up);
    let (status, headers, _) = text(send(&router, "127.0.0.1:9", get("/")).await).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get("cache-control").unwrap(), "no-store");
    assert_security(&headers);
    let (status, headers, body) =
        text(send(&router, "127.0.0.1:9", get("/assets/app.js")).await).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "console.log(1)");
    let cache = headers.get("cache-control").unwrap().to_str().unwrap();
    assert!(cache.contains("immutable"), "{cache}");
    assert!(!cache.contains("no-store"), "{cache}");
    assert_security(&headers);
    let (_, headers, _) = text(
        send(
            &router,
            "127.0.0.1:9",
            bearer("/api/v1/dashboard", &token(&key, "owner-a", Scope::Read)),
        )
        .await,
    )
    .await;
    assert_eq!(headers.get("cache-control").unwrap(), "no-store");
    assert_security(&headers);
    let (_, headers, _) = text(send(&router, "127.0.0.1:9", get("/api/v1/missing")).await).await;
    assert_eq!(headers.get("cache-control").unwrap(), "no-store");
    assert_security(&headers);
}

#[tokio::test]
async fn preview_query_is_limited_to_one() {
    let root = tempfile::tempdir().unwrap();
    archive(root.path());
    let up = upstream_json().await;
    let (router, key) = open(root.path(), &up);
    let (status, headers, body) =
        text(send(&router, "127.0.0.1:9", get("/boxes?preview=1")).await).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    assert_eq!(body, HTML);
    for uri in ["/boxes?preview=2", "/boxes?x=1", "/boxes?preview=1&x=1"] {
        let (status, _, _) = text(send(&router, "127.0.0.1:9", get(uri)).await).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
    }
    let (status, _, _) = text(
        send(
            &router,
            "127.0.0.1:9",
            bearer(
                "/api/v1/dashboard?preview=1",
                &token(&key, "owner-a", Scope::Read),
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(up.hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn mutations_are_not_forwarded() {
    let root = tempfile::tempdir().unwrap();
    archive(root.path());
    let up = upstream_json().await;
    let (router, key) = open(root.path(), &up);
    let request = Request::builder()
        .method("POST")
        .uri("/v1/operations")
        .header(
            "authorization",
            format!("Bearer {}", token(&key, "owner-a", Scope::Operate)),
        )
        .body(Body::from("{\"name\":\"box\"}"))
        .unwrap();
    let (status, _, body) = text(send(&router, "127.0.0.1:9", request).await).await;
    assert_ne!(status, StatusCode::OK);
    assert!(!body.contains("<html"));
    assert_eq!(up.hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn door_still_rejects_an_unauthenticated_templates_read() {
    let root = tempfile::tempdir().unwrap();
    let (auth, _) = auth(root.path());
    let router = door::router(auth, "https://127.0.0.1:1", pem().as_bytes()).unwrap();
    let response = router
        .oneshot(
            Request::builder()
                .uri("/v1/templates")
                .header("cookie", "__Host-kiln_app=dashboard-session")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn static_file_over_eight_mib_is_refused() {
    let root = tempfile::tempdir().unwrap();
    archive(root.path());
    let limit = 8 * 1024 * 1024;
    std::fs::write(root.path().join("assets/exact.bin"), vec![b'z'; limit]).unwrap();
    std::fs::write(root.path().join("assets/over.bin"), vec![b'z'; limit + 1]).unwrap();
    let up = upstream_json().await;
    let (router, _) = open(root.path(), &up);
    let (status, _, body) =
        text(send(&router, "127.0.0.1:9", get("/assets/exact.bin")).await).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.len(), limit);
    let (status, _, body) = text(send(&router, "127.0.0.1:9", get("/assets/over.bin")).await).await;
    assert_ne!(status, StatusCode::OK);
    assert!(body.len() < 1024, "{}", body.len());
}

#[tokio::test]
async fn dashboard_body_over_one_mib_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let up = upstream_huge().await;
    let (auth, key) = auth(root.path());
    let (router, cookie) = app_listener::router_with_read_session(
        auth,
        &format!("https://127.0.0.1:{}", up.port),
        up.ca_pem.as_bytes(),
        Some(root.path()),
        &token(&key, "owner-a", Scope::Read),
    )
    .unwrap();
    let request = Request::builder()
        .uri("/api/v1/dashboard")
        .header("cookie", format!("__Host-kiln_app={cookie}"))
        .body(Body::empty())
        .unwrap();
    let (status, _, body) = text(send(&router, "127.0.0.1:9", request).await).await;
    assert_ne!(status, StatusCode::OK);
    assert!(!body.contains(&"y".repeat(64)));
    assert!(body.len() < 1024, "{}", body.len());
}

#[tokio::test]
async fn failed_dashboard_upstream_refresh_is_audited() {
    let root = tempfile::tempdir().unwrap();
    archive(root.path());
    let up = upstream_json().await;
    let (auth, key) = auth(root.path());
    let closed = up.port.saturating_add(50_000);
    let session = app_listener::router_with_operate_session(
        auth,
        &format!("https://127.0.0.1:{closed}"),
        up.ca_pem.as_bytes(),
        Some(root.path()),
        &token(&key, "owner-a", Scope::Read),
    )
    .unwrap();
    let request = Request::builder()
        .uri("/api/v1/dashboard")
        .header("cookie", format!("__Host-kiln_app={}", session.session))
        .body(Body::empty())
        .unwrap();
    let (status, _, body) = text(send(&session.router, "127.0.0.1:9", request).await).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert!(
        session
            .audit
            .records()
            .iter()
            .any(|record| record.outcome == "dashboard_unreachable"),
        "{:?}",
        session.audit.records()
    );
}

#[tokio::test]
async fn rate_limit_is_checked_before_the_upstream_dial() {
    let root = tempfile::tempdir().unwrap();
    let up = upstream_json().await;
    let (auth, key) = auth(root.path());
    let (router, cookie) = app_listener::router_with_read_session(
        auth,
        &format!("https://127.0.0.1:{}", up.port),
        up.ca_pem.as_bytes(),
        Some(root.path()),
        &token(&key, "owner-a", Scope::Read),
    )
    .unwrap();
    for _ in 0..60 {
        let request = Request::builder()
            .uri("/api/v1/dashboard")
            .header("cookie", format!("__Host-kiln_app={cookie}"))
            .body(Body::empty())
            .unwrap();
        let (status, _, _) = text(send(&router, "127.0.0.1:9", request).await).await;
        assert_eq!(status, StatusCode::OK);
    }
    assert_eq!(up.hits.load(Ordering::SeqCst), 60);
    let request = Request::builder()
        .uri("/api/v1/dashboard")
        .header("cookie", format!("__Host-kiln_app={cookie}"))
        .body(Body::empty())
        .unwrap();
    let (status, _, _) = text(send(&router, "127.0.0.1:9", request).await).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(up.hits.load(Ordering::SeqCst), 60);
}

#[tokio::test]
async fn forwarded_for_is_trusted_only_from_loopback() {
    let root = tempfile::tempdir().unwrap();
    let up = upstream_json().await;
    let (router, _) = open(root.path(), &up);
    for _ in 0..60 {
        let request = Request::builder()
            .uri("/api/v1/dashboard")
            .header("x-forwarded-for", "203.0.113.8")
            .body(Body::empty())
            .unwrap();
        let (status, _, _) = text(send(&router, "10.1.2.3:9", request).await).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let request = Request::builder()
        .uri("/api/v1/dashboard")
        .header("x-forwarded-for", "198.51.100.9")
        .body(Body::empty())
        .unwrap();
    let (status, _, _) = text(send(&router, "10.1.2.3:9", request).await).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    for _ in 0..60 {
        let request = Request::builder()
            .uri("/api/v1/dashboard")
            .header("x-forwarded-for", "203.0.113.8, 10.0.0.1")
            .body(Body::empty())
            .unwrap();
        let (status, _, _) = text(send(&router, "127.0.0.1:9", request).await).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let request = Request::builder()
        .uri("/api/v1/dashboard")
        .header("x-forwarded-for", "203.0.113.8")
        .body(Body::empty())
        .unwrap();
    let (status, _, _) = text(send(&router, "127.0.0.1:9", request).await).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    let request = Request::builder()
        .uri("/api/v1/dashboard")
        .header("x-forwarded-for", "198.51.100.4")
        .body(Body::empty())
        .unwrap();
    let (status, _, _) = text(send(&router, "127.0.0.1:9", request).await).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(up.hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn listener_accepts_on_the_loopback_port() {
    assert_eq!(
        app_listener::listen_line(BIND),
        "app listener listening on 127.0.0.1:8093"
    );
    let root = tempfile::tempdir().unwrap();
    archive(root.path());
    let ca = pem();
    let (auth, _) = auth(root.path());
    let router = app_listener::router(
        auth,
        "https://127.0.0.1:1",
        ca.as_bytes(),
        Some(root.path()),
    )
    .unwrap();
    let server = tokio::spawn(async move {
        app_listener::listen(router, BIND).await.unwrap();
    });
    let mut stream = None;
    let mut last = String::new();
    for _ in 0..50 {
        match tokio::net::TcpStream::connect("127.0.0.1:8093").await {
            Ok(connected) => {
                stream = Some(connected);
                break;
            }
            Err(err) => last = err.to_string(),
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let Some(mut stream) = stream else {
        server.abort();
        panic!("127.0.0.1:8093 did not accept: {last}");
    };
    stream
        .write_all(b"GET /boxes HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n")
        .await
        .unwrap();
    stream.flush().await.unwrap();
    let mut raw_bytes = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut raw_bytes)).await;
    server.abort();
    let raw = String::from_utf8_lossy(&raw_bytes);
    let lower = raw.to_ascii_lowercase();
    assert!(
        lower.contains("http/1.0 200") || lower.contains("http/1.1 200"),
        "{raw}"
    );
    assert!(lower.contains("text/html"), "{raw}");
    assert!(lower.contains("nosniff"), "{raw}");
    assert!(lower.contains("strict-origin"), "{raw}");
    assert!(raw.contains(HTML), "{raw}");
}

fn assert_host_cookie(cookie: &str, name: &str, max_age: u64, http_only: bool) {
    assert!(cookie.starts_with(&format!("{name}=")), "{cookie}");
    assert!(cookie.contains("; Secure; "), "{cookie}");
    assert!(cookie.contains("SameSite=Lax"), "{cookie}");
    assert!(cookie.contains("Path=/"), "{cookie}");
    assert!(cookie.contains(&format!("Max-Age={max_age}")), "{cookie}");
    assert!(!cookie.to_ascii_lowercase().contains("domain"), "{cookie}");
    assert_eq!(cookie.contains("HttpOnly"), http_only, "{cookie}");
}

fn cookie_value(header: &str) -> String {
    header
        .split(';')
        .next()
        .unwrap()
        .split_once('=')
        .unwrap()
        .1
        .to_owned()
}

fn named_cookie<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
    headers
        .get_all("set-cookie")
        .iter()
        .find_map(|value| {
            let text = value.to_str().unwrap();
            text.starts_with(&format!("{name}=")).then_some(text)
        })
        .unwrap_or_else(|| panic!("missing {name}"))
}

struct Loopback {
    port: u16,
    token_hits: Arc<AtomicUsize>,
    status_hits: Arc<AtomicUsize>,
    revoke_hits: Arc<AtomicUsize>,
    bodies: Arc<std::sync::Mutex<Vec<String>>>,
    authorizations: Arc<std::sync::Mutex<Vec<String>>>,
}

async fn loopback(jwt: String, status_mode: u8) -> Loopback {
    scripted_loopback(jwt.clone(), jwt, 300, status_mode).await
}

async fn scripted_loopback(
    login_jwt: String,
    refresh_jwt: String,
    expires_in: u64,
    status_mode: u8,
) -> Loopback {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let token_hits = Arc::new(AtomicUsize::new(0));
    let status_hits = Arc::new(AtomicUsize::new(0));
    let revoke_hits = Arc::new(AtomicUsize::new(0));
    let bodies = Arc::new(std::sync::Mutex::new(Vec::new()));
    let authorizations = Arc::new(std::sync::Mutex::new(Vec::new()));
    let tokens = token_hits.clone();
    let statuses = status_hits.clone();
    let revokes = revoke_hits.clone();
    let seen_bodies = bodies.clone();
    let seen_auth = authorizations.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let login_jwt = login_jwt.clone();
            let refresh_jwt = refresh_jwt.clone();
            let tokens = tokens.clone();
            let statuses = statuses.clone();
            let revokes = revokes.clone();
            let seen_bodies = seen_bodies.clone();
            let seen_auth = seen_auth.clone();
            tokio::spawn(async move {
                let raw = read_http(&mut socket).await;
                let body = raw
                    .split_once("\r\n\r\n")
                    .map(|(_, body)| body.to_owned())
                    .unwrap_or_default();
                let authorization = header_value(&raw, "authorization");
                let line = raw.lines().next().unwrap_or("");
                if line.contains("POST /oauth/server/token ") {
                    tokens.fetch_add(1, Ordering::SeqCst);
                    let issued = if body.contains("\"reader\"") {
                        refresh_jwt
                    } else {
                        login_jwt
                    };
                    seen_bodies.lock().unwrap().push(body);
                    seen_auth.lock().unwrap().push(authorization);
                    let payload = serde_json::json!({
                        "access_token": issued,
                        "token_type": "Bearer",
                        "expires_in": expires_in,
                        "reader": "fixture-reader",
                    });
                    write_http(&mut socket, "200 OK", &payload.to_string()).await;
                } else if line.contains("POST /oauth/server/revoke ") {
                    revokes.fetch_add(1, Ordering::SeqCst);
                    seen_auth.lock().unwrap().push(authorization);
                    write_http(&mut socket, "204 No Content", "").await;
                } else if line.contains("POST /v1/server-status ") {
                    statuses.fetch_add(1, Ordering::SeqCst);
                    if status_mode == 2 {
                        return;
                    }
                    let account_active = status_mode != 1;
                    let payload = serde_json::json!({
                        "account_active": account_active,
                        "server_active": true,
                    });
                    write_http(&mut socket, "200 OK", &payload.to_string()).await;
                } else {
                    write_http(&mut socket, "404 Not Found", "{}").await;
                }
            });
        }
    });
    Loopback {
        port,
        token_hits,
        status_hits,
        revoke_hits,
        bodies,
        authorizations,
    }
}

async fn read_http(socket: &mut tokio::net::TcpStream) -> String {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        let read = tokio::time::timeout(Duration::from_secs(2), socket.read(&mut tmp)).await;
        let Ok(Ok(n)) = read else { break };
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(end) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&buf[..end]);
            if buf.len() >= end + 4 + content_length(&headers) {
                break;
            }
        }
        if buf.len() > 65_536 {
            break;
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn content_length(headers: &str) -> usize {
    headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())?
        })
        .unwrap_or(0)
}

fn header_value(raw: &str, name: &str) -> String {
    raw.lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name)
                .then(|| value.trim().to_owned())
        })
        .unwrap_or_default()
}

async fn write_http(socket: &mut tokio::net::TcpStream, status: &str, body: &str) {
    let message = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = socket.write_all(message.as_bytes()).await;
}

fn router_for(
    auth: Authenticator,
    up: &Upstream,
    root: &Path,
    secret: &str,
    port: u16,
) -> axum::Router {
    app_listener::router_with(
        auth,
        &format!("https://127.0.0.1:{}", up.port),
        up.ca_pem.as_bytes(),
        Some(root),
        app_listener::SignIn {
            authorize_origin: "https://dark-forge.dev".into(),
            loopback: format!("http://127.0.0.1:{port}"),
            login_secret: secret.into(),
            server_id: "server-b".into(),
        },
    )
    .unwrap()
}

async fn browser_login(router: &axum::Router, peer: &str) -> (String, String) {
    let (status, headers, body) = text(send(router, peer, get("/login")).await).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    let attempt = named_cookie(&headers, "__Host-kiln_app_attempt");
    assert_host_cookie(attempt, "__Host-kiln_app_attempt", 600, true);
    let location = reqwest::Url::parse(headers.get("location").unwrap().to_str().unwrap()).unwrap();
    assert_eq!(location.host_str(), Some("dark-forge.dev"));
    assert_eq!(location.path(), "/oauth/server");
    let pairs: std::collections::HashMap<String, String> =
        location.query_pairs().into_owned().collect();
    assert_eq!(pairs["redirect_uri"], kiln_api::auth::APP_OAUTH_REDIRECT);
    assert_eq!(pairs["server_id"], "server-b");
    assert_eq!(pairs["code_challenge_method"], "S256");
    assert_eq!(pairs["code_challenge"].len(), 43);
    let attempt_id = cookie_value(attempt);
    let request = Request::builder()
        .uri(format!(
            "/oauth/callback?code=fixture-code&state={}",
            pairs["state"]
        ))
        .header("cookie", format!("__Host-kiln_app_attempt={attempt_id}"))
        .body(Body::empty())
        .unwrap();
    let (status, headers, body) = text(send(router, peer, request).await).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert_eq!(headers.get("location").unwrap(), "/boxes");
    assert_host_cookie(
        named_cookie(&headers, "__Host-kiln_app"),
        "__Host-kiln_app",
        43_200,
        true,
    );
    assert_host_cookie(
        named_cookie(&headers, "__Host-kiln_csrf"),
        "__Host-kiln_csrf",
        43_200,
        false,
    );
    assert_host_cookie(
        named_cookie(&headers, "__Host-kiln_app_attempt"),
        "__Host-kiln_app_attempt",
        0,
        true,
    );
    (
        cookie_value(named_cookie(&headers, "__Host-kiln_app")),
        cookie_value(named_cookie(&headers, "__Host-kiln_csrf")),
    )
}

#[test]
fn identity_exchange_must_dial_loopback() {
    let root = tempfile::tempdir().unwrap();
    let (auth, _) = auth(root.path());
    let rejected = app_listener::router_with(
        auth,
        "https://127.0.0.1:9",
        pem().as_bytes(),
        None,
        app_listener::SignIn {
            authorize_origin: "https://dark-forge.dev".into(),
            loopback: "https://dark-forge.dev".into(),
            login_secret: "fixture-login".into(),
            server_id: "server-b".into(),
        },
    );
    assert!(rejected.is_err());
}

#[tokio::test]
async fn read_grant_is_checked_and_another_server_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    let (auth, key) = auth(root.path());
    assert!(matches!(
        auth.authenticate_central(&token(&key, "owner-a", Scope::Read))
            .await,
        Ok(Scope::Read)
    ));
    assert!(
        auth.authenticate_central(&token(&key, "someone-else", Scope::Read))
            .await
            .is_err()
    );
    let now = jsonwebtoken::get_current_timestamp();
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("first".into());
    let other = jsonwebtoken::encode(
        &header,
        &ServerClaims {
            iss: "https://identity.example.test".into(),
            sub: "owner-a".into(),
            aud: "kiln:server:other".into(),
            tenant_id: Some("ten_other".into()),
            iat: now,
            nbf: now,
            exp: now + 300,
            credential_id: "app_other".into(),
            scope: Scope::Read,
            role: Some(MemberRole::Owner),
        },
        &key,
    )
    .unwrap();
    assert!(auth.authenticate_central(&other).await.is_err());
}

#[tokio::test]
async fn missing_login_secret_refuses_sign_in_without_stopping_the_listener() {
    let root = tempfile::tempdir().unwrap();
    archive(root.path());
    let up = upstream_json().await;
    let (router, _) = open(root.path(), &up);
    for path in ["/login", "/oauth/callback?code=one&state=two"] {
        let (status, _, body) = text(send(&router, "127.0.0.1:9", get(path)).await).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{path} {body}");
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["code"], "login_unavailable");
        assert!(!body.contains("<html"));
    }
    let (status, _, body) = text(send(&router, "127.0.0.1:9", get("/boxes")).await).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, HTML);
}

#[tokio::test]
async fn browser_sign_in_sets_one_session_and_logout_clears_only_that_session() {
    let root = tempfile::tempdir().unwrap();
    archive(root.path());
    let up = upstream_json().await;
    let (auth, key) = auth(root.path());
    let identity = loopback(token(&key, "owner-a", Scope::Read), 0).await;
    let router = app_listener::router_with(
        auth,
        &format!("https://127.0.0.1:{}", up.port),
        up.ca_pem.as_bytes(),
        Some(root.path()),
        app_listener::SignIn {
            authorize_origin: "https://dark-forge.dev".into(),
            loopback: format!("http://127.0.0.1:{}", identity.port),
            login_secret: "fixture-login".into(),
            server_id: "server-b".into(),
        },
    )
    .unwrap();
    let (status, headers, _) = text(send(&router, "127.0.0.1:9", get("/login")).await).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let attempt = cookie_value(named_cookie(&headers, "__Host-kiln_app_attempt"));
    let state = reqwest::Url::parse(headers.get("location").unwrap().to_str().unwrap())
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == "state")
        .unwrap()
        .1
        .into_owned();
    let mismatch = Request::builder()
        .uri("/oauth/callback?code=fixture-code&state=not-this-browser")
        .header("cookie", format!("__Host-kiln_app_attempt={attempt}"))
        .body(Body::empty())
        .unwrap();
    let (status, _, body) = text(send(&router, "127.0.0.1:9", mismatch).await).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["code"], "invalid_state");
    let retry = Request::builder()
        .uri(format!("/oauth/callback?code=fixture-code&state={state}"))
        .header("cookie", format!("__Host-kiln_app_attempt={attempt}"))
        .body(Body::empty())
        .unwrap();
    let (status, _, _) = text(send(&router, "127.0.0.1:9", retry).await).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (first, first_csrf) = browser_login(&router, "127.0.0.1:9").await;
    let (second, _) = browser_login(&router, "127.0.0.1:9").await;
    assert_ne!(first, second);
    assert_eq!(identity.token_hits.load(Ordering::SeqCst), 2);
    let recorded = identity.bodies.lock().unwrap().clone();
    assert_eq!(recorded.len(), 2, "{recorded:?}");
    for body in &recorded {
        let value: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(value["redirect_uri"], kiln_api::auth::APP_OAUTH_REDIRECT);
        assert_eq!(value["code_verifier"].as_str().unwrap().len(), 43);
    }
    assert!(
        identity
            .authorizations
            .lock()
            .unwrap()
            .iter()
            .all(|value| value == "Bearer fixture-login")
    );
    let dashboard = |session: &str| {
        Request::builder()
            .uri("/api/v1/dashboard")
            .header("cookie", format!("__Host-kiln_app={session}"))
            .header("authorization", "Bearer from-the-browser")
            .body(Body::empty())
            .unwrap()
    };
    let (status, _, body) = text(send(&router, "127.0.0.1:9", dashboard(&first)).await).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap()["schema_version"],
        1
    );
    let (status, _, body) = text(send(&router, "127.0.0.1:9", dashboard(&first)).await).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(identity.status_hits.load(Ordering::SeqCst), 1);
    assert_eq!(up.hits.load(Ordering::SeqCst), 2);

    let get_logout = Request::builder()
        .method("GET")
        .uri("/logout")
        .header("cookie", format!("__Host-kiln_app={first}"))
        .body(Body::empty())
        .unwrap();
    let (status, headers, body) = text(send(&router, "127.0.0.1:9", get_logout).await).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, HTML);
    assert!(headers.get("set-cookie").is_none());
    let (status, _, _) = text(send(&router, "127.0.0.1:9", dashboard(&first)).await).await;
    assert_eq!(status, StatusCode::OK);

    let logout = |session: &str, csrf: &str, origin: &str, posted: &str| {
        Request::builder()
            .method("POST")
            .uri("/logout")
            .header("origin", origin)
            .header(
                "cookie",
                format!("__Host-kiln_app={session}; __Host-kiln_csrf={csrf}"),
            )
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(format!("csrf={posted}")))
            .unwrap()
    };
    let (status, _, body) = text(
        send(
            &router,
            "127.0.0.1:9",
            logout(&first, &first_csrf, "https://evil.example", &first_csrf),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, _, body) = text(
        send(
            &router,
            "127.0.0.1:9",
            logout(&first, &first_csrf, "https://app.dark-forge.dev", "nope"),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, _, _) = text(send(&router, "127.0.0.1:9", dashboard(&first)).await).await;
    assert_eq!(status, StatusCode::OK);
    let (status, headers, body) = text(
        send(
            &router,
            "127.0.0.1:9",
            logout(
                &first,
                &first_csrf,
                "https://app.dark-forge.dev",
                &first_csrf,
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_host_cookie(
        named_cookie(&headers, "__Host-kiln_app"),
        "__Host-kiln_app",
        0,
        true,
    );
    assert_host_cookie(
        named_cookie(&headers, "__Host-kiln_csrf"),
        "__Host-kiln_csrf",
        0,
        false,
    );
    let (status, _, _) = text(send(&router, "127.0.0.1:9", dashboard(&first)).await).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(identity.revoke_hits.load(Ordering::SeqCst), 1);
    let (status, _, body) = text(send(&router, "127.0.0.1:9", dashboard(&second)).await).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    for index in 0..11 {
        let (status, _, _) = text(send(&router, "192.0.2.40:9", get("/login")).await).await;
        assert_eq!(
            status,
            if index < 10 {
                StatusCode::SEE_OTHER
            } else {
                StatusCode::TOO_MANY_REQUESTS
            }
        );
    }
    let (status, _, _) = text(send(&router, "192.0.2.41:9", get("/login")).await).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn inactive_owner_clears_the_dashboard_session() {
    let root = tempfile::tempdir().unwrap();
    archive(root.path());
    let up = upstream_json().await;
    let (auth, key) = auth(root.path());
    let inactive = loopback(token(&key, "owner-a", Scope::Read), 1).await;
    let router = router_for(auth, &up, root.path(), "fixture-login", inactive.port);
    let (session, _) = browser_login(&router, "127.0.0.1:11").await;
    let request = Request::builder()
        .uri("/api/v1/dashboard")
        .header("cookie", format!("__Host-kiln_app={session}"))
        .body(Body::empty())
        .unwrap();
    let (status, _, _) = text(send(&router, "127.0.0.1:11", request).await).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(up.hits.load(Ordering::SeqCst), 0);
    assert_eq!(inactive.status_hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn unreachable_identity_keeps_an_unexpired_session() {
    let root = tempfile::tempdir().unwrap();
    archive(root.path());
    let up = upstream_json().await;
    let (auth, key) = auth(root.path());
    let down = loopback(token(&key, "owner-a", Scope::Read), 2).await;
    let router = router_for(auth, &up, root.path(), "fixture-login", down.port);
    let (session, _) = browser_login(&router, "127.0.0.1:12").await;
    let request = Request::builder()
        .uri("/api/v1/dashboard")
        .header("cookie", format!("__Host-kiln_app={session}"))
        .body(Body::empty())
        .unwrap();
    let (status, _, body) = text(send(&router, "127.0.0.1:12", request).await).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(up.hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_expired_cookie_write_is_refused_before_the_host() {
    let root = tempfile::tempdir().unwrap();
    archive(root.path());
    let up = upstream_json().await;
    let (auth, key) = auth(root.path());
    let expired = app_listener::router_with_expired_session(
        auth,
        &format!("https://127.0.0.1:{}", up.port),
        up.ca_pem.as_bytes(),
        Some(root.path()),
        &token(&key, "owner-a", Scope::Operate),
    )
    .unwrap();
    let (status, _, _) = text(
        send(
            &expired.router,
            "127.0.0.1:13",
            Request::builder()
                .method("POST")
                .uri("/v1/operations")
                .header("origin", "https://app.dark-forge.dev")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, body) = text(
        send(
            &expired.router,
            "127.0.0.1:13",
            create_request(&expired.session, &expired.csrf, &expired.csrf, b"{}"),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(up.hits.load(Ordering::SeqCst), 0);
    assert_eq!(
        expired
            .audit
            .records()
            .iter()
            .filter(|record| record.outcome == "unauthorized")
            .count(),
        2
    );
}

struct ServedOperations {
    _root: tempfile::TempDir,
    operate: app_listener::OperateSession,
    read: app_listener::OperateSession,
    other_owner: app_listener::OperateSession,
    journal: std::path::PathBuf,
    hits: Arc<AtomicUsize>,
    server: tokio::task::JoinHandle<()>,
}

async fn serve_operations() -> ServedOperations {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = Config {
        runtime_dir: root.path().join("runtime"),
        journal_dir: root.path().join("journal"),
        socket: root.path().join("host.sock"),
        isolation_config: None,
        allow_unsafe_development: true,
        templates: Default::default(),
    };
    drop(Host::open(&config).unwrap());
    let template = new_id();
    let directory = config.runtime_dir.join("snapshots").join(&template);
    std::fs::create_dir(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(
        directory.join("snapshot.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "id": template,
            "source_box": new_id(),
            "template": true,
            "host": "incompatible-test-host",
            "memory_mib": 384,
            "vcpus": 2,
            "hashes": {},
            "image": {
                "schema_version": 1,
                "architecture": "x86_64",
                "kernel_path": "/private/canary",
                "kernel_sha256": "unused",
                "rootfs_path": "/private/canary",
                "rootfs_sha256": "unused",
                "agent_protocol_version": 1
            }
        }))
        .unwrap(),
    )
    .unwrap();
    config.templates.insert("small".into(), template);
    let host = Host::open(&config).unwrap();
    let listener = bind_socket(&config.socket).await.unwrap();
    let host_router = host.router();
    let server = tokio::spawn(async move {
        axum::serve(listener, host_router).await.unwrap();
    });
    let (auth, key) = auth(root.path());
    let hits = Arc::new(AtomicUsize::new(0));
    let seen = hits.clone();
    let gateway = kiln_server::gateway::router_with_auth(&config.socket, central(root.path()))
        .unwrap()
        .layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let seen = seen.clone();
                async move {
                    seen.fetch_add(1, Ordering::SeqCst);
                    next.run(request).await
                }
            },
        ));
    let (port, ca_pem) = listen_tls(gateway).await;
    let journal = config.journal_dir.join("operations.sqlite");
    let operate = app_listener::router_with_operate_session(
        auth,
        &format!("https://127.0.0.1:{port}"),
        ca_pem.as_bytes(),
        Some(root.path()),
        &token(&key, "owner-a", Scope::Operate),
    )
    .unwrap();
    let read = app_listener::router_with_operate_session(
        central(root.path()),
        &format!("https://127.0.0.1:{port}"),
        ca_pem.as_bytes(),
        Some(root.path()),
        &token(&key, "owner-a", Scope::Read),
    )
    .unwrap();
    let other_owner = app_listener::router_with_operate_session(
        central(root.path()),
        &format!("https://127.0.0.1:{port}"),
        ca_pem.as_bytes(),
        Some(root.path()),
        &tenant_token(&key, "member-b", Scope::Operate, "tenant-a", MemberRole::Member),
    )
    .unwrap();
    ServedOperations {
        _root: root,
        operate,
        read,
        other_owner,
        journal,
        hits,
        server,
    }
}

#[tokio::test]
async fn create_box_checks_session_and_csrf_and_reserves_one_box() {
    let served = serve_operations().await;
    let operate = &served.operate;
    let read = &served.read;
    let journal = served.journal.clone();
    let hits = &served.hits;
    let id = new_id();
    let body = serde_json::to_vec(&Submit {
        id: id.clone(),
        action: Action::Create {
            template: "small".into(),
            name: "first".into(),
        },
    })
    .unwrap();
    let peer = "127.0.0.1:9";
    let (status, _, denied) = text(
        send(
            &operate.router,
            peer,
            Request::builder()
                .method("POST")
                .uri("/v1/operations")
                .header("content-type", "application/json")
                .body(Body::from(body.clone()))
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{denied}");
    let (status, _, _) = text(
        send(
            &operate.router,
            peer,
            Request::builder()
                .uri(format!("/v1/operations/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    assert_eq!(create_rows(&journal), 0);
    let (status, _, refused) = text(
        send(
            &operate.router,
            peer,
            create_request(&operate.session, &operate.csrf, "nope", &body),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert!(refused.contains("invalid_csrf"), "{refused}");
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    assert_eq!(create_rows(&journal), 0);
    let (status, _, scoped) = text(
        send(
            &read.router,
            peer,
            create_request(&read.session, &read.csrf, &read.csrf, &body),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{scoped}");
    assert!(scoped.contains("insufficient_scope"), "{scoped}");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        create_rows(&journal),
        0,
        "a read session must not reserve a box"
    );
    let (status, _, created) = text(
        send(
            &operate.router,
            peer,
            create_request(&operate.session, &operate.csrf, &operate.csrf, &body),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    let created: serde_json::Value = serde_json::from_str(&created).unwrap();
    let box_id = created["box_id"].as_str().unwrap().to_owned();
    assert_eq!(box_id.len(), 32);
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    assert_eq!(create_rows(&journal), 1);
    let (status, _, replay) = text(
        send(
            &operate.router,
            peer,
            create_request(&operate.session, &operate.csrf, &operate.csrf, &body),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    let replay: serde_json::Value = serde_json::from_str(&replay).unwrap();
    assert_eq!(replay["box_id"], box_id);
    assert_eq!(create_rows(&journal), 1, "same request id reserves one box");
    let mut terminal = String::new();
    for _ in 0..100 {
        let (status, _, polled) = text(
            send(
                &operate.router,
                peer,
                Request::builder()
                    .uri(format!("/v1/operations/{id}"))
                    .header(
                        "cookie",
                        format!(
                            "__Host-kiln_app={}; __Host-kiln_csrf={}",
                            operate.session, operate.csrf
                        ),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{polled}");
        terminal = serde_json::from_str::<serde_json::Value>(&polled).unwrap()["state"]
            .as_str()
            .unwrap()
            .to_owned();
        if terminal != "running" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        terminal, "failed",
        "incompatible template fails after one reservation"
    );
    assert_eq!(create_rows(&journal), 1);
    let changed = serde_json::to_vec(&Submit {
        id: id.clone(),
        action: Action::Create {
            template: "small".into(),
            name: "changed".into(),
        },
    })
    .unwrap();
    let (status, _, conflict) = text(
        send(
            &operate.router,
            peer,
            create_request(&operate.session, &operate.csrf, &operate.csrf, &changed),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{conflict}");
    assert_eq!(create_rows(&journal), 1);
    let outcomes: Vec<_> = operate
        .audit
        .records()
        .into_iter()
        .map(|record| record.outcome)
        .collect();
    assert!(outcomes.contains(&"unauthorized"), "{outcomes:?}");
    assert!(outcomes.contains(&"invalid_csrf"), "{outcomes:?}");
    assert_eq!(
        operate
            .audit
            .records()
            .into_iter()
            .filter(|record| record.outcome == "forwarded" && record.request_id == id)
            .count(),
        3,
        "create, replay, and the conflicting retry are audited"
    );
    assert!(
        read.audit
            .records()
            .iter()
            .any(|record| record.outcome == "forwarded" && record.request_id == id)
    );
    served.server.abort();
}

#[tokio::test]
async fn lifecycle_actions_reuse_the_operation_forwarder() {
    let served = serve_operations().await;
    let peer = "127.0.0.1:9";
    let box_id = reserve_box(&served, peer).await;
    let rejected_id = new_id();
    let rejected = serde_json::to_vec(&Submit {
        id: rejected_id.clone(),
        action: Action::Pause { id: box_id.clone() },
    })
    .unwrap();
    let (status, _, denied) = text(
        send(
            &served.operate.router,
            peer,
            Request::builder()
                .method("POST")
                .uri("/v1/operations")
                .header("content-type", "application/json")
                .body(Body::from(rejected.clone()))
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{denied}");
    let (status, _, csrf) = text(
        send(
            &served.operate.router,
            peer,
            posted_operation(
                &served.operate.session,
                &served.operate.csrf,
                "nope",
                "https://app.dark-forge.dev",
                &rejected,
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{csrf}");
    assert!(csrf.contains("invalid_csrf"), "{csrf}");
    let (status, _, origin) = text(
        send(
            &served.operate.router,
            peer,
            posted_operation(
                &served.operate.session,
                &served.operate.csrf,
                &served.operate.csrf,
                "https://evil.example",
                &rejected,
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{origin}");
    assert!(origin.contains("invalid_origin"), "{origin}");
    let (status, _, scoped) = text(
        send(
            &served.read.router,
            peer,
            posted_operation(
                &served.read.session,
                &served.read.csrf,
                &served.read.csrf,
                "https://app.dark-forge.dev",
                &rejected,
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{scoped}");
    assert!(scoped.contains("insufficient_scope"), "{scoped}");
    assert_eq!(
        rows_for(&served.journal, &rejected_id),
        0,
        "rejected lifecycle writes must not be journaled"
    );
    let actions = [
        Action::Pause { id: box_id.clone() },
        Action::Resume { id: box_id.clone() },
        Action::Stop {
            id: box_id.clone(),
            force: false,
        },
        Action::Start { id: box_id.clone() },
    ];
    for action in actions {
        assert_one_operation(&served, peer, &box_id, action).await;
    }
    let conflict_id = new_id();
    let gentle = serde_json::to_vec(&Submit {
        id: conflict_id.clone(),
        action: Action::Stop {
            id: box_id.clone(),
            force: false,
        },
    })
    .unwrap();
    let forced = serde_json::to_vec(&Submit {
        id: conflict_id.clone(),
        action: Action::Stop {
            id: box_id.clone(),
            force: true,
        },
    })
    .unwrap();
    let (status, _, accepted) = text(
        send(
            &served.operate.router,
            peer,
            create_request(
                &served.operate.session,
                &served.operate.csrf,
                &served.operate.csrf,
                &gentle,
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    let (status, _, replay) = text(
        send(
            &served.operate.router,
            peer,
            create_request(
                &served.operate.session,
                &served.operate.csrf,
                &served.operate.csrf,
                &gentle,
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&replay).unwrap()["id"],
        conflict_id
    );
    assert_eq!(rows_for(&served.journal, &conflict_id), 1);
    let (status, _, conflict) = text(
        send(
            &served.operate.router,
            peer,
            create_request(
                &served.operate.session,
                &served.operate.csrf,
                &served.operate.csrf,
                &forced,
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{conflict}");
    assert_eq!(rows_for(&served.journal, &conflict_id), 1);
    assert_one_operation(
        &served,
        peer,
        &box_id,
        Action::Stop {
            id: box_id.clone(),
            force: true,
        },
    )
    .await;
    let outcomes: Vec<_> = served
        .operate
        .audit
        .records()
        .into_iter()
        .map(|record| record.outcome)
        .collect();
    assert!(outcomes.contains(&"unauthorized"), "{outcomes:?}");
    assert!(outcomes.contains(&"invalid_csrf"), "{outcomes:?}");
    assert!(outcomes.contains(&"invalid_origin"), "{outcomes:?}");
    assert!(
        served
            .read
            .audit
            .records()
            .iter()
            .any(|record| record.outcome == "forwarded" && record.request_id == rejected_id),
        "a read-scoped write is audited and still refused"
    );
    served.server.abort();
}

#[tokio::test]
async fn delete_reuses_the_operation_forwarder() {
    let served = serve_operations().await;
    let peer = "127.0.0.1:9";
    let box_id = reserve_box(&served, peer).await;
    let rejected_id = new_id();
    let rejected = serde_json::to_vec(&Submit {
        id: rejected_id.clone(),
        action: Action::Delete { id: box_id.clone() },
    })
    .unwrap();
    let (status, _, denied) = text(
        send(
            &served.operate.router,
            peer,
            Request::builder()
                .method("POST")
                .uri("/v1/operations")
                .header("content-type", "application/json")
                .body(Body::from(rejected.clone()))
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{denied}");
    let (status, _, csrf) = text(
        send(
            &served.operate.router,
            peer,
            posted_operation(
                &served.operate.session,
                &served.operate.csrf,
                "nope",
                "https://app.dark-forge.dev",
                &rejected,
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{csrf}");
    assert!(csrf.contains("invalid_csrf"), "{csrf}");
    let (status, _, origin) = text(
        send(
            &served.operate.router,
            peer,
            posted_operation(
                &served.operate.session,
                &served.operate.csrf,
                &served.operate.csrf,
                "https://evil.example",
                &rejected,
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{origin}");
    assert!(origin.contains("invalid_origin"), "{origin}");
    let (status, _, scoped) = text(
        send(
            &served.read.router,
            peer,
            posted_operation(
                &served.read.session,
                &served.read.csrf,
                &served.read.csrf,
                "https://app.dark-forge.dev",
                &rejected,
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{scoped}");
    assert!(scoped.contains("insufficient_scope"), "{scoped}");
    assert_eq!(rows_for(&served.journal, &rejected_id), 0);
    let foreign_id = new_id();
    let foreign_box = new_id();
    let foreign = serde_json::to_vec(&Submit {
        id: foreign_id.clone(),
        action: Action::Delete { id: foreign_box },
    })
    .unwrap();
    let (status, _, missing) = text(
        send(
            &served.operate.router,
            peer,
            create_request(
                &served.operate.session,
                &served.operate.csrf,
                &served.operate.csrf,
                &foreign,
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");
    assert!(missing.contains("not_found"), "{missing}");
    assert_eq!(
        rows_for(&served.journal, &foreign_id),
        0,
        "another user's box is not journaled"
    );
    assert_eq!(create_rows(&served.journal), 1);
    let delete_id = new_id();
    let body = serde_json::to_vec(&Submit {
        id: delete_id.clone(),
        action: Action::Delete { id: box_id.clone() },
    })
    .unwrap();
    let post = || {
        create_request(
            &served.operate.session,
            &served.operate.csrf,
            &served.operate.csrf,
            &body,
        )
    };
    let (status, _, accepted) = text(send(&served.operate.router, peer, post()).await).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    let (status, _, replay) = text(send(&served.operate.router, peer, post()).await).await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&replay).unwrap()["id"],
        delete_id
    );
    assert_eq!(rows_for(&served.journal, &delete_id), 1);
    let changed = serde_json::to_vec(&Submit {
        id: delete_id.clone(),
        action: Action::Pause { id: box_id.clone() },
    })
    .unwrap();
    let (status, _, conflict) = text(
        send(
            &served.operate.router,
            peer,
            create_request(
                &served.operate.session,
                &served.operate.csrf,
                &served.operate.csrf,
                &changed,
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{conflict}");
    assert_eq!(rows_for(&served.journal, &delete_id), 1);
    let outcomes: Vec<_> = served
        .operate
        .audit
        .records()
        .into_iter()
        .map(|record| record.outcome)
        .collect();
    assert!(outcomes.contains(&"unauthorized"), "{outcomes:?}");
    assert!(outcomes.contains(&"invalid_csrf"), "{outcomes:?}");
    assert!(outcomes.contains(&"invalid_origin"), "{outcomes:?}");
    assert!(
        outcomes.contains(&"forwarded"),
        "audit line missing for delete"
    );
    assert!(
        served
            .operate
            .audit
            .records()
            .iter()
            .any(|record| record.outcome == "forwarded" && record.request_id == delete_id),
        "delete audit line missing"
    );
    assert!(
        served
            .read
            .audit
            .records()
            .iter()
            .any(|record| record.outcome == "forwarded" && record.request_id == rejected_id),
        "a read-scoped delete is audited and still refused"
    );
    served.server.abort();
}

#[tokio::test]
async fn exec_reuses_the_operation_forwarder() {
    let served = serve_operations().await;
    let peer = "127.0.0.1:9";
    let box_id = reserve_box(&served, peer).await;
    plant_stopped_box(served._root.path(), &box_id);
    let rejected_id = new_id();
    let rejected = exec_body(&rejected_id, &box_id);
    let (status, _, denied) = text(
        send(
            &served.operate.router,
            peer,
            Request::builder()
                .method("POST")
                .uri("/v1/operations")
                .header("content-type", "application/json")
                .body(Body::from(rejected.clone()))
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{denied}");
    let (status, _, csrf) = text(
        send(
            &served.operate.router,
            peer,
            posted_operation(
                &served.operate.session,
                &served.operate.csrf,
                "nope",
                "https://app.dark-forge.dev",
                &rejected,
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{csrf}");
    assert!(csrf.contains("invalid_csrf"), "{csrf}");
    let (status, _, origin) = text(
        send(
            &served.operate.router,
            peer,
            posted_operation(
                &served.operate.session,
                &served.operate.csrf,
                &served.operate.csrf,
                "https://evil.example",
                &rejected,
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{origin}");
    assert!(origin.contains("invalid_origin"), "{origin}");
    let (status, _, scoped) = text(
        send(
            &served.read.router,
            peer,
            posted_operation(
                &served.read.session,
                &served.read.csrf,
                &served.read.csrf,
                "https://app.dark-forge.dev",
                &rejected,
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{scoped}");
    assert!(scoped.contains("insufficient_scope"), "{scoped}");
    assert_eq!(rows_for(&served.journal, &rejected_id), 0);
    let foreign_id = new_id();
    let foreign = exec_body(&foreign_id, &new_id());
    let (status, _, missing) = text(
        send(
            &served.operate.router,
            peer,
            create_request(
                &served.operate.session,
                &served.operate.csrf,
                &served.operate.csrf,
                &foreign,
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");
    assert!(missing.contains("not_found"), "{missing}");
    assert_eq!(rows_for(&served.journal, &foreign_id), 0);
    let exec_id = new_id();
    let body = exec_body(&exec_id, &box_id);
    let post = || {
        create_request(
            &served.operate.session,
            &served.operate.csrf,
            &served.operate.csrf,
            &body,
        )
    };
    let (status, _, accepted) = text(send(&served.operate.router, peer, post()).await).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    let (status, _, replay) = text(send(&served.operate.router, peer, post()).await).await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(rows_for(&served.journal, &exec_id), 1);
    let mut terminal = String::new();
    let mut polled = String::new();
    for _ in 0..100 {
        let (status, _, body) = text(
            send(
                &served.operate.router,
                peer,
                Request::builder()
                    .uri(format!("/v1/operations/{exec_id}"))
                    .header(
                        "cookie",
                        format!(
                            "__Host-kiln_app={}; __Host-kiln_csrf={}",
                            served.operate.session, served.operate.csrf
                        ),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        polled = body;
        terminal = serde_json::from_str::<serde_json::Value>(&polled).unwrap()["state"]
            .as_str()
            .unwrap()
            .to_owned();
        if terminal != "running" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(terminal, "unknown", "{polled}");
    let value: serde_json::Value = serde_json::from_str(&polled).unwrap();
    assert!(value["result"].is_null(), "{polled}");
    assert!(!polled.contains("kiln-exec-canary"), "{polled}");
    assert_eq!(rows_for(&served.journal, &exec_id), 1);
    let outcomes: Vec<_> = served
        .operate
        .audit
        .records()
        .into_iter()
        .map(|record| record.outcome)
        .collect();
    assert!(outcomes.contains(&"unauthorized"), "{outcomes:?}");
    assert!(outcomes.contains(&"invalid_csrf"), "{outcomes:?}");
    assert!(outcomes.contains(&"invalid_origin"), "{outcomes:?}");
    assert!(
        served
            .operate
            .audit
            .records()
            .iter()
            .any(|record| record.outcome == "forwarded" && record.request_id == exec_id),
        "exec audit line missing"
    );
    assert!(
        served
            .read
            .audit
            .records()
            .iter()
            .any(|record| record.outcome == "forwarded" && record.request_id == rejected_id),
        "a read-scoped exec is audited and still refused"
    );
    served.server.abort();
}

fn exec_body(id: &str, box_id: &str) -> Vec<u8> {
    serde_json::to_vec(&Submit {
        id: id.to_owned(),
        action: Action::Exec {
            id: box_id.to_owned(),
            request: kiln_api::ExecRequest {
                argv: vec!["/bin/printf".into(), "kiln-exec-canary".into()],
                cwd: None,
                env: Default::default(),
                timeout_ms: 1000,
            },
        },
    })
    .unwrap()
}

fn plant_stopped_box(root: &Path, id: &str) {
    use std::os::unix::fs::PermissionsExt;
    let directory = root.join("runtime").join("boxes").join(id);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    let record = serde_json::json!({
        "schema_version": 1,
        "id": id,
        "name": "stopped",
        "state": "stopped",
        "run": new_id(),
        "process": null,
        "image": {
            "schema_version": 1,
            "architecture": "x86_64",
            "kernel_path": "kernel",
            "kernel_sha256": "ab".repeat(32),
            "rootfs_path": "rootfs",
            "rootfs_sha256": "cd".repeat(32),
            "agent_protocol_version": 1
        },
        "host": "test",
        "memory_mib": 256,
        "vcpus": 1,
        "source": null,
        "operation": null,
        "last_error": null,
        "disk_copy": ""
    });
    std::fs::write(
        directory.join("box.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
}

async fn reserve_box(served: &ServedOperations, peer: &str) -> String {
    let body = serde_json::to_vec(&Submit {
        id: new_id(),
        action: Action::Create {
            template: "small".into(),
            name: "lifecycle".into(),
        },
    })
    .unwrap();
    let (status, _, created) = text(
        send(
            &served.operate.router,
            peer,
            create_request(
                &served.operate.session,
                &served.operate.csrf,
                &served.operate.csrf,
                &body,
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    serde_json::from_str::<serde_json::Value>(&created).unwrap()["box_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn assert_one_operation(served: &ServedOperations, peer: &str, box_id: &str, action: Action) {
    let id = new_id();
    let body = serde_json::to_vec(&Submit {
        id: id.clone(),
        action,
    })
    .unwrap();
    let post = |body: &[u8]| {
        create_request(
            &served.operate.session,
            &served.operate.csrf,
            &served.operate.csrf,
            body,
        )
    };
    let (status, _, accepted) = text(send(&served.operate.router, peer, post(&body)).await).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    let accepted: serde_json::Value = serde_json::from_str(&accepted).unwrap();
    assert_eq!(accepted["id"], id);
    assert_eq!(accepted["box_id"], box_id);
    let (status, _, replay) = text(send(&served.operate.router, peer, post(&body)).await).await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&replay).unwrap()["id"],
        id
    );
    assert_eq!(
        rows_for(&served.journal, &id),
        1,
        "same request id is one operation"
    );
    assert!(
        served
            .operate
            .audit
            .records()
            .iter()
            .any(|record| record.outcome == "forwarded" && record.request_id == id),
        "audit line missing for {id}"
    );
}

fn central(dir: &Path) -> Authenticator {
    Authenticator::central_only(
        Enrollment {
            version: 1,
            issuer: "https://identity.example.test".into(),
            owner_id: "owner-a".into(),
            server_id: "server-b".into(),
        },
        dir.join("jwks.json"),
    )
    .unwrap()
}

fn create_request(session: &str, csrf: &str, presented: &str, body: &[u8]) -> Request<Body> {
    posted_operation(session, csrf, presented, "https://app.dark-forge.dev", body)
}

fn posted_operation(
    session: &str,
    csrf: &str,
    presented: &str,
    origin: &str,
    body: &[u8],
) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/operations")
        .header("origin", origin)
        .header(
            "cookie",
            format!("__Host-kiln_app={session}; __Host-kiln_csrf={csrf}"),
        )
        .header("x-kiln-csrf", presented)
        .header("content-type", "application/json")
        .body(Body::from(body.to_vec()))
        .unwrap()
}

fn rows_for(path: &Path, id: &str) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row("SELECT count(*) FROM operations WHERE id=?1", [id], |row| {
            row.get(0)
        })
        .unwrap()
}

fn create_rows(path: &Path) -> i64 {
    let db = rusqlite::Connection::open(path).unwrap();
    db.query_row(
        "SELECT count(*) FROM operations WHERE is_create=1",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

#[tokio::test]
async fn a_signed_in_operate_session_reaches_the_host() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = Config {
        runtime_dir: root.path().join("runtime"),
        journal_dir: root.path().join("journal"),
        socket: root.path().join("host.sock"),
        isolation_config: None,
        allow_unsafe_development: true,
        templates: Default::default(),
    };
    drop(Host::open(&config).unwrap());
    let template = new_id();
    let directory = config.runtime_dir.join("snapshots").join(&template);
    std::fs::create_dir(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(
        directory.join("snapshot.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "id": template,
            "source_box": new_id(),
            "template": true,
            "host": "incompatible-test-host",
            "memory_mib": 384,
            "vcpus": 2,
            "hashes": {},
            "image": {
                "schema_version": 1,
                "architecture": "x86_64",
                "kernel_path": "/private/canary",
                "kernel_sha256": "unused",
                "rootfs_path": "/private/canary",
                "rootfs_sha256": "unused",
                "agent_protocol_version": 1
            }
        }))
        .unwrap(),
    )
    .unwrap();
    config.templates.insert("small".into(), template);
    let host = Host::open(&config).unwrap();
    let listener = bind_socket(&config.socket).await.unwrap();
    let host_router = host.router();
    let server = tokio::spawn(async move {
        axum::serve(listener, host_router).await.unwrap();
    });
    let (auth, key) = auth(root.path());
    let authorizations = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = authorizations.clone();
    let gateway = kiln_server::gateway::router_with_auth(&config.socket, central(root.path()))
        .unwrap()
        .layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let recorded = recorded.clone();
                async move {
                    if let Some(value) = request
                        .headers()
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                    {
                        recorded.lock().unwrap().push(value.to_owned());
                    }
                    next.run(request).await
                }
            },
        ));
    let (port, ca_pem) = listen_tls(gateway).await;
    let read_identity = loopback(token(&key, "owner-a", Scope::Read), 0).await;
    let refresh = token_with(&key, "owner-a", Scope::Operate, "app_server-b");
    let operate_identity = scripted_loopback(
        token(&key, "owner-a", Scope::Operate),
        refresh.clone(),
        30,
        0,
    )
    .await;
    let read_router = signed_in_router(auth, root.path(), &ca_pem, port, read_identity.port);
    let operate_router = signed_in_router(
        central(root.path()),
        root.path(),
        &ca_pem,
        port,
        operate_identity.port,
    );
    let (read_session, read_csrf) = browser_login(&read_router, "127.0.0.1:30").await;
    let id = new_id();
    let body = serde_json::to_vec(&Submit {
        id: id.clone(),
        action: Action::Create {
            template: "small".into(),
            name: "first".into(),
        },
    })
    .unwrap();
    let journal = config.journal_dir.join("operations.sqlite");
    let (status, _, scoped) = text(
        send(
            &read_router,
            "127.0.0.1:30",
            create_request(&read_session, &read_csrf, &read_csrf, &body),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{scoped}");
    assert!(scoped.contains("insufficient_scope"), "{scoped}");
    assert_eq!(create_rows(&journal), 0);
    let (operate_session, operate_csrf) = browser_login(&operate_router, "127.0.0.1:31").await;
    let (status, _, _) = text(
        send(
            &operate_router,
            "127.0.0.1:31",
            Request::builder()
                .method("POST")
                .uri("/v1/operations")
                .header("origin", "https://app.dark-forge.dev")
                .header("content-type", "application/json")
                .body(Body::from(body.clone()))
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, refused) = text(
        send(
            &operate_router,
            "127.0.0.1:31",
            Request::builder()
                .method("POST")
                .uri("/v1/operations")
                .header("origin", "https://evil.example")
                .header(
                    "cookie",
                    format!("__Host-kiln_app={operate_session}; __Host-kiln_csrf={operate_csrf}"),
                )
                .header("x-kiln-csrf", &operate_csrf)
                .header("content-type", "application/json")
                .body(Body::from(body.clone()))
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert!(refused.contains("invalid_origin"), "{refused}");
    let (status, _, refused) = text(
        send(
            &operate_router,
            "127.0.0.1:31",
            create_request(&operate_session, &operate_csrf, "nope", &body),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert!(refused.contains("invalid_csrf"), "{refused}");
    assert_eq!(create_rows(&journal), 0);
    let forwarded_before = authorizations.lock().unwrap().len();
    let (status, _, created) = text(
        send(
            &operate_router,
            "127.0.0.1:31",
            create_request(&operate_session, &operate_csrf, &operate_csrf, &body),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    assert_eq!(create_rows(&journal), 1);
    let seen = authorizations.lock().unwrap();
    let expected = format!("Bearer {refresh}");
    assert_eq!(seen.last().map(String::as_str), Some(expected.as_str()));
    assert_eq!(seen.len(), forwarded_before + 1);
    server.abort();
}

fn terminal_request(id: &str, session: &str, csrf: &str, origin: &str) -> Request<Body> {
    let mut builder = Request::builder().uri(format!("/v1/boxes/{id}/terminal"));
    if !session.is_empty() {
        builder = builder
            .header(
                "cookie",
                format!("__Host-kiln_app={session}; __Host-kiln_csrf={csrf}"),
            )
            .header("origin", origin);
    }
    builder.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn terminal_rejects_unauthenticated_read_scope_bad_origin_and_other_owner() {
    let served = serve_operations().await;
    let peer = "127.0.0.1:9";
    let box_id = new_id();
    let hits = served.hits.load(Ordering::SeqCst);
    let (status, _, denied) = text(
        send(
            &served.operate.router,
            peer,
            Request::builder()
                .uri(format!("/v1/boxes/{box_id}/terminal"))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{denied}");
    assert!(denied.contains("unauthorized"), "{denied}");
    let (status, _, scoped) = text(
        send(
            &served.read.router,
            peer,
            terminal_request(
                &box_id,
                &served.read.session,
                &served.read.csrf,
                "https://app.dark-forge.dev",
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{scoped}");
    assert!(scoped.contains("insufficient_scope"), "{scoped}");
    let (status, _, origin) = text(
        send(
            &served.operate.router,
            peer,
            terminal_request(
                &box_id,
                &served.operate.session,
                &served.operate.csrf,
                "https://evil.example",
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{origin}");
    assert!(origin.contains("invalid_origin"), "{origin}");
    let (status, _, other) = text(
        send(
            &served.other_owner.router,
            peer,
            terminal_request(
                &box_id,
                &served.other_owner.session,
                &served.other_owner.csrf,
                "https://app.dark-forge.dev",
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{other}");
    assert!(other.contains("not_found"), "{other}");
    assert_eq!(
        served.hits.load(Ordering::SeqCst),
        hits + 1,
        "rejected terminal requests must not open SSH; one box-metadata GET is allowed"
    );
    for (audit, outcome) in [
        (&served.operate.audit, "unauthorized"),
        (&served.read.audit, "insufficient_scope"),
        (&served.operate.audit, "invalid_origin"),
        (&served.other_owner.audit, "rejected"),
    ] {
        assert!(
            audit
                .records()
                .iter()
                .any(|record| record.outcome == outcome),
            "missing audit {outcome}: {:?}",
            audit.records()
        );
    }
    let (status, _, missing) = text(
        send(
            &served.operate.router,
            peer,
            terminal_request(
                &box_id,
                &served.operate.session,
                &served.operate.csrf,
                "https://app.dark-forge.dev",
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");
    assert!(missing.contains("not_found"), "{missing}");
    assert!(
        !served
            .operate
            .audit
            .records()
            .iter()
            .any(|record| record.outcome == "opened"),
        "a rejected box must not open a terminal session"
    );
    served.server.abort();
}

#[tokio::test]
async fn operate_session_gets_a_pty_echo_from_a_running_box() {
    use std::{
        os::unix::fs::PermissionsExt,
        process::{Child, Stdio},
        sync::Mutex,
    };
    use tokio::net::UnixListener;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    assert!(
        std::path::Path::new("/usr/sbin/sshd").is_file(),
        "openssh-server is required for the terminal PTY test"
    );
    assert!(
        Command::new("sudo")
            .args(["-n", "mkdir", "-p", "/run/sshd"])
            .status()
            .expect("sudo")
            .success(),
        "could not create /run/sshd"
    );
    assert!(
        Command::new("sudo")
            .args(["-n", "chmod", "755", "/run/sshd"])
            .status()
            .unwrap()
            .success(),
        "could not set /run/sshd mode"
    );

    let served = serve_operations().await;
    let peer = "127.0.0.1:9";
    let operation = new_id();
    let body = serde_json::to_vec(&Submit {
        id: operation.clone(),
        action: Action::Create {
            template: "small".into(),
            name: "terminal".into(),
        },
    })
    .unwrap();
    let (status, _, created) = text(
        send(
            &served.operate.router,
            peer,
            create_request(
                &served.operate.session,
                &served.operate.csrf,
                &served.operate.csrf,
                &body,
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    let box_id = serde_json::from_str::<serde_json::Value>(&created).unwrap()["box_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut terminal_state = String::new();
    let mut polled = String::new();
    for _ in 0..200 {
        let (status, _, body) = text(
            send(
                &served.operate.router,
                peer,
                Request::builder()
                    .uri(format!("/v1/operations/{operation}"))
                    .header(
                        "cookie",
                        format!(
                            "__Host-kiln_app={}; __Host-kiln_csrf={}",
                            served.operate.session, served.operate.csrf
                        ),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        polled = body;
        terminal_state = serde_json::from_str::<serde_json::Value>(&polled).unwrap()["state"]
            .as_str()
            .unwrap()
            .to_owned();
        if terminal_state != "running" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_ne!(terminal_state, "running", "{polled}");

    let root = served
        .journal
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let work = root.join("terminal-guest");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::set_permissions(&work, std::fs::Permissions::from_mode(0o700)).unwrap();
    let bin_dir = work.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let source = work.join("vmm.rs");
    std::fs::write(
        &source,
        "fn main() {\n    if std::env::args().nth(1).as_deref() == Some(\"--version\") {\n        println!(\"Firecracker v1.17.0\");\n        return;\n    }\n    std::thread::sleep(std::time::Duration::from_secs(180));\n}\n",
    )
    .unwrap();
    let firecracker = bin_dir.join("firecracker");
    assert!(
        Command::new("rustc")
            .current_dir(&work)
            .args(["--edition=2024", "-O", "-o"])
            .arg(&firecracker)
            .arg(&source)
            .status()
            .expect("rustc")
            .success(),
        "could not compile the stand-in VMM"
    );
    let host_key = ssh_public_key(&work.join("hostkey"));
    let root_key = work.join("hostkey.root");
    assert!(sudo_cp(work.join("hostkey"), &root_key));
    assert!(sudo_ok(["chown", "root:root"], &root_key));
    assert!(sudo_ok(["chmod", "600"], &root_key));
    let keys = work.join("authorized_keys");
    std::fs::write(&keys, b"").unwrap();
    std::fs::set_permissions(&keys, std::fs::Permissions::from_mode(0o600)).unwrap();
    let config = work.join("sshd_config");
    std::fs::write(
        &config,
        format!(
            "HostKey {}\nAuthorizedKeysFile {}\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nChallengeResponseAuthentication no\nPubkeyAuthentication yes\nAuthenticationMethods publickey\nUsePAM no\nPermitRootLogin prohibit-password\nAllowUsers root\nAllowAgentForwarding no\nX11Forwarding no\nAllowTcpForwarding no\nPermitTunnel no\nPidFile {}\nStrictModes no\nUseDNS no\nPrintMotd no\nPrintLastLog no\nLogLevel ERROR\n",
            root_key.display(),
            keys.display(),
            work.join("sshd.pid").display()
        ),
    )
    .unwrap();
    let run_id = new_id();
    let run = root
        .join("runtime")
        .join("boxes")
        .join(&box_id)
        .join(&run_id);
    std::fs::create_dir_all(&run).unwrap();
    std::fs::set_permissions(
        run.parent().unwrap(),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o700)).unwrap();
    let api = UnixListener::bind(run.join("api.sock")).unwrap();
    let vsock = UnixListener::bind(run.join("vsock.sock")).unwrap();
    let children = Arc::new(Mutex::new(Vec::<Child>::new()));
    let reap = Reap(children.clone());
    let guest_key = host_key.clone();
    let guest_work = work.clone();
    let guest_children = children.clone();
    let guest = tokio::spawn(async move {
        serve_guest(api, vsock, guest_key, guest_work, guest_children).await;
    });
    let _path = PathGuard::prepend(&bin_dir);
    let mut sleeper = Command::new(&firecracker)
        .current_dir(&run)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let identity = kiln_runtime::process::identify(sleeper.id()).unwrap();
    let record = serde_json::json!({
        "schema_version": 1,
        "id": box_id,
        "name": "terminal",
        "state": "running",
        "run": run_id,
        "process": identity,
        "image": {
            "schema_version": 1,
            "architecture": "x86_64",
            "kernel_path": "kernel",
            "kernel_sha256": "ab".repeat(32),
            "rootfs_path": "rootfs",
            "rootfs_sha256": "cd".repeat(32),
            "agent_protocol_version": 1
        },
        "host": "test",
        "memory_mib": 256,
        "vcpus": 1,
        "source": null,
        "operation": null,
        "last_error": null,
        "disk_copy": ""
    });
    std::fs::write(
        root.join("runtime")
            .join("boxes")
            .join(&box_id)
            .join("box.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = served.operate.router.clone();
    let http = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let mut request = format!("ws://{addr}/v1/boxes/{box_id}/terminal")
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(
        "cookie",
        format!(
            "__Host-kiln_app={}; __Host-kiln_csrf={}",
            served.operate.session, served.operate.csrf
        )
        .parse()
        .unwrap(),
    );
    request
        .headers_mut()
        .insert("origin", "https://app.dark-forge.dev".parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("terminal websocket");
    use futures_util::{SinkExt, StreamExt};
    socket
        .send(tokio_tungstenite::tungstenite::Message::text(
            "printf 'kiln-term-ok\\n'\n",
        ))
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    let mut collected = Vec::new();
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let message = tokio::time::timeout(remaining, socket.next()).await;
        let Ok(Some(Ok(message))) = message else {
            break;
        };
        match message {
            tokio_tungstenite::tungstenite::Message::Binary(bytes) => {
                collected.extend_from_slice(&bytes)
            }
            tokio_tungstenite::tungstenite::Message::Text(text) => {
                collected.extend_from_slice(text.as_bytes())
            }
            tokio_tungstenite::tungstenite::Message::Close(_) => break,
            _ => {}
        }
        if collected
            .windows(b"kiln-term-ok".len())
            .any(|window| window == b"kiln-term-ok")
        {
            break;
        }
    }
    let output = String::from_utf8_lossy(&collected);
    let sshd_err = std::fs::read_to_string(work.join("sshd.err")).unwrap_or_default();
    assert!(
        output.contains("kiln-term-ok"),
        "pty output {output:?}\nsshd: {sshd_err}"
    );
    assert!(
        served
            .operate
            .audit
            .records()
            .iter()
            .any(|record| { record.outcome == "opened" && record.request_id == box_id }),
        "opened audit missing: {:?}",
        served.operate.audit.records()
    );
    drop(socket);
    let _ = sleeper.kill();
    let _ = sleeper.wait();
    http.abort();
    guest.abort();
    drop(reap);
    served.server.abort();
}

fn sudo_cp(from: impl AsRef<Path>, to: &Path) -> bool {
    Command::new("sudo")
        .args(["-n", "cp"])
        .arg(from.as_ref())
        .arg(to)
        .status()
        .is_ok_and(|status| status.success())
}

fn sudo_ok(args: [&str; 2], path: &Path) -> bool {
    Command::new("sudo")
        .arg("-n")
        .args(args)
        .arg(path)
        .status()
        .is_ok_and(|status| status.success())
}

fn ssh_public_key(path: &Path) -> String {
    let status = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "", "-f"])
        .arg(path)
        .status()
        .expect("ssh-keygen");
    assert!(status.success(), "ssh-keygen failed");
    let public = std::fs::read_to_string(path.with_extension("pub")).unwrap();
    let public = public.trim().to_owned();
    assert!(kiln_api::valid_ssh_public_key(&public), "{public}");
    public
}

struct PathGuard(Option<std::ffi::OsString>);

impl PathGuard {
    fn prepend(dir: &Path) -> Self {
        let previous = std::env::var_os("PATH");
        let mut next = std::ffi::OsString::from(dir);
        if let Some(previous) = &previous {
            next.push(":");
            next.push(previous);
        }
        unsafe { std::env::set_var("PATH", next) };
        Self(previous)
    }
}

impl Drop for PathGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.0 {
                Some(value) => std::env::set_var("PATH", value),
                None => std::env::remove_var("PATH"),
            }
        }
    }
}

struct Reap(Arc<std::sync::Mutex<Vec<std::process::Child>>>);

impl Drop for Reap {
    fn drop(&mut self) {
        let mut children = self.0.lock().unwrap_or_else(|poison| poison.into_inner());
        for child in children.iter_mut() {
            let id = child.id();
            let _ = child.kill();
            let _ = child.wait();
            let _ = Command::new("sudo")
                .args(["-n", "kill", "-9", &format!("-{id}")])
                .status();
        }
    }
}

async fn serve_guest(
    api: tokio::net::UnixListener,
    vsock: tokio::net::UnixListener,
    host_key: String,
    work: std::path::PathBuf,
    children: Arc<std::sync::Mutex<Vec<std::process::Child>>>,
) {
    use std::os::unix::process::CommandExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = api.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut received = Vec::new();
                let mut byte = [0u8; 1];
                while !received.ends_with(b"\r\n\r\n") {
                    if stream.read(&mut byte).await.unwrap_or(0) == 0 || received.len() > 65536 {
                        return;
                    }
                    received.push(byte[0]);
                }
                let body = br#"{"state":"Running"}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.write_all(body).await;
            });
        }
    });
    loop {
        let Ok((mut stream, _)) = vsock.accept().await else {
            break;
        };
        let host_key = host_key.clone();
        let work = work.clone();
        let children = children.clone();
        tokio::spawn(async move {
            let line = read_unix_line(&mut stream).await;
            let _ = stream.write_all(b"OK 1\n").await;
            if line == "CONNECT 1024" {
                let _request = read_unix_frame(&mut stream).await;
                write_unix_frame(
                    &mut stream,
                    &serde_json::json!({"type": "ssh_host_key", "public_key": host_key}),
                )
                .await;
                return;
            }
            if line != "CONNECT 1025" {
                return;
            }
            let admission = read_unix_frame(&mut stream).await;
            let public_key = admission["public_key"].as_str().unwrap_or("").to_owned();
            std::fs::write(work.join("authorized_keys"), format!("{public_key}\n")).unwrap();
            write_unix_frame(&mut stream, &serde_json::json!({"public_key": host_key})).await;
            let _ = stream.flush().await;
            let std_stream = stream.into_std().unwrap();
            std_stream.set_nonblocking(false).unwrap();
            let stdout = std_stream.try_clone().unwrap();
            let stderr = std::fs::File::create(work.join("sshd.err")).unwrap();
            let child = std::process::Command::new("sudo")
                .args(["-n", "/usr/sbin/sshd", "-i", "-f"])
                .arg(work.join("sshd_config"))
                .stdin(std::process::Stdio::from(std::os::fd::OwnedFd::from(
                    std_stream,
                )))
                .stdout(std::process::Stdio::from(std::os::fd::OwnedFd::from(
                    stdout,
                )))
                .stderr(stderr)
                .process_group(0)
                .spawn()
                .unwrap();
            children.lock().unwrap().push(child);
        });
    }
}

async fn read_unix_line(stream: &mut tokio::net::UnixStream) -> String {
    use tokio::io::AsyncReadExt;
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let count = stream.read(&mut byte).await.unwrap();
        assert_ne!(count, 0, "vsock handshake closed");
        if byte[0] == b'\n' {
            break;
        }
        line.push(byte[0]);
        assert!(line.len() < 80, "vsock handshake too long");
    }
    String::from_utf8(line).unwrap()
}

async fn read_unix_frame(stream: &mut tokio::net::UnixStream) -> serde_json::Value {
    use tokio::io::AsyncReadExt;
    let mut len = [0u8; 4];
    stream.read_exact(&mut len).await.unwrap();
    let count = u32::from_be_bytes(len) as usize;
    assert!(count <= 1024 * 1024);
    let mut payload = vec![0; count];
    stream.read_exact(&mut payload).await.unwrap();
    serde_json::from_slice(&payload).unwrap()
}

async fn write_unix_frame(stream: &mut tokio::net::UnixStream, value: &serde_json::Value) {
    use tokio::io::AsyncWriteExt;
    let payload = serde_json::to_vec(value).unwrap();
    stream
        .write_all(&(payload.len() as u32).to_be_bytes())
        .await
        .unwrap();
    stream.write_all(&payload).await.unwrap();
    stream.flush().await.unwrap();
}

fn signed_in_router(
    auth: Authenticator,
    root: &Path,
    ca_pem: &str,
    port: u16,
    identity_port: u16,
) -> axum::Router {
    app_listener::router_with(
        auth,
        &format!("https://127.0.0.1:{port}"),
        ca_pem.as_bytes(),
        Some(root),
        app_listener::SignIn {
            authorize_origin: "https://dark-forge.dev".into(),
            loopback: format!("http://127.0.0.1:{identity_port}"),
            login_secret: "fixture-login".into(),
            server_id: "server-b".into(),
        },
    )
    .unwrap()
}

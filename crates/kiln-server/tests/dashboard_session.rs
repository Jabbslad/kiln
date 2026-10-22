use std::{
    os::unix::fs::MetadataExt,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use axum::{
    Json, Router,
    body::{Body, Bytes, to_bytes},
    extract::Path as AxumPath,
    http::{HeaderMap, Request, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{any, delete, get, post},
};
use base64::Engine;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use kiln_api::auth::{
    AccessToken, DeviceAuthorization, DeviceTokens, MemberRole, Scope, ServerClaims,
};
use kiln_server::{
    auth::{Authenticator, Enrollment},
    door::{self, Dashboard},
};
use p256::{elliptic_curve::sec1::ToEncodedPoint, pkcs8::EncodePrivateKey};
use tower::ServiceExt;

struct Seen {
    path: String,
    authorization: Option<String>,
    cookie: Option<String>,
}

#[derive(Clone)]
struct Identity {
    port: u16,
    approved: Arc<AtomicBool>,
    wrong: Arc<AtomicBool>,
    form: Arc<Mutex<String>>,
    events: Arc<Mutex<Vec<String>>>,
    revoke_auth: Arc<Mutex<Option<String>>>,
    revoke_path: Arc<Mutex<Option<String>>>,
    central: String,
    refresh: String,
    jwt: String,
    device_id: String,
    user_code: String,
    device_code: String,
}

struct World {
    _tmp: tempfile::TempDir,
    router: Router,
    hits: Arc<AtomicUsize>,
    seen: Arc<Mutex<Vec<Seen>>>,
    port: u16,
    identity: Identity,
    identity_server: axum_server::Handle<std::net::SocketAddr>,
    key: EncodingKey,
    issuer: String,
    session_dir: std::path::PathBuf,
}

#[tokio::test]
async fn missing_bogus_and_admin_credentials_do_not_dial() {
    let world = world(false).await;
    let before = sockets(world.port);
    let admin = format!("Bearer {}", "ab".repeat(32));
    for (label, cookie, authorization) in [
        ("no-cookie", None, None),
        ("bogus-cookie", Some("__Host-kiln_dash=not-a-session"), None),
        ("admin", None, Some(admin.as_str())),
        ("bogus-bearer", None, Some("Bearer not-a-jwt")),
    ] {
        let response = world
            .call(req("GET", "/api/v1/dashboard", cookie, authorization, None))
            .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{label}");
        assert_eq!(world.hits.load(Ordering::SeqCst), 0, "{label}");
        assert_eq!(sockets(world.port), before, "{label}");
    }
    let response = world
        .call(req(
            "POST",
            "/v1/operations",
            None,
            Some(admin.as_str()),
            Some(b"{}".as_slice()),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(world.hits.load(Ordering::SeqCst), 0);
    let response = world
        .call(req("GET", "/login", None, Some(admin.as_str()), None))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(world.identity.events.lock().unwrap().is_empty());
    let response = world
        .call(
            Request::builder()
                .method("GET")
                .uri("/api/v1/dashboard")
                .header(header::UPGRADE, "websocket")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", mint(&world, "owner-a")),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(world.hits.load(Ordering::SeqCst), 0);
    assert_eq!(sockets(world.port), before);
}

#[tokio::test]
async fn device_grant_strips_browser_authorization() {
    let world = world(true).await;
    let admin = format!("Bearer {}", "ab".repeat(32));
    let response = world.call(req("GET", "/login", None, None, None)).await;
    assert_eq!(response.status(), StatusCode::FOUND);
    let location = header_str(&response, header::LOCATION);
    assert!(location.contains("user_code=CODE"));
    assert!(!location.contains(&world.identity.device_code));
    let attempt = set_cookie(&response, "__Host-kiln_dash_attempt");
    assert!(attempt.contains("Secure"));
    assert!(attempt.contains("HttpOnly"));
    assert!(attempt.contains("SameSite=Lax"));
    assert!(attempt.contains("Path=/"));
    assert!(!attempt.to_ascii_lowercase().contains("domain="));
    let form = world.identity.form.lock().unwrap().clone();
    assert!(form.contains("client_id=kiln-cli"));
    assert!(form.contains("device_name=kiln-dashboard"));
    let attempt_value = cookie_pair(&attempt).1;

    let response = world
        .call(req(
            "GET",
            "/login/wait",
            Some(&format!("__Host-kiln_dash_attempt={attempt_value}")),
            None,
            None,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(world.hits.load(Ordering::SeqCst), 0);
    assert!(
        set_cookies(&response)
            .iter()
            .all(|c| !c.starts_with("__Host-kiln_dash="))
    );

    world.identity.approved.store(true, Ordering::SeqCst);
    let response = world
        .call(req(
            "GET",
            "/login/wait",
            Some(&format!("__Host-kiln_dash_attempt={attempt_value}")),
            Some(admin.as_str()),
            None,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let dash = set_cookie(&response, "__Host-kiln_dash");
    let ready = body_bytes(response).await;
    let ready_text = String::from_utf8(ready.to_vec()).unwrap();
    assert_eq!(ready_text, r#"{"status":"ready"}"#);
    assert!(!ready_text.contains(&world.identity.jwt));
    assert!(!ready_text.contains(&world.identity.refresh));
    assert!(!ready_text.contains(&world.identity.central));
    assert!(dash.contains("Secure"));
    assert!(dash.contains("HttpOnly"));
    assert!(dash.contains("SameSite=Lax"));
    assert!(dash.contains("Path=/"));
    assert!(dash.contains("Max-Age=43200"));
    assert!(!dash.to_ascii_lowercase().contains("domain="));
    assert!(!dash.contains(&world.identity.jwt));
    let session_id = cookie_pair(&dash).1;
    let cookie = format!("__Host-kiln_dash={session_id}");
    let file = session_file(&world.session_dir);
    let meta = std::fs::metadata(&file).unwrap();
    assert_eq!(meta.mode() & 0o777, 0o600);
    assert!(meta.file_type().is_file());
    assert_eq!(meta.uid(), unsafe { libc::geteuid() });
    let stored = std::fs::read_to_string(&file).unwrap();
    assert!(stored.contains(&world.identity.refresh));
    assert!(!stored.contains(&world.identity.jwt));

    let response = world
        .call(req(
            "GET",
            "/api/v1/session",
            Some(&cookie),
            Some(admin.as_str()),
            None,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(world.hits.load(Ordering::SeqCst), 0);
    let csrf_body: serde_json::Value = serde_json::from_slice(&body_bytes(response).await).unwrap();
    let csrf = csrf_body["csrf"].as_str().unwrap().to_owned();
    assert_eq!(csrf_body.as_object().unwrap().len(), 1);

    let response = world
        .call(req(
            "GET",
            "/api/v1/dashboard",
            Some(&cookie),
            Some(admin.as_str()),
            None,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        body_bytes(response).await.as_ref(),
        br#"{"schema_version":1}"#
    );
    let hit = last_seen(&world);
    assert_eq!(hit.path, "/api/v1/dashboard");
    assert_eq!(
        hit.authorization.as_deref(),
        Some(format!("Bearer {}", world.identity.jwt).as_str())
    );
    assert_ne!(hit.authorization.as_deref(), Some(admin.as_str()));
    assert!(hit.cookie.is_none());

    let hits = world.hits.load(Ordering::SeqCst);
    let response = world
        .call(req(
            "POST",
            "/v1/operations",
            Some(&cookie),
            None,
            Some(br#"{"type":"create"}"#),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(world.hits.load(Ordering::SeqCst), hits);
    let response = world.call(post_op(&cookie, Some(&csrf), &admin)).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(world.hits.load(Ordering::SeqCst), hits);
    let cli = format!("Bearer {}", mint(&world, "owner-a"));
    let response = world
        .call(req(
            "POST",
            "/v1/operations",
            Some(&cookie),
            Some(&cli),
            Some(br#"{"type":"create"}"#),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let hit = last_seen(&world);
    assert_eq!(hit.path, "/v1/operations");
    assert_eq!(hit.authorization.as_deref(), Some(cli.as_str()));
    assert!(hit.cookie.is_none());

    let op = "a".repeat(32);
    let hits = world.hits.load(Ordering::SeqCst);
    let response = world
        .call(req(
            "GET",
            &format!("/v1/operations/{op}"),
            Some(&cookie),
            None,
            None,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(world.hits.load(Ordering::SeqCst), hits);

    let response = world
        .call(req("GET", "/v1/templates", Some(&cookie), None, None))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let after_cookie_only = world.hits.load(Ordering::SeqCst);
    let response = world
        .call(req("GET", "/v1/templates", None, Some(&cli), None))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let hit = last_seen(&world);
    assert_eq!(hit.path, "/v1/templates");
    assert_eq!(hit.authorization.as_deref(), Some(cli.as_str()));
    assert!(after_cookie_only < world.hits.load(Ordering::SeqCst));

    let response = world
        .call(
            Request::builder()
                .method("POST")
                .uri("/logout")
                .header(header::HOST, "cli.test")
                .header(header::ORIGIN, "https://cli.test")
                .header(header::COOKIE, &cookie)
                .header("x-kiln-csrf", &csrf)
                .header(header::AUTHORIZATION, &admin)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let cleared = set_cookie(&response, "__Host-kiln_dash");
    assert!(cleared.contains("Max-Age=0"));
    assert!(
        !set_cookies(&response)
            .iter()
            .any(|c| c.contains("__Host-kiln_session"))
    );
    assert_eq!(
        world.identity.revoke_path.lock().unwrap().as_deref(),
        Some(world.identity.device_id.as_str())
    );
    assert_eq!(
        world.identity.revoke_auth.lock().unwrap().as_deref(),
        Some(format!("Bearer {}", world.identity.central).as_str())
    );
    assert!(
        std::fs::read_dir(&world.session_dir)
            .unwrap()
            .next()
            .is_none()
    );
    let hits = world.hits.load(Ordering::SeqCst);
    let response = world
        .call(req("GET", "/api/v1/dashboard", Some(&cookie), None, None))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(world.hits.load(Ordering::SeqCst), hits);
}

#[tokio::test]
async fn logout_clears_the_session_when_identity_is_down() {
    let world = world(false).await;
    let (cookie, csrf) = dashboard_cookie(&world).await;
    world.identity_server.shutdown();
    wait_until_identity_stops(&world).await;
    let hits = world.hits.load(Ordering::SeqCst);
    let response = world
        .call(
            Request::builder()
                .method("POST")
                .uri("/logout")
                .header(header::HOST, "cli.test")
                .header(header::ORIGIN, "https://cli.test")
                .header(header::COOKIE, &cookie)
                .header("x-kiln-csrf", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert!(set_cookie(&response, "__Host-kiln_dash").contains("Max-Age=0"));
    assert!(
        std::fs::read_dir(&world.session_dir)
            .unwrap()
            .next()
            .is_none()
    );
    let response = world
        .call(req("GET", "/api/v1/dashboard", Some(&cookie), None, None))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(world.hits.load(Ordering::SeqCst), hits);
}

#[tokio::test]
async fn wrong_account_does_not_open_a_session() {
    let world = world(true).await;
    world.identity.wrong.store(true, Ordering::SeqCst);
    world.identity.approved.store(true, Ordering::SeqCst);
    let response = world.call(req("GET", "/login", None, None, None)).await;
    let attempt = cookie_pair(&set_cookie(&response, "__Host-kiln_dash_attempt")).1;
    let response = world
        .call(req(
            "GET",
            "/login/wait",
            Some(&format!("__Host-kiln_dash_attempt={attempt}")),
            None,
            None,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(
        set_cookies(&response)
            .iter()
            .all(|c| !c.starts_with("__Host-kiln_dash="))
    );
    assert!(
        std::fs::read_dir(&world.session_dir)
            .unwrap()
            .next()
            .is_none()
    );
    assert_eq!(world.hits.load(Ordering::SeqCst), 0);
    assert_eq!(
        world.identity.revoke_path.lock().unwrap().as_deref(),
        Some(world.identity.device_id.as_str())
    );
}

#[tokio::test]
async fn preview_document_does_not_replace_api_paths() {
    let world = world(true).await;
    std::fs::create_dir_all(world.session_dir.parent().unwrap().join("dash/assets")).unwrap();
    let root = world.session_dir.parent().unwrap().join("dash");
    std::fs::write(root.join("index.html"), b"<p>preview</p>").unwrap();
    std::fs::write(root.join("assets/app.js"), b"console.log(1)").unwrap();
    let response = world
        .call(req("GET", "/boxes?preview=1", None, None, None))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header_str(&response, header::CACHE_CONTROL), "no-store");
    assert!(header_str(&response, header::CONTENT_SECURITY_POLICY).contains("default-src 'self'"));
    assert_eq!(body_bytes(response).await.as_ref(), b"<p>preview</p>");
    assert_eq!(world.hits.load(Ordering::SeqCst), 0);
    let response = world
        .call(req("GET", "/assets/app.js", None, None, None))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header_str(&response, header::CACHE_CONTROL),
        "public, max-age=31536000, immutable"
    );
    let response = world
        .call(req(
            "GET",
            "/boxes?preview=1",
            None,
            Some("Bearer not-a-jwt"),
            None,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(world.hits.load(Ordering::SeqCst), 0);
    let cli = format!("Bearer {}", mint(&world, "owner-a"));
    let response = world
        .call(req("GET", "/api/v1/missing", None, Some(&cli), None))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(header_str(&response, header::CONTENT_TYPE).contains("application/json"));
    let bytes = body_bytes(response).await;
    assert!(
        !bytes
            .windows(b"<p>preview</p>".len())
            .any(|w| w == b"<p>preview</p>")
    );

    let bare = world_without_root().await;
    let response = bare
        .call(req("GET", "/boxes?preview=1", None, None, None))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(bare.hits.load(Ordering::SeqCst), 0);
}

async fn world(with_root: bool) -> World {
    let tmp = tempfile::tempdir().unwrap();
    let (identity_listener, identity_port) = bind().await;
    let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
    let identity_ca = cert.cert.pem();
    let issuer = format!("https://127.0.0.1:{identity_port}/");
    let (key, jwks) = signing_key();
    let jwt = mint_with(&key, &issuer, "owner-a");
    let wrong_jwt = mint_with(&key, &issuer, "owner-b");
    let identity = Identity {
        port: identity_port,
        approved: Arc::new(AtomicBool::new(false)),
        wrong: Arc::new(AtomicBool::new(false)),
        form: Arc::new(Mutex::new(String::new())),
        events: Arc::new(Mutex::new(Vec::new())),
        revoke_auth: Arc::new(Mutex::new(None)),
        revoke_path: Arc::new(Mutex::new(None)),
        central: "central-token".into(),
        refresh: "refresh-token".into(),
        jwt: jwt.clone(),
        device_id: "dev_dashboard".into(),
        user_code: "CODE".into(),
        device_code: "device-secret".into(),
    };
    let identity_server = spawn_tls(
        identity_listener,
        cert.cert.pem(),
        cert.signing_key.serialize_pem(),
        identity_router(identity.clone(), wrong_jwt),
    )
    .await;
    let hits = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (upstream_listener, port) = bind().await;
    let upstream_cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
    let ca_pem = upstream_cert.cert.pem();
    let _ = spawn_tls(
        upstream_listener,
        upstream_cert.cert.pem(),
        upstream_cert.signing_key.serialize_pem(),
        upstream_router(hits.clone(), seen.clone()),
    )
    .await;
    let session_dir = tmp.path().join("sessions");
    let dash_root = tmp.path().join("dash");
    std::fs::create_dir_all(&dash_root).unwrap();
    let enrollment = Enrollment {
        version: 1,
        issuer: issuer.clone(),
        owner_id: "owner-a".into(),
        server_id: "server-b".into(),
    };
    let cache = tmp.path().join("jwks.json");
    std::fs::write(
        &cache,
        serde_json::to_vec(&serde_json::json!({"issuer": enrollment.issuer, "jwks": jwks}))
            .unwrap(),
    )
    .unwrap();
    let auth = Authenticator::central_only(enrollment.clone(), cache).unwrap();
    let router = door::router_with_dashboard(
        auth,
        &format!("https://127.0.0.1:{port}"),
        ca_pem.as_bytes(),
        Dashboard {
            enrollment,
            session_dir: session_dir.clone(),
            root: with_root.then_some(dash_root),
            identity_ca: Some(identity_ca.into_bytes()),
        },
    )
    .unwrap();
    World {
        _tmp: tmp,
        router,
        hits,
        seen,
        port,
        identity,
        identity_server,
        key,
        issuer,
        session_dir,
    }
}

async fn world_without_root() -> World {
    world(false).await
}

fn identity_router(identity: Identity, wrong_jwt: String) -> Router {
    let wrong = identity.wrong.clone();
    let jwt = identity.jwt.clone();
    Router::new()
        .route(
            "/oauth/device/code",
            post({
                let identity = identity.clone();
                move |body: Bytes| {
                    let identity = identity.clone();
                    async move {
                        *identity.form.lock().unwrap() = String::from_utf8(body.to_vec()).unwrap();
                        identity.events.lock().unwrap().push("device".into());
                        Json(DeviceAuthorization {
                            device_code: identity.device_code.clone(),
                            user_code: identity.user_code.clone(),
                            verification_uri: format!("https://127.0.0.1:{}/device", identity.port),
                            verification_uri_complete: format!(
                                "https://127.0.0.1:{}/device?user_code={}",
                                identity.port, identity.user_code
                            ),
                            expires_in: 600,
                            interval: 5,
                        })
                    }
                }
            }),
        )
        .route(
            "/oauth/token",
            post({
                let identity = identity.clone();
                move || {
                    let identity = identity.clone();
                    async move {
                        identity.events.lock().unwrap().push("token".into());
                        if !identity.approved.load(Ordering::SeqCst) {
                            return (
                                StatusCode::BAD_REQUEST,
                                Json(serde_json::json!({"error": "authorization_pending"})),
                            )
                                .into_response();
                        }
                        Json(DeviceTokens {
                            access_token: identity.central.clone(),
                            token_type: "Bearer".into(),
                            expires_in: 300,
                            refresh_token: identity.refresh.clone(),
                        })
                        .into_response()
                    }
                }
            }),
        )
        .route(
            "/v1/me",
            get({
                let identity = identity.clone();
                move |headers: HeaderMap| {
                    let identity = identity.clone();
                    async move {
                        identity.events.lock().unwrap().push("me".into());
                        if presented_token(&headers) != identity.central {
                            return StatusCode::UNAUTHORIZED.into_response();
                        }
                        Json(serde_json::json!({
                            "account_id": "owner-a",
                            "device_id": identity.device_id,
                        }))
                        .into_response()
                    }
                }
            }),
        )
        .route(
            "/v1/access",
            post({
                let identity = identity.clone();
                move |headers: HeaderMap| {
                    let identity = identity.clone();
                    async move {
                        identity.events.lock().unwrap().push("access".into());
                        if presented_token(&headers) != identity.central {
                            return StatusCode::UNAUTHORIZED.into_response();
                        }
                        let access_token = if wrong.load(Ordering::SeqCst) {
                            wrong_jwt.clone()
                        } else {
                            jwt.clone()
                        };
                        Json(AccessToken {
                            access_token,
                            token_type: "Bearer".into(),
                            expires_in: 300,
                        })
                        .into_response()
                    }
                }
            }),
        )
        .route(
            "/v1/devices/{id}",
            delete({
                let identity = identity.clone();
                move |headers: HeaderMap, AxumPath(id): AxumPath<String>| {
                    let identity = identity.clone();
                    async move {
                        identity.events.lock().unwrap().push("revoke".into());
                        *identity.revoke_auth.lock().unwrap() = headers
                            .get(header::AUTHORIZATION)
                            .and_then(|value| value.to_str().ok())
                            .map(str::to_owned);
                        *identity.revoke_path.lock().unwrap() = Some(id);
                        StatusCode::NO_CONTENT
                    }
                }
            }),
        )
}

fn upstream_router(hits: Arc<AtomicUsize>, seen: Arc<Mutex<Vec<Seen>>>) -> Router {
    Router::new().fallback(any(move |request: Request<Body>| {
        let hits = hits.clone();
        let seen = seen.clone();
        async move {
            hits.fetch_add(1, Ordering::SeqCst);
            let path = request.uri().path().to_owned();
            let authorization = request
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let cookie = request
                .headers()
                .get(header::COOKIE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            seen.lock().unwrap().push(Seen {
                path: path.clone(),
                authorization,
                cookie,
            });
            if path == "/api/v1/dashboard" {
                Json(serde_json::json!({"schema_version": 1})).into_response()
            } else if path.starts_with("/api/") {
                (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({"code": "not_found"})),
                )
                    .into_response()
            } else {
                (StatusCode::OK, "ok").into_response()
            }
        }
    }))
}

impl World {
    async fn call(&self, request: Request<Body>) -> Response {
        self.router.clone().oneshot(request).await.unwrap()
    }
}

fn req(
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    authorization: Option<&str>,
    body: Option<&[u8]>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    if let Some(authorization) = authorization {
        builder = builder.header(header::AUTHORIZATION, authorization);
    }
    builder
        .body(match body {
            Some(bytes) => Body::from(Bytes::copy_from_slice(bytes)),
            None => Body::empty(),
        })
        .unwrap()
}

fn post_op(cookie: &str, csrf: Option<&str>, admin: &str) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/v1/operations")
        .header(header::HOST, "cli.test")
        .header(header::ORIGIN, "https://cli.test")
        .header(header::COOKIE, cookie)
        .header(header::AUTHORIZATION, admin)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(csrf) = csrf {
        builder = builder.header("x-kiln-csrf", csrf);
    }
    builder.body(Body::from(b"{}".as_slice())).unwrap()
}

fn mint(world: &World, sub: &str) -> String {
    mint_with(&world.key, &world.issuer, sub)
}

fn mint_with(key: &EncodingKey, issuer: &str, sub: &str) -> String {
    let now = jsonwebtoken::get_current_timestamp();
    let claims = ServerClaims {
        iss: issuer.into(),
        sub: sub.into(),
        aud: "kiln:server:server-b".into(),
        tenant_id: "ten_test".into(),
        iat: now,
        nbf: now,
        exp: now + 300,
        credential_id: "dev_dashboard".into(),
        scope: Scope::Operate,
        role: MemberRole::Owner,
    };
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("first".into());
    jsonwebtoken::encode(&header, &claims, key).unwrap()
}

fn signing_key() -> (EncodingKey, serde_json::Value) {
    let key = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
    let pem = key.to_pkcs8_pem(Default::default()).unwrap();
    let point = key.public_key().to_encoded_point(false);
    let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let jwks = serde_json::json!({"keys":[{
        "kty":"EC","crv":"P-256","use":"sig","alg":"ES256","kid":"first",
        "x":enc.encode(point.x().unwrap()),"y":enc.encode(point.y().unwrap())
    }]});
    (EncodingKey::from_ec_pem(pem.as_bytes()).unwrap(), jwks)
}

fn session_file(dir: &Path) -> std::path::PathBuf {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1);
    files.pop().unwrap()
}

fn last_seen(world: &World) -> Seen {
    world.seen.lock().unwrap().pop().unwrap()
}

fn header_str(response: &Response, name: axum::http::HeaderName) -> String {
    response
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_owned()
}

fn set_cookies(response: &Response) -> Vec<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .collect()
}

fn set_cookie(response: &Response, name: &str) -> String {
    set_cookies(response)
        .into_iter()
        .find(|cookie| cookie.starts_with(&format!("{name}=")))
        .unwrap()
}

fn cookie_pair(cookie: &str) -> (String, String) {
    let (pair, _) = cookie.split_once(';').unwrap();
    let (name, value) = pair.split_once('=').unwrap();
    (name.to_owned(), value.to_owned())
}

fn presented_token(headers: &HeaderMap) -> String {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("")
        .to_owned()
}

async fn body_bytes(response: Response) -> Bytes {
    to_bytes(response.into_body(), 1024 * 1024).await.unwrap()
}

async fn bind() -> (std::net::TcpListener, u16) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

async fn spawn_tls(
    listener: std::net::TcpListener,
    cert_pem: String,
    key_pem: String,
    app: Router,
) -> axum_server::Handle<std::net::SocketAddr> {
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        cert_pem.into_bytes(),
        key_pem.into_bytes(),
    )
    .await
    .unwrap();
    let handle = axum_server::Handle::<std::net::SocketAddr>::new();
    let server = handle.clone();
    tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, tls)
            .unwrap()
            .handle(server)
            .serve(app.into_make_service())
            .await
            .unwrap();
    });
    handle
}

async fn dashboard_cookie(world: &World) -> (String, String) {
    let response = world.call(req("GET", "/login", None, None, None)).await;
    let attempt = cookie_pair(&set_cookie(&response, "__Host-kiln_dash_attempt")).1;
    let response = world
        .call(req(
            "GET",
            "/login/wait",
            Some(&format!("__Host-kiln_dash_attempt={attempt}")),
            None,
            None,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    world.identity.approved.store(true, Ordering::SeqCst);
    let response = world
        .call(req(
            "GET",
            "/login/wait",
            Some(&format!("__Host-kiln_dash_attempt={attempt}")),
            None,
            None,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = format!(
        "__Host-kiln_dash={}",
        cookie_pair(&set_cookie(&response, "__Host-kiln_dash")).1
    );
    let response = world
        .call(req("GET", "/api/v1/session", Some(&cookie), None, None))
        .await;
    let body: serde_json::Value = serde_json::from_slice(&body_bytes(response).await).unwrap();
    (cookie, body["csrf"].as_str().unwrap().to_owned())
}

async fn wait_until_identity_stops(world: &World) {
    let addr = format!("127.0.0.1:{}", world.identity.port);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let refused = matches!(
            tokio::time::timeout(
                std::time::Duration::from_millis(50),
                tokio::net::TcpStream::connect(&addr),
            )
            .await,
            Ok(Err(_))
        );
        if refused && world.identity_server.connection_count() == 0 {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "identity kept accepting"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

fn sockets(port: u16) -> usize {
    let marker = format!(":{port:04X}");
    std::fs::read_to_string("/proc/net/tcp")
        .unwrap()
        .lines()
        .filter(|line| line.contains(&marker))
        .count()
}

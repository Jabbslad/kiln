//! HTTP listener on 127.0.0.1:8093.
//! It serves the dashboard archive and proxies one read route to loopback kiln-api.
//! The administrator token is not a central JWT and is not forwarded.

use crate::{
    Failure,
    auth::{Authenticator, Enrollment},
};
use anyhow::{Context, Result, ensure};
use axum::{
    Router,
    body::Body,
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::any,
};
use base64::Engine;
use futures_util::StreamExt;
use kiln_api::auth::Scope;
use sha2::Digest;
use std::{
    collections::HashMap,
    fs,
    io::Read,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, Instant},
};

pub const BIND: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8093);

const LIMIT: u32 = 60;
const LOGIN_LIMIT: u32 = 10;
const WINDOW: Duration = Duration::from_secs(60);
const SESSION_SECONDS: u64 = 43200;
const ATTEMPT_SECONDS: u64 = 600;
const STATUS_INTERVAL: Duration = Duration::from_secs(300);
const STATIC_LIMIT: u64 = 8 * 1024 * 1024;
const UPSTREAM_READ: Duration = Duration::from_secs(10);
const CSP: &str = "default-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'";

pub struct Options {
    pub listen: SocketAddr,
    pub upstream: String,
    pub upstream_ca: PathBuf,
    pub enrollment_file: PathBuf,
    pub jwks_cache: PathBuf,
    pub dashboard_root: Option<PathBuf>,
}

#[derive(Clone)]
pub struct SignIn {
    pub authorize_origin: String,
    pub loopback: String,
    pub login_secret: String,
    pub server_id: String,
}

struct Attempt {
    verifier: String,
    state: String,
    expires: Instant,
}

struct Session {
    expires: Instant,
    csrf: String,
    reader: String,
    jwt: String,
    jwt_exp: u64,
}

struct App {
    auth: Authenticator,
    client: reqwest::Client,
    identity: reqwest::Client,
    upstream: reqwest::Url,
    loopback: reqwest::Url,
    authorize_origin: String,
    login_secret: String,
    server_id: String,
    root: Option<PathBuf>,
    hits: StdMutex<HashMap<IpAddr, Hit>>,
    login_hits: StdMutex<HashMap<IpAddr, Hit>>,
    attempts: StdMutex<HashMap<String, Attempt>>,
    sessions: StdMutex<HashMap<String, Session>>,
    status_at: StdMutex<Option<Instant>>,
}

struct Hit {
    start: Instant,
    count: u32,
}

enum Kind {
    Dashboard,
    MissingApi,
    Login,
    Callback,
    Logout,
    Page,
    Other,
}

enum Found {
    File(Vec<u8>),
    TooLarge,
    Missing,
}

pub fn require_bind(listen: SocketAddr, euid: u32) -> Result<()> {
    ensure!(listen == BIND, "app listener must bind 127.0.0.1:8093");
    ensure!(euid != 0, "app listener must run as an unprivileged user");
    Ok(())
}

pub fn listen_line(listen: SocketAddr) -> String {
    format!("app listener listening on {listen}")
}

pub fn router(
    auth: Authenticator,
    upstream: &str,
    ca_pem: &[u8],
    root: Option<&Path>,
) -> Result<Router> {
    router_with(
        auth,
        upstream,
        ca_pem,
        root,
        SignIn {
            authorize_origin: "https://dark-forge.dev".into(),
            loopback: "http://127.0.0.1:9".into(),
            login_secret: String::new(),
            server_id: String::new(),
        },
    )
}

pub fn router_with(
    auth: Authenticator,
    upstream: &str,
    ca_pem: &[u8],
    root: Option<&Path>,
    sign_in: SignIn,
) -> Result<Router> {
    let upstream = loopback_upstream(upstream)?;
    // The login secret is posted here, on the host, and never to the public identity name.
    let loopback = loopback_identity(&sign_in.loopback)?;
    let client = reqwest::Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(3))
        .read_timeout(UPSTREAM_READ)
        .add_root_certificate(reqwest::Certificate::from_pem(ca_pem)?)
        .tls_built_in_root_certs(false)
        .build()?;
    let identity = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(10))
        .build()?;
    let state = Arc::new(App {
        auth,
        client,
        identity,
        upstream,
        loopback,
        authorize_origin: sign_in.authorize_origin.trim_end_matches('/').to_owned(),
        login_secret: sign_in.login_secret,
        server_id: sign_in.server_id,
        root: root.map(Path::to_path_buf),
        hits: StdMutex::new(HashMap::new()),
        login_hits: StdMutex::new(HashMap::new()),
        attempts: StdMutex::new(HashMap::new()),
        sessions: StdMutex::new(HashMap::new()),
        status_at: StdMutex::new(None),
    });
    Ok(into_router(state))
}

fn into_router(state: Arc<App>) -> Router {
    Router::new().fallback(any(handle)).with_state(state)
}

/// Insert one read session so proxy limits can be tested without a browser login.
#[doc(hidden)]
pub fn router_with_read_session(
    auth: Authenticator,
    upstream: &str,
    ca_pem: &[u8],
    root: Option<&Path>,
    jwt: &str,
) -> Result<(Router, String)> {
    let upstream_url = loopback_upstream(upstream)?;
    let loopback = loopback_identity("http://127.0.0.1:9")?;
    let client = reqwest::Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(3))
        .read_timeout(UPSTREAM_READ)
        .add_root_certificate(reqwest::Certificate::from_pem(ca_pem)?)
        .tls_built_in_root_certs(false)
        .build()?;
    let identity = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(10))
        .build()?;
    let id = random_token();
    let mut sessions = HashMap::new();
    sessions.insert(
        id.clone(),
        Session {
            expires: Instant::now() + Duration::from_secs(SESSION_SECONDS),
            csrf: random_token(),
            reader: String::new(),
            jwt: jwt.to_owned(),
            jwt_exp: jsonwebtoken::get_current_timestamp().saturating_add(240),
        },
    );
    let state = Arc::new(App {
        auth,
        client,
        identity,
        upstream: upstream_url,
        loopback,
        authorize_origin: "https://dark-forge.dev".into(),
        login_secret: String::new(),
        server_id: String::new(),
        root: root.map(Path::to_path_buf),
        hits: StdMutex::new(HashMap::new()),
        login_hits: StdMutex::new(HashMap::new()),
        attempts: StdMutex::new(HashMap::new()),
        sessions: StdMutex::new(sessions),
        status_at: StdMutex::new(Some(Instant::now())),
    });
    Ok((into_router(state), id))
}

pub async fn run(options: Options) -> Result<()> {
    require_bind(options.listen, euid())?;
    let enrollment =
        Enrollment::load(&options.enrollment_file)?.context("server is not enrolled")?;
    let server_id = enrollment.server_id.clone();
    let authorize_origin = enrollment.issuer.clone();
    let auth = Authenticator::central_only(enrollment, options.jwks_cache)?;
    let ca = fs::read(&options.upstream_ca).context("upstream CA required")?;
    let login_secret = read_login_secret(Path::new("/etc/kiln/login.token"))?;
    let router = router_with(
        auth,
        &options.upstream,
        &ca,
        options.dashboard_root.as_deref(),
        SignIn {
            authorize_origin,
            loopback: "http://127.0.0.1:8091".into(),
            login_secret,
            server_id,
        },
    )?;
    listen(router, options.listen).await
}

pub async fn listen(router: Router, listen: SocketAddr) -> Result<()> {
    require_bind(listen, euid())?;
    let listener = tokio::net::TcpListener::bind(listen).await?;
    eprintln!("{}", listen_line(listen));
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

fn euid() -> u32 {
    unsafe { libc::geteuid() }
}

fn loopback_upstream(upstream: &str) -> Result<reqwest::Url> {
    let upstream = reqwest::Url::parse(upstream).context("invalid upstream URL")?;
    ensure!(
        upstream.scheme() == "https"
            && upstream.host_str() == Some("127.0.0.1")
            && upstream.username().is_empty()
            && upstream.password().is_none()
            && matches!(upstream.path(), "" | "/")
            && upstream.query().is_none()
            && upstream.fragment().is_none(),
        "app listener upstream must be https://127.0.0.1"
    );
    Ok(upstream)
}

async fn handle(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    finish(dispatch(&app, peer, request).await)
}

async fn dispatch(app: &App, peer: SocketAddr, request: Request) -> Response {
    let ip = client_ip(peer, request.headers());
    let kind = classify(request.method(), request.uri().path());
    let limited = match kind {
        Kind::Login | Kind::Callback => charge(&app.login_hits, ip, LOGIN_LIMIT),
        _ => charge(&app.hits, ip, LIMIT),
    };
    if !limited {
        return deny(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Too many requests from this address.",
        );
    }
    match kind {
        Kind::Dashboard => proxy(app, request).await,
        Kind::Login => start_login(app).await,
        Kind::Callback => finish_login(app, request).await,
        Kind::Logout => logout(app, request).await,
        Kind::MissingApi => api_miss(),
        Kind::Page => page(app, &request),
        Kind::Other => api_miss(),
    }
}

fn classify(method: &Method, path: &str) -> Kind {
    if path == "/api" || path.starts_with("/api/") {
        if method == Method::GET && path == "/api/v1/dashboard" {
            Kind::Dashboard
        } else {
            Kind::MissingApi
        }
    } else if method == Method::GET && path == "/login" {
        Kind::Login
    } else if method == Method::GET && path == "/oauth/callback" {
        Kind::Callback
    } else if method == Method::POST && path == "/logout" {
        Kind::Logout
    } else if method == Method::GET {
        Kind::Page
    } else {
        Kind::Other
    }
}

fn client_ip(peer: SocketAddr, headers: &HeaderMap) -> IpAddr {
    let direct = peer.ip();
    if direct.is_loopback()
        && let Some(ip) = headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(',').next())
            .and_then(|value| value.trim().parse().ok())
    {
        return ip;
    }
    direct
}

fn charge(hits: &StdMutex<HashMap<IpAddr, Hit>>, ip: IpAddr, limit: u32) -> bool {
    let mut map = lock_map(hits);
    let now = Instant::now();
    if map.len() > 256 {
        map.retain(|_, hit| now.duration_since(hit.start) < WINDOW);
    }
    let hit = map.entry(ip).or_insert(Hit {
        start: now,
        count: 0,
    });
    if now.duration_since(hit.start) >= WINDOW {
        *hit = Hit {
            start: now,
            count: 0,
        };
    }
    if hit.count >= limit {
        return false;
    }
    hit.count += 1;
    true
}

async fn proxy(app: &App, request: Request) -> Response {
    if request.uri().query().is_some() {
        return deny(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Query parameters are not accepted.",
        );
    }
    let Some(raw) = cookie_value(request.headers(), "__Host-kiln_app") else {
        return unauthorized();
    };
    refresh_status(app).await;
    let Some(jwt) = fresh_jwt(app, &raw).await else {
        return unauthorized();
    };
    let Ok(authorization) = HeaderValue::from_str(&format!("Bearer {jwt}")) else {
        return unauthorized();
    };
    match forward(app, authorization).await {
        Ok(response) => response,
        Err(failure) => nostore(failure.into_response()),
    }
}

async fn forward(app: &App, authorization: HeaderValue) -> Result<Response, Failure> {
    let mut url = app.upstream.clone();
    url.set_path("/api/v1/dashboard");
    let response = app
        .client
        .get(url)
        .header(header::AUTHORIZATION, authorization)
        .send()
        .await
        .map_err(|_| upstream_error())?;
    if response
        .content_length()
        .is_some_and(|len| len > kiln_api::MAX_RESPONSE_BYTES as u64)
    {
        return Err(upstream_error());
    }
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = response.headers().get(header::CONTENT_TYPE).cloned();
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| upstream_error())?;
        if body.len().saturating_add(chunk.len()) > kiln_api::MAX_RESPONSE_BYTES {
            return Err(upstream_error());
        }
        body.extend_from_slice(&chunk);
    }
    let mut response = Response::builder()
        .status(status)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    if let Some(content_type) = content_type {
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, content_type);
    }
    Ok(response)
}

fn page(app: &App, request: &Request) -> Response {
    let query = request.uri().query();
    if query.is_some() && query != Some("preview=1") {
        return deny(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Query parameters are not accepted.",
        );
    }
    let Some(root) = app.root.as_deref().and_then(canonical_dir) else {
        return api_miss();
    };
    let Some(rel) = safe_rel(request.uri().path()) else {
        return deny(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "The path is not valid.",
        );
    };
    match read_inside(&root, &rel) {
        Found::File(bytes) => static_body(&rel, &rel, bytes),
        Found::TooLarge => deny(
            StatusCode::PAYLOAD_TOO_LARGE,
            "body_too_large",
            "Dashboard file exceeds 8 MiB.",
        ),
        Found::Missing if rel.extension().is_none() => {
            match read_inside(&root, Path::new("index.html")) {
                Found::File(bytes) => static_body(Path::new("index.html"), &rel, bytes),
                Found::TooLarge => deny(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "body_too_large",
                    "Dashboard file exceeds 8 MiB.",
                ),
                Found::Missing => api_miss(),
            }
        }
        Found::Missing => api_miss(),
    }
}

fn canonical_dir(path: &Path) -> Option<PathBuf> {
    let canon = path.canonicalize().ok()?;
    let meta = fs::symlink_metadata(&canon).ok()?;
    (meta.is_dir() && !meta.file_type().is_symlink()).then_some(canon)
}

fn safe_rel(path: &str) -> Option<PathBuf> {
    let path = path.strip_prefix('/')?;
    if path.is_empty() {
        return Some(PathBuf::from("index.html"));
    }
    let mut rel = PathBuf::new();
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".."
            || segment.contains('\\')
            || segment.contains('\0')
            || segment.contains('%')
        {
            return None;
        }
        rel.push(segment);
    }
    if rel.as_os_str().is_empty() {
        Some(PathBuf::from("index.html"))
    } else {
        Some(rel)
    }
}

fn read_inside(root: &Path, rel: &Path) -> Found {
    let candidate = root.join(rel);
    let Ok(meta) = fs::symlink_metadata(&candidate) else {
        return Found::Missing;
    };
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Found::Missing;
    }
    if meta.len() > STATIC_LIMIT {
        return Found::TooLarge;
    }
    let Ok(canon) = candidate.canonicalize() else {
        return Found::Missing;
    };
    if !canon.starts_with(root) {
        return Found::Missing;
    }
    match fs::read(&canon) {
        Ok(bytes) if bytes.len() as u64 > STATIC_LIMIT => Found::TooLarge,
        Ok(bytes) => Found::File(bytes),
        Err(_) => Found::Missing,
    }
}

fn static_body(served: &Path, requested: &Path, bytes: Vec<u8>) -> Response {
    let cache = if served.starts_with("assets") && requested.starts_with("assets") {
        "public, max-age=31536000, immutable"
    } else {
        "no-store"
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type(served))
        .header(header::CACHE_CONTROL, cache)
        .body(Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("woff2") => "font/woff2",
        Some("ico") => "image/x-icon",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn api_miss() -> Response {
    nostore(Failure::missing().into_response())
}

fn finish(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("strict-origin"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

fn nostore(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn deny(status: StatusCode, code: &str, message: &str) -> Response {
    nostore(Failure::new(status, code, message).into_response())
}

fn unauthorized() -> Response {
    deny(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "A valid credential is required.",
    )
}

fn upstream_error() -> Failure {
    Failure::new(
        StatusCode::BAD_GATEWAY,
        "upstream",
        "The local API did not answer.",
    )
}

fn lock_map<T>(mutex: &StdMutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

fn loopback_identity(value: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(value).context("invalid identity URL")?;
    ensure!(
        url.scheme() == "http"
            && url.host_str() == Some("127.0.0.1")
            && url.port().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && matches!(url.path(), "" | "/")
            && url.query().is_none()
            && url.fragment().is_none(),
        "identity exchange must dial http://127.0.0.1"
    );
    Ok(url)
}

fn read_login_secret(path: &Path) -> Result<String> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(err) => return Err(err.into()),
    };
    use std::os::unix::fs::MetadataExt;
    ensure!(
        meta.is_file()
            && !meta.file_type().is_symlink()
            && meta.uid() == 0
            && meta.mode() & 0o007 == 0,
        "login token must be a root-owned file that is not world-readable"
    );
    let mut file = fs::File::open(path)?;
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    let token = text.trim();
    ensure!(
        !token.is_empty() && token.len() <= 128 && !token.contains(['\0', '\n', '\r']),
        "invalid login token"
    );
    Ok(token.to_owned())
}

fn random_token() -> String {
    let mut bytes = [0u8; 32];
    fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .expect("system random source");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn pkce_challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(sha2::Sha256::digest(verifier.as_bytes()))
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|part| {
            part.trim()
                .strip_prefix(&format!("{name}="))
                .map(str::to_owned)
        })
}

fn host_cookie(name: &str, value: &str, max_age: u64, http_only: bool) -> HeaderValue {
    let http_only = if http_only { "HttpOnly; " } else { "" };
    HeaderValue::from_str(&format!(
        "{name}={value}; Secure; {http_only}SameSite=Lax; Path=/; Max-Age={max_age}"
    ))
    .expect("cookie")
}

fn query_value(uri: &axum::http::Uri, name: &str) -> Option<String> {
    uri.query()?.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then(|| value.to_owned())
    })
}

fn form_value(body: &str, name: &str) -> Option<String> {
    body.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then(|| value.to_owned())
    })
}

async fn start_login(app: &App) -> Response {
    if app.login_secret.is_empty() {
        return deny(
            StatusCode::SERVICE_UNAVAILABLE,
            "login_unavailable",
            "Dashboard sign-in is not configured.",
        );
    }
    let verifier = random_token();
    let state = random_token();
    let id = random_token();
    let mut url = match reqwest::Url::parse(&format!("{}/oauth/server", app.authorize_origin)) {
        Ok(url) => url,
        Err(_) => {
            return deny(
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "Sign-in could not start.",
            );
        }
    };
    url.query_pairs_mut()
        .append_pair("redirect_uri", kiln_api::auth::APP_OAUTH_REDIRECT)
        .append_pair("state", &state)
        .append_pair("code_challenge", &pkce_challenge(&verifier))
        .append_pair("code_challenge_method", "S256");
    lock_map(&app.attempts).insert(
        id.clone(),
        Attempt {
            verifier,
            state,
            expires: Instant::now() + Duration::from_secs(ATTEMPT_SECONDS),
        },
    );
    let mut response = Redirect::to(url.as_str()).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        host_cookie("__Host-kiln_app_attempt", &id, ATTEMPT_SECONDS, true),
    );
    nostore(response)
}

async fn finish_login(app: &App, request: Request) -> Response {
    if app.login_secret.is_empty() {
        return deny(
            StatusCode::SERVICE_UNAVAILABLE,
            "login_unavailable",
            "Dashboard sign-in is not configured.",
        );
    }
    let Some(attempt_id) = cookie_value(request.headers(), "__Host-kiln_app_attempt") else {
        return deny(
            StatusCode::BAD_REQUEST,
            "invalid_state",
            "Sign-in did not start in this browser.",
        );
    };
    let attempt = {
        let mut attempts = lock_map(&app.attempts);
        let Some(attempt) = attempts.remove(&attempt_id) else {
            return deny(
                StatusCode::BAD_REQUEST,
                "invalid_state",
                "Sign-in did not start in this browser.",
            );
        };
        if attempt.expires <= Instant::now() {
            return deny(
                StatusCode::BAD_REQUEST,
                "invalid_state",
                "Sign-in expired. Start again.",
            );
        }
        attempt
    };
    let code = query_value(request.uri(), "code").unwrap_or_default();
    let state = query_value(request.uri(), "state").unwrap_or_default();
    if code.is_empty() || state != attempt.state {
        return deny(
            StatusCode::BAD_REQUEST,
            "invalid_state",
            "Sign-in did not match this browser.",
        );
    }
    let exchanged = exchange_code(app, &code, &attempt.verifier).await;
    let Ok((jwt, reader, expires_in)) = exchanged else {
        return deny(
            StatusCode::BAD_GATEWAY,
            "upstream",
            "Identity did not complete sign-in.",
        );
    };
    if !matches!(app.auth.authenticate_central(&jwt).await, Ok(Scope::Read)) {
        return deny(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Identity did not grant a read session.",
        );
    }
    let id = random_token();
    let csrf = random_token();
    let jwt_exp = jsonwebtoken::get_current_timestamp().saturating_add(expires_in);
    lock_map(&app.sessions).insert(
        id.clone(),
        Session {
            expires: Instant::now() + Duration::from_secs(SESSION_SECONDS),
            csrf: csrf.clone(),
            reader,
            jwt,
            jwt_exp,
        },
    );
    let mut response = Redirect::to("/boxes").into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::SET_COOKIE,
        host_cookie("__Host-kiln_app", &id, SESSION_SECONDS, true),
    );
    headers.append(
        header::SET_COOKIE,
        host_cookie("__Host-kiln_csrf", &csrf, SESSION_SECONDS, false),
    );
    headers.append(
        header::SET_COOKIE,
        host_cookie("__Host-kiln_app_attempt", "", 0, true),
    );
    nostore(response)
}

async fn exchange_code(app: &App, code: &str, verifier: &str) -> Result<(String, String, u64)> {
    let mut url = app.loopback.clone();
    url.set_path("/oauth/server/token");
    let response = app
        .identity
        .post(url)
        .bearer_auth(&app.login_secret)
        .json(&serde_json::json!({
            "code": code,
            "code_verifier": verifier,
            "redirect_uri": kiln_api::auth::APP_OAUTH_REDIRECT,
        }))
        .send()
        .await?
        .error_for_status()?;
    let body: serde_json::Value = response.json().await?;
    let jwt = body["access_token"]
        .as_str()
        .context("missing access token")?
        .to_owned();
    let reader = body["reader"]
        .as_str()
        .context("missing reader")?
        .to_owned();
    let expires_in = body["expires_in"].as_u64().context("missing expiry")?;
    ensure!(
        !jwt.is_empty() && jwt.len() <= 8192 && !reader.is_empty() && reader.len() <= 128,
        "invalid identity grant"
    );
    ensure!((1..=300).contains(&expires_in), "invalid identity grant");
    Ok((jwt, reader, expires_in))
}

async fn fresh_jwt(app: &App, raw: &str) -> Option<String> {
    let (jwt, jwt_exp, reader) = {
        let mut sessions = lock_map(&app.sessions);
        let session = sessions.get(raw)?;
        if session.expires <= Instant::now() {
            sessions.remove(raw);
            return None;
        }
        (session.jwt.clone(), session.jwt_exp, session.reader.clone())
    };
    let now = jsonwebtoken::get_current_timestamp();
    if jwt_exp > now.saturating_add(60) {
        return Some(jwt);
    }
    if reader.is_empty() {
        return (jwt_exp > now).then_some(jwt);
    }
    match refresh_reader(app, &reader).await {
        Ok((next, expires_in)) => {
            if let Some(session) = lock_map(&app.sessions).get_mut(raw) {
                session.jwt = next.clone();
                session.jwt_exp = now.saturating_add(expires_in);
            }
            Some(next)
        }
        Err(_) if jwt_exp > now => Some(jwt),
        Err(_) => None,
    }
}

async fn refresh_reader(app: &App, reader: &str) -> Result<(String, u64)> {
    let mut url = app.loopback.clone();
    url.set_path("/oauth/server/token");
    let response = app
        .identity
        .post(url)
        .json(&serde_json::json!({ "reader": reader }))
        .send()
        .await?
        .error_for_status()?;
    let body: serde_json::Value = response.json().await?;
    let jwt = body["access_token"]
        .as_str()
        .context("missing access token")?
        .to_owned();
    let expires_in = body["expires_in"].as_u64().unwrap_or(300);
    ensure!((1..=300).contains(&expires_in), "invalid identity grant");
    if !matches!(app.auth.authenticate_central(&jwt).await, Ok(Scope::Read)) {
        anyhow::bail!("refreshed token was not a read grant");
    }
    Ok((jwt, expires_in))
}

async fn refresh_status(app: &App) {
    if app.login_secret.is_empty() || app.server_id.is_empty() {
        return;
    }
    {
        let mut checked = lock_map(&app.status_at);
        if checked.is_some_and(|at| at.elapsed() < STATUS_INTERVAL) {
            return;
        }
        *checked = Some(Instant::now());
    }
    let mut url = app.loopback.clone();
    url.set_path("/v1/server-status");
    let Ok(response) = app
        .identity
        .post(url)
        .bearer_auth(&app.login_secret)
        .json(&serde_json::json!({ "server_id": app.server_id }))
        .send()
        .await
    else {
        return;
    };
    if !response.status().is_success() {
        return;
    }
    let Ok(body) = response.json::<serde_json::Value>().await else {
        return;
    };
    let account_active = body["account_active"].as_bool();
    let server_active = body["server_active"].as_bool();
    if account_active == Some(false) || server_active == Some(false) {
        lock_map(&app.sessions).clear();
    }
}

async fn logout(app: &App, request: Request) -> Response {
    if request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        != Some("https://app.dark-forge.dev")
    {
        return deny(
            StatusCode::FORBIDDEN,
            "invalid_origin",
            "Logout was refused.",
        );
    }
    let Some(raw) = cookie_value(request.headers(), "__Host-kiln_app") else {
        return unauthorized();
    };
    let Some(csrf_cookie) = cookie_value(request.headers(), "__Host-kiln_csrf") else {
        return deny(StatusCode::FORBIDDEN, "invalid_csrf", "Logout was refused.");
    };
    let body = axum::body::to_bytes(request.into_body(), 4096)
        .await
        .unwrap_or_default();
    let body = String::from_utf8_lossy(&body);
    let posted = form_value(&body, "csrf").unwrap_or_default();
    let removed = {
        let mut sessions = lock_map(&app.sessions);
        let Some(session) = sessions.get(&raw) else {
            return unauthorized();
        };
        if session.expires <= Instant::now() || session.csrf != csrf_cookie || posted != csrf_cookie
        {
            return deny(StatusCode::FORBIDDEN, "invalid_csrf", "Logout was refused.");
        }
        sessions.remove(&raw).is_some()
    };
    if !removed {
        return unauthorized();
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::SET_COOKIE,
        host_cookie("__Host-kiln_app", "", 0, true),
    );
    headers.append(
        header::SET_COOKIE,
        host_cookie("__Host-kiln_csrf", "", 0, false),
    );
    nostore(response)
}

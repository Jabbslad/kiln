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
    response::{IntoResponse, Response},
    routing::any,
};
use futures_util::StreamExt;
use std::{
    collections::HashMap,
    fs,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, Instant},
};

pub const BIND: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8093);

const LIMIT: u32 = 60;
const WINDOW: Duration = Duration::from_secs(60);
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

struct App {
    auth: Authenticator,
    client: reqwest::Client,
    upstream: reqwest::Url,
    root: Option<PathBuf>,
    hits: StdMutex<HashMap<IpAddr, Hit>>,
}

struct Hit {
    start: Instant,
    count: u32,
}

enum Kind {
    Dashboard,
    MissingApi,
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
    let upstream = loopback_upstream(upstream)?;
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
    let state = Arc::new(App {
        auth,
        client,
        upstream,
        root: root.map(Path::to_path_buf),
        hits: StdMutex::new(HashMap::new()),
    });
    Ok(Router::new().fallback(any(handle)).with_state(state))
}

pub async fn run(options: Options) -> Result<()> {
    require_bind(options.listen, euid())?;
    let enrollment =
        Enrollment::load(&options.enrollment_file)?.context("server is not enrolled")?;
    let auth = Authenticator::central_only(enrollment, options.jwks_cache)?;
    let ca = fs::read(&options.upstream_ca).context("upstream CA required")?;
    let router = router(
        auth,
        &options.upstream,
        &ca,
        options.dashboard_root.as_deref(),
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
    if !charge(&app.hits, ip) {
        return deny(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Too many requests from this address.",
        );
    }
    match classify(request.method(), request.uri().path()) {
        Kind::Dashboard => proxy(app, request).await,
        Kind::MissingApi => api_miss(),
        Kind::Page => page(app, &request),
        Kind::Other => api_miss(),
    }
}

fn classify(method: &Method, path: &str) -> Kind {
    if path == "/api" || path.starts_with("/api/") || path == "/v1" || path.starts_with("/v1/") {
        if method == Method::GET && path == "/api/v1/dashboard" {
            Kind::Dashboard
        } else {
            Kind::MissingApi
        }
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

fn charge(hits: &StdMutex<HashMap<IpAddr, Hit>>, ip: IpAddr) -> bool {
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
    if hit.count >= LIMIT {
        return false;
    }
    hit.count += 1;
    true
}

async fn proxy(app: &App, request: Request) -> Response {
    let bearer = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("");
    if app.auth.authenticate_central(bearer).await.is_err() {
        return unauthorized();
    }
    if request.uri().query().is_some() {
        return deny(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Query parameters are not accepted.",
        );
    }
    let Some(authorization) = request.headers().get(header::AUTHORIZATION).cloned() else {
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

//! Public TLS door. It checks an identity JWT, then dials loopback kiln-api.
//! The administrator token is not a JWT and never opens that dial.
//! A dashboard cookie authorizes `GET /api/v1/dashboard`. That route sends
//! the session JWT and does not copy the browser `Authorization` header.
//! `GET /v1/boxes/{id}/ssh` with `Upgrade: kiln-ssh` is proxied for an
//! operate-scope JWT for a tenant member (members only for boxes they own).
//! The caller's `Authorization` header is forwarded unchanged.
//! Every other upgrade is rejected.
use crate::{
    Failure,
    auth::{Authenticator, Enrollment},
};
use anyhow::{Context, Result, bail, ensure};
use axum::{
    Json, Router,
    body::Body,
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use futures_util::{Stream, StreamExt};
use kiln_api::auth::{AccessToken, DeviceAuthorization, DeviceTokens, Scope};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

struct Door {
    auth: Authenticator,
    client: reqwest::Client,
    ssh_client: reqwest::Client,
    upstream: reqwest::Url,
    capacity: Arc<Semaphore>,
    ssh_sessions: Arc<Semaphore>,
    ssh_audit: SshAudit,
    dash: Option<Dash>,
}

struct Dash {
    enrollment: Enrollment,
    issuer: reqwest::Url,
    identity: reqwest::Client,
    sessions: StdMutex<HashMap<String, SessionSlot>>,
    attempts: StdMutex<HashMap<String, Attempt>>,
    dir: PathBuf,
    root: Option<PathBuf>,
}

type SessionSlot = Arc<Mutex<Session>>;

struct Session {
    key: String,
    disk: SessionDisk,
    central: Option<Cached>,
    operate: Option<Cached>,
    dead: bool,
}

struct Cached {
    value: String,
    exp: Instant,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionDisk {
    refresh_token: String,
    device_id: String,
    csrf: String,
    created_at: u64,
}

struct Attempt {
    device_code: String,
    expires: Instant,
}

/// Browser session configuration for `kiln-api door`.
pub struct Dashboard {
    pub enrollment: Enrollment,
    pub session_dir: PathBuf,
    pub root: Option<PathBuf>,
    /// Trust this CA for the identity issuer. Production uses the process roots.
    pub identity_ca: Option<Vec<u8>>,
}

/// Idle time allowed while waiting for the next upstream byte.
///
/// This is not a total deadline. A 35s total cap expired during a silent
/// `kiln create` and while `kiln exec` was still streaming. 3605s is the
/// client's maximum wait for the next byte. Each chunk starts that wait over.
pub const READ_TIMEOUT: Duration = Duration::from_secs(3605);

const SESSION_SECONDS: u64 = 43_200;
const REFRESH_MARGIN: Duration = Duration::from_secs(30);
const MAX_ATTEMPTS: usize = 64;
const DASH_COOKIE: &str = "__Host-kiln_dash";
const ATTEMPT_COOKIE: &str = "__Host-kiln_dash_attempt";

pub fn router(auth: Authenticator, upstream: &str, ca_pem: &[u8]) -> Result<Router> {
    router_with_read_timeout(auth, upstream, ca_pem, READ_TIMEOUT)
}

pub fn router_with_read_timeout(
    auth: Authenticator,
    upstream: &str,
    ca_pem: &[u8],
    read_timeout: Duration,
) -> Result<Router> {
    Ok(router_with_ssh_audit(auth, upstream, ca_pem, read_timeout)?.0)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshAuditRecord {
    pub outcome: &'static str,
    pub box_id: String,
    pub caller: String,
    pub addr: IpAddr,
}

/// Paseo Caddy is pinned at this address on `paseo_default`.
const CADDY_ADDR: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(172, 18, 0, 3));

/// In-memory copy of the SSH audit lines, capped so a public flood cannot grow it.
#[derive(Clone)]
pub struct SshAudit {
    records: Arc<StdMutex<Vec<SshAuditRecord>>>,
}

impl SshAudit {
    fn new() -> Self {
        Self {
            records: Arc::new(StdMutex::new(Vec::new())),
        }
    }

    pub fn records(&self) -> Vec<SshAuditRecord> {
        lock_map(&self.records).clone()
    }

    fn record(&self, outcome: &'static str, box_id: &str, caller: &str, addr: IpAddr) {
        eprintln!("audit ssh outcome={outcome} box_id={box_id} caller={caller} addr={addr}");
        let mut records = lock_map(&self.records);
        if records.len() == 128 {
            records.remove(0);
        }
        records.push(SshAuditRecord {
            outcome,
            box_id: box_id.to_owned(),
            caller: caller.to_owned(),
            addr,
        });
    }
}

pub fn router_with_ssh_audit(
    auth: Authenticator,
    upstream: &str,
    ca_pem: &[u8],
    read_timeout: Duration,
) -> Result<(Router, SshAudit)> {
    let audit = SshAudit::new();
    let router = assemble(auth, upstream, ca_pem, read_timeout, None, audit.clone())?;
    Ok((router, audit))
}

pub fn router_with_dashboard(
    auth: Authenticator,
    upstream: &str,
    ca_pem: &[u8],
    dashboard: Dashboard,
) -> Result<Router> {
    assemble(
        auth,
        upstream,
        ca_pem,
        READ_TIMEOUT,
        Some(open_dash(dashboard)?),
        SshAudit::new(),
    )
}

fn assemble(
    auth: Authenticator,
    upstream: &str,
    ca_pem: &[u8],
    read_timeout: Duration,
    dash: Option<Dash>,
    ssh_audit: SshAudit,
) -> Result<Router> {
    let upstream = reqwest::Url::parse(upstream).context("invalid upstream URL")?;
    ensure!(
        upstream.scheme() == "https"
            && upstream.host_str() == Some("127.0.0.1")
            && upstream.username().is_empty()
            && upstream.password().is_none()
            && matches!(upstream.path(), "" | "/")
            && upstream.query().is_none()
            && upstream.fragment().is_none(),
        "public door upstream must be https://127.0.0.1"
    );
    let certificate = reqwest::Certificate::from_pem(ca_pem)?;
    let client = reqwest::Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(3))
        .read_timeout(read_timeout)
        .add_root_certificate(certificate.clone())
        .tls_built_in_root_certs(false)
        .build()?;
    // HTTP/1.1 only. An upgraded SSH stream must not be returned to a pool,
    // and it must not inherit the management read timeout.
    let ssh_client = reqwest::Client::builder()
        .https_only(true)
        .http1_only()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(3))
        .pool_max_idle_per_host(0)
        .add_root_certificate(certificate)
        .tls_built_in_root_certs(false)
        .build()?;
    let state = Arc::new(Door {
        auth,
        client,
        ssh_client,
        upstream,
        capacity: Arc::new(Semaphore::new(32)),
        ssh_sessions: Arc::new(Semaphore::new(32)),
        ssh_audit,
        dash,
    });
    Ok(Router::new().fallback(any(handle)).with_state(state))
}

pub async fn serve(
    listen: SocketAddr,
    tls_cert: &Path,
    tls_key: &Path,
    auth: Authenticator,
    upstream: &str,
    ca_pem: &[u8],
    dashboard: Dashboard,
) -> Result<()> {
    ensure!(
        !listen.ip().is_unspecified(),
        "door must bind one address so it does not take kiln-api's loopback port"
    );
    ensure!(
        unsafe { libc::geteuid() } != 0,
        "public door must run as an unprivileged user"
    );
    let router = router_with_dashboard(auth, upstream, ca_pem, dashboard)?;
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(tls_cert, tls_key).await?;
    eprintln!("public door listening on {listen}");
    axum_server::bind_rustls(listen, tls)
        .serve(router.into_make_service_with_connect_info::<SocketAddr>())
        .await?;
    Ok(())
}

fn open_dash(dashboard: Dashboard) -> Result<Dash> {
    dashboard.enrollment.validate()?;
    prepare_session_dir(&dashboard.session_dir)?;
    let issuer = reqwest::Url::parse(&dashboard.enrollment.issuer).context("invalid issuer")?;
    Ok(Dash {
        enrollment: dashboard.enrollment,
        issuer,
        identity: identity_client(dashboard.identity_ca.as_deref())?,
        sessions: StdMutex::new(HashMap::new()),
        attempts: StdMutex::new(HashMap::new()),
        dir: dashboard.session_dir,
        root: dashboard.root,
    })
}

fn identity_client(ca_pem: Option<&[u8]>) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(10));
    if let Some(pem) = ca_pem {
        builder = builder
            .add_root_certificate(reqwest::Certificate::from_pem(pem)?)
            .tls_built_in_root_certs(false);
    }
    Ok(builder.build()?)
}

fn prepare_session_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::symlink_metadata(dir) {
        ensure!(
            meta.is_dir() && !meta.file_type().is_symlink(),
            "session directory must be a real directory"
        );
    } else {
        fs::create_dir_all(dir)?;
    }
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

async fn handle(State(door): State<Arc<Door>>, request: Request) -> Response {
    finish(route(&door, request).await)
}

async fn route(door: &Arc<Door>, request: Request) -> Response {
    if door.dash.is_some() {
        let method = request.method().clone();
        let path = request.uri().path().to_owned();
        if method == Method::GET && path == "/login" {
            return login_start(
                door,
                request.headers().clone(),
                request.uri().query().is_some(),
            )
            .await;
        }
        if method == Method::GET && path == "/login/wait" {
            return login_wait(
                door,
                request.headers().clone(),
                request.uri().query().is_some(),
            )
            .await;
        }
        if method == Method::POST && path == "/logout" {
            return logout(door, request).await;
        }
        if method == Method::GET && path == "/api/v1/session" {
            return session_csrf(
                door,
                request.headers().clone(),
                request.uri().query().is_some(),
            )
            .await;
        }
        if let Some(response) = static_response(door, &request) {
            return response;
        }
        if cookie_route(&method, &path) && cookie_value(request.headers(), DASH_COOKIE).is_some() {
            return cookie_proxy(door, request).await;
        }
    }
    bearer_proxy(door, request).await
}

fn cookie_route(method: &Method, path: &str) -> bool {
    method == Method::GET && path == "/api/v1/dashboard"
}

fn is_api(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/") || path == "/v1" || path.starts_with("/v1/")
}

fn ssh_bearer(request: &Request) -> &str {
    request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("")
}

fn ssh_forwarded_client(headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .and_then(|value| value.trim().parse().ok())
}

fn ssh_audit_addr(request: &Request) -> IpAddr {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| *addr)
        .unwrap_or_else(|| "127.0.0.1:0".parse().unwrap());
    let direct = peer.ip();
    if direct == CADDY_ADDR {
        return ssh_forwarded_client(request.headers()).unwrap_or(direct);
    }
    direct
}

async fn bearer_proxy(door: &Door, request: Request) -> Response {
    if request.method() == Method::GET && ssh_box_id(request.uri().path()).is_some() {
        if kiln_ssh_upgrade(request.headers()) {
            return ssh_proxy(door, request).await;
        }
        let box_id = ssh_box_id(request.uri().path()).unwrap_or("").to_owned();
        let bearer = ssh_bearer(&request);
        let addr = ssh_audit_addr(&request);
        if door.auth.authenticate_central(bearer).await.is_err() {
            door.ssh_audit
                .record("unauthorized", &box_id, "anonymous", addr);
            return unauthorized();
        }
    }
    let bearer = ssh_bearer(&request);
    if door.auth.authenticate_central(bearer).await.is_err() {
        return unauthorized();
    }
    if request.uri().query().is_some()
        && let Err(message) =
            kiln_api::OperationSubmitQuery::parse(request.uri().path(), request.uri().query())
    {
        return deny(StatusCode::BAD_REQUEST, "invalid_request", message);
    }
    if request.method() != Method::GET && request.method() != Method::POST {
        return deny(
            StatusCode::METHOD_NOT_ALLOWED,
            "invalid_request",
            "Only GET and POST are accepted.",
        );
    }
    if request.headers().get(header::UPGRADE).is_some() {
        if let Some(box_id) = ssh_box_id(request.uri().path()).map(str::to_owned) {
            let caller = door.auth.central_caller(bearer).await;
            door.ssh_audit
                .record("invalid_ssh", &box_id, &caller, ssh_audit_addr(&request));
        }
        return deny(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "SSH is not served by the public door.",
        );
    }
    let Some(authorization) = request.headers().get(header::AUTHORIZATION).cloned() else {
        return unauthorized();
    };
    proxy(door, request, authorization).await
}

fn ssh_box_id(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/v1/boxes/")?;
    let (id, suffix) = rest.split_once('/')?;
    (suffix == "ssh" && kiln_api::valid_id(id)).then_some(id)
}

fn kiln_ssh_upgrade(headers: &HeaderMap) -> bool {
    headers
        .get(header::UPGRADE)
        .and_then(|value| value.to_str().ok())
        == Some(kiln_api::SSH_UPGRADE)
}

async fn ssh_proxy(door: &Door, request: Request) -> Response {
    let box_id = ssh_box_id(request.uri().path()).unwrap_or("").to_owned();
    let bearer = ssh_bearer(&request);
    let addr = ssh_audit_addr(&request);
    let (caller, scope) = match door.auth.authenticate_caller(bearer).await {
        Ok(pair) => pair,
        Err(_) => {
            door.ssh_audit
                .record("unauthorized", &box_id, "anonymous", addr);
            return unauthorized();
        }
    };
    if scope != Scope::Operate {
        door.ssh_audit
            .record("insufficient_scope", &box_id, &caller.sub, addr);
        return deny(
            StatusCode::FORBIDDEN,
            "insufficient_scope",
            "An operate credential is required.",
        );
    }
    if request.uri().query().is_some() {
        door.ssh_audit
            .record("invalid_ssh", &box_id, &caller.sub, addr);
        return deny(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Query parameters are not accepted.",
        );
    }
    let key = match crate::ssh::admission(&request) {
        Ok(key) => key,
        Err(failure) => {
            door.ssh_audit
                .record("invalid_ssh", &box_id, &caller.sub, addr);
            return no_store(failure.into_response());
        }
    };
    let Some(mut authorization) = request.headers().get(header::AUTHORIZATION).cloned() else {
        door.ssh_audit
            .record("unauthorized", &box_id, "anonymous", addr);
        return unauthorized();
    };
    authorization.set_sensitive(true);
    let Some(tenant_owner) = door.auth.tenant_owner() else {
        door.ssh_audit
            .record("unauthorized", &box_id, &caller.sub, addr);
        return unauthorized();
    };
    if let Err(failure) = crate::box_access::ensure_member_box_access(
        &door.client,
        &door.upstream,
        &authorization,
        &box_id,
        &caller,
        tenant_owner,
    )
    .await
    {
        door.ssh_audit
            .record("not_found", &box_id, &caller.sub, addr);
        return no_store(failure.into_response());
    }
    let Ok(permit) = door.ssh_sessions.clone().try_acquire_owned() else {
        door.ssh_audit.record("busy", &box_id, &caller.sub, addr);
        return deny(
            StatusCode::TOO_MANY_REQUESTS,
            "ssh_busy",
            "SSH session capacity reached.",
        );
    };
    let mut url = door.upstream.clone();
    url.set_path(request.uri().path());
    let upstream = match door
        .ssh_client
        .get(url)
        .header(header::AUTHORIZATION, authorization)
        .header(header::CONNECTION, "upgrade")
        .header(header::UPGRADE, kiln_api::SSH_UPGRADE)
        .header(kiln_api::SSH_KEY_HEADER, key)
        .send()
        .await
    {
        Ok(upstream) => upstream,
        Err(_) => {
            door.ssh_audit
                .record("upstream", &box_id, &caller.sub, addr);
            return deny(
                StatusCode::BAD_GATEWAY,
                "host_unavailable",
                "SSH upstream did not accept the tunnel.",
            );
        }
    };
    if upstream.status() != StatusCode::SWITCHING_PROTOCOLS {
        door.ssh_audit
            .record("upstream", &box_id, &caller.sub, addr);
        return ssh_refusal(upstream).await;
    }
    if upstream
        .headers()
        .get(header::UPGRADE)
        .and_then(|value| value.to_str().ok())
        != Some(kiln_api::SSH_UPGRADE)
    {
        door.ssh_audit
            .record("upstream", &box_id, &caller.sub, addr);
        return deny(
            StatusCode::BAD_GATEWAY,
            "host_unavailable",
            "SSH upstream did not accept the tunnel.",
        );
    }
    let upstream = match upstream.upgrade().await {
        Ok(upstream) => upstream,
        Err(_) => {
            door.ssh_audit
                .record("upstream", &box_id, &caller.sub, addr);
            return deny(
                StatusCode::BAD_GATEWAY,
                "host_unavailable",
                "SSH upstream did not accept the tunnel.",
            );
        }
    };
    door.ssh_audit
        .record("upgraded", &box_id, &caller.sub, addr);
    crate::ssh::upgrade(request, upstream, permit)
}

async fn ssh_refusal(upstream: reqwest::Response) -> Response {
    let status = upstream.status();
    let bytes = match upstream.bytes().await {
        Ok(bytes) if bytes.len() <= 8192 => bytes,
        _ => {
            return deny(
                StatusCode::BAD_GATEWAY,
                "host_unavailable",
                "SSH upstream did not accept the tunnel.",
            );
        }
    };
    if let Ok(error) = serde_json::from_slice::<kiln_api::ApiError>(&bytes)
        && !error.code.is_empty()
    {
        return no_store((status, Json(error)).into_response());
    }
    deny(
        StatusCode::BAD_GATEWAY,
        "host_unavailable",
        "SSH upstream did not accept the tunnel.",
    )
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn cookie_proxy(door: &Door, request: Request) -> Response {
    if let Some(response) = reject_upgrade(request.headers()) {
        return response;
    }
    if request.uri().query().is_some() {
        return deny(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Query parameters are not accepted.",
        );
    }
    let Some(dash) = door.dash.as_ref() else {
        return unauthorized();
    };
    let Some(raw) = cookie_value(request.headers(), DASH_COOKIE) else {
        return unauthorized();
    };
    let Some(slot) = load_session(dash, &raw) else {
        return unauthorized();
    };
    let jwt = match operate_jwt(door, dash, &slot).await {
        Jwt::Ready(jwt) => jwt,
        Jwt::Dead => return unauthorized(),
        Jwt::Down => return identity_down(),
    };
    let Ok(authorization) = HeaderValue::from_str(&format!("Bearer {jwt}")) else {
        return unauthorized();
    };
    proxy(door, request, authorization).await
}

async fn proxy(door: &Door, request: Request, authorization: HeaderValue) -> Response {
    let Ok(permit) = Arc::clone(&door.capacity).try_acquire_owned() else {
        return deny(
            StatusCode::TOO_MANY_REQUESTS,
            "busy",
            "Gateway is busy; request was not forwarded.",
        );
    };
    match forward(door, request, permit, authorization).await {
        Ok(response) => response,
        Err(failure) => {
            let mut response = failure.into_response();
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
    }
}

async fn forward(
    door: &Door,
    request: Request,
    permit: OwnedSemaphorePermit,
    authorization: HeaderValue,
) -> Result<Response, Failure> {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let query = request.uri().query().map(str::to_owned);
    let wait = query.as_deref().and_then(|query| {
        kiln_api::OperationSubmitQuery::parse(&path, Some(query))
            .ok()
            .filter(|parsed| parsed.wait)
    });
    let content_type = request.headers().get(header::CONTENT_TYPE).cloned();
    if declared_length(request.headers(), kiln_api::MAX_REQUEST_BYTES) {
        return Err(Failure::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "body_too_large",
            "Request body exceeds 128 KiB.",
        ));
    }
    let empty = method == Method::GET
        || request
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            == Some("0");
    let mut url = door.upstream.clone();
    url.set_path(&path);
    url.set_query(query.as_deref());
    let mut upstream = door
        .client
        .request(method.clone(), url)
        .header(header::AUTHORIZATION, authorization);
    if let Some(wait) = wait {
        upstream = upstream.timeout(Duration::from_secs(wait.wait_seconds.saturating_add(5)));
    }
    if let Some(content_type) = content_type {
        upstream = upstream.header(header::CONTENT_TYPE, content_type);
    }
    let upstream = if empty {
        drop(request);
        upstream
    } else {
        upstream.body(reqwest::Body::wrap_stream(limited(
            request.into_body().into_data_stream(),
            kiln_api::MAX_REQUEST_BYTES,
        )))
    };
    let response = upstream.send().await.map_err(|_| upstream_error())?;
    if response
        .content_length()
        .is_some_and(|len| len > kiln_api::MAX_RESPONSE_BYTES as u64)
    {
        return Err(upstream_error());
    }
    let status = response.status();
    let content_type = response.headers().get(header::CONTENT_TYPE).cloned();
    let stream = hold(
        limited(response.bytes_stream(), kiln_api::MAX_RESPONSE_BYTES),
        permit,
    );
    let mut response = (status, Body::from_stream(stream)).into_response();
    if let Some(content_type) = content_type {
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, content_type);
    }
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

async fn login_start(door: &Door, headers: HeaderMap, has_query: bool) -> Response {
    if let Some(response) = reject_upgrade(&headers) {
        return response;
    }
    if headers.get(header::AUTHORIZATION).is_some() {
        return unauthorized();
    }
    if has_query {
        return deny(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Query parameters are not accepted.",
        );
    }
    let Some(dash) = door.dash.as_ref() else {
        return unauthorized();
    };
    {
        let mut attempts = lock_map(&dash.attempts);
        let now = Instant::now();
        attempts.retain(|_, attempt| attempt.expires > now);
        if attempts.len() >= MAX_ATTEMPTS {
            return deny(
                StatusCode::TOO_MANY_REQUESTS,
                "busy",
                "Sign-in is busy; try again.",
            );
        }
    }
    let Ok(url) = identity_url(&dash.issuer, "/oauth/device/code") else {
        return identity_down();
    };
    let body = form_encode(&[("client_id", "kiln-cli"), ("device_name", "kiln-dashboard")]);
    let Ok((status, bytes)) = post_form(&dash.identity, url, body).await else {
        return identity_down();
    };
    if !status.is_success() {
        return identity_down();
    }
    let Ok(grant) = serde_json::from_slice::<DeviceAuthorization>(&bytes) else {
        return identity_down();
    };
    if !(1..=600).contains(&grant.expires_in)
        || !(1..=60).contains(&grant.interval)
        || !same_issuer(&dash.issuer, &grant.verification_uri)
        || !same_issuer(&dash.issuer, &grant.verification_uri_complete)
        || grant.verification_uri_complete.contains(&grant.device_code)
        || !valid_secret(&grant.device_code)
    {
        return identity_down();
    }
    let Ok(raw) = random_hex(32) else {
        return identity_down();
    };
    let key = digest_hex(&raw);
    lock_map(&dash.attempts).insert(
        key,
        Attempt {
            device_code: grant.device_code,
            expires: Instant::now() + Duration::from_secs(grant.expires_in),
        },
    );
    let mut response = Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, grant.verification_uri_complete)
        .body(Body::empty())
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response());
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    append_cookie(&mut response, ATTEMPT_COOKIE, &raw, grant.expires_in);
    response
}

async fn login_wait(door: &Door, headers: HeaderMap, has_query: bool) -> Response {
    if let Some(response) = reject_upgrade(&headers) {
        return response;
    }
    if has_query {
        return deny(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Query parameters are not accepted.",
        );
    }
    let Some(dash) = door.dash.as_ref() else {
        return unauthorized();
    };
    let Some(raw) = cookie_value(&headers, ATTEMPT_COOKIE) else {
        return unauthorized();
    };
    let key = digest_hex(&raw);
    let device_code = {
        let mut attempts = lock_map(&dash.attempts);
        let Some(attempt) = attempts.get(&key) else {
            return unauthorized();
        };
        if attempt.expires <= Instant::now() {
            attempts.remove(&key);
            return unauthorized();
        }
        attempt.device_code.clone()
    };
    let Ok(url) = identity_url(&dash.issuer, "/oauth/token") else {
        return identity_down();
    };
    let body = form_encode(&[
        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ("device_code", &device_code),
    ]);
    let Ok((status, bytes)) = post_form(&dash.identity, url, body).await else {
        return identity_down();
    };
    if !status.is_success() {
        let error = serde_json::from_slice::<OAuthError>(&bytes)
            .ok()
            .map(|item| item.error);
        if matches!(
            error.as_deref(),
            Some("authorization_pending" | "slow_down")
        ) {
            return (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({"status": "pending"})),
            )
                .into_response();
        }
        lock_map(&dash.attempts).remove(&key);
        return unauthorized();
    }
    lock_map(&dash.attempts).remove(&key);
    let Ok(tokens) = serde_json::from_slice::<DeviceTokens>(&bytes) else {
        return unauthorized();
    };
    if !tokens.token_type.eq_ignore_ascii_case("bearer")
        || !(1..=300).contains(&tokens.expires_in)
        || !valid_secret(&tokens.access_token)
        || !valid_secret(&tokens.refresh_token)
    {
        return unauthorized();
    }
    match approve(door, dash, tokens).await {
        Approve::Ready(response) => response,
        Approve::Denied => unauthorized(),
        Approve::Down => identity_down(),
    }
}

enum Approve {
    Ready(Response),
    Denied,
    Down,
}

async fn approve(door: &Door, dash: &Dash, tokens: DeviceTokens) -> Approve {
    let central = tokens.access_token;
    let central_exp = Instant::now() + Duration::from_secs(tokens.expires_in);
    let Some(device_id) = device_id(dash, &central).await else {
        return Approve::Denied;
    };
    let Some(grant) = operate_grant(dash, &central).await else {
        let _ = revoke_device(dash, &device_id, &central).await;
        return Approve::Denied;
    };
    if !matches!(
        door.auth.authenticate_central(&grant.access_token).await,
        Ok(Scope::Operate)
    ) {
        let _ = revoke_device(dash, &device_id, &central).await;
        return Approve::Denied;
    }
    let Ok(raw) = random_hex(32) else {
        let _ = revoke_device(dash, &device_id, &central).await;
        return Approve::Down;
    };
    let Ok(csrf) = random_hex(32) else {
        let _ = revoke_device(dash, &device_id, &central).await;
        return Approve::Down;
    };
    let key = digest_hex(&raw);
    let disk = SessionDisk {
        refresh_token: tokens.refresh_token,
        device_id,
        csrf,
        created_at: jsonwebtoken::get_current_timestamp(),
    };
    if write_session(&dash.dir, &key, &disk).is_err() {
        let _ = revoke_device(dash, &disk.device_id, &central).await;
        return Approve::Down;
    }
    let slot = Arc::new(Mutex::new(Session {
        key: key.clone(),
        disk,
        central: Some(Cached {
            value: central,
            exp: central_exp,
        }),
        operate: Some(Cached {
            value: grant.access_token,
            exp: Instant::now() + Duration::from_secs(grant.expires_in),
        }),
        dead: false,
    }));
    lock_map(&dash.sessions).insert(key, slot);
    let mut response =
        (StatusCode::OK, Json(serde_json::json!({"status": "ready"}))).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    append_cookie(&mut response, DASH_COOKIE, &raw, SESSION_SECONDS);
    append_cookie(&mut response, ATTEMPT_COOKIE, "", 0);
    Approve::Ready(response)
}

async fn session_csrf(door: &Door, headers: HeaderMap, has_query: bool) -> Response {
    if let Some(response) = reject_upgrade(&headers) {
        return response;
    }
    let Some(dash) = door.dash.as_ref() else {
        return unauthorized();
    };
    let Some(raw) = cookie_value(&headers, DASH_COOKIE) else {
        return unauthorized();
    };
    if has_query {
        return deny(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Query parameters are not accepted.",
        );
    }
    let Some(slot) = load_session(dash, &raw) else {
        return unauthorized();
    };
    match operate_jwt(door, dash, &slot).await {
        Jwt::Ready(_) => {}
        Jwt::Dead => return unauthorized(),
        Jwt::Down => return identity_down(),
    }
    let csrf = slot.lock().await.disk.csrf.clone();
    let mut response = Json(serde_json::json!({"csrf": csrf})).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn logout(door: &Door, request: Request) -> Response {
    if let Some(response) = reject_upgrade(request.headers()) {
        return response;
    }
    if request.uri().query().is_some() {
        return deny(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Query parameters are not accepted.",
        );
    }
    let Some(dash) = door.dash.as_ref() else {
        return unauthorized();
    };
    let Some(raw) = cookie_value(request.headers(), DASH_COOKIE) else {
        return unauthorized();
    };
    let Some(slot) = load_session(dash, &raw) else {
        return unauthorized();
    };
    if !origin_matches(request.headers()) || !csrf_matches(request.headers(), &slot).await {
        return deny(
            StatusCode::FORBIDDEN,
            "forbidden",
            "The session check failed.",
        );
    }
    let mut session = slot.lock().await;
    if session.dead {
        return unauthorized();
    }
    let device_id = session.disk.device_id.clone();
    let refresh_token = session.disk.refresh_token.clone();
    let cached = session
        .central
        .as_ref()
        .filter(|item| fresh(item))
        .map(|item| item.value.clone());
    // The browser session ends before identity is asked. A failed revoke must not restore it.
    kill(dash, &mut session);
    drop(session);
    let central = match cached {
        Some(token) => token,
        None => match exchange_refresh(dash, &refresh_token).await {
            Ok(tokens) => tokens.access_token,
            Err(RefreshError::Dead) => return cleared_session(unauthorized()),
            Err(RefreshError::Down) => return cleared_session(identity_down()),
        },
    };
    if !revoke_device(dash, &device_id, &central).await {
        return cleared_session(identity_down());
    }
    cleared_session(StatusCode::NO_CONTENT.into_response())
}

fn cleared_session(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    append_cookie(&mut response, DASH_COOKIE, "", 0);
    response
}

enum RefreshError {
    Dead,
    Down,
}

enum Jwt {
    Ready(String),
    Dead,
    Down,
}

async fn operate_jwt(door: &Door, dash: &Dash, slot: &SessionSlot) -> Jwt {
    let mut session = slot.lock().await;
    if session.dead {
        return Jwt::Dead;
    }
    if let Some(jwt) = session.operate.as_ref().filter(|item| fresh(item)) {
        return Jwt::Ready(jwt.value.clone());
    }
    let central = match fresh_central(dash, &mut session).await {
        Ok(token) => token,
        Err(RefreshError::Dead) => {
            kill(dash, &mut session);
            return Jwt::Dead;
        }
        Err(RefreshError::Down) => return Jwt::Down,
    };
    let Some(grant) = operate_grant(dash, &central).await else {
        kill(dash, &mut session);
        return Jwt::Dead;
    };
    if !matches!(
        door.auth.authenticate_central(&grant.access_token).await,
        Ok(Scope::Operate)
    ) || !(1..=300).contains(&grant.expires_in)
    {
        kill(dash, &mut session);
        return Jwt::Dead;
    }
    session.operate = Some(Cached {
        value: grant.access_token.clone(),
        exp: Instant::now() + Duration::from_secs(grant.expires_in),
    });
    Jwt::Ready(grant.access_token)
}

fn kill(dash: &Dash, session: &mut Session) {
    session.dead = true;
    forget(dash, &session.key);
}

async fn fresh_central(dash: &Dash, session: &mut Session) -> Result<String, RefreshError> {
    if let Some(token) = session.central.as_ref().filter(|item| fresh(item)) {
        return Ok(token.value.clone());
    }
    let tokens = exchange_refresh(dash, &session.disk.refresh_token).await?;
    session.disk.refresh_token = tokens.refresh_token;
    if write_session(&dash.dir, &session.key, &session.disk).is_err() {
        return Err(RefreshError::Down);
    }
    session.central = Some(Cached {
        value: tokens.access_token.clone(),
        exp: Instant::now() + Duration::from_secs(tokens.expires_in),
    });
    Ok(tokens.access_token)
}

async fn exchange_refresh(dash: &Dash, refresh_token: &str) -> Result<DeviceTokens, RefreshError> {
    let url = identity_url(&dash.issuer, "/oauth/token").map_err(|_| RefreshError::Down)?;
    let body = form_encode(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
    ]);
    let (status, bytes) = post_form(&dash.identity, url, body)
        .await
        .map_err(|_| RefreshError::Down)?;
    if !status.is_success() {
        return Err(RefreshError::Dead);
    }
    let tokens: DeviceTokens = serde_json::from_slice(&bytes).map_err(|_| RefreshError::Dead)?;
    if !tokens.token_type.eq_ignore_ascii_case("bearer")
        || !(1..=300).contains(&tokens.expires_in)
        || !valid_secret(&tokens.access_token)
        || !valid_secret(&tokens.refresh_token)
    {
        return Err(RefreshError::Dead);
    }
    Ok(tokens)
}

fn fresh(token: &Cached) -> bool {
    token.exp.saturating_duration_since(Instant::now()) > REFRESH_MARGIN
}

async fn device_id(dash: &Dash, central: &str) -> Option<String> {
    let url = identity_url(&dash.issuer, "/v1/me").ok()?;
    let (status, bytes) = authed_json(&dash.identity, Method::GET, url, central, None)
        .await
        .ok()?;
    if !status.is_success() {
        return None;
    }
    let body: MeBody = serde_json::from_slice(&bytes).ok()?;
    valid_device_id(&body.device_id).then_some(body.device_id)
}

async fn operate_grant(dash: &Dash, central: &str) -> Option<AccessToken> {
    let url = identity_url(&dash.issuer, "/v1/access").ok()?;
    let body = serde_json::json!({"server_id": dash.enrollment.server_id});
    let (status, bytes) = authed_json(&dash.identity, Method::POST, url, central, Some(body))
        .await
        .ok()?;
    if !status.is_success() {
        return None;
    }
    let grant: AccessToken = serde_json::from_slice(&bytes).ok()?;
    if grant.token_type.eq_ignore_ascii_case("bearer") && (1..=300).contains(&grant.expires_in) {
        Some(grant)
    } else {
        None
    }
}

async fn revoke_device(dash: &Dash, device_id: &str, central: &str) -> bool {
    let Ok(url) = identity_url(&dash.issuer, &format!("/v1/devices/{device_id}")) else {
        return false;
    };
    let Ok((status, _)) = authed_json(&dash.identity, Method::DELETE, url, central, None).await
    else {
        return false;
    };
    status == StatusCode::NO_CONTENT || status == StatusCode::NOT_FOUND
}

async fn post_form(
    client: &reqwest::Client,
    url: reqwest::Url,
    body: String,
) -> Result<(StatusCode, Vec<u8>)> {
    let response = client
        .post(url)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        )
        .body(body)
        .send()
        .await?;
    read_identity(response).await
}

async fn authed_json(
    client: &reqwest::Client,
    method: Method,
    url: reqwest::Url,
    token: &str,
    body: Option<serde_json::Value>,
) -> Result<(StatusCode, Vec<u8>)> {
    let mut request = client
        .request(method, url)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    if let Some(body) = body {
        request = request.json(&body);
    }
    read_identity(request.send().await?).await
}

async fn read_identity(mut response: reqwest::Response) -> Result<(StatusCode, Vec<u8>)> {
    let status = response.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= 65_536,
            "identity response too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok((status, bytes))
}

fn load_session(dash: &Dash, raw: &str) -> Option<SessionSlot> {
    if raw.len() > 128 || raw.is_empty() {
        return None;
    }
    let key = digest_hex(raw);
    if let Some(slot) = lock_map(&dash.sessions).get(&key).cloned() {
        return Some(slot);
    }
    let path = session_path(&dash.dir, &key)?;
    let bytes = fs::read(&path).ok()?;
    let disk: SessionDisk = serde_json::from_slice(&bytes).ok()?;
    if !disk_ok(&disk) {
        let _ = fs::remove_file(&path);
        return None;
    }
    let slot = Arc::new(Mutex::new(Session {
        key: key.clone(),
        disk,
        central: None,
        operate: None,
        dead: false,
    }));
    Some(
        lock_map(&dash.sessions)
            .entry(key)
            .or_insert_with(|| slot)
            .clone(),
    )
}

fn disk_ok(disk: &SessionDisk) -> bool {
    let now = jsonwebtoken::get_current_timestamp();
    disk.created_at <= now.saturating_add(30)
        && now.saturating_sub(disk.created_at) <= SESSION_SECONDS
        && valid_device_id(&disk.device_id)
        && valid_secret(&disk.refresh_token)
        && (1..=128).contains(&disk.csrf.len())
        && disk.csrf.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn forget(dash: &Dash, key: &str) {
    lock_map(&dash.sessions).remove(key);
    if let Some(path) = session_path(&dash.dir, key) {
        let _ = fs::remove_file(path);
    }
}

fn write_session(dir: &Path, key: &str, disk: &SessionDisk) -> Result<()> {
    let path = session_path(dir, key).context("session path")?;
    let bytes = serde_json::to_vec(disk)?;
    write_private(&path, &bytes)
}

fn session_path(dir: &Path, key: &str) -> Option<PathBuf> {
    if key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Some(dir.join(format!("{key}.json")))
    } else {
        None
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let parent = path.parent().context("session directory missing")?;
    let meta = fs::symlink_metadata(parent)?;
    ensure!(
        meta.is_dir() && !meta.file_type().is_symlink(),
        "session directory must be a real directory"
    );
    if fs::symlink_metadata(path).is_ok_and(|item| item.file_type().is_symlink()) {
        bail!("session file is a symlink");
    }
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn static_response(door: &Door, request: &Request) -> Option<Response> {
    if request.method() != Method::GET {
        return None;
    }
    let path = request.uri().path();
    if is_api(path) || path == "/login" || path == "/login/wait" {
        return None;
    }
    let root = door
        .dash
        .as_ref()
        .and_then(|dash| dash.root.as_deref().and_then(canonical_dir))?;
    if let Some(response) = reject_upgrade(request.headers()) {
        return Some(response);
    }
    if request.headers().get(header::AUTHORIZATION).is_some() {
        return Some(unauthorized());
    }
    let query = request.uri().query();
    if query.is_some() {
        return Some(deny(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Query parameters are not accepted.",
        ));
    }
    let Some(rel) = safe_rel(path) else {
        return Some(deny(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "The path is not valid.",
        ));
    };
    Some(read_document(&root, &rel))
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

fn read_document(root: &Path, rel: &Path) -> Response {
    match read_inside(root, rel) {
        Found::File(bytes) => static_body(rel, rel, bytes),
        Found::TooLarge => deny(
            StatusCode::PAYLOAD_TOO_LARGE,
            "body_too_large",
            "Dashboard file exceeds 8 MiB.",
        ),
        Found::Missing if rel.extension().is_none() => {
            match read_inside(root, Path::new("index.html")) {
                Found::File(bytes) => static_body(Path::new("index.html"), rel, bytes),
                Found::TooLarge => deny(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "body_too_large",
                    "Dashboard file exceeds 8 MiB.",
                ),
                Found::Missing => Failure::missing().into_response(),
            }
        }
        Found::Missing => Failure::missing().into_response(),
    }
}

enum Found {
    File(Vec<u8>),
    TooLarge,
    Missing,
}

fn read_inside(root: &Path, rel: &Path) -> Found {
    let candidate = root.join(rel);
    let Ok(meta) = fs::symlink_metadata(&candidate) else {
        return Found::Missing;
    };
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Found::Missing;
    }
    if meta.len() > 8 * 1024 * 1024 {
        return Found::TooLarge;
    }
    let Ok(canon) = candidate.canonicalize() else {
        return Found::Missing;
    };
    if !canon.starts_with(root) {
        return Found::Missing;
    }
    match fs::read(&canon) {
        Ok(bytes) if bytes.len() > 8 * 1024 * 1024 => Found::TooLarge,
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

fn origin_matches(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let Some(host) = headers.get(header::HOST).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    !host.is_empty()
        && !host.contains('/')
        && !host.contains('@')
        && !host.contains('\\')
        && origin == format!("https://{host}")
}

async fn csrf_matches(headers: &HeaderMap, slot: &SessionSlot) -> bool {
    let sent = headers
        .get("x-kiln-csrf")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let csrf = slot.lock().await.disk.csrf.clone();
    let (left, right) = (sent.as_bytes(), csrf.as_bytes());
    left.len() == right.len() && bool::from(left.ct_eq(right))
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    let prefix = format!("{name}=");
    let value = raw.split(';').find_map(|part| {
        let part = part.trim();
        part.strip_prefix(&prefix).map(str::to_owned)
    })?;
    if value.is_empty()
        || value.len() > 128
        || value
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte == b',' || byte == b';')
    {
        None
    } else {
        Some(value)
    }
}

fn append_cookie(response: &mut Response, name: &str, value: &str, max_age: u64) {
    let cookie =
        format!("{name}={value}; Secure; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}");
    if let Ok(parsed) = HeaderValue::from_str(&cookie) {
        response.headers_mut().append(header::SET_COOKIE, parsed);
    }
}

fn reject_upgrade(headers: &HeaderMap) -> Option<Response> {
    headers.get(header::UPGRADE).is_some().then(|| {
        deny(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "SSH is not served by the public door.",
        )
    })
}

fn identity_url(issuer: &reqwest::Url, path: &str) -> Result<reqwest::Url> {
    let mut url = issuer.clone();
    url.set_path(path);
    url.set_query(None);
    url.set_fragment(None);
    ensure!(
        url.origin() == issuer.origin(),
        "identity URL left the issuer"
    );
    Ok(url)
}

fn same_issuer(issuer: &reqwest::Url, value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    value.len() <= 2048
        && url.origin() == issuer.origin()
        && url.username().is_empty()
        && url.password().is_none()
}

fn form_encode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{}={}", percent(key), percent(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn percent(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn random_hex(len: usize) -> Result<String> {
    let mut bytes = vec![0u8; len];
    fs::File::open("/dev/urandom")
        .context("random source")?
        .read_exact(&mut bytes)?;
    Ok(hex(&bytes))
}

fn digest_hex(value: &str) -> String {
    hex(Sha256::digest(value.as_bytes()).as_slice())
}

fn hex(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(ALPHABET[(byte >> 4) as usize] as char);
        out.push(ALPHABET[(byte & 0xf) as usize] as char);
    }
    out
}

fn valid_secret(value: &str) -> bool {
    (1..=512).contains(&value.len()) && value.bytes().all(|byte| matches!(byte, 0x21..=0x7e))
}

fn valid_device_id(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn finish(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        ),
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

fn deny(status: StatusCode, code: &str, message: &str) -> Response {
    let mut response = Failure::new(status, code, message).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn unauthorized() -> Response {
    deny(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "A valid credential is required.",
    )
}

fn identity_down() -> Response {
    deny(
        StatusCode::BAD_GATEWAY,
        "upstream",
        "Identity did not answer.",
    )
}

#[derive(Deserialize)]
struct OAuthError {
    error: String,
}

#[derive(Deserialize)]
struct MeBody {
    device_id: String,
}

fn declared_length(headers: &HeaderMap, max: usize) -> bool {
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|text| text.parse::<u64>().ok())
        .is_some_and(|len| len > max as u64)
}

fn limited<S, E>(
    stream: S,
    max: usize,
) -> impl Stream<Item = Result<bytes::Bytes, std::io::Error>> + Send
where
    S: Stream<Item = Result<bytes::Bytes, E>> + Send,
    E: std::error::Error + Send + Sync + 'static,
{
    let mut seen = 0usize;
    stream.map(move |item| {
        let chunk = item.map_err(std::io::Error::other)?;
        seen = seen.saturating_add(chunk.len());
        if seen > max {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "body exceeds the door limit",
            ));
        }
        Ok(chunk)
    })
}

fn hold<S, E>(
    stream: S,
    permit: OwnedSemaphorePermit,
) -> impl Stream<Item = Result<bytes::Bytes, E>> + Send
where
    S: Stream<Item = Result<bytes::Bytes, E>> + Send + 'static,
    E: Send + 'static,
{
    let stream: Pin<Box<dyn Stream<Item = Result<bytes::Bytes, E>> + Send>> = Box::pin(stream);
    futures_util::stream::unfold((stream, Some(permit)), |(mut stream, permit)| async move {
        stream.next().await.map(|item| (item, (stream, permit)))
    })
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

#[cfg(test)]
mod ssh_audit_tests {
    use super::*;
    use axum::body::Body;

    fn ssh_request(peer: SocketAddr, forwarded: Option<&str>) -> Request<Body> {
        let mut request = Request::builder()
            .uri("/v1/boxes/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/ssh")
            .body(Body::empty())
            .unwrap();
        if let Some(value) = forwarded {
            request
                .headers_mut()
                .insert("x-forwarded-for", HeaderValue::from_str(value).unwrap());
        }
        request.extensions_mut().insert(ConnectInfo(peer));
        request
    }

    #[test]
    fn audit_addr_uses_forwarded_only_from_caddy() {
        let laptop: IpAddr = "203.0.113.44".parse().unwrap();
        let caddy_peer = SocketAddr::new(CADDY_ADDR, 443);
        assert_eq!(
            ssh_audit_addr(&ssh_request(caddy_peer, Some("203.0.113.44"))),
            laptop
        );
        let loopback = SocketAddr::from(([127, 0, 0, 1], 8444));
        assert_eq!(
            ssh_audit_addr(&ssh_request(loopback, Some("203.0.113.44"))),
            loopback.ip()
        );
    }
}

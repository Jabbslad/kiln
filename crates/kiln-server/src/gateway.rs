//! Unprivileged TLS edge. The only upstream is the configured Unix socket.
use crate::{Failure, auth::Authenticator};
use anyhow::Result;
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use kiln_api::auth::Scope;
use std::{path::Path, sync::Arc, time::Duration};
use tokio::sync::Semaphore;

struct Gateway {
    client: reqwest::Client,
    auth: Authenticator,
    capacity: Semaphore,
    sessions: Arc<Semaphore>,
}

pub fn router(socket: &Path, token: &str) -> Result<Router> {
    router_with_auth(socket, Authenticator::new(token, None)?)
}

pub fn router_with_auth(socket: &Path, auth: Authenticator) -> Result<Router> {
    let state = Arc::new(Gateway {
        client: reqwest::Client::builder()
            .unix_socket(socket)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(35))
            .build()?,
        auth,
        capacity: Semaphore::new(32),
        sessions: Arc::new(Semaphore::new(32)),
    });
    Ok(Router::new()
        .route("/v1/templates", get(read_proxy))
        .route("/v1/boxes", get(read_proxy))
        .route("/v1/boxes/{id}", get(read_proxy))
        .route("/v1/boxes/{id}/ssh-key", get(operate_proxy))
        .route("/v1/boxes/{id}/ssh", get(ssh_tunnel))
        .route("/v1/operations/{id}", get(operate_proxy))
        .route("/v1/operations", post(operate_proxy))
        .with_state(state))
}

async fn authenticate(
    state: &Gateway,
    headers: &HeaderMap,
    uri: &Uri,
    required: Scope,
) -> Result<(), Failure> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");
    let scope = state.auth.authenticate(token).await.map_err(|_| {
        Failure::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "A valid credential is required.",
        )
    })?;
    if required == Scope::Operate && scope != Scope::Operate {
        return Err(Failure::new(
            StatusCode::FORBIDDEN,
            "insufficient_scope",
            "An operate credential is required.",
        ));
    }
    if uri.query().is_some() {
        return Err(Failure::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Query parameters are not accepted.",
        ));
    }
    Ok(())
}

async fn ssh_tunnel(
    State(state): State<Arc<Gateway>>,
    request: Request,
) -> Result<Response, Failure> {
    authenticate(&state, request.headers(), request.uri(), Scope::Operate).await?;
    let key = crate::ssh::admission(&request)?;
    let permit = state
        .sessions
        .clone()
        .try_acquire_owned()
        .map_err(|_| crate::ssh::busy())?;
    let upstream = state
        .client
        .get(format!("http://localhost{}", request.uri().path()))
        .header(header::CONNECTION, "upgrade")
        .header(header::UPGRADE, kiln_api::SSH_UPGRADE)
        .header(kiln_api::SSH_KEY_HEADER, key)
        .send()
        .await
        .map_err(|_| upstream_error())?;
    if upstream.status() != StatusCode::SWITCHING_PROTOCOLS {
        return Err(Failure::new(
            upstream.status(),
            "ssh_unavailable",
            "SSH unavailable; the box must be running a guest-access image.",
        ));
    }
    if upstream
        .headers()
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        != Some(kiln_api::SSH_UPGRADE)
    {
        return Err(upstream_error());
    }
    let upstream = upstream.upgrade().await.map_err(|_| upstream_error())?;
    Ok(crate::ssh::upgrade(request, upstream, permit))
}

async fn read_proxy(
    State(state): State<Arc<Gateway>>,
    request: Request,
) -> Result<Response, Failure> {
    authenticate(&state, request.headers(), request.uri(), Scope::Read).await?;
    proxy(state, request).await
}

async fn operate_proxy(
    State(state): State<Arc<Gateway>>,
    request: Request,
) -> Result<Response, Failure> {
    authenticate(&state, request.headers(), request.uri(), Scope::Operate).await?;
    proxy(state, request).await
}

async fn proxy(state: Arc<Gateway>, request: Request) -> Result<Response, Failure> {
    let _permit = state.capacity.try_acquire().map_err(|_| {
        Failure::new(
            StatusCode::TOO_MANY_REQUESTS,
            "busy",
            "Gateway is busy; request was not forwarded.",
        )
    })?;
    let method = request.method().clone();
    let uri = format!("http://localhost{}", request.uri().path());
    let content_type = request.headers().get(header::CONTENT_TYPE).cloned();
    let bytes = tokio::time::timeout(
        Duration::from_secs(5),
        to_bytes(request.into_body(), kiln_api::MAX_REQUEST_BYTES),
    )
    .await
    .map_err(|_| {
        Failure::new(
            StatusCode::REQUEST_TIMEOUT,
            "body_timeout",
            "Request body timed out.",
        )
    })?
    .map_err(|_| {
        Failure::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "body_too_large",
            "Request body exceeds 128 KiB.",
        )
    })?;
    let mut upstream = state.client.request(method, uri).body(bytes);
    if let Some(content_type) = content_type {
        upstream = upstream.header(header::CONTENT_TYPE, content_type);
    }
    let mut response = upstream.send().await.map_err(|_| upstream_error())?;
    let status = response.status();
    let content_type = response.headers().get(header::CONTENT_TYPE).cloned();
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| upstream_error())? {
        if body.len() + chunk.len() > kiln_api::MAX_RESPONSE_BYTES {
            return Err(upstream_error());
        }
        body.extend_from_slice(&chunk);
    }
    let mut response = (status, Body::from(body)).into_response();
    if let Some(content_type) = content_type {
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, content_type);
    }
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    Ok(response)
}

fn upstream_error() -> Failure {
    Failure::new(
        StatusCode::BAD_GATEWAY,
        "host_unavailable",
        "Host connection failed. A submitted operation may have been accepted; query its request ID before retrying.",
    )
}

//! Unprivileged TLS edge. The only upstream is the configured Unix socket.
use crate::Failure;
use anyhow::{Result, ensure};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc, time::Duration};
use subtle::ConstantTimeEq;
use tokio::sync::Semaphore;

struct Gateway {
    client: reqwest::Client,
    token_hash: [u8; 32],
    capacity: Semaphore,
}

pub fn router(socket: &Path, token: &str) -> Result<Router> {
    ensure!(
        token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()),
        "token must be 32 random bytes encoded as 64 hexadecimal characters"
    );
    let state = Arc::new(Gateway {
        client: reqwest::Client::builder()
            .unix_socket(socket)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(35))
            .build()?,
        token_hash: Sha256::digest(token.as_bytes()).into(),
        capacity: Semaphore::new(32),
    });
    Ok(Router::new()
        .route("/v1/templates", get(proxy))
        .route("/v1/boxes", get(proxy))
        .route("/v1/boxes/{id}", get(proxy))
        .route("/v1/operations/{id}", get(proxy))
        .route("/v1/operations", post(proxy))
        .with_state(state))
}

async fn proxy(State(state): State<Arc<Gateway>>, request: Request) -> Result<Response, Failure> {
    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");
    let supplied: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    if !bool::from(supplied.ct_eq(&state.token_hash)) {
        return Err(Failure::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "A valid administrator token is required.",
        ));
    }
    if request.uri().query().is_some() {
        return Err(Failure::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Query parameters are not accepted.",
        ));
    }
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
        to_bytes(request.into_body(), box_api::MAX_REQUEST_BYTES),
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
        if body.len() + chunk.len() > box_api::MAX_RESPONSE_BYTES {
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

//! Public TLS door. It checks an identity JWT, then dials loopback kiln-api.
//! The administrator token is not a JWT and never opens that dial.
use crate::{Failure, auth::Authenticator};
use anyhow::{Context, Result, ensure};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::Semaphore;

struct Door {
    auth: Authenticator,
    client: reqwest::Client,
    upstream: reqwest::Url,
    capacity: Semaphore,
}

pub fn router(auth: Authenticator, upstream: &str, ca_pem: &[u8]) -> Result<Router> {
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
    let client = reqwest::Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(35))
        .add_root_certificate(reqwest::Certificate::from_pem(ca_pem)?)
        .tls_built_in_root_certs(false)
        .build()?;
    let state = Arc::new(Door {
        auth,
        client,
        upstream,
        capacity: Semaphore::new(32),
    });
    Ok(Router::new().fallback(any(handle)).with_state(state))
}

pub async fn serve(
    listen: SocketAddr,
    tls_cert: &std::path::Path,
    tls_key: &std::path::Path,
    auth: Authenticator,
    upstream: &str,
    ca_pem: &[u8],
) -> Result<()> {
    ensure!(
        !listen.ip().is_unspecified(),
        "door must bind one address so it does not take kiln-api's loopback port"
    );
    ensure!(
        unsafe { libc::geteuid() } != 0,
        "public door must run as an unprivileged user"
    );
    let router = router(auth, upstream, ca_pem)?;
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(tls_cert, tls_key).await?;
    eprintln!("public door listening on {listen}");
    axum_server::bind_rustls(listen, tls)
        .serve(router.into_make_service())
        .await?;
    Ok(())
}

async fn handle(State(door): State<Arc<Door>>, request: Request) -> Response {
    let bearer = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");
    if door.auth.authenticate_central(bearer).await.is_err() {
        return Failure::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "A valid credential is required.",
        )
        .into_response();
    }
    if request.uri().query().is_some() {
        return Failure::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Query parameters are not accepted.",
        )
        .into_response();
    }
    if request.method() != axum::http::Method::GET && request.method() != axum::http::Method::POST {
        return Failure::new(
            StatusCode::METHOD_NOT_ALLOWED,
            "invalid_request",
            "Only GET and POST are accepted.",
        )
        .into_response();
    }
    if request.headers().get(header::UPGRADE).is_some() {
        return Failure::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "SSH is not served by the public door.",
        )
        .into_response();
    }
    let Ok(_permit) = door.capacity.try_acquire() else {
        return Failure::new(
            StatusCode::TOO_MANY_REQUESTS,
            "busy",
            "Gateway is busy; request was not forwarded.",
        )
        .into_response();
    };
    match forward(&door, request).await {
        Ok(response) => response,
        Err(failure) => failure.into_response(),
    }
}

async fn forward(door: &Door, request: Request) -> Result<Response, Failure> {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let content_type = request.headers().get(header::CONTENT_TYPE).cloned();
    let authorization = request
        .headers()
        .get(header::AUTHORIZATION)
        .cloned()
        .ok_or_else(|| {
            Failure::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "A valid credential is required.",
            )
        })?;
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
    let mut url = door.upstream.clone();
    url.set_path(&path);
    let mut upstream = door
        .client
        .request(method, url)
        .header(header::AUTHORIZATION, authorization)
        .body(bytes);
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
        "upstream",
        "The local API did not answer.",
    )
}

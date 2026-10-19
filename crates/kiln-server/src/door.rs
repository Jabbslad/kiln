//! Public TLS door. It checks an identity JWT, then dials loopback kiln-api.
//! The administrator token is not a JWT and never opens that dial.
use crate::{Failure, auth::Authenticator};
use anyhow::{Context, Result, ensure};
use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use futures_util::{Stream, StreamExt};
use std::{net::SocketAddr, pin::Pin, sync::Arc, time::Duration};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

struct Door {
    auth: Authenticator,
    client: reqwest::Client,
    upstream: reqwest::Url,
    capacity: Arc<Semaphore>,
}

/// Idle time allowed while waiting for the next upstream byte.
///
/// This is not a total deadline. A 35s total cap expired during a silent
/// `kiln create` and while `kiln exec` was still streaming. 3605s is the
/// client's maximum wait for the next byte. Each chunk starts that wait over.
pub const READ_TIMEOUT: Duration = Duration::from_secs(3605);

pub fn router(auth: Authenticator, upstream: &str, ca_pem: &[u8]) -> Result<Router> {
    router_with_read_timeout(auth, upstream, ca_pem, READ_TIMEOUT)
}

pub fn router_with_read_timeout(
    auth: Authenticator,
    upstream: &str,
    ca_pem: &[u8],
    read_timeout: Duration,
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
    let client = reqwest::Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(3))
        .read_timeout(read_timeout)
        .add_root_certificate(reqwest::Certificate::from_pem(ca_pem)?)
        .tls_built_in_root_certs(false)
        .build()?;
    let state = Arc::new(Door {
        auth,
        client,
        upstream,
        capacity: Arc::new(Semaphore::new(32)),
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
    let Ok(permit) = Arc::clone(&door.capacity).try_acquire_owned() else {
        return Failure::new(
            StatusCode::TOO_MANY_REQUESTS,
            "busy",
            "Gateway is busy; request was not forwarded.",
        )
        .into_response();
    };
    match forward(&door, request, permit).await {
        Ok(response) => response,
        Err(failure) => failure.into_response(),
    }
}

async fn forward(
    door: &Door,
    request: Request,
    permit: OwnedSemaphorePermit,
) -> Result<Response, Failure> {
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
    if declared_length(request.headers(), kiln_api::MAX_REQUEST_BYTES) {
        return Err(Failure::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "body_too_large",
            "Request body exceeds 128 KiB.",
        ));
    }
    let empty = method == axum::http::Method::GET
        || request
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            == Some("0");
    let mut url = door.upstream.clone();
    url.set_path(&path);
    let mut upstream = door
        .client
        .request(method.clone(), url)
        .header(header::AUTHORIZATION, authorization);
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
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    Ok(response)
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

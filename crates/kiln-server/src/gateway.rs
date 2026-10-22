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
use kiln_api::{OperationSubmitQuery, auth::Scope};
use std::{path::Path, sync::Arc, time::Duration};
use tokio::sync::Semaphore;

struct Gateway {
    client: reqwest::Client,
    auth: Authenticator,
    capacity: Semaphore,
    sessions: Arc<Semaphore>,
    workspace: String,
}

pub fn router(socket: &Path, token: &str) -> Result<Router> {
    router_with_auth(socket, Authenticator::new(token, None)?)
}

pub fn router_with_auth(socket: &Path, auth: Authenticator) -> Result<Router> {
    router_with_workspace(socket, auth, String::new())
}

pub fn router_with_workspace(
    socket: &Path,
    auth: Authenticator,
    workspace: String,
) -> Result<Router> {
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
        workspace,
    });
    Ok(Router::new()
        .route("/api/v1/dashboard", get(dashboard))
        .route("/v1/templates", get(read_proxy))
        .route("/v1/boxes", get(read_proxy))
        .route("/v1/boxes/{id}", get(read_proxy))
        .route("/v1/boxes/{id}/ssh-key", get(operate_proxy))
        .route("/v1/boxes/{id}/ssh", get(ssh_tunnel))
        .route("/v1/operations/{id}", get(operate_proxy))
        .route("/v1/operations", post(operate_proxy))
        .with_state(state))
}

fn validate_uri_query(
    method: &axum::http::Method,
    uri: &Uri,
) -> Result<OperationSubmitQuery, Failure> {
    match OperationSubmitQuery::parse(uri.path(), uri.query()) {
        Ok(query) => {
            if uri.query().is_some()
                && !(method == axum::http::Method::POST && uri.path() == "/v1/operations")
            {
                return Err(Failure::new(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "Query parameters are not accepted.",
                ));
            }
            Ok(query)
        }
        Err(message) => Err(Failure::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            message,
        )),
    }
}

async fn authenticate(
    state: &Gateway,
    headers: &HeaderMap,
    method: &axum::http::Method,
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
    validate_uri_query(method, uri)?;
    Ok(())
}

async fn ssh_tunnel(
    State(state): State<Arc<Gateway>>,
    request: Request,
) -> Result<Response, Failure> {
    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");
    let cancel = match state.auth.central_identity(token).await {
        Ok(identity) => {
            if identity.scope != Scope::Operate {
                return Err(Failure::new(
                    StatusCode::FORBIDDEN,
                    "insufficient_scope",
                    "An operate credential is required.",
                ));
            }
            state.auth.track_stream(&identity)
        }
        Err(_) if state.auth.authenticate(token).await.is_ok() => {
            crate::revocation::StreamCancel::new()
        }
        Err(_) => {
            return Err(Failure::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "A valid credential is required.",
            ));
        }
    };
    validate_uri_query(request.method(), request.uri())?;
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
        return Err(host_ssh_error(upstream).await);
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
    Ok(crate::ssh::upgrade(request, upstream, permit, cancel))
}

async fn dashboard(
    State(state): State<Arc<Gateway>>,
    request: Request,
) -> Result<Response, Failure> {
    let _grant = authenticate(
        &state,
        request.headers(),
        request.method(),
        request.uri(),
        Scope::Read,
    )
    .await?;
    let observed_at = crate::dashboard::utc_timestamp(crate::dashboard::unix_now());
    match proxy(
        Arc::clone(&state),
        request,
        OperationSubmitQuery {
            wait: false,
            wait_seconds: kiln_api::DEFAULT_OPERATION_WAIT_SECONDS,
        },
        None,
    )
    .await
    {
        Ok(response) if response.status().is_success() => {
            stamp(response, &state.workspace, &observed_at).await
        }
        Err(failure) if socket_failed(&failure) => Ok(json_document(
            StatusCode::OK,
            crate::dashboard::document(
                &state.workspace,
                &observed_at,
                "offline",
                crate::metrics::Snapshot::default(),
                &crate::dashboard::Inventory::default(),
            ),
        )),
        Err(failure) => Err(failure),
        Ok(response) => Ok(response),
    }
}

fn socket_failed(failure: &Failure) -> bool {
    failure.0 == StatusCode::BAD_GATEWAY && failure.1.code == "host_unavailable"
}

async fn stamp(
    response: Response,
    workspace: &str,
    observed_at: &str,
) -> Result<Response, Failure> {
    let (parts, body) = response.into_parts();
    let bytes = to_bytes(body, kiln_api::MAX_RESPONSE_BYTES)
        .await
        .map_err(|_| upstream_error())?;
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Ok(Response::from_parts(parts, Body::from(bytes)));
    };
    crate::dashboard::stamp_value(&mut value, workspace, observed_at);
    Ok(json_document(parts.status, value))
}

fn json_document(status: StatusCode, body: impl serde::Serialize) -> Response {
    let mut response = (status, axum::Json(body)).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}

async fn read_proxy(
    State(state): State<Arc<Gateway>>,
    request: Request,
) -> Result<Response, Failure> {
    let _grant = authenticate(
        &state,
        request.headers(),
        request.method(),
        request.uri(),
        Scope::Read,
    )
    .await?;
    proxy(
        state,
        request,
        OperationSubmitQuery {
            wait: false,
            wait_seconds: kiln_api::DEFAULT_OPERATION_WAIT_SECONDS,
        },
        None,
    )
    .await
}

async fn operate_proxy(
    State(state): State<Arc<Gateway>>,
    request: Request,
) -> Result<Response, Failure> {
    let query = validate_uri_query(request.method(), request.uri())?;
    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");
    let _grant = authenticate(
        &state,
        request.headers(),
        request.method(),
        request.uri(),
        Scope::Operate,
    )
    .await?;
    let actor = state.auth.central_subject(token).await;
    proxy(state, request, query, actor).await
}

async fn proxy(
    state: Arc<Gateway>,
    request: Request,
    query: OperationSubmitQuery,
    actor: Option<String>,
) -> Result<Response, Failure> {
    let _permit = state.capacity.try_acquire().map_err(|_| {
        Failure::new(
            StatusCode::TOO_MANY_REQUESTS,
            "busy",
            "Gateway is busy; request was not forwarded.",
        )
    })?;
    let method = request.method().clone();
    let mut uri = format!("http://localhost{}", request.uri().path());
    if let Some(query) = request.uri().query() {
        uri.push('?');
        uri.push_str(query);
    }
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
    if let Some(actor) = actor {
        upstream = upstream.header(kiln_api::OPERATION_ACTOR_HEADER, actor);
    }
    if query.wait {
        upstream = upstream.timeout(Duration::from_secs(query.wait_seconds.saturating_add(5)));
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

async fn host_ssh_error(upstream: reqwest::Response) -> Failure {
    let status = upstream.status();
    let bytes = match upstream.bytes().await {
        Ok(bytes) if bytes.len() <= kiln_api::MAX_RESPONSE_BYTES => bytes,
        _ => return upstream_error(),
    };
    if let Ok(error) = serde_json::from_slice::<kiln_api::ApiError>(&bytes)
        && !error.code.is_empty()
    {
        return Failure::new(status, &error.code, &error.message);
    }
    Failure::new(
        status,
        "ssh_unavailable",
        "SSH unavailable; the box must be running a guest-access image.",
    )
}

fn upstream_error() -> Failure {
    Failure::new(
        StatusCode::BAD_GATEWAY,
        "host_unavailable",
        "Host connection failed. A submitted operation may have been accepted; query its request ID before retrying.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn oversized_host_body_is_an_error() {
        let response = Response::builder()
            .status(StatusCode::OK)
            .body(Body::from(vec![b'x'; kiln_api::MAX_RESPONSE_BYTES + 1]))
            .unwrap();
        let failure = stamp(response, "server-b", "1970-01-01T00:00:00Z")
            .await
            .expect_err("oversized body");
        assert_eq!(failure.0, StatusCode::BAD_GATEWAY);
        assert_eq!(failure.1.code, "host_unavailable");
        let response = failure.into_response();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        assert!(!bytes.is_empty());
    }
}

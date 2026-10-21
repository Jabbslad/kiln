//! Reproduce deploy/github/check-dashboard-metrics.sh host CPU parsing against
//! the live gateway dashboard handler (same path as https://ADDRESS:8443/...).

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use kiln_server::{
    gateway,
    host::{Config, Host, bind_socket},
};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tower::ServiceExt;

fn extract_host_cpu_global(body: &str) -> Option<f64> {
    let line = body.replace('\n', "");
    let key = "\"cpu_percent\":";
    let at = line.find(key)?;
    let rest = line[at + key.len()..].trim_start();
    let first = rest.chars().next()?;
    if !first.is_ascii_digit() {
        return None;
    }
    let end = rest
        .char_indices()
        .find(|(_, ch)| !ch.is_ascii_digit() && *ch != '.')
        .map(|(idx, _)| idx)
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn extract_host_cpu_host_metrics(body: &str) -> Option<f64> {
    let line = body.replace('\n', "");
    let host = line.find("\"host\":")?;
    let metrics = line[host..].find("\"metrics\":")? + host;
    let key = "\"cpu_percent\":";
    let at = line[metrics..].find(key)? + metrics;
    let rest = line[at + key.len()..].trim_start();
    let first = rest.chars().next()?;
    if !first.is_ascii_digit() {
        return None;
    }
    let end = rest
        .char_indices()
        .find(|(_, ch)| !ch.is_ascii_digit() && *ch != '.')
        .map(|(idx, _)| idx)
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

async fn gateway_dashboard_body() -> String {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config = Config {
        runtime_dir: root.path().join("runtime"),
        journal_dir: root.path().join("journal"),
        socket: root.path().join("host.sock"),
        isolation_config: None,
        allow_unsafe_development: true,
        templates: Default::default(),
    };
    let hits = Arc::new(AtomicUsize::new(0));
    let seen = hits.clone();
    let router = Host::open(&config)
        .unwrap()
        .router()
        .layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let seen = seen.clone();
                async move {
                    seen.fetch_add(1, Ordering::SeqCst);
                    next.run(request).await
                }
            },
        ));
    let listener = bind_socket(&config.socket).await.unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let token = "f".repeat(64);
    let gateway = gateway::router_with_workspace(
        &config.socket,
        kiln_server::auth::Authenticator::new(&token, None).unwrap(),
        String::new(),
    )
    .unwrap();
    let response = gateway
        .oneshot(
            Request::builder()
                .uri("/api/v1/dashboard")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "expected one host dashboard read"
    );
    String::from_utf8(
        to_bytes(response.into_body(), kiln_api::MAX_RESPONSE_BYTES)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
async fn fresh_gateway_dashboard_passes_deploy_host_cpu_check() {
    let body = gateway_dashboard_body().await;
    let host_at = body.find("\"host\":").expect("host object");
    let boxes_at = body.find("\"boxes\":").unwrap_or(body.len());
    assert!(
        boxes_at < host_at,
        "gateway stamp alphabetizes keys; deploy must read host.metrics explicitly"
    );
    assert!(
        extract_host_cpu_host_metrics(&body).is_some(),
        "host.metrics.cpu_percent must be numeric: {body}"
    );
}

#[test]
fn deploy_global_cpu_parse_fails_when_boxes_precede_host() {
    let body = r#"{"boxes":[{"state":"stopped","metrics":{"cpu_percent":null,"memory_bytes":1}}],"host":{"name":"local","state":"online","metrics":{"cpu_percent":0.25,"memory_bytes":100}}}"#;
    assert!(extract_host_cpu_host_metrics(body).is_some());
    assert!(extract_host_cpu_global(body).is_none());
}

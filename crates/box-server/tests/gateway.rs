use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use box_server::{
    gateway,
    host::{Config, Host, bind_socket},
};
use tower::ServiceExt;

#[tokio::test]
async fn ssh_routes_require_authentication_before_connecting() {
    let token = "a".repeat(64);
    let router = gateway::router(std::path::Path::new("/missing/host.sock"), &token).unwrap();
    for suffix in ["ssh", "ssh-key"] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/boxes/{}/{suffix}", "b".repeat(32)))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn authentication_precedes_proxying_and_credentials_never_reach_the_host() {
    let root = tempfile::tempdir().unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let config = Config {
        runtime_dir: root.path().join("runtime"),
        journal_dir: root.path().join("journal"),
        socket: root.path().join("host.sock"),
        templates: Default::default(),
        isolation_config: None,
        allow_unsafe_development: true,
    };
    let router = Host::open(&config)
        .unwrap()
        .router()
        .layer(axum::middleware::from_fn(
            |request: axum::extract::Request, next: axum::middleware::Next| async move {
                assert!(!request.headers().contains_key("authorization"));
                next.run(request).await
            },
        ));
    let listener = bind_socket(&config.socket).await.unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let token = "b".repeat(64);
    let router = gateway::router(&config.socket, &token).unwrap();
    for value in [
        None,
        Some("Bearer wrong".to_string()),
        Some(format!("Basic {token}")),
    ] {
        let mut request = Request::builder().uri("/v1/templates");
        if let Some(value) = value {
            request = request.header("authorization", value);
        }
        let response = router
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(
            !String::from_utf8_lossy(&to_bytes(response.into_body(), 1024).await.unwrap())
                .contains(&token)
        );
    }
    let response = router
        .oneshot(
            Request::builder()
                .uri("/v1/templates")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        &to_bytes(response.into_body(), 1024).await.unwrap()[..],
        b"[]"
    );
    server.abort();
    let _ = server.await;
}

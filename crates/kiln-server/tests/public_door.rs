use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    body::{Body, Bytes, to_bytes},
    http::{Request, StatusCode},
};
use futures_util::{StreamExt, stream};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use kiln_api::auth::{Scope, ServerClaims};
use kiln_server::{
    auth::{Authenticator, Enrollment},
    door,
};
use tower::ServiceExt;

fn sockets(port: u16) -> usize {
    let marker = format!(":{port:04X}");
    std::fs::read_to_string("/proc/net/tcp")
        .unwrap()
        .lines()
        .filter(|line| line.contains(&marker))
        .count()
}

fn token(key: &EncodingKey, sub: &str, audience: &str) -> String {
    let now = jsonwebtoken::get_current_timestamp();
    let claims = ServerClaims {
        iss: "https://identity.example.test".into(),
        sub: sub.into(),
        aud: audience.into(),
        iat: now,
        nbf: now,
        exp: now + 300,
        credential_id: "device-1".into(),
        scope: Scope::Operate,
        tenant_id: None,
        role: None,
    };
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("first".into());
    jsonwebtoken::encode(&header, &claims, key).unwrap()
}

struct Upstream {
    port: u16,
    hits: Arc<AtomicUsize>,
    ca_pem: String,
}

async fn listen_tls(app: axum::Router) -> (u16, String) {
    let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
    let ca_pem = cert.cert.pem();
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        ca_pem.clone().into_bytes(),
        cert.signing_key.serialize_pem().into_bytes(),
    )
    .await
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, tls)
            .unwrap()
            .serve(app.into_make_service())
            .await
            .unwrap();
    });
    (port, ca_pem)
}

async fn upstream() -> Upstream {
    let hits = Arc::new(AtomicUsize::new(0));
    let seen = hits.clone();
    let app = axum::Router::new().route(
        "/v1/templates",
        axum::routing::get(move || {
            let seen = seen.clone();
            async move {
                seen.fetch_add(1, Ordering::SeqCst);
                axum::Json(serde_json::json!([]))
            }
        }),
    );
    let (port, ca_pem) = listen_tls(app).await;
    Upstream { port, hits, ca_pem }
}

fn open_door(
    up: &Upstream,
    root: &std::path::Path,
    read_timeout: Duration,
) -> (axum::Router, EncodingKey) {
    let (auth, key) = auth(root);
    let router = door::router_with_read_timeout(
        auth,
        &format!("https://127.0.0.1:{}", up.port),
        up.ca_pem.as_bytes(),
        read_timeout,
    )
    .unwrap();
    (router, key)
}

fn authed(key: &EncodingKey, method: &str, uri: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(
            "authorization",
            format!("Bearer {}", token(key, "owner-a", "kiln:server:server-b")),
        )
        .body(body)
        .unwrap()
}

fn auth(dir: &std::path::Path) -> (Authenticator, EncodingKey) {
    use base64::Engine;
    use p256::{elliptic_curve::sec1::ToEncodedPoint, pkcs8::EncodePrivateKey};
    let key = p256::SecretKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
    let pem = key.to_pkcs8_pem(Default::default()).unwrap();
    let point = key.public_key().to_encoded_point(false);
    let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let jwks = serde_json::json!({"keys":[{
        "kty":"EC","crv":"P-256","use":"sig","alg":"ES256","kid":"first",
        "x":enc.encode(point.x().unwrap()),"y":enc.encode(point.y().unwrap())
    }]});
    let enrollment = Enrollment {
        version: 1,
        issuer: "https://identity.example.test".into(),
        owner_id: "owner-a".into(),
        server_id: "server-b".into(),
        tenant_id: String::new(),
    };
    let cache = dir.join("jwks.json");
    std::fs::write(
        &cache,
        serde_json::to_vec(&serde_json::json!({"issuer": enrollment.issuer, "jwks": jwks}))
            .unwrap(),
    )
    .unwrap();
    (
        Authenticator::central_only(enrollment, cache).unwrap(),
        EncodingKey::from_ec_pem(pem.as_bytes()).unwrap(),
    )
}

#[tokio::test]
async fn unauthenticated_and_wrong_account_requests_do_not_reach_the_api() {
    let root = tempfile::tempdir().unwrap();
    let up = upstream().await;
    let (auth, key) = auth(root.path());
    let router = door::router(
        auth,
        &format!("https://127.0.0.1:{}", up.port),
        up.ca_pem.as_bytes(),
    )
    .unwrap();
    let before = sockets(up.port);
    for (label, header) in [
        ("missing", None),
        ("admin", Some(format!("Bearer {}", "ab".repeat(32)))),
        (
            "wrong-account",
            Some(format!(
                "Bearer {}",
                token(&key, "owner-b", "kiln:server:server-b")
            )),
        ),
        (
            "wrong-server",
            Some(format!(
                "Bearer {}",
                token(&key, "owner-a", "kiln:server:server-c")
            )),
        ),
    ] {
        let mut request = Request::builder().uri("/v1/templates");
        if let Some(header) = header {
            request = request.header("authorization", header);
        }
        let response = router
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{label}");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(up.hits.load(Ordering::SeqCst), 0, "{label}");
        assert_eq!(sockets(up.port), before, "{label}");
    }
    let response = router
        .oneshot(
            Request::builder()
                .uri("/v1/templates")
                .header(
                    "authorization",
                    format!("Bearer {}", token(&key, "owner-a", "kiln:server:server-b")),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(up.hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn slow_upstream_outlasts_a_stand_in_for_the_old_total_cap() {
    assert_eq!(door::READ_TIMEOUT, Duration::from_secs(3605));
    // 400ms stands in for the removed 35s total deadline.
    let stand_in_for_35s = Duration::from_millis(400);
    let stall = stand_in_for_35s + Duration::from_millis(500);
    assert!(stall > stand_in_for_35s);
    let app = axum::Router::new().route(
        "/v1/templates",
        axum::routing::get(move || async move {
            tokio::time::sleep(stall).await;
            "ok"
        }),
    );
    let (port, ca_pem) = listen_tls(app).await;
    let root = tempfile::tempdir().unwrap();
    let up = Upstream {
        port,
        hits: Arc::new(AtomicUsize::new(0)),
        ca_pem,
    };
    let (router, key) = open_door(&up, root.path(), Duration::from_secs(5));
    let response = tokio::time::timeout(
        Duration::from_secs(4),
        router.oneshot(authed(&key, "GET", "/v1/templates", Body::empty())),
    )
    .await
    .expect("silent upstream exceeded the read timeout")
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 64).await.unwrap();
    assert_eq!(&body[..], b"ok");
}

#[tokio::test]
async fn streamed_gaps_outlast_a_total_deadline() {
    // The bound is the door's read timeout and the stand-in for a total cap.
    // Each gap fits in a read. The sum does not fit in a total deadline.
    let bound = Duration::from_millis(1500);
    let gap = Duration::from_millis(900);
    assert!(gap < bound);
    assert!(gap + gap > bound);
    let app = axum::Router::new().route(
        "/v1/templates",
        axum::routing::get(move || async move {
            Body::from_stream(stream::unfold(0, move |step| async move {
                match step {
                    0 => Some((Ok::<_, std::io::Error>(Bytes::from_static(b"a")), 1)),
                    1 => {
                        tokio::time::sleep(gap).await;
                        Some((Ok(Bytes::from_static(b"b")), 2))
                    }
                    2 => {
                        tokio::time::sleep(gap).await;
                        Some((Ok(Bytes::from_static(b"c")), 3))
                    }
                    _ => None,
                }
            }))
        }),
    );
    let (port, ca_pem) = listen_tls(app).await;
    let root = tempfile::tempdir().unwrap();
    let up = Upstream {
        port,
        hits: Arc::new(AtomicUsize::new(0)),
        ca_pem,
    };
    let (router, key) = open_door(&up, root.path(), bound);
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        router.oneshot(authed(&key, "GET", "/v1/templates", Body::empty())),
    )
    .await
    .expect("gaps were treated as one total deadline")
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 64).await.unwrap();
    assert_eq!(&body[..], b"abc");
}

#[tokio::test]
async fn response_chunks_are_forwarded_as_they_arrive() {
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let release_rx = Arc::new(std::sync::Mutex::new(Some(release_rx)));
    let app = axum::Router::new().route(
        "/v1/exec",
        axum::routing::post(move || {
            let release_rx = release_rx.lock().unwrap().take().unwrap();
            async move {
                Body::from_stream(stream::unfold(
                    (0, Some(release_rx)),
                    |(step, release_rx)| async move {
                        match step {
                            0 => Some((
                                Ok::<_, std::io::Error>(Bytes::from_static(b"one")),
                                (1, release_rx),
                            )),
                            1 => {
                                let release_rx = release_rx.unwrap();
                                let _ = release_rx.await;
                                Some((Ok(Bytes::from_static(b"two")), (2, None)))
                            }
                            _ => None,
                        }
                    },
                ))
            }
        }),
    );
    let (port, ca_pem) = listen_tls(app).await;
    let root = tempfile::tempdir().unwrap();
    let up = Upstream {
        port,
        hits: Arc::new(AtomicUsize::new(0)),
        ca_pem,
    };
    let (router, key) = open_door(&up, root.path(), Duration::from_secs(5));
    let response = tokio::time::timeout(
        Duration::from_secs(3),
        router.oneshot(authed(&key, "POST", "/v1/exec", Body::empty())),
    )
    .await
    .expect("door buffered the response until the upstream finished")
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut incoming = response.into_body().into_data_stream();
    let first = tokio::time::timeout(Duration::from_secs(3), incoming.next())
        .await
        .expect("first chunk was held back")
        .unwrap()
        .unwrap();
    assert_eq!(&first[..], b"one");
    release_tx.send(()).unwrap();
    let second = tokio::time::timeout(Duration::from_secs(3), incoming.next())
        .await
        .expect("second chunk did not arrive")
        .unwrap()
        .unwrap();
    assert_eq!(&second[..], b"two");
    assert!(incoming.next().await.is_none());
}

#[tokio::test]
async fn request_chunks_reach_upstream_before_the_body_finishes() {
    let (saw_tx, saw_rx) = tokio::sync::oneshot::channel::<()>();
    let saw_tx = Arc::new(std::sync::Mutex::new(Some(saw_tx)));
    let app = axum::Router::new().route(
        "/v1/exec",
        axum::routing::post(move |body: Body| {
            let saw_tx = saw_tx.lock().unwrap().take().unwrap();
            async move {
                let mut saw_tx = Some(saw_tx);
                let mut incoming = body.into_data_stream();
                let mut got = Vec::new();
                loop {
                    let Some(chunk) = incoming.next().await else {
                        break;
                    };
                    got.extend_from_slice(&chunk.unwrap());
                    if got == b"one"
                        && let Some(tx) = saw_tx.take()
                    {
                        tx.send(()).unwrap();
                    }
                }
                assert_eq!(got, b"onetwo");
                "ok"
            }
        }),
    );
    let (port, ca_pem) = listen_tls(app).await;
    let root = tempfile::tempdir().unwrap();
    let up = Upstream {
        port,
        hits: Arc::new(AtomicUsize::new(0)),
        ca_pem,
    };
    let (router, key) = open_door(&up, root.path(), Duration::from_secs(5));
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let body = Body::from_stream(stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    }));
    let send = tokio::spawn(async move {
        tx.send(Ok::<_, std::io::Error>(Bytes::from_static(b"one")))
            .await
            .unwrap();
        saw_rx.await.unwrap();
        tx.send(Ok(Bytes::from_static(b"two"))).await.unwrap();
    });
    let response = tokio::time::timeout(
        Duration::from_secs(3),
        router.oneshot(authed(&key, "POST", "/v1/exec", body)),
    )
    .await
    .expect("door buffered the request until the client finished")
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response_body = to_bytes(response.into_body(), 64).await.unwrap();
    assert_eq!(&response_body[..], b"ok");
    send.await.unwrap();
}

#[tokio::test]
async fn dashboard_read_rejects_untrusted_bearers_and_forwards_the_owner_jwt() {
    let hits = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(std::sync::Mutex::new(None::<String>));
    let hit = hits.clone();
    let recorded = seen.clone();
    let app = axum::Router::new().route(
        "/api/v1/dashboard",
        axum::routing::get(move |headers: axum::http::HeaderMap| {
            let hit = hit.clone();
            let recorded = recorded.clone();
            async move {
                hit.fetch_add(1, Ordering::SeqCst);
                *recorded.lock().unwrap() = headers
                    .get(axum::http::header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                "ok"
            }
        }),
    );
    let (port, ca_pem) = listen_tls(app).await;
    let root = tempfile::tempdir().unwrap();
    let (auth, key) = auth(root.path());
    let router = door::router(
        auth,
        &format!("https://127.0.0.1:{port}"),
        ca_pem.as_bytes(),
    )
    .unwrap();
    let before = sockets(port);
    let admin = format!("Bearer {}", "ab".repeat(32));
    for (label, header) in [
        ("missing", None),
        ("bogus", Some("Bearer not-a-jwt".to_string())),
        ("admin", Some(admin)),
    ] {
        let mut request = Request::builder().uri("/api/v1/dashboard");
        if let Some(header) = header {
            request = request.header("authorization", header);
        }
        let response = router
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{label}");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(hits.load(Ordering::SeqCst), 0, "{label}");
        assert_eq!(sockets(port), before, "{label}");
    }
    let authorization = format!("Bearer {}", token(&key, "owner-a", "kiln:server:server-b"));
    let response = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/dashboard")
                .header("authorization", &authorization)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 64).await.unwrap();
    assert_eq!(&body[..], b"ok");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        seen.lock().unwrap().as_deref(),
        Some(authorization.as_str())
    );
}

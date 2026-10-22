//! Public-door SSH proxy. Auth failures never dial loopback. An operate JWT
//! for the enrolled owner is forwarded as itself and round-trips through the
//! gateway into the host harness.
use std::{
    net::IpAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    body::Body,
    extract::Request,
    http::{StatusCode, header},
    middleware::Next,
    response::IntoResponse,
};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use kiln_api::auth::{Role, Scope, ServerClaims};
use kiln_server::{
    auth::{Authenticator, Enrollment},
    door, gateway,
};
#[path = "common.rs"]
mod common;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

const SSH_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

fn token(key: &EncodingKey, sub: &str, scope: Scope) -> String {
    token_for_tenant(
        key,
        sub,
        common::FIXTURE_TENANT_ID,
        if sub == "owner-a" {
            Role::Owner
        } else {
            Role::Member
        },
        scope,
    )
}

fn token_for_tenant(
    key: &EncodingKey,
    sub: &str,
    tenant_id: &str,
    role: Role,
    scope: Scope,
) -> String {
    let now = jsonwebtoken::get_current_timestamp();
    let claims = ServerClaims {
        iss: "https://identity.example.test".into(),
        sub: sub.into(),
        aud: "kiln:server:server-b".into(),
        tenant_id: Some(tenant_id.into()),
        iat: now,
        nbf: now,
        exp: now + 300,
        credential_id: "device-1".into(),
        scope,
        role: Some(role),
    };
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("first".into());
    jsonwebtoken::encode(&header, &claims, key).unwrap()
}

fn enrollment() -> Enrollment {
    Enrollment {
        version: 1,
        issuer: "https://identity.example.test".into(),
        owner_id: "owner-a".into(),
        server_id: "server-b".into(),
        tenant_id: common::fixture_tenant_id(),
    }
}

fn auth(dir: &std::path::Path) -> (Authenticator, Authenticator, EncodingKey, String) {
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
    let enrolled = enrollment();
    let cache = dir.join("jwks.json");
    std::fs::write(
        &cache,
        serde_json::to_vec(&serde_json::json!({"issuer": enrolled.issuer, "jwks": jwks})).unwrap(),
    )
    .unwrap();
    let admin = "ab".repeat(32);
    let gateway_auth = Authenticator::new(&admin, Some((enrolled.clone(), cache.clone()))).unwrap();
    let door_auth = Authenticator::central_only(enrolled, cache).unwrap();
    (
        door_auth,
        gateway_auth,
        EncodingKey::from_ec_pem(pem.as_bytes()).unwrap(),
        admin,
    )
}

struct Stack {
    _root: tempfile::TempDir,
    door: String,
    key: EncodingKey,
    admin: String,
    hits: Arc<AtomicUsize>,
    forwarded: Arc<Mutex<Vec<String>>>,
    audit: door::SshAudit,
}

async fn stack() -> Stack {
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("host.sock");
    let host = axum::Router::new().route(
        "/v1/boxes/{id}/ssh",
        axum::routing::get(|mut request: Request| async move {
            if request.headers().contains_key(header::AUTHORIZATION) {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            let (mut stream, mut echo) = tokio::io::duplex(64 * 1024);
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                echo.read_to_end(&mut bytes).await.unwrap();
                bytes.reverse();
                echo.write_all(&bytes).await.unwrap();
                echo.shutdown().await.unwrap();
            });
            let connection = hyper::upgrade::on(&mut request);
            tokio::spawn(async move {
                if let Ok(upgraded) = connection.await {
                    let mut io = hyper_util::rt::TokioIo::new(upgraded);
                    let _ = tokio::io::copy_bidirectional(&mut io, &mut stream).await;
                }
            });
            axum::response::Response::builder()
                .status(StatusCode::SWITCHING_PROTOCOLS)
                .header(header::CONNECTION, "upgrade")
                .header(header::UPGRADE, kiln_api::SSH_UPGRADE)
                .body(Body::empty())
                .unwrap()
        }),
    );
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    tokio::spawn(async move {
        axum::serve(listener, host).await.unwrap();
    });

    let (door_auth, gateway_auth, key, admin) = auth(root.path());
    let hits = Arc::new(AtomicUsize::new(0));
    let forwarded = Arc::new(Mutex::new(Vec::new()));
    let seen_hits = hits.clone();
    let seen_auth = forwarded.clone();
    let gateway = gateway::router_with_auth(&socket, gateway_auth)
        .unwrap()
        .layer(axum::middleware::from_fn(
            move |request: Request, next: Next| {
                let seen_hits = seen_hits.clone();
                let seen_auth = seen_auth.clone();
                async move {
                    seen_hits.fetch_add(1, Ordering::SeqCst);
                    let value = request
                        .headers()
                        .get(header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("")
                        .to_owned();
                    seen_auth.lock().unwrap().push(value);
                    next.run(request).await
                }
            },
        ));
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
    let gateway_port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, tls)
            .unwrap()
            .serve(gateway.into_make_service())
            .await
            .unwrap();
    });

    let (router, audit) = door::router_with_ssh_audit(
        door_auth,
        &format!("https://127.0.0.1:{gateway_port}"),
        ca_pem.as_bytes(),
        Duration::from_secs(30),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let door_port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    Stack {
        _root: root,
        door: format!("http://127.0.0.1:{door_port}"),
        key,
        admin,
        hits,
        forwarded,
        audit,
    }
}

fn ssh_request(url: &str, bearer: Option<&str>) -> reqwest::RequestBuilder {
    let mut request = reqwest::Client::builder()
        .http1_only()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
        .get(url)
        .header(header::CONNECTION, "upgrade")
        .header(header::UPGRADE, kiln_api::SSH_UPGRADE)
        .header(kiln_api::SSH_KEY_HEADER, SSH_KEY);
    if let Some(bearer) = bearer {
        request = request.bearer_auth(bearer);
    }
    request
}

async fn send(request: reqwest::RequestBuilder) -> reqwest::Response {
    let mut last = None;
    for _ in 0..50 {
        match request.try_clone().unwrap().send().await {
            Ok(response) => return response,
            Err(error) => {
                last = Some(error);
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    }
    panic!("door did not accept a connection: {last:?}");
}

#[tokio::test]
async fn door_ssh_rejects_bad_callers_and_round_trips_an_operate_jwt() {
    let world = stack().await;
    let owner_box = "b".repeat(32);
    let other_box = "c".repeat(32);
    let operate = token(&world.key, "owner-a", Scope::Operate);
    let other_tenant = token_for_tenant(
        &world.key,
        "member-z",
        "tenant-other",
        Role::Member,
        Scope::Operate,
    );
    let read = token(&world.key, "owner-a", Scope::Read);
    let cases = [
        (
            "missing",
            None,
            owner_box.as_str(),
            StatusCode::UNAUTHORIZED,
        ),
        (
            "invalid",
            Some("not-a-jwt"),
            owner_box.as_str(),
            StatusCode::UNAUTHORIZED,
        ),
        (
            "other-tenant",
            Some(other_tenant.as_str()),
            other_box.as_str(),
            StatusCode::UNAUTHORIZED,
        ),
        (
            "read-scope",
            Some(read.as_str()),
            owner_box.as_str(),
            StatusCode::FORBIDDEN,
        ),
        (
            "admin",
            Some(world.admin.as_str()),
            owner_box.as_str(),
            StatusCode::UNAUTHORIZED,
        ),
    ];
    for (label, bearer, id, status) in cases {
        let response = send(ssh_request(
            &format!("{}/v1/boxes/{id}/ssh", world.door),
            bearer,
        ))
        .await;
        assert_eq!(response.status(), status, "{label}");
        let body = response.text().await.unwrap();
        assert!(!body.contains(&world.admin), "{label}: {body}");
        assert!(!body.contains("SWITCHING"), "{label}");
        if label == "read-scope" {
            assert!(body.contains("insufficient_scope"), "{body}");
        }
        assert_eq!(
            world.hits.load(Ordering::SeqCst),
            0,
            "{label} dialed the gateway"
        );
    }

    let templates = send(
        reqwest::Client::builder()
            .http1_only()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
            .get(format!("{}/v1/templates", world.door))
            .bearer_auth(&operate)
            .header(header::UPGRADE, kiln_api::SSH_UPGRADE)
            .header(header::CONNECTION, "upgrade"),
    )
    .await;
    assert_eq!(templates.status(), StatusCode::BAD_REQUEST);
    let body = templates.text().await.unwrap();
    assert!(
        body.contains("SSH is not served by the public door."),
        "{body}"
    );
    assert_eq!(world.hits.load(Ordering::SeqCst), 0);

    let response = send(ssh_request(
        &format!("{}/v1/boxes/{owner_box}/ssh", world.door),
        Some(&operate),
    ))
    .await;
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    assert_eq!(
        response
            .headers()
            .get(header::UPGRADE)
            .and_then(|value| value.to_str().ok()),
        Some(kiln_api::SSH_UPGRADE)
    );
    let mut stream = response.upgrade().await.unwrap();
    let mut bytes: Vec<_> = (0..100_003).map(|i| ((i * 7 + 11) % 256) as u8).collect();
    stream.write_all(&bytes).await.unwrap();
    stream.shutdown().await.unwrap();
    let mut returned = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut returned))
        .await
        .unwrap()
        .unwrap();
    bytes.reverse();
    assert_eq!(returned, bytes);
    assert_eq!(world.hits.load(Ordering::SeqCst), 1);
    let forwarded = world.forwarded.lock().unwrap().clone();
    assert_eq!(forwarded, vec![format!("Bearer {operate}")]);
    assert!(forwarded.iter().all(|value| !value.contains(&world.admin)));

    let records = world.audit.records();
    let expected = [
        ("missing", "unauthorized", "anonymous", owner_box.as_str()),
        ("invalid", "unauthorized", "anonymous", owner_box.as_str()),
        (
            "other-tenant",
            "unauthorized",
            "anonymous",
            other_box.as_str(),
        ),
        (
            "read-scope",
            "insufficient_scope",
            "owner-a",
            owner_box.as_str(),
        ),
        ("admin", "unauthorized", "anonymous", owner_box.as_str()),
        ("success", "upgraded", "owner-a", owner_box.as_str()),
    ];
    assert_eq!(records.len(), expected.len(), "{records:?}");
    for (record, (label, outcome, caller, box_id)) in records.iter().zip(expected) {
        assert_eq!(record.outcome, outcome, "{label}");
        assert_eq!(record.caller, caller, "{label}");
        assert_eq!(record.box_id, box_id, "{label}");
        assert_eq!(record.addr, IpAddr::from([127, 0, 0, 1]), "{label}");
    }
    assert!(
        records
            .iter()
            .all(|record| !record.caller.contains(&world.admin)),
        "{records:?}"
    );
}

#[tokio::test]
async fn door_ssh_route_audits_stripped_upgrade_attempts() {
    let world = stack().await;
    let box_id = "d".repeat(32);

    let stripped = reqwest::Client::builder()
        .http1_only()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
        .get(format!("{}/v1/boxes/{box_id}/ssh", world.door))
        .send()
        .await
        .unwrap();
    assert_eq!(stripped.status(), StatusCode::UNAUTHORIZED);

    let wrong = send(
        reqwest::Client::builder()
            .http1_only()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
            .get(format!("{}/v1/boxes/{box_id}/ssh", world.door))
            .header(header::CONNECTION, "upgrade")
            .header(header::UPGRADE, "websocket"),
    )
    .await;
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

    let read = token(&world.key, "owner-a", Scope::Read);
    let invalid_upgrade = send(
        reqwest::Client::builder()
            .http1_only()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
            .get(format!("{}/v1/boxes/{box_id}/ssh", world.door))
            .bearer_auth(&read)
            .header(header::CONNECTION, "upgrade")
            .header(header::UPGRADE, "websocket"),
    )
    .await;
    assert_eq!(invalid_upgrade.status(), StatusCode::BAD_REQUEST);

    let records = world.audit.records();
    assert_eq!(records.len(), 3);
    assert_eq!(records[0].outcome, "unauthorized");
    assert_eq!(records[0].caller, "anonymous");
    assert_eq!(records[1].outcome, "unauthorized");
    assert_eq!(records[1].caller, "anonymous");
    assert_eq!(records[2].outcome, "invalid_ssh");
    assert_eq!(records[2].caller, "owner-a");
    assert!(
        records
            .iter()
            .all(|record| record.addr == IpAddr::from([127, 0, 0, 1]))
    );
}

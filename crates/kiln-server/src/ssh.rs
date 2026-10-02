//! Fixed-purpose SSH upgrades. No destination ports or host paths from clients.
use crate::Failure;
use axum::{
    extract::Request,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use std::time::Duration;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::OwnedSemaphorePermit,
};

pub fn admission(request: &Request) -> Result<String, Failure> {
    let headers = request.headers();
    let key = headers
        .get(kiln_api::SSH_KEY_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if request.uri().query().is_some()
        || !kiln_api::valid_ssh_public_key(key)
        || headers.get(header::UPGRADE).and_then(|v| v.to_str().ok()) != Some(kiln_api::SSH_UPGRADE)
        || !headers
            .get(header::CONNECTION)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| {
                v.split(',')
                    .any(|s| s.trim().eq_ignore_ascii_case("upgrade"))
            })
        || headers.contains_key(header::TRANSFER_ENCODING)
        || headers
            .get(header::CONTENT_LENGTH)
            .is_some_and(|v| v != "0")
    {
        return Err(Failure::new(
            StatusCode::BAD_REQUEST,
            "invalid_ssh",
            "A bodyless SSH upgrade with an Ed25519 public key is required.",
        ));
    }
    Ok(key.to_owned())
}

pub fn busy() -> Failure {
    Failure::new(
        StatusCode::TOO_MANY_REQUESTS,
        "ssh_busy",
        "SSH session capacity reached.",
    )
}

pub fn upgrade<S>(mut request: Request, mut upstream: S, permit: OwnedSemaphorePermit) -> Response
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let connection = hyper::upgrade::on(&mut request);
    tokio::spawn(async move {
        let _permit = permit;
        if let Ok(Ok(stream)) = tokio::time::timeout(Duration::from_secs(10), connection).await {
            let mut stream = hyper_util::rt::TokioIo::new(stream);
            // Bound abandoned sessions independently of the management API.
            let _ = tokio::time::timeout(
                Duration::from_secs(24 * 3600),
                tokio::io::copy_bidirectional(&mut stream, &mut upstream),
            )
            .await;
        }
    });
    (
        StatusCode::SWITCHING_PROTOCOLS,
        [
            (header::CONNECTION, "upgrade"),
            (header::UPGRADE, kiln_api::SSH_UPGRADE),
            (header::CACHE_CONTROL, "no-store"),
        ],
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        sync::Semaphore,
    };

    #[tokio::test]
    async fn gateway_upgrade_streams_binary_and_releases_session_on_eof() {
        let root = tempfile::tempdir().unwrap();
        let socket = root.path().join("host.sock");
        let capacity = Arc::new(Semaphore::new(1));
        let captured = capacity.clone();
        let route = axum::Router::new().route(
            "/v1/boxes/{id}/ssh",
            axum::routing::get(move |request: Request| {
                let captured = captured.clone();
                async move {
                    assert!(!request.headers().contains_key(header::AUTHORIZATION));
                    assert!(admission(&request).is_ok());
                    let permit = captured.try_acquire_owned().unwrap();
                    let (stream, mut echo) = tokio::io::duplex(8192);
                    tokio::spawn(async move {
                        let mut bytes = Vec::new();
                        echo.read_to_end(&mut bytes).await.unwrap();
                        bytes.reverse();
                        echo.write_all(&bytes).await.unwrap();
                        echo.shutdown().await.unwrap();
                    });
                    upgrade(request, stream, permit)
                }
            }),
        );
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let host = tokio::spawn(async move {
            axum::serve(listener, route).await.unwrap();
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let token = "a".repeat(64);
        let gateway = crate::gateway::router(&socket, &token).unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, gateway).await.unwrap();
        });
        let response = reqwest::Client::new()
            .get(format!("http://{address}/v1/boxes/{}/ssh", "b".repeat(32)))
            .bearer_auth(token)
            .header(header::CONNECTION, "upgrade")
            .header(header::UPGRADE, kiln_api::SSH_UPGRADE)
            .header(
                kiln_api::SSH_KEY_HEADER,
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            )
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        let mut stream = response.upgrade().await.unwrap();
        let mut bytes: Vec<_> = (0..100_003).map(|i| ((i * 7 + 11) % 256) as u8).collect();
        stream.write_all(&bytes).await.unwrap();
        stream.shutdown().await.unwrap();
        let mut returned = Vec::new();
        tokio::time::timeout(Duration::from_secs(3), stream.read_to_end(&mut returned))
            .await
            .unwrap()
            .unwrap();
        bytes.reverse();
        assert_eq!(returned, bytes);
        for _ in 0..100 {
            if capacity.available_permits() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(capacity.available_permits(), 1);
        server.abort();
        host.abort();
    }
}

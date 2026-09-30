use crate::{Error, Result};
use box_protocol::{Request, Response, VSOCK_PORT, read_frame, write_frame};
use std::{path::Path, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};

pub async fn request(socket: &Path, request: &Request, timeout: Duration) -> Result<Response> {
    let mut phase = "connect";
    tokio::time::timeout(timeout, async {
        phase = "handshake";
        let mut stream = connect(socket).await?;
        phase = "send request";
        write_frame(&mut stream, request).await?;
        phase = "receive response";
        let response: Response = read_frame(&mut stream).await?;
        match response {
            Response::Error(e) => Err(Error::Invalid(format!("guest {:?}: {}", e.code, e.message))),
            response => Ok(response),
        }
    })
    .await
    .map_err(|_| {
        Error::Invalid(format!(
            "guest transport timed out during {phase}; command outcome may be unknown"
        ))
    })?
}

async fn connect(socket: &Path) -> Result<UnixStream> {
    let mut stream = UnixStream::connect(socket).await?;
    stream
        .write_all(format!("CONNECT {VSOCK_PORT}\n").as_bytes())
        .await?;
    let mut line = Vec::new();
    loop {
        let byte = stream.read_u8().await?;
        if byte == b'\n' {
            break;
        }
        if line.len() >= 80 {
            return Err(Error::Invalid("invalid vsock handshake".into()));
        }
        line.push(byte);
    }
    if !line.starts_with(b"OK ") {
        return Err(Error::Invalid(format!(
            "vsock rejected connection: {}",
            String::from_utf8_lossy(&line)
        )));
    }
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::UnixListener;

    #[tokio::test]
    async fn never_replays_a_request_when_its_response_is_lost() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("vsock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (mut connected, _) = listener.accept().await.unwrap();
            let mut handshake = [0; 13];
            connected.read_exact(&mut handshake).await.unwrap();
            assert_eq!(&handshake, b"CONNECT 1024\n");
            connected.write_all(b"OK 123\n").await.unwrap();
            let command: Request = read_frame(&mut connected).await.unwrap();
            assert!(matches!(command, Request::Exec(_)));
            // Deliberately lose the response AFTER execution was requested.
            assert!(
                tokio::time::timeout(Duration::from_millis(600), listener.accept())
                    .await
                    .is_err()
            );
        });
        let result = request(
            &socket,
            &Request::Exec(box_protocol::ExecRequest {
                argv: vec!["increment-counter".into()],
                cwd: None,
                env: Default::default(),
                timeout_ms: 100,
            }),
            Duration::from_millis(500),
        )
        .await;
        assert!(result.unwrap_err().to_string().contains("receive response"));
        server.await.unwrap();
    }
}

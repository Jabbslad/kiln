use crate::{Error, Result};
use kiln_protocol::{Request, Response, VSOCK_PORT, read_frame, write_frame};
use std::{path::Path, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};

pub async fn request(socket: &Path, request: &Request, timeout: Duration) -> Result<Response> {
    let mut phase = "connect";
    tokio::time::timeout(timeout, async {
        phase = "handshake";
        let mut stream = connect(socket, VSOCK_PORT).await?;
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

pub async fn ssh(socket: &Path, public_key: &str) -> Result<(UnixStream, kiln_protocol::SshReady)> {
    use kiln_protocol::{SSH_VSOCK_PORT, SshConnect, SshReady, valid_ssh_public_key};
    if !valid_ssh_public_key(public_key) {
        return Err(Error::Invalid("invalid SSH public key".into()));
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut stream = connect(socket, SSH_VSOCK_PORT).await?;
        write_frame(
            &mut stream,
            &SshConnect {
                public_key: public_key.into(),
            },
        )
        .await?;
        let ready: SshReady = read_frame(&mut stream).await?;
        if !valid_ssh_public_key(&ready.public_key) {
            return Err(Error::Invalid("invalid SSH host key".into()));
        }
        Ok((stream, ready))
    })
    .await
    .map_err(|_| Error::Invalid("guest SSH connection timed out".into()))?
}

async fn connect(socket: &Path, port: u32) -> Result<UnixStream> {
    let mut stream = UnixStream::connect(socket).await?;
    stream
        .write_all(format!("CONNECT {port}\n").as_bytes())
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
    async fn ssh_admission_preserves_first_raw_bytes_and_validates_host_key() {
        use kiln_protocol::{SshConnect, SshReady};
        let key =
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("ssh");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut handshake = [0; 13];
            stream.read_exact(&mut handshake).await.unwrap();
            assert_eq!(&handshake, b"CONNECT 1025\n");
            stream.write_all(b"OK 123\n").await.unwrap();
            let admission: SshConnect = read_frame(&mut stream).await.unwrap();
            assert_eq!(admission.public_key, key);
            write_frame(
                &mut stream,
                &SshReady {
                    public_key: key.into(),
                },
            )
            .await
            .unwrap();
            stream.write_all(b"SSH-2.0-fixture\r\n").await.unwrap();
        });
        let (mut stream, ready) = ssh(&socket, key).await.unwrap();
        assert_eq!(ready.public_key, key);
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(bytes, b"SSH-2.0-fixture\r\n");
        server.await.unwrap();
        assert!(ssh(&socket, "ssh-rsa junk").await.is_err());
    }

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
            &Request::Exec(kiln_protocol::ExecRequest {
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

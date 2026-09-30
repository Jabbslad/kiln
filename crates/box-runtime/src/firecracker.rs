use crate::{Error, Result};
use serde_json::Value;
use std::{path::Path, time::Duration};

pub struct Client {
    http: reqwest::Client,
}

impl Client {
    pub fn new(socket: &Path, timeout: Duration) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .unix_socket(socket)
                .no_proxy()
                .timeout(timeout)
                .build()?,
        })
    }
    pub async fn request(&self, method: &str, path: &str, body: Value) -> Result<Value> {
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|e| Error::Invalid(e.to_string()))?;
        let mut request = self.http.request(method, format!("http://localhost{path}"));
        if !body.is_null() {
            request = request.json(&body);
        }
        let mut response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                Error::Invalid("Firecracker API timed out; operation outcome unknown".into())
            } else {
                Error::Http(e)
            }
        })?;
        let status = response.status();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len() + chunk.len() > 65536 {
                return Err(Error::Invalid("Firecracker response too large".into()));
            }
            bytes.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            return Err(Error::Invalid(format!(
                "Firecracker {path}: {status}: {}",
                String::from_utf8_lossy(&bytes)
            )));
        }
        if bytes.is_empty() {
            Ok(Value::Null)
        } else {
            Ok(serde_json::from_slice(&bytes)?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::UnixListener,
    };

    #[tokio::test]
    async fn encodes_requests_and_preserves_faults() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("api");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            for (status, body) in [(204, ""), (400, "{\"fault_message\":\"bad snapshot\"}")] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut received = vec![];
                loop {
                    let mut buf = [0; 4096];
                    let n = stream.read(&mut buf).await.unwrap();
                    received.extend_from_slice(&buf[..n]);
                    if received.ends_with(b"{\"state\":\"Paused\"}") {
                        break;
                    }
                    assert_ne!(n, 0);
                }
                let text = String::from_utf8(received).unwrap();
                assert!(text.starts_with("PATCH /vm HTTP/1.1"));
                stream.write_all(format!("HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            }
        });
        let client = Client::new(&socket, Duration::from_secs(1)).unwrap();
        assert_eq!(
            client
                .request("PATCH", "/vm", serde_json::json!({"state":"Paused"}))
                .await
                .unwrap(),
            Value::Null
        );
        assert!(
            client
                .request("PATCH", "/vm", serde_json::json!({"state":"Paused"}))
                .await
                .unwrap_err()
                .to_string()
                .contains("bad snapshot")
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn times_out_an_unresponsive_api() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("api");
        let _listener = UnixListener::bind(&socket).unwrap();
        let client = Client::new(&socket, Duration::from_millis(20)).unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            client.request("GET", "/", Value::Null),
        )
        .await
        .unwrap();
        assert!(result.unwrap_err().to_string().contains("timed out"));
    }
}

use anyhow::{Result, bail, ensure};
use box_api::{ApiError, BoxView, Operation, Submit, TemplateView, valid_id, valid_ssh_public_key};
use reqwest::{Method, Url, header};
use serde::de::DeserializeOwned;
use std::time::Duration;

pub struct Client {
    http: reqwest::Client,
    base: Url,
}

impl Client {
    pub fn new(url: &str, token: &str, ca_pem: Option<&[u8]>) -> Result<Self> {
        let base = Url::parse(url)?;
        ensure!(
            base.scheme() == "https"
                && base.host_str().is_some()
                && base.username().is_empty()
                && base.password().is_none()
                && base.query().is_none()
                && base.fragment().is_none()
                && base.path() == "/",
            "server URL must be an HTTPS origin without credentials, path, query or fragment"
        );
        ensure!(
            token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()),
            "token must be 64 hexadecimal characters"
        );
        let mut authorization = header::HeaderValue::from_str(&format!("Bearer {token}"))?;
        authorization.set_sensitive(true);
        let mut builder = reqwest::Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(40))
            .default_headers(
                [(header::AUTHORIZATION, authorization)]
                    .into_iter()
                    .collect(),
            );
        if let Some(ca) = ca_pem {
            builder = builder.add_root_certificate(reqwest::Certificate::from_pem(ca)?);
        }
        Ok(Self {
            http: builder.build()?,
            base,
        })
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&Submit>,
    ) -> Result<T> {
        let mut request = self.http.request(method, self.base.join(path)?);
        if let Some(body) = body {
            let bytes = serde_json::to_vec(body)?;
            ensure!(
                bytes.len() <= box_api::MAX_REQUEST_BYTES,
                "request exceeds 128 KiB"
            );
            request = request
                .header(header::CONTENT_TYPE, "application/json")
                .body(bytes);
        }
        let mut response = request.send().await?;
        let status = response.status();
        ensure!(
            !status.is_redirection(),
            "server redirected the request; redirects are disabled"
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= box_api::MAX_RESPONSE_BYTES,
                "server response exceeds 1 MiB"
            );
            bytes.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            if let Ok(error) = serde_json::from_slice::<ApiError>(&bytes) {
                bail!(
                    "{} (HTTP {}): {}",
                    error.code,
                    status.as_u16(),
                    error.message
                );
            }
            bail!("server returned HTTP {}", status.as_u16());
        }
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub async fn templates(&self) -> Result<Vec<TemplateView>> {
        self.request(Method::GET, "v1/templates", None).await
    }
    pub async fn list(&self) -> Result<Vec<BoxView>> {
        self.request(Method::GET, "v1/boxes", None).await
    }
    pub async fn inspect(&self, id: &str) -> Result<BoxView> {
        ensure!(valid_id(id), "invalid box ID");
        self.request(Method::GET, &format!("v1/boxes/{id}"), None)
            .await
    }
    pub async fn operation(&self, id: &str) -> Result<Operation> {
        ensure!(valid_id(id), "invalid request ID");
        self.request(Method::GET, &format!("v1/operations/{id}"), None)
            .await
    }
    pub async fn submit(&self, request: &Submit) -> Result<Operation> {
        ensure!(valid_id(&request.id), "invalid request ID");
        request.action.validate().map_err(anyhow::Error::msg)?;
        self.request(Method::POST, "v1/operations", Some(request))
            .await
    }

    pub async fn ssh_host_key(&self, id: &str) -> Result<String> {
        ensure!(valid_id(id), "invalid box ID");
        let ready: box_api::SshReady = self
            .request(Method::GET, &format!("v1/boxes/{id}/ssh-key"), None)
            .await?;
        ensure!(
            valid_ssh_public_key(&ready.public_key),
            "server returned an invalid SSH host key"
        );
        Ok(ready.public_key)
    }

    pub async fn ssh_tunnel(&self, id: &str, public_key: &str) -> Result<reqwest::Upgraded> {
        ensure!(valid_id(id), "invalid box ID");
        ensure!(valid_ssh_public_key(public_key), "invalid SSH public key");
        let response = self
            .http
            .get(self.base.join(&format!("v1/boxes/{id}/ssh"))?)
            .header(header::CONNECTION, "upgrade")
            .header(header::UPGRADE, box_api::SSH_UPGRADE)
            .header(box_api::SSH_KEY_HEADER, public_key)
            .send()
            .await?;
        ensure!(
            response.status() == reqwest::StatusCode::SWITCHING_PROTOCOLS,
            "server refused SSH tunnel (HTTP {})",
            response.status().as_u16()
        );
        ensure!(
            response
                .headers()
                .get(header::UPGRADE)
                .and_then(|v| v.to_str().ok())
                == Some(box_api::SSH_UPGRADE),
            "server returned an invalid SSH upgrade response"
        );
        Ok(response.upgrade().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_cleartext_and_credential_bearing_server_urls() {
        for url in [
            "http://localhost:8443",
            "https://secret@localhost",
            "https://localhost/api",
            "https://localhost/?token=abc",
            "https://localhost/#fragment",
        ] {
            assert!(Client::new(url, &"a".repeat(64), None).is_err(), "{url}");
        }
        assert!(Client::new("https://localhost:8443", &"a".repeat(64), None).is_ok());
        assert!(Client::new("https://localhost", "short", None).is_err());
    }
}

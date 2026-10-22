use anyhow::{Result, bail, ensure};
use kiln_api::{
    ApiError, BoxView, Operation, OperationSubmitQuery, Submit, TemplateView, valid_id,
    valid_ssh_public_key,
};
use reqwest::{Method, Url, header};
use serde::de::DeserializeOwned;
use std::time::Duration;

pub mod auth;
pub mod credentials;
pub mod profile;

pub struct Client {
    http: reqwest::Client,
    base: Url,
    session: Option<auth::Session>,
}

impl Client {
    pub fn new(url: &str, token: &str, ca_pem: Option<&[u8]>) -> Result<Self> {
        ensure!(
            token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()),
            "token must be 64 hexadecimal characters"
        );
        Self::build(url, ca_pem, Some(token), None)
    }

    pub fn with_bearer(url: &str, token: &str, ca_pem: Option<&[u8]>) -> Result<Self> {
        ensure!(
            !token.is_empty()
                && token.len() <= 8192
                && !token.bytes().any(|b| b.is_ascii_control()),
            "invalid bearer token"
        );
        Self::build(url, ca_pem, Some(token), None)
    }

    pub fn with_session(profile: &profile::CentralProfile, session: auth::Session) -> Result<Self> {
        Self::build(
            &profile.server.origin,
            pinned_ca(profile),
            None,
            Some(session),
        )
    }

    fn build(
        url: &str,
        ca_pem: Option<&[u8]>,
        token: Option<&str>,
        session: Option<auth::Session>,
    ) -> Result<Self> {
        let base = Url::parse(url)?;
        ensure!(
            base.scheme() == "https"
                && base.host_str().is_some()
                && base.path() == "/"
                && base.query().is_none()
                && base.fragment().is_none()
                && base.username().is_empty()
                && base.password().is_none(),
            "server URL must be an HTTPS origin without credentials, path, query or fragment"
        );
        let mut builder = reqwest::Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(40));
        if let Some(ca) = ca_pem {
            builder = builder
                .add_root_certificate(reqwest::Certificate::from_pem(ca)?)
                .tls_built_in_root_certs(false);
        }
        if let Some(token) = token {
            let mut h = header::HeaderValue::from_str(&format!("Bearer {token}"))?;
            h.set_sensitive(true);
            builder = builder.default_headers([(header::AUTHORIZATION, h)].into_iter().collect());
        }
        Ok(Self {
            http: builder.build()?,
            base,
            session,
        })
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&Submit>,
        timeout: Option<Duration>,
    ) -> Result<T> {
        let mut request = self.http.request(method, self.base.join(path)?);
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        if let Some(session) = &self.session {
            request = request.bearer_auth(session.server_token().await?);
        }
        if let Some(body) = body {
            let bytes = serde_json::to_vec(body)?;
            ensure!(
                bytes.len() <= kiln_api::MAX_REQUEST_BYTES,
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
                bytes.len() + chunk.len() <= kiln_api::MAX_RESPONSE_BYTES,
                "server response exceeds 1 MiB"
            );
            bytes.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            let hint = if status == reqwest::StatusCode::UNAUTHORIZED {
                "; authentication rejected: run `kiln login` for a central profile, or check the token file for a direct profile"
            } else {
                ""
            };
            if let Ok(error) = serde_json::from_slice::<ApiError>(&bytes) {
                bail!(
                    "{} (HTTP {}): {}{hint}",
                    error.code,
                    status.as_u16(),
                    error.message
                );
            }
            bail!("server returned HTTP {}{hint}", status.as_u16());
        }
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub async fn templates(&self) -> Result<Vec<TemplateView>> {
        self.request(Method::GET, "v1/templates", None, None).await
    }
    pub async fn list(&self) -> Result<Vec<BoxView>> {
        self.request(Method::GET, "v1/boxes", None, None).await
    }
    pub async fn inspect(&self, id: &str) -> Result<BoxView> {
        ensure!(valid_id(id), "invalid box ID");
        self.request(Method::GET, &format!("v1/boxes/{id}"), None, None)
            .await
    }
    pub async fn operation(&self, id: &str) -> Result<Operation> {
        ensure!(valid_id(id), "invalid request ID");
        self.request(Method::GET, &format!("v1/operations/{id}"), None, None)
            .await
    }
    pub async fn submit(&self, request: &Submit) -> Result<Operation> {
        self.submit_with_wait(request, false, kiln_api::DEFAULT_OPERATION_WAIT_SECONDS)
            .await
    }

    pub async fn submit_with_wait(
        &self,
        request: &Submit,
        wait: bool,
        wait_seconds: u64,
    ) -> Result<Operation> {
        ensure!(valid_id(&request.id), "invalid request ID");
        request.action.validate().map_err(anyhow::Error::msg)?;
        let query = OperationSubmitQuery { wait, wait_seconds };
        query
            .validate_for_action(&request.action)
            .map_err(anyhow::Error::msg)?;
        let path = if wait {
            format!("v1/operations?wait=true&wait_seconds={wait_seconds}")
        } else {
            "v1/operations".into()
        };
        let timeout = wait.then(|| Duration::from_secs(wait_seconds.saturating_add(30)));
        self.request(Method::POST, &path, Some(request), timeout)
            .await
    }

    pub async fn ssh_host_key(&self, id: &str) -> Result<String> {
        ensure!(valid_id(id), "invalid box ID");
        let ready: kiln_api::SshReady = self
            .request(Method::GET, &format!("v1/boxes/{id}/ssh-key"), None, None)
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
        let mut tunnel = self
            .http
            .get(self.base.join(&format!("v1/boxes/{id}/ssh"))?);
        if let Some(s) = &self.session {
            tunnel = tunnel.bearer_auth(s.server_token().await?);
        }
        let response = tunnel
            .header(header::CONNECTION, "upgrade")
            .header(header::UPGRADE, kiln_api::SSH_UPGRADE)
            .header(kiln_api::SSH_KEY_HEADER, public_key)
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
                == Some(kiln_api::SSH_UPGRADE),
            "server returned an invalid SSH upgrade response"
        );
        Ok(response.upgrade().await?)
    }
}

fn pinned_ca(profile: &profile::CentralProfile) -> Option<&[u8]> {
    match profile.door_trust {
        profile::DoorTrust::PublicRoots => None,
        profile::DoorTrust::Pin => Some(profile.server.ca_pem.as_bytes()),
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

    fn error_text(err: &anyhow::Error) -> String {
        err.chain()
            .map(|item| item.to_string())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[tokio::test]
    async fn private_ca_reaches_its_own_server_and_not_a_different_ca() {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let pem = cert.cert.pem();
        let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
            cert.cert.pem().into_bytes(),
            cert.signing_key.serialize_pem().into_bytes(),
        )
        .await
        .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let router = axum::Router::new().route(
            "/v1/templates",
            axum::routing::get(|| async { axum::Json(serde_json::json!([])) }),
        );
        let handle = axum_server::Handle::new();
        let task = tokio::spawn(
            axum_server::from_tcp_rustls(listener, tls)
                .unwrap()
                .handle(handle.clone())
                .serve(router.into_make_service()),
        );
        let token = "a".repeat(64);
        let url = format!("https://localhost:{port}/");
        let trusted = Client::new(&url, &token, Some(pem.as_bytes())).unwrap();
        assert!(trusted.templates().await.unwrap().is_empty());
        let other = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let untrusted = Client::new(&url, &token, Some(other.cert.pem().as_bytes())).unwrap();
        let err = error_text(&untrusted.templates().await.unwrap_err());
        assert!(err.to_ascii_lowercase().contains("certificate"), "{err}");
        let public_roots = Client::new(&url, &token, None).unwrap();
        let err = error_text(&public_roots.templates().await.unwrap_err());
        assert!(err.to_ascii_lowercase().contains("certificate"), "{err}");
        handle.shutdown();
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn old_profile_fails_closed_and_login_adopts_public_roots() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("profiles.json");
        let credential = root.path().join("credential.json");
        assert!(credential.is_absolute());
        let ca = rcgen::generate_simple_self_signed(vec!["cli.dark-forge.dev".into()]).unwrap();
        let pem = ca.cert.pem();
        let stored = serde_json::json!({
            "profiles": {
                "default": {
                    "issuer": "https://dark-forge.dev",
                    "server": {
                        "id": "srv_dark_forge",
                        "name": "dark-forge",
                        "origin": "https://cli.dark-forge.dev:8443",
                        "ca_pem": pem,
                        "revision": 1
                    },
                    "credential_file": credential
                }
            }
        });
        std::fs::write(&path, serde_json::to_vec_pretty(&stored).unwrap()).unwrap();
        let before = std::fs::read(&path).unwrap();
        let loaded = crate::profile::load(&path).unwrap();
        let crate::profile::Profile::Central(profile) =
            loaded.profiles.get("default").expect("central profile")
        else {
            panic!("stored profile is central");
        };
        assert_eq!(profile.door_trust, crate::profile::DoorTrust::Pin);
        assert_eq!(profile.server.ca_pem, pem);
        let token = "ab".repeat(32);
        let stayed = Client::new(&profile.server.origin, &token, pinned_ca(profile)).unwrap();
        assert_eq!(stayed.base.host_str(), Some("cli.dark-forge.dev"));
        assert_eq!(stayed.base.port(), Some(8443));
        let pinned = Client::new("https://example.com/", &token, pinned_ca(profile)).unwrap();
        let err = error_text(&pinned.templates().await.unwrap_err());
        assert!(
            err.to_ascii_lowercase().contains("certificate"),
            "saved pin should fail closed, got {err}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let credentials = crate::credentials::Credentials {
            issuer: "https://dark-forge.dev".into(),
            access_token: String::new(),
            access_expires_at: 0,
            refresh_token: "refresh".into(),
            automation_key_file: None,
            server_tokens: Default::default(),
        };
        crate::profile::save_login(
            &path,
            "default",
            crate::profile::CentralProfile {
                issuer: "https://dark-forge.dev".into(),
                server: kiln_api::auth::ServerDescriptor {
                    id: "srv_dark_forge".into(),
                    name: "dark-forge".into(),
                    origin: "https://cli.dark-forge.dev".into(),
                    ca_pem: pem.clone(),
                    revision: 2,
                },
                credential_file: root.path().join("private/credential.json"),
                door_trust: crate::profile::DoorTrust::Pin,
                server_chosen: true,
            },
            &credentials,
        )
        .unwrap();
        let adopted = crate::profile::load(&path).unwrap();
        let crate::profile::Profile::Central(adopted) =
            adopted.profiles.get("default").expect("central profile")
        else {
            panic!("stored profile is central");
        };
        assert_eq!(adopted.server.ca_pem, pem);
        assert_eq!(adopted.door_trust, crate::profile::DoorTrust::PublicRoots);
        assert!(pinned_ca(adopted).is_none());
        let public = Client::new("https://example.com/", &token, pinned_ca(adopted)).unwrap();
        let err = public.templates().await.unwrap_err().to_string();
        assert!(
            err.contains("HTTP"),
            "login should dial with public roots, got {err}"
        );
    }

    #[tokio::test]
    async fn private_ca_does_not_trust_public_roots() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let token = "ab".repeat(32);
        let url = format!("https://localhost:{port}/");
        let wrong_ca = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let private = Client::new(&url, &token, Some(wrong_ca.cert.pem().as_bytes())).unwrap();
        let err = error_text(&private.templates().await.unwrap_err());
        assert!(err.to_ascii_lowercase().contains("certificate"), "{err}");
        let public = Client::new(&url, &token, None).unwrap();
        let err = error_text(&public.templates().await.unwrap_err());
        assert!(
            err.to_ascii_lowercase().contains("certificate"),
            "built-in roots must not trust this loopback certificate, got {err}"
        );
    }

    #[tokio::test]
    async fn identity_client_keeps_public_roots() {
        let client = crate::auth::IdentityClient::new("https://example.com").unwrap();
        let err = match client.begin_login("laptop").await {
            Ok(_) => panic!("example.com is not an identity issuer"),
            Err(err) => err.to_string(),
        };
        assert!(
            err.contains("404") || err.contains("HTTP"),
            "identity client should finish the TLS handshake, got {err}"
        );
    }
}

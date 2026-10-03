use crate::{
    credentials::{CachedToken, CredentialStore, Credentials},
    profile::{CentralProfile, Profile},
};
use anyhow::{Context, Result, bail, ensure};
use kiln_api::auth::{
    AccessToken, DeviceAuthorization, DeviceTokens, KeyToken, Scope, ServerDescriptor,
};
use reqwest::{Method, Url};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone)]
pub struct Session {
    inner: Arc<Inner>,
}
struct Inner {
    profile: CentralProfile,
    store: CredentialStore,
    http: reqwest::Client,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn origin(value: &str) -> Result<Url> {
    let u = Url::parse(value)?;
    ensure!(
        u.scheme() == "https"
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
            && u.path() == "/"
            && u.query().is_none()
            && u.fragment().is_none(),
        "identity URL must be an HTTPS origin"
    );
    Ok(u)
}

impl Session {
    pub fn new(profile: CentralProfile) -> Result<Self> {
        origin(&profile.issuer)?;
        let store = CredentialStore::new(profile.credential_file.clone());
        let http = reqwest::Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(Duration::from_secs(40))
            .build()?;
        Ok(Self {
            inner: Arc::new(Inner {
                profile,
                store,
                http,
            }),
        })
    }

    pub async fn server_token(&self) -> Result<String> {
        let _lock = self.inner.store.acquire().await?;
        let mut c = self
            .inner
            .store
            .load()?
            .context("not logged in; run `kiln login`")?;
        ensure!(
            c.issuer == self.inner.profile.issuer,
            "credential issuer changed; log in again"
        );
        let key = format!("{}:credential", self.inner.profile.server.id);
        if let Some(t) = c.server_tokens.get(&key)
            && t.expires_at > now() + 30
        {
            return Ok(t.token.clone());
        }
        let token = if let Some(path) = &c.automation_key_file {
            let identity = IdentityClient {
                issuer: origin(&c.issuer)?,
                http: self.inner.http.clone(),
            };
            let grant = identity.token_file(path).await?;
            ensure!(
                grant.server.id == self.inner.profile.server.id
                    && grant.server.origin == self.inner.profile.server.origin
                    && grant.server.ca_pem == self.inner.profile.server.ca_pem,
                "automation server or trust changed; explicitly log in again"
            );
            grant.token
        } else {
            self.ensure_central(&mut c).await?;
            #[derive(Serialize)]
            struct Req<'a> {
                server_id: &'a str,
            }
            let token: AccessToken = self
                .central(
                    Method::POST,
                    "v1/access",
                    Some(&Req {
                        server_id: &self.inner.profile.server.id,
                    }),
                    &c.access_token,
                )
                .await?;
            token
        };
        ensure!(
            token.token_type.eq_ignore_ascii_case("bearer")
                && (1..=300).contains(&token.expires_in),
            "identity returned an invalid token grant"
        );
        c.server_tokens.insert(
            key.clone(),
            CachedToken {
                token: token.access_token,
                expires_at: now() + token.expires_in,
            },
        );
        self.inner.store.save(&c)?;
        Ok(c.server_tokens[&key].token.clone())
    }

    async fn ensure_central(&self, c: &mut Credentials) -> Result<()> {
        ensure!(
            c.issuer == self.inner.profile.issuer,
            "credential issuer changed; log in again"
        );
        ensure!(
            c.automation_key_file.is_none(),
            "automation keys cannot manage accounts or credentials"
        );
        if c.access_expires_at > now() + 30 {
            return Ok(());
        }
        ensure!(
            !c.refresh_token.is_empty(),
            "session renewal was interrupted; log in again"
        );
        let url = origin(&self.inner.profile.issuer)?.join("oauth/token")?;
        // Persist an invalid session before transmission. Atomic write-through
        // replacement also works on Windows, where unlink has no directory fsync.
        let mut invalidated = c.clone();
        invalidated.refresh_token.clear();
        invalidated.access_token.clear();
        invalidated.access_expires_at = 0;
        invalidated.server_tokens.clear();
        self.inner.store.save(&invalidated)?;
        let response = self
            .inner
            .http
            .post(url)
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", c.refresh_token.as_str()),
            ])
            .send()
            .await;
        let response = match response {
            Ok(r) => r,
            Err(e) => {
                self.inner.store.clear()?;
                bail!("refresh result is unknown; local session invalidated, log in again: {e}")
            }
        };
        if !response.status().is_success() {
            self.inner.store.clear()?;
            bail!("session refresh rejected; log in again")
        }
        let tokens: DeviceTokens = match read_json(response).await {
            Ok(tokens) => tokens,
            Err(error) => {
                self.inner.store.clear()?;
                bail!("invalid refresh response; local session invalidated, log in again: {error}")
            }
        };
        ensure!(
            tokens.token_type.eq_ignore_ascii_case("bearer")
                && (1..=300).contains(&tokens.expires_in),
            "invalid refresh response; log in again"
        );
        c.access_token = tokens.access_token;
        c.access_expires_at = now() + tokens.expires_in;
        c.refresh_token = tokens.refresh_token;
        c.server_tokens.clear();
        self.inner.store.save(c)?; // rotated secret is durable before it is used
        Ok(())
    }

    async fn central<T: DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        m: Method,
        path: &str,
        body: Option<&B>,
        token: &str,
    ) -> Result<T> {
        let mut r = self
            .inner
            .http
            .request(m, origin(&self.inner.profile.issuer)?.join(path)?)
            .bearer_auth(token);
        if let Some(b) = body {
            r = r.json(b)
        }
        let x = r.send().await?;
        ensure!(
            x.status().is_success(),
            "identity returned HTTP {}",
            x.status()
        );
        read_json(x).await
    }

    pub async fn identity_json(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let _lock = self.inner.store.acquire().await?;
        let mut c = self.inner.store.load()?.context("not logged in")?;
        self.ensure_central(&mut c).await?;
        let mut r = self
            .inner
            .http
            .request(method, origin(&self.inner.profile.issuer)?.join(path)?)
            .bearer_auth(&c.access_token);
        if let Some(v) = body {
            r = r.json(&v)
        }
        let x = r.send().await?;
        if x.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(serde_json::Value::Null);
        }
        read_json(x.error_for_status()?).await
    }
    pub async fn clear(&self) -> Result<()> {
        let _lock = self.inner.store.acquire().await?;
        self.inner.store.clear()
    }
}

pub async fn resolve_client(profile: &Profile) -> Result<crate::Client> {
    profile.client()
}

pub struct IdentityClient {
    issuer: Url,
    http: reqwest::Client,
}
pub enum LoginPoll {
    Pending,
    SlowDown,
    Ready(DeviceTokens),
}
impl IdentityClient {
    pub fn new(issuer: &str) -> Result<Self> {
        Ok(Self {
            issuer: origin(issuer)?,
            http: reqwest::Client::builder()
                .https_only(true)
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(40))
                .build()?,
        })
    }
    pub async fn begin_login(&self, name: &str) -> Result<DeviceAuthorization> {
        let flow: DeviceAuthorization = read_json(
            self.http
                .post(self.issuer.join("oauth/device/code")?)
                .form(&[("client_id", "kiln-cli"), ("device_name", name)])
                .send()
                .await?
                .error_for_status()?,
        )
        .await?;
        ensure!(
            (1..=600).contains(&flow.expires_in) && (1..=60).contains(&flow.interval),
            "invalid login lifetime"
        );
        for value in [&flow.verification_uri, &flow.verification_uri_complete] {
            let u = Url::parse(value)?;
            ensure!(
                u.origin() == self.issuer.origin()
                    && u.username().is_empty()
                    && u.password().is_none(),
                "login URL left the configured identity origin"
            );
        }
        Ok(flow)
    }
    pub async fn poll_login(&self, code: &str) -> Result<LoginPoll> {
        let r = self
            .http
            .post(self.issuer.join("oauth/token")?)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", code),
            ])
            .send()
            .await?;
        if r.status().is_success() {
            let t: DeviceTokens = read_json(r).await?;
            ensure!(
                t.token_type.eq_ignore_ascii_case("bearer") && (1..=300).contains(&t.expires_in),
                "invalid login response"
            );
            return Ok(LoginPoll::Ready(t));
        }
        let v: serde_json::Value = read_json(r).await?;
        match v["error"].as_str() {
            Some("authorization_pending") => Ok(LoginPoll::Pending),
            Some("slow_down") => Ok(LoginPoll::SlowDown),
            Some(x) => bail!("login failed: {x}"),
            None => bail!("invalid login response"),
        }
    }
    pub async fn servers(&self, token: &str) -> Result<Vec<ServerDescriptor>> {
        read_json(
            self.http
                .get(self.issuer.join("v1/servers")?)
                .bearer_auth(token)
                .send()
                .await?
                .error_for_status()?,
        )
        .await
    }
    pub async fn token_file(&self, path: &std::path::Path) -> Result<KeyToken> {
        let token = crate::credentials::read_secret(path)?;
        read_json(
            self.http
                .post(self.issuer.join("v1/key-token")?)
                .bearer_auth(token)
                .send()
                .await?
                .error_for_status()?,
        )
        .await
    }
    pub async fn authorized_json(
        &self,
        method: Method,
        path: &str,
        token: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let mut r = self
            .http
            .request(method, self.issuer.join(path)?)
            .bearer_auth(token);
        if let Some(v) = body {
            r = r.json(&v)
        }
        let x = r.send().await?;
        if x.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(serde_json::Value::Null);
        }
        read_json(x.error_for_status()?).await
    }
}

pub fn credential_from_login(issuer: &str, t: DeviceTokens) -> Credentials {
    Credentials {
        issuer: issuer.into(),
        access_token: t.access_token,
        access_expires_at: now() + t.expires_in,
        refresh_token: t.refresh_token,
        automation_key_file: None,
        server_tokens: Default::default(),
    }
}
pub fn scope(value: &str) -> Result<Scope> {
    match value {
        "read" => Ok(Scope::Read),
        "operate" => Ok(Scope::Operate),
        _ => bail!("scope must be read or operate"),
    }
}

async fn read_json<T: DeserializeOwned>(mut response: reqwest::Response) -> Result<T> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= 1024 * 1024,
            "identity response exceeds 1 MiB"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, http::HeaderMap, routing::post};

    #[test]
    fn identity_origins_reject_embedded_credentials() {
        for issuer in [
            "https://user:secret@identity.example",
            "https://user@identity.example",
        ] {
            assert!(IdentityClient::new(issuer).is_err());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn concurrent_sessions_rotate_once_and_lost_responses_cannot_replay() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = CredentialStore::new(root.path().join("credentials.json"));
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
            cert.cert.pem().into_bytes(),
            cert.signing_key.serialize_pem().into_bytes(),
        )
        .await
        .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let issuer = format!(
            "https://localhost:{}",
            listener.local_addr().unwrap().port()
        );
        let refreshes = Arc::new(AtomicUsize::new(0));
        let grants = Arc::new(AtomicUsize::new(0));
        let observed = refreshes.clone();
        let disk = store.clone();
        let issued = grants.clone();
        let router=Router::new().route("/oauth/token",post(move |axum::Form(body): axum::Form<std::collections::HashMap<String,String>>| {
            let n=observed.fetch_add(1,Ordering::SeqCst);
            assert!(disk.load().unwrap().unwrap().refresh_token.is_empty(),"spent candidate must be durably invalidated before transmission");
            async move {
                if n==0 {
                    assert_eq!(body["refresh_token"],"original-refresh");
                    serde_json::json!({"access_token":"central-access","refresh_token":"rotated-refresh","token_type":"Bearer","expires_in":300}).to_string()
                } else {
                    assert_eq!(body["refresh_token"],"rotated-refresh");
                    "truncated response".into()
                }
            }
        })).route("/v1/access",post(move |h: HeaderMap| {
            issued.fetch_add(1,Ordering::SeqCst);
            assert_eq!(h["authorization"],"Bearer central-access");
            async { Json(serde_json::json!({"access_token":"server-access","token_type":"Bearer","expires_in":300})) }
        }));
        let handle = axum_server::Handle::new();
        let task = tokio::spawn(
            axum_server::from_tcp_rustls(listener, tls)
                .unwrap()
                .handle(handle.clone())
                .serve(router.into_make_service()),
        );
        let http = reqwest::Client::builder()
            .add_root_certificate(
                reqwest::Certificate::from_pem(cert.cert.pem().as_bytes()).unwrap(),
            )
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .build()
            .unwrap();
        let profile = CentralProfile {
            issuer: issuer.clone(),
            credential_file: store.path().into(),
            server: ServerDescriptor {
                id: "one".into(),
                name: "one".into(),
                origin: "https://server.example".into(),
                ca_pem: "unused".into(),
                revision: 1,
            },
        };
        let session = Session {
            inner: Arc::new(Inner {
                profile,
                store: store.clone(),
                http,
            }),
        };
        store
            .save(&Credentials {
                issuer,
                access_token: "expired".into(),
                access_expires_at: 0,
                refresh_token: "original-refresh".into(),
                automation_key_file: None,
                server_tokens: Default::default(),
            })
            .unwrap();
        let (a, b) = tokio::join!(session.server_token(), session.server_token());
        assert_eq!(a.unwrap(), "server-access");
        assert_eq!(b.unwrap(), "server-access");
        assert_eq!(refreshes.load(Ordering::SeqCst), 1);
        assert_eq!(grants.load(Ordering::SeqCst), 1);
        let mut expired = store.load().unwrap().unwrap();
        assert_eq!(expired.refresh_token, "rotated-refresh");
        expired.access_expires_at = 0;
        expired.server_tokens.clear();
        store.save(&expired).unwrap();
        assert!(session.server_token().await.is_err());
        assert!(store.load().unwrap().is_none());
        assert!(session.server_token().await.is_err());
        assert_eq!(
            refreshes.load(Ordering::SeqCst),
            2,
            "a lost response cannot trigger replay"
        );
        handle.shutdown();
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn automation_exchange_uses_opaque_private_key_and_canonical_route() {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
            cert.cert.pem().into_bytes(),
            cert.signing_key.serialize_pem().into_bytes(),
        )
        .await
        .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let handle = axum_server::Handle::new();
        let router = Router::new().route("/v1/key-token", post(|h: HeaderMap| async move {
            assert_eq!(h["authorization"], format!("Bearer {}", "k".repeat(43)));
            Json(serde_json::json!({"token":{"access_token":"server-jwt","token_type":"Bearer","expires_in":300},"server":{"id":"one","name":"one","origin":"https://private.example","ca_pem":"pem","revision":1}}))
        }));
        let task = tokio::spawn(
            axum_server::from_tcp_rustls(listener, tls)
                .unwrap()
                .handle(handle.clone())
                .serve(router.into_make_service()),
        );
        let http = reqwest::Client::builder()
            .add_root_certificate(
                reqwest::Certificate::from_pem(cert.cert.pem().as_bytes()).unwrap(),
            )
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .unwrap();
        let client = IdentityClient {
            issuer: Url::parse(&format!("https://localhost:{}", address.port())).unwrap(),
            http,
        };
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("key");
        crate::credentials::write_secret(&path, &"k".repeat(43)).unwrap();
        let result = client.token_file(&path).await;
        handle.shutdown();
        task.await.unwrap().unwrap();
        assert_eq!(result.unwrap().token.access_token, "server-jwt");
    }
}

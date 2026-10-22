//! Local enrollment remains the authority for issuer, owner and server audience.
use anyhow::{Context, Result, ensure};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use kiln_api::auth::{AuthGrant, Caller, Role, Scope, ServerClaims};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Enrollment {
    pub version: u32,
    pub issuer: String,
    pub owner_id: String,
    pub server_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<String>,
}

impl Enrollment {
    pub fn validate(&self) -> Result<()> {
        let url = reqwest::Url::parse(&self.issuer)?;
        ensure!(
            self.version == 1
                && url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none(),
            "invalid identity enrollment"
        );
        ensure!(
            !self.owner_id.is_empty()
                && self.owner_id.len() <= 128
                && !self.server_id.is_empty()
                && self.server_id.len() <= 128,
            "invalid enrollment identity"
        );
        if let Some(tenant_id) = &self.tenant_id {
            ensure!(
                !tenant_id.is_empty() && tenant_id.len() <= 128,
                "invalid enrollment tenant"
            );
        }
        Ok(())
    }

    /// Effective tenant for this server. Legacy enrollments without `tenant_id`
    /// treat the enrolled owner as the sole tenant owner.
    pub fn tenant_id(&self) -> &str {
        self.tenant_id
            .as_deref()
            .filter(|id| !id.is_empty())
            .unwrap_or(&self.owner_id)
    }

    pub fn load(path: &Path) -> Result<Option<Self>> {
        use std::os::unix::fs::MetadataExt;
        let meta = match fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        ensure!(
            meta.is_file() && meta.uid() == 0 && meta.mode() & 0o027 == 0 && meta.len() < 8192,
            "enrollment must be a root-owned private regular file"
        );
        let value: Self = serde_json::from_slice(&fs::read(path)?)?;
        value.validate()?;
        Ok(Some(value))
    }
}

fn keys(value: &Value) -> Result<JwkSet> {
    let entries = value
        .get("keys")
        .and_then(Value::as_array)
        .context("invalid JWKS")?;
    ensure!(
        !entries.is_empty() && entries.len() <= 16,
        "invalid key count"
    );
    let mut seen = std::collections::HashSet::new();
    for key in entries {
        let kid = key
            .get("kid")
            .and_then(Value::as_str)
            .context("missing key id")?;
        ensure!(
            !kid.is_empty()
                && kid.len() <= 128
                && seen.insert(kid)
                && key["kty"] == "EC"
                && key["crv"] == "P-256"
                && key["alg"] == "ES256"
                && key["use"] == "sig"
                && key.get("d").is_none(),
            "invalid verification key"
        );
    }
    Ok(serde_json::from_value(value.clone())?)
}

pub fn verify_claims(token: &str, enrollment: &Enrollment, jwks: &Value) -> Result<ServerClaims> {
    enrollment.validate()?;
    ensure!(token.len() <= 8192, "invalid token");
    let header = decode_header(token)?;
    ensure!(
        header.alg == Algorithm::ES256
            && header.jku.is_none()
            && header.jwk.is_none()
            && header.x5u.is_none()
            && header.x5c.is_none(),
        "invalid token header"
    );
    let kid = header.kid.context("missing token key id")?;
    let set = keys(jwks)?;
    let key = DecodingKey::from_jwk(set.find(&kid).context("unknown token key")?)?;
    let mut validation = Validation::new(Algorithm::ES256);
    validation.set_required_spec_claims(&["exp", "nbf", "iss", "aud", "sub"]);
    validation.set_issuer(&[&enrollment.issuer]);
    validation.set_audience(&[format!("kiln:server:{}", enrollment.server_id)]);
    validation.validate_nbf = true;
    validation.leeway = 30;
    let c = decode::<ServerClaims>(token, &key, &validation)?.claims;
    let tenant_id = c
        .tenant_id
        .clone()
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| c.sub.clone());
    ensure!(
        tenant_id == enrollment.tenant_id()
            && !c.credential_id.is_empty()
            && c.credential_id.len() <= 128
            && c.exp > c.iat
            && c.exp - c.iat <= 300
            && c.nbf >= c.iat
            && c.nbf < c.exp
            && c.iat <= jsonwebtoken::get_current_timestamp().saturating_add(30),
        "invalid token authority or lifetime"
    );
    Ok(c)
}

fn auth_grant_from_claims(c: &ServerClaims, enrollment: &Enrollment) -> AuthGrant {
    let sub = c.sub.clone();
    let tenant_id = c
        .tenant_id
        .clone()
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| sub.clone());
    let role = c.role.unwrap_or_else(|| {
        if sub == enrollment.owner_id {
            Role::Owner
        } else {
            Role::Member
        }
    });
    AuthGrant {
        caller: Caller {
            sub,
            tenant_id,
            role,
        },
        scope: c.scope,
    }
}

pub fn verify(token: &str, enrollment: &Enrollment, jwks: &Value) -> Result<AuthGrant> {
    Ok(auth_grant_from_claims(
        &verify_claims(token, enrollment, jwks)?,
        enrollment,
    ))
}

struct Cache {
    jwks: Value,
    last_fetch: Option<Instant>,
}
pub struct Authenticator {
    admin_hash: Option<[u8; 32]>,
    central: Option<(Enrollment, PathBuf)>,
    cache: Mutex<Cache>,
    http: reqwest::Client,
}

impl Authenticator {
    pub fn new(admin: &str, central: Option<(Enrollment, PathBuf)>) -> Result<Self> {
        ensure!(
            admin.len() == 64 && admin.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid administrator token"
        );
        let mut jwks = serde_json::json!({"keys":[]});
        if let Some((enrollment, path)) = &central {
            enrollment.validate()?;
            if let Ok(meta) = fs::symlink_metadata(path) {
                ensure!(meta.is_file() && meta.len() <= 65536, "unsafe key cache");
                let cached: Value = serde_json::from_slice(&fs::read(path)?)?;
                ensure!(
                    cached["issuer"] == enrollment.issuer,
                    "key cache issuer mismatch"
                );
                keys(&cached["jwks"])?;
                jwks = cached["jwks"].clone();
            }
        }
        Ok(Self {
            admin_hash: Some(Sha256::digest(admin.as_bytes()).into()),
            central,
            cache: Mutex::new(Cache {
                jwks,
                last_fetch: None,
            }),
            http: reqwest::Client::builder()
                .https_only(true)
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .connect_timeout(Duration::from_secs(3))
                .timeout(Duration::from_secs(10))
                .build()?,
        })
    }

    /// Identity JWTs only. The local administrator token stays on the loopback API.
    pub fn central_only(enrollment: Enrollment, cache: PathBuf) -> Result<Self> {
        let mut auth = Self::new(&"0".repeat(64), Some((enrollment, cache)))?;
        auth.admin_hash = None;
        Ok(auth)
    }

    pub async fn authenticate(&self, bearer: &str) -> Result<AuthGrant> {
        ensure!(bearer.len() <= 8192, "invalid token");
        if let Some(admin_hash) = self.admin_hash {
            let supplied: [u8; 32] = Sha256::digest(bearer.as_bytes()).into();
            if bool::from(supplied.ct_eq(&admin_hash)) {
                let tenant_id = self
                    .central
                    .as_ref()
                    .map(|(enrollment, _)| enrollment.tenant_id().to_owned())
                    .unwrap_or_default();
                return Ok(AuthGrant {
                    caller: Caller {
                        sub: "administrator".into(),
                        tenant_id,
                        role: Role::Owner,
                    },
                    scope: Scope::Operate,
                });
            }
        }
        self.central_jwt(bearer).await
    }

    pub async fn authenticate_central(&self, bearer: &str) -> Result<AuthGrant> {
        ensure!(bearer.len() <= 8192, "invalid token");
        self.central_jwt(bearer).await
    }

    /// JWT `sub` when the bearer is a valid central token; otherwise `anonymous`.
    pub async fn central_caller(&self, bearer: &str) -> String {
        self.central_subject(bearer)
            .await
            .unwrap_or_else(|| "anonymous".into())
    }

    pub async fn central_subject(&self, bearer: &str) -> Option<String> {
        if bearer.is_empty() {
            return None;
        }
        self.central_claims(bearer).await.ok().map(|c| c.sub)
    }

    async fn central_jwt(&self, bearer: &str) -> Result<AuthGrant> {
        let (enrollment, _) = self
            .central
            .as_ref()
            .context("central authentication not enrolled")?;
        Ok(auth_grant_from_claims(
            &self.central_claims(bearer).await?,
            enrollment,
        ))
    }

    async fn central_claims(&self, bearer: &str) -> Result<ServerClaims> {
        let (enrollment, path) = self
            .central
            .as_ref()
            .context("central authentication not enrolled")?;
        let header = decode_header(bearer)?;
        ensure!(
            header.alg == Algorithm::ES256
                && header.jku.is_none()
                && header.jwk.is_none()
                && header.x5u.is_none()
                && header.x5c.is_none(),
            "invalid token header"
        );
        let kid = header.kid.context("missing token key id")?;
        let mut cache = self.cache.lock().await;
        let known = cache.jwks["keys"]
            .as_array()
            .is_some_and(|k| k.iter().any(|k| k["kid"] == kid));
        if !known {
            ensure!(
                cache
                    .last_fetch
                    .is_none_or(|t| t.elapsed() >= Duration::from_secs(60)),
                "unknown token key"
            );
            cache.last_fetch = Some(Instant::now());
            let mut response = self
                .http
                .get(format!(
                    "{}/.well-known/jwks.json",
                    enrollment.issuer.trim_end_matches('/')
                ))
                .send()
                .await?;
            ensure!(response.status().is_success(), "identity keys unavailable");
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                ensure!(bytes.len() + chunk.len() <= 65536, "key response too large");
                bytes.extend_from_slice(&chunk);
            }
            let value: Value = serde_json::from_slice(&bytes)?;
            keys(&value)?;
            let parent = path.parent().context("key cache directory missing")?;
            let mut file = tempfile::NamedTempFile::new_in(parent)?;
            file.write_all(&serde_json::to_vec(
                &serde_json::json!({"issuer":enrollment.issuer,"jwks":value}),
            )?)?;
            file.as_file().sync_all()?;
            file.persist(path)?;
            fs::File::open(parent)?.sync_all()?;
            cache.jwks = value;
        }
        verify_claims(bearer, enrollment, &cache.jwks)
    }
}

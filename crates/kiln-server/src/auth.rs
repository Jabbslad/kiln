//! Local enrollment remains the authority for issuer, tenant, and server audience.
use anyhow::{Context, Result, bail, ensure};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use kiln_api::{
    Caller, Role,
    auth::{Scope, ServerClaims, TokenRole},
};
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
    /// Tenant this server belongs to. Empty means the legacy single-tenant default (`owner_id`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tenant_id: String,
}

impl Enrollment {
    pub fn enrolled_tenant_id(&self) -> &str {
        if self.tenant_id.is_empty() {
            &self.owner_id
        } else {
            &self.tenant_id
        }
    }

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
                && self.server_id.len() <= 128
                && self.enrolled_tenant_id().len() <= 128,
            "invalid enrollment identity"
        );
        Ok(())
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

fn token_tenant(claims: &ServerClaims, enrollment: &Enrollment) -> Result<String> {
    if let Some(tenant_id) = &claims.tenant_id {
        ensure!(
            !tenant_id.is_empty() && tenant_id.len() <= 128,
            "invalid token tenant"
        );
        return Ok(tenant_id.clone());
    }
    // Pre-tenant tokens: the enrolled owner is the sole tenant member.
    if claims.sub == enrollment.owner_id {
        return Ok(enrollment.enrolled_tenant_id().to_owned());
    }
    bail!("invalid token authority")
}

fn token_role(claims: &ServerClaims, enrollment: &Enrollment) -> Result<Role> {
    if let Some(role) = claims.role {
        return Ok(match role {
            TokenRole::Owner => Role::Owner,
            TokenRole::Member => Role::Member,
        });
    }
    if claims.sub == enrollment.owner_id {
        return Ok(Role::Owner);
    }
    bail!("invalid token authority")
}

pub fn verify_caller(
    token: &str,
    enrollment: &Enrollment,
    jwks: &Value,
) -> Result<(Caller, Scope)> {
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
    ensure!(
        !c.credential_id.is_empty()
            && c.credential_id.len() <= 128
            && c.exp > c.iat
            && c.exp - c.iat <= 300
            && c.nbf >= c.iat
            && c.nbf < c.exp
            && c.iat <= jsonwebtoken::get_current_timestamp().saturating_add(30),
        "invalid token authority or lifetime"
    );
    let tenant_id = token_tenant(&c, enrollment)?;
    ensure!(
        tenant_id == enrollment.enrolled_tenant_id(),
        "invalid token authority"
    );
    let role = token_role(&c, enrollment)?;
    Ok((
        Caller {
            sub: c.sub,
            tenant_id,
            role,
        },
        c.scope,
    ))
}

pub fn verify(token: &str, enrollment: &Enrollment, jwks: &Value) -> Result<Scope> {
    verify_caller(token, enrollment, jwks).map(|(_, scope)| scope)
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

    pub async fn authenticate(&self, bearer: &str) -> Result<Scope> {
        self.authenticate_with_caller(bearer)
            .await
            .map(|(_, scope)| scope)
    }

    pub async fn authenticate_with_caller(&self, bearer: &str) -> Result<(Caller, Scope)> {
        ensure!(bearer.len() <= 8192, "invalid token");
        if let Some(admin_hash) = self.admin_hash {
            let supplied: [u8; 32] = Sha256::digest(bearer.as_bytes()).into();
            if bool::from(supplied.ct_eq(&admin_hash)) {
                if let Some((enrollment, _)) = &self.central {
                    return Ok((
                        Caller {
                            sub: enrollment.owner_id.clone(),
                            tenant_id: enrollment.enrolled_tenant_id().to_owned(),
                            role: Role::Owner,
                        },
                        Scope::Operate,
                    ));
                }
                return Ok((
                    Caller {
                        sub: "administrator".into(),
                        tenant_id: String::new(),
                        role: Role::Owner,
                    },
                    Scope::Operate,
                ));
            }
        }
        self.central_jwt(bearer).await
    }

    pub async fn authenticate_central(&self, bearer: &str) -> Result<Scope> {
        self.authenticate_caller(bearer)
            .await
            .map(|(_, scope)| scope)
    }

    pub async fn authenticate_caller(&self, bearer: &str) -> Result<(Caller, Scope)> {
        ensure!(bearer.len() <= 8192, "invalid token");
        self.central_jwt(bearer).await
    }

    /// Tenant owner account id for box ownership defaults (A3 migration).
    pub fn tenant_owner(&self) -> Option<&str> {
        self.central
            .as_ref()
            .map(|(enrollment, _)| enrollment.owner_id.as_str())
    }

    pub fn enrolled_owner(&self) -> Option<&str> {
        self.tenant_owner()
    }

    /// Caller subject when the bearer is a valid central JWT; otherwise `anonymous`.
    pub async fn central_caller(&self, bearer: &str) -> String {
        if bearer.is_empty() {
            return "anonymous".into();
        }
        match self.authenticate_caller(bearer).await {
            Ok((caller, _)) => caller.sub,
            Err(_) => "anonymous".into(),
        }
    }

    async fn central_jwt(&self, bearer: &str) -> Result<(Caller, Scope)> {
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
        verify_caller(bearer, enrollment, &cache.jwks)
    }
}

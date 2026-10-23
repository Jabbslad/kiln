//! Local enrollment remains the authority for issuer, owner and server audience.
use anyhow::{Context, Result, ensure};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use kiln_api::auth::{AuthGrant, Caller, Role, Scope, ServerClaims};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Mutex as StdMutex;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;

const DENIAL_LIMIT: usize = 256;
static DENIALS: StdMutex<Vec<&'static str>> = StdMutex::new(Vec::new());

/// Record why a credential was refused. The line is only the check name.
pub fn deny(reason: &'static str) -> anyhow::Error {
    record_denial(reason);
    anyhow::anyhow!(reason)
}

fn record_denial(reason: &'static str) {
    if let Ok(mut rows) = DENIALS.lock() {
        if rows.len() == DENIAL_LIMIT {
            rows.remove(0);
        }
        rows.push(reason);
    }
    eprintln!("auth deny reason={reason}");
}

/// Reason codes recorded by [`deny`], oldest first.
pub fn denial_records() -> Vec<&'static str> {
    DENIALS
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .clone()
}

fn keys_reason(err: &anyhow::Error) -> &'static str {
    if err.to_string() == "invalid verification key" {
        "invalid_verification_key"
    } else {
        "invalid_jwks"
    }
}

fn jwt_reason(err: &jsonwebtoken::errors::Error) -> &'static str {
    use jsonwebtoken::errors::ErrorKind;
    match err.kind() {
        ErrorKind::ExpiredSignature => "expired_token",
        ErrorKind::ImmatureSignature => "token_not_yet_valid",
        ErrorKind::InvalidAudience => "audience_mismatch",
        ErrorKind::InvalidIssuer => "issuer_mismatch",
        ErrorKind::InvalidSubject => "invalid_subject",
        ErrorKind::InvalidSignature => "invalid_signature",
        ErrorKind::InvalidAlgorithm
        | ErrorKind::MissingAlgorithm
        | ErrorKind::InvalidAlgorithmName => "invalid_token_header",
        _ => "invalid_token",
    }
}

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

    /// Tenant id bound in `/etc/kiln/identity.json` for JWT verification.
    pub fn bound_tenant_id(&self) -> Option<&str> {
        self.tenant_id.as_deref().filter(|id| !id.is_empty())
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

    fn bind_owner_tenant_from_claim(&mut self, claims: &ServerClaims) -> Result<()> {
        ensure!(
            self.bound_tenant_id().is_none(),
            "enrollment tenant already bound"
        );
        ensure!(
            claims.sub == self.owner_id,
            "invalid token authority or lifetime"
        );
        let role = claims.role.unwrap_or(Role::Owner);
        ensure!(role == Role::Owner, "invalid token authority or lifetime");
        let tenant = claims
            .tenant_id
            .as_deref()
            .filter(|id| !id.is_empty())
            .unwrap_or(claims.sub.as_str());
        ensure!(
            !tenant.is_empty() && tenant.len() <= 128,
            "invalid token authority or lifetime"
        );
        self.tenant_id = Some(tenant.to_owned());
        Ok(())
    }

    pub fn persist(&self, path: &Path) -> Result<()> {
        use std::os::unix::fs::MetadataExt;
        self.validate()?;
        let meta = fs::symlink_metadata(path)?;
        ensure!(
            meta.is_file() && meta.uid() == 0 && meta.mode() & 0o027 == 0 && meta.len() < 8192,
            "enrollment must be a root-owned private regular file"
        );
        let parent = path.parent().context("enrollment directory missing")?;
        let bytes = serde_json::to_vec(self)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        file.persist(path)?;
        fs::File::open(parent)?.sync_all()?;
        Ok(())
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

pub fn verify_claims(
    token: &str,
    enrollment: &mut Enrollment,
    enrollment_file: Option<&Path>,
    jwks: &Value,
) -> Result<ServerClaims> {
    if let Err(err) = enrollment.validate() {
        record_denial("invalid_enrollment");
        return Err(err);
    }
    if token.len() > 8192 {
        record_denial("invalid_token");
    }
    ensure!(token.len() <= 8192, "invalid token");
    let header = match decode_header(token) {
        Ok(header) => header,
        Err(err) => {
            record_denial("invalid_token_header");
            return Err(err.into());
        }
    };
    let header_ok = header.alg == Algorithm::ES256
        && header.jku.is_none()
        && header.jwk.is_none()
        && header.x5u.is_none()
        && header.x5c.is_none();
    if !header_ok {
        record_denial("invalid_token_header");
    }
    ensure!(header_ok, "invalid token header");
    if header.kid.is_none() {
        record_denial("missing_key_id");
    }
    let kid = header.kid.context("missing token key id")?;
    let set = match keys(jwks) {
        Ok(set) => set,
        Err(err) => {
            record_denial(keys_reason(&err));
            return Err(err);
        }
    };
    if set.find(&kid).is_none() {
        record_denial("unknown_token_key");
    }
    let key = match DecodingKey::from_jwk(set.find(&kid).context("unknown token key")?) {
        Ok(key) => key,
        Err(err) => {
            record_denial("invalid_verification_key");
            return Err(err.into());
        }
    };
    let mut validation = Validation::new(Algorithm::ES256);
    validation.set_required_spec_claims(&["exp", "nbf", "iss", "aud", "sub"]);
    validation.set_issuer(&[&enrollment.issuer]);
    validation.set_audience(&[format!("kiln:server:{}", enrollment.server_id)]);
    validation.validate_nbf = true;
    validation.leeway = 30;
    let c = match decode::<ServerClaims>(token, &key, &validation) {
        Ok(data) => data.claims,
        Err(err) => {
            record_denial(jwt_reason(&err));
            return Err(err.into());
        }
    };
    if enrollment.bound_tenant_id().is_none() {
        if let Err(err) = enrollment.bind_owner_tenant_from_claim(&c) {
            record_denial("invalid_token_authority");
            return Err(err);
        }
        if let Some(path) = enrollment_file {
            // identity.json is root-owned; kiln-api binds in memory when persist is not allowed.
            let _ = enrollment.persist(path);
        }
    }
    let claim_tenant = c
        .tenant_id
        .clone()
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| c.sub.clone());
    let Some(bound) = enrollment.bound_tenant_id().map(str::to_owned) else {
        record_denial("invalid_token_authority");
        anyhow::bail!("invalid token authority or lifetime");
    };
    let authority_ok = claim_tenant == bound
        && !c.credential_id.is_empty()
        && c.credential_id.len() <= 128
        && c.exp > c.iat
        && c.exp - c.iat <= 300
        && c.nbf >= c.iat
        && c.nbf < c.exp
        && c.iat <= jsonwebtoken::get_current_timestamp().saturating_add(30);
    if !authority_ok {
        record_denial("invalid_token_authority");
    }
    ensure!(authority_ok, "invalid token authority or lifetime");
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
    let mut enrollment = enrollment.clone();
    Ok(auth_grant_from_claims(
        &verify_claims(token, &mut enrollment, None, jwks)?,
        &enrollment,
    ))
}

struct Cache {
    jwks: Value,
    last_fetch: Option<Instant>,
}
struct CentralAuth {
    enrollment: std::sync::Mutex<Enrollment>,
    jwks_cache: PathBuf,
    enrollment_file: Option<PathBuf>,
}

pub struct Authenticator {
    admin_hash: Option<[u8; 32]>,
    central: Option<CentralAuth>,
    cache: Mutex<Cache>,
    http: reqwest::Client,
}

impl Authenticator {
    pub fn new(admin: &str, central: Option<(Enrollment, PathBuf)>) -> Result<Self> {
        let central = match central {
            Some((enrollment, cache)) => Self::central(enrollment, cache, None)?,
            None => None,
        };
        Self::with_central(admin, central)
    }

    pub fn new_with_enrollment_file(
        admin: &str,
        enrollment: Enrollment,
        jwks_cache: PathBuf,
        enrollment_file: PathBuf,
    ) -> Result<Self> {
        let central = Self::central(enrollment, jwks_cache, Some(enrollment_file))?;
        Self::with_central(admin, central)
    }

    fn with_central(admin: &str, central: Option<CentralAuth>) -> Result<Self> {
        ensure!(
            admin.len() == 64 && admin.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid administrator token"
        );
        let mut jwks = serde_json::json!({"keys":[]});
        if let Some(central) = &central {
            central
                .enrollment
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .validate()?;
            if let Ok(meta) = fs::symlink_metadata(&central.jwks_cache) {
                ensure!(meta.is_file() && meta.len() <= 65536, "unsafe key cache");
                let cached: Value = serde_json::from_slice(&fs::read(&central.jwks_cache)?)?;
                ensure!(
                    cached["issuer"]
                        == central
                            .enrollment
                            .lock()
                            .unwrap_or_else(|poison| poison.into_inner())
                            .issuer,
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

    fn central(
        enrollment: Enrollment,
        jwks_cache: PathBuf,
        enrollment_file: Option<PathBuf>,
    ) -> Result<Option<CentralAuth>> {
        enrollment.validate()?;
        Ok(Some(CentralAuth {
            enrollment: std::sync::Mutex::new(enrollment),
            jwks_cache,
            enrollment_file,
        }))
    }

    /// Identity JWTs only. The local administrator token stays on the loopback API.
    pub fn central_only(enrollment: Enrollment, cache: PathBuf) -> Result<Self> {
        Self::central_with_enrollment_file(enrollment, cache, None)
    }

    pub fn central_with_enrollment_file(
        enrollment: Enrollment,
        cache: PathBuf,
        enrollment_file: Option<PathBuf>,
    ) -> Result<Self> {
        let mut auth = Self::with_central(
            &"0".repeat(64),
            Self::central(enrollment, cache, enrollment_file)?,
        )?;
        auth.admin_hash = None;
        Ok(auth)
    }

    pub async fn authenticate(&self, bearer: &str) -> Result<AuthGrant> {
        if bearer.len() > 8192 {
            record_denial("invalid_token");
        }
        ensure!(bearer.len() <= 8192, "invalid token");
        if let Some(admin_hash) = self.admin_hash {
            let supplied: [u8; 32] = Sha256::digest(bearer.as_bytes()).into();
            if bool::from(supplied.ct_eq(&admin_hash)) {
                let tenant_id = self
                    .central
                    .as_ref()
                    .and_then(|central| {
                        central
                            .enrollment
                            .lock()
                            .unwrap_or_else(|poison| poison.into_inner())
                            .bound_tenant_id()
                            .map(str::to_owned)
                    })
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
        if bearer.len() > 8192 {
            record_denial("invalid_token");
        }
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
        let claims = self.central_claims(bearer).await?;
        let enrollment = self
            .central
            .as_ref()
            .context("central authentication not enrolled")?
            .enrollment
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        Ok(auth_grant_from_claims(&claims, &enrollment))
    }

    async fn central_claims(&self, bearer: &str) -> Result<ServerClaims> {
        let central = match self.central.as_ref() {
            Some(central) => central,
            None => {
                record_denial("not_enrolled");
                anyhow::bail!("central authentication not enrolled");
            }
        };
        let header = match decode_header(bearer) {
            Ok(header) => header,
            Err(err) => {
                record_denial("invalid_token_header");
                return Err(err.into());
            }
        };
        let header_ok = header.alg == Algorithm::ES256
            && header.jku.is_none()
            && header.jwk.is_none()
            && header.x5u.is_none()
            && header.x5c.is_none();
        if !header_ok {
            record_denial("invalid_token_header");
        }
        ensure!(header_ok, "invalid token header");
        if header.kid.is_none() {
            record_denial("missing_key_id");
        }
        let kid = header.kid.context("missing token key id")?;
        let mut cache = self.cache.lock().await;
        let known = cache.jwks["keys"]
            .as_array()
            .is_some_and(|k| k.iter().any(|k| k["kid"] == kid));
        if !known {
            let key_fetch_allowed = cache
                .last_fetch
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(60));
            if !key_fetch_allowed {
                record_denial("unknown_token_key");
            }
            ensure!(key_fetch_allowed, "unknown token key");
            cache.last_fetch = Some(Instant::now());
            let issuer = central
                .enrollment
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .issuer
                .clone();
            let mut response = match self
                .http
                .get(format!(
                    "{}/.well-known/jwks.json",
                    issuer.trim_end_matches('/')
                ))
                .send()
                .await
            {
                Ok(response) => response,
                Err(err) => {
                    record_denial("identity_keys_unavailable");
                    return Err(err.into());
                }
            };
            if !response.status().is_success() {
                record_denial("identity_keys_unavailable");
            }
            ensure!(response.status().is_success(), "identity keys unavailable");
            let mut bytes = Vec::new();
            loop {
                let chunk = match response.chunk().await {
                    Ok(chunk) => chunk,
                    Err(err) => {
                        record_denial("identity_keys_unavailable");
                        return Err(err.into());
                    }
                };
                let Some(chunk) = chunk else {
                    break;
                };
                let fits = bytes.len() + chunk.len() <= 65536;
                if !fits {
                    record_denial("key_response_too_large");
                }
                ensure!(fits, "key response too large");
                bytes.extend_from_slice(&chunk);
            }
            let value: Value = match serde_json::from_slice(&bytes) {
                Ok(value) => value,
                Err(err) => {
                    record_denial("invalid_jwks");
                    return Err(err.into());
                }
            };
            if let Err(err) = keys(&value) {
                record_denial(keys_reason(&err));
                return Err(err);
            }
            let parent = match central.jwks_cache.parent() {
                Some(parent) => parent,
                None => {
                    record_denial("key_cache_directory_missing");
                    anyhow::bail!("key cache directory missing");
                }
            };
            let mut file = match tempfile::NamedTempFile::new_in(parent) {
                Ok(file) => file,
                Err(err) => {
                    record_denial("key_cache_unavailable");
                    return Err(err.into());
                }
            };
            let body = match serde_json::to_vec(&serde_json::json!({"issuer":issuer,"jwks":value}))
            {
                Ok(body) => body,
                Err(err) => {
                    record_denial("key_cache_unavailable");
                    return Err(err.into());
                }
            };
            if let Err(err) = file.write_all(&body) {
                record_denial("key_cache_unavailable");
                return Err(err.into());
            }
            if let Err(err) = file.as_file().sync_all() {
                record_denial("key_cache_unavailable");
                return Err(err.into());
            }
            if let Err(err) = file.persist(&central.jwks_cache) {
                record_denial("key_cache_unavailable");
                return Err(err.into());
            }
            let dir = match fs::File::open(parent) {
                Ok(dir) => dir,
                Err(err) => {
                    record_denial("key_cache_unavailable");
                    return Err(err.into());
                }
            };
            if let Err(err) = dir.sync_all() {
                record_denial("key_cache_unavailable");
                return Err(err.into());
            }
            cache.jwks = value;
        }
        let mut enrollment = central
            .enrollment
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        verify_claims(
            bearer,
            &mut enrollment,
            central.enrollment_file.as_deref(),
            &cache.jwks,
        )
    }
}

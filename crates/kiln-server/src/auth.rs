//! Local enrollment remains the authority for issuer, owner and server audience.
use anyhow::{Context, Result, ensure};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use kiln_api::auth::{Scope, ServerClaims};
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

pub fn verify(token: &str, enrollment: &Enrollment, jwks: &Value) -> Result<Scope> {
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
        c.sub == enrollment.owner_id
            && !c.credential_id.is_empty()
            && c.credential_id.len() <= 128
            && c.exp > c.iat
            && c.exp - c.iat <= 300
            && c.nbf >= c.iat
            && c.nbf < c.exp
            && c.iat <= jsonwebtoken::get_current_timestamp().saturating_add(30),
        "invalid token authority or lifetime"
    );
    Ok(c.scope)
}

struct Cache {
    jwks: Value,
    last_fetch: Option<Instant>,
}
pub struct Authenticator {
    admin_hash: [u8; 32],
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
            admin_hash: Sha256::digest(admin.as_bytes()).into(),
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

    pub async fn authenticate(&self, bearer: &str) -> Result<Scope> {
        ensure!(bearer.len() <= 8192, "invalid token");
        let supplied: [u8; 32] = Sha256::digest(bearer.as_bytes()).into();
        if bool::from(supplied.ct_eq(&self.admin_hash)) {
            return Ok(Scope::Operate);
        }
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
        let kid = header.kid.context("missing key id")?;
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
        verify(bearer, enrollment, &cache.jwks)
    }
}

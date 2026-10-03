use anyhow::Context;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use kiln_api::auth::ServerClaims;
use p256::{elliptic_curve::sec1::ToEncodedPoint, pkcs8::DecodePrivateKey};
use serde::Serialize;

#[derive(Clone)]
pub struct Signer {
    kid: String,
    encoding: EncodingKey,
    x: String,
    y: String,
    retiring: Vec<serde_json::Value>,
}

impl Signer {
    pub fn from_pem(kid: &str, pem: &[u8]) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !kid.is_empty() && kid.len() <= 128,
            "invalid signing key id"
        );
        let text = std::str::from_utf8(pem)?;
        let key = p256::SecretKey::from_pkcs8_pem(text).context("invalid ES256 PKCS#8 key")?;
        let point = key.public_key().to_encoded_point(false);
        Ok(Self {
            kid: kid.into(),
            encoding: EncodingKey::from_ec_pem(pem)?,
            x: URL_SAFE_NO_PAD.encode(point.x().context("x")?),
            y: URL_SAFE_NO_PAD.encode(point.y().context("y")?),
            retiring: Vec::new(),
        })
    }
    pub fn issue(&self, claims: &ServerClaims) -> anyhow::Result<String> {
        let mut h = Header::new(Algorithm::ES256);
        h.kid = Some(self.kid.clone());
        Ok(jsonwebtoken::encode(&h, claims, &self.encoding)?)
    }
    pub fn jwks(&self) -> serde_json::Value {
        let mut keys = self.retiring.clone();
        keys.push(serde_json::json!({"kty":"EC","crv":"P-256","use":"sig","alg":"ES256","kid":self.kid,"x":self.x,"y":self.y}));
        serde_json::json!({"keys":keys})
    }
    pub fn with_retiring(mut self, value: serde_json::Value) -> anyhow::Result<Self> {
        let entries = value["keys"].as_array().context("invalid retiring JWKS")?;
        anyhow::ensure!(entries.len() < 16, "too many retiring keys");
        let mut seen = std::collections::HashSet::from([self.kid.clone()]);
        for entry in entries {
            let kid = entry["kid"].as_str().context("missing retiring key ID")?;
            anyhow::ensure!(
                !kid.is_empty()
                    && kid.len() <= 128
                    && seen.insert(kid.to_owned())
                    && entry["kty"] == "EC"
                    && entry["crv"] == "P-256"
                    && entry["alg"] == "ES256"
                    && entry["use"] == "sig"
                    && entry.as_object().is_some_and(|m| m
                        .keys()
                        .all(
                            |k| ["kid", "kty", "crv", "alg", "use", "x", "y"].contains(&k.as_str())
                        )),
                "invalid retiring public key"
            );
            let jwk: jsonwebtoken::jwk::Jwk = serde_json::from_value(entry.clone())?;
            jsonwebtoken::DecodingKey::from_jwk(&jwk)?;
        }
        self.retiring = entries.clone();
        Ok(self)
    }
}

#[derive(Serialize)]
pub struct Grant {
    pub access_token: String,
    pub token_type: &'static str,
    pub expires_in: u64,
}

//! Identity-service wire types. These contain no runtime records or host paths.
use serde::{Deserialize, Serialize};

/// Public pilot identity service. Private builds can override it at compile time.
pub const DEFAULT_IDENTITY_ORIGIN: &str = match option_env!("KILN_DEFAULT_IDENTITY_URL") {
    Some(origin) => origin,
    None => "https://dark-forge.dev",
};

/// Enrolled redirect for the dashboard authorization code.
/// Identity refuses every other redirect.
pub const APP_OAUTH_REDIRECT: &str = "https://app.dark-forge.dev/oauth/callback";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Read,
    Operate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    #[default]
    Owner,
    Member,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Caller {
    pub sub: String,
    pub tenant_id: String,
    pub role: Role,
}

/// Per-server member box cap; enforced locally on each server (not tenant-wide).
pub const DEFAULT_MEMBER_BOX_QUOTA: u32 = 4;
pub const SERVER_BOX_QUOTA: u32 = 8;

pub const CALLER_SUB_HEADER: &str = "x-kiln-account";
pub const CALLER_TENANT_HEADER: &str = "x-kiln-tenant";
pub const CALLER_ROLE_HEADER: &str = "x-kiln-account-role";

pub fn caller_may_access_box(caller: &Caller, box_owner: &str) -> bool {
    match caller.role {
        Role::Owner => true,
        Role::Member => caller.sub == box_owner,
    }
}

pub fn filter_visible_boxes<'a>(
    caller: &Caller,
    box_owners: impl IntoIterator<Item = &'a str>,
) -> impl Iterator<Item = &'a str> {
    box_owners
        .into_iter()
        .filter(move |owner| caller_may_access_box(caller, owner))
}

pub fn parse_caller_headers(
    sub: Option<&str>,
    tenant_id: Option<&str>,
    role: Option<&str>,
) -> Option<Caller> {
    let sub = sub?;
    let tenant_id = tenant_id?;
    if sub.is_empty() || sub.len() > 128 || tenant_id.is_empty() || tenant_id.len() > 128 {
        return None;
    }
    let role = match role {
        Some("member") => Role::Member,
        Some("owner") | None => Role::Owner,
        Some(_) => return None,
    };
    Some(Caller {
        sub: sub.into(),
        tenant_id: tenant_id.into(),
        role,
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerDescriptor {
    pub id: String,
    pub name: String,
    pub origin: String,
    pub ca_pem: String,
    pub revision: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceAuthorization {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceTokens {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
    pub refresh_token: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessToken {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyToken {
    pub token: AccessToken,
    pub server: ServerDescriptor,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registration {
    pub server_id: String,
    pub owner_id: String,
    pub directory_token: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerClaims {
    pub iss: String,
    pub sub: String,
    pub aud: String,
    pub iat: u64,
    pub nbf: u64,
    pub exp: u64,
    pub credential_id: String,
    pub scope: Scope,
    #[serde(default)]
    pub role: Role,
    #[serde(default)]
    pub tenant_id: String,
}

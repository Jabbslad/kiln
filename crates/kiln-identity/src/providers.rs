use anyhow::Context;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet,
    EndpointNotSet, EndpointSet, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
    TokenResponse, TokenUrl, basic::BasicClient,
};
use openidconnect::TokenResponse as _;
use openidconnect::{
    AccessTokenHash, AuthenticationFlow, IssuerUrl, Nonce,
    core::{CoreClient, CoreProviderMetadata, CoreResponseType},
};
use serde::Deserialize;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Github,
    Google,
}
impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Google => "google",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderIdentity {
    pub provider: Provider,
    pub subject: String,
    pub display_name: String,
}

pub struct Authorization {
    pub url: String,
    pub state: String,
    pub pkce: String,
    pub nonce: Option<String>,
}

pub fn http_client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .no_proxy()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()?)
}

fn github_client(
    client_id: &str,
    secret: Option<&str>,
    redirect: &str,
    auth_url: &str,
    token_url: &str,
) -> anyhow::Result<
    BasicClient<
        oauth2::EndpointSet,
        oauth2::EndpointNotSet,
        oauth2::EndpointNotSet,
        oauth2::EndpointNotSet,
        oauth2::EndpointSet,
    >,
> {
    let mut client = BasicClient::new(ClientId::new(client_id.to_owned()))
        .set_auth_uri(AuthUrl::new(auth_url.into())?)
        .set_token_uri(TokenUrl::new(token_url.into())?)
        .set_redirect_uri(RedirectUrl::new(redirect.to_owned())?);
    if let Some(secret) = secret {
        client = client.set_client_secret(ClientSecret::new(secret.to_owned()));
    }
    Ok(client)
}

pub fn github_begin(client_id: &str, redirect: &str) -> anyhow::Result<Authorization> {
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let (url, state) = github_client(
        client_id,
        None,
        redirect,
        "https://github.com/login/oauth/authorize",
        "https://github.com/login/oauth/access_token",
    )?
    .authorize_url(CsrfToken::new_random)
    .set_pkce_challenge(challenge)
    .url();
    Ok(Authorization {
        url: url.into(),
        state: state.secret().clone(),
        pkce: verifier.secret().clone(),
        nonce: None,
    })
}

type GoogleClient = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

async fn google_client(
    http: &reqwest::Client,
    client_id: &str,
    secret: Option<&str>,
    redirect: &str,
    issuer: &str,
) -> anyhow::Result<GoogleClient> {
    let metadata =
        CoreProviderMetadata::discover_async(IssuerUrl::new(issuer.into())?, http).await?;
    anyhow::ensure!(
        metadata.issuer().as_str() == issuer,
        "unexpected Google issuer"
    );
    Ok(CoreClient::from_provider_metadata(
        metadata,
        ClientId::new(client_id.into()),
        secret.map(|s| ClientSecret::new(s.into())),
    )
    .set_redirect_uri(RedirectUrl::new(redirect.into())?))
}

pub async fn google_begin(
    http: &reqwest::Client,
    client_id: &str,
    redirect: &str,
) -> anyhow::Result<Authorization> {
    let client = google_client(
        http,
        client_id,
        None,
        redirect,
        "https://accounts.google.com",
    )
    .await?;
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let (url, state, nonce) = client
        .authorize_url(
            AuthenticationFlow::<CoreResponseType>::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .add_scope(Scope::new("profile".into()))
        .set_pkce_challenge(challenge)
        .url();
    Ok(Authorization {
        url: url.into(),
        state: state.secret().clone(),
        pkce: verifier.secret().clone(),
        nonce: Some(nonce.secret().clone()),
    })
}

#[derive(Deserialize)]
struct GhUser {
    id: u64,
    login: String,
}
pub async fn github_finish(
    client: &reqwest::Client,
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect: &str,
    pkce: &str,
) -> anyhow::Result<ProviderIdentity> {
    github_finish_at(
        client,
        client_id,
        client_secret,
        code,
        redirect,
        pkce,
        "https://github.com/login/oauth/access_token",
        "https://api.github.com/user",
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn github_finish_at(
    client: &reqwest::Client,
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect: &str,
    pkce: &str,
    token_url: &str,
    user_url: &str,
) -> anyhow::Result<ProviderIdentity> {
    let token = github_client(
        client_id,
        Some(client_secret),
        redirect,
        "https://github.com/login/oauth/authorize",
        token_url,
    )?
    .exchange_code(AuthorizationCode::new(code.into()))
    .set_pkce_verifier(PkceCodeVerifier::new(pkce.into()))
    .request_async(client)
    .await?;
    let user: GhUser = client
        .get(user_url)
        .bearer_auth(token.access_token().secret())
        .header("User-Agent", "kiln-identity")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(ProviderIdentity {
        provider: Provider::Github,
        subject: user.id.to_string(),
        display_name: user.login,
    })
}

pub async fn google_finish(
    client: &reqwest::Client,
    client_id: &str,
    client_secret: &str,
    redirect: &str,
    code: &str,
    nonce: &str,
    pkce: &str,
) -> anyhow::Result<ProviderIdentity> {
    google_finish_at(
        client,
        client_id,
        client_secret,
        redirect,
        code,
        nonce,
        pkce,
        "https://accounts.google.com",
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn google_finish_at(
    client: &reqwest::Client,
    client_id: &str,
    client_secret: &str,
    redirect: &str,
    code: &str,
    nonce: &str,
    pkce: &str,
    issuer: &str,
) -> anyhow::Result<ProviderIdentity> {
    let oidc = google_client(client, client_id, Some(client_secret), redirect, issuer).await?;
    let response = oidc
        .exchange_code(AuthorizationCode::new(code.into()))?
        .set_pkce_verifier(PkceCodeVerifier::new(pkce.into()))
        .request_async(client)
        .await?;
    let id_token = response.id_token().context("Google omitted id_token")?;
    let verifier = oidc.id_token_verifier();
    let claims = id_token.claims(&verifier, &Nonce::new(nonce.into()))?;
    if let Some(expected) = claims.access_token_hash() {
        let actual = AccessTokenHash::from_token(
            response.access_token(),
            id_token.signing_alg()?,
            id_token.signing_key(&verifier)?,
        )?;
        anyhow::ensure!(actual == *expected, "invalid access token hash");
    }
    let display_name = claims
        .name()
        .and_then(|n| n.get(None))
        .map(|n| n.as_str().to_owned())
        .unwrap_or_else(|| "Google user".into());
    Ok(ProviderIdentity {
        provider: Provider::Google,
        subject: claims.subject().as_str().into(),
        display_name,
    })
}

// Endpoint substitution is compiled only into the protocol test crate. Production callers can
// reach only the fixed provider endpoints above.
#[cfg(test)]
#[allow(dead_code, clippy::too_many_arguments)] // exercised by tests/providers.rs
pub async fn test_github_finish(
    client: &reqwest::Client,
    client_id: &str,
    secret: &str,
    code: &str,
    redirect: &str,
    pkce: &str,
    token_url: &str,
    user_url: &str,
) -> anyhow::Result<ProviderIdentity> {
    github_finish_at(
        client, client_id, secret, code, redirect, pkce, token_url, user_url,
    )
    .await
}

#[cfg(test)]
#[allow(dead_code, clippy::too_many_arguments)] // exercised by tests/providers.rs
pub async fn test_google_finish(
    client: &reqwest::Client,
    client_id: &str,
    secret: &str,
    redirect: &str,
    code: &str,
    nonce: &str,
    pkce: &str,
    issuer: &str,
) -> anyhow::Result<ProviderIdentity> {
    google_finish_at(
        client, client_id, secret, redirect, code, nonce, pkce, issuer,
    )
    .await
}

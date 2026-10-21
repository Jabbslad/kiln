use clap::{CommandFactory, FromArgMatches, Parser};
use kiln_identity::{Config, Store, router, unix_now};
use std::{net::SocketAddr, path::PathBuf};

#[derive(Parser)]
#[command(about = "Kiln central identity service")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:8091")]
    listen: SocketAddr,
    #[arg(long)]
    public_origin: String,
    #[arg(long)]
    database: PathBuf,
    #[arg(long)]
    github_client_id: String,
    #[arg(long)]
    github_secret_file: PathBuf,
    #[arg(long)]
    google_client_id: String,
    #[arg(long)]
    google_secret_file: PathBuf,
    #[arg(long)]
    signing_key_file: PathBuf,
    #[arg(long)]
    signing_key_id: String,
    #[arg(long)]
    retiring_jwks_file: Option<PathBuf>,
    /// Trust an X-Forwarded-For value overwritten by the loopback HTTPS proxy.
    #[arg(long)]
    trusted_proxy_loopback: bool,
    #[cfg(feature = "lab-provider")]
    #[arg(long, default_value = kiln_identity::GITHUB_AUTHORIZE_URL)]
    github_authorize_url: String,
    #[cfg(feature = "lab-provider")]
    #[arg(long, default_value = kiln_identity::GITHUB_TOKEN_URL)]
    github_token_url: String,
    #[cfg(feature = "lab-provider")]
    #[arg(long, default_value = kiln_identity::GITHUB_USER_URL)]
    github_user_url: String,
    /// Extra CA for a lab provider. Refused when the public origin is dark-forge.dev.
    #[cfg(feature = "lab-provider")]
    #[arg(long)]
    provider_ca_file: Option<PathBuf>,
}

#[cfg(feature = "lab-provider")]
fn provider_fields(x: &Args) -> anyhow::Result<(String, String, String, Option<Vec<u8>>)> {
    Ok((
        x.github_authorize_url.clone(),
        x.github_token_url.clone(),
        x.github_user_url.clone(),
        x.provider_ca_file
            .as_deref()
            .map(std::fs::read)
            .transpose()?,
    ))
}

#[cfg(not(feature = "lab-provider"))]
fn provider_fields(_x: &Args) -> anyhow::Result<(String, String, String, Option<Vec<u8>>)> {
    Ok((
        kiln_identity::GITHUB_AUTHORIZE_URL.to_owned(),
        kiln_identity::GITHUB_TOKEN_URL.to_owned(),
        kiln_identity::GITHUB_USER_URL.to_owned(),
        None,
    ))
}

fn parse_args() -> Args {
    let matches = Args::command()
        .version(kiln_api::release_version())
        .get_matches();
    Args::from_arg_matches(&matches).unwrap_or_else(|error| error.exit())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let x = parse_args();
    anyhow::ensure!(
        x.listen.ip().is_loopback(),
        "identity service must bind to loopback"
    );
    let (github_authorize_url, github_token_url, github_user_url, provider_ca_pem) =
        provider_fields(&x)?;
    let store = Store::open(&x.database)?;
    let app = router(
        Config {
            public_origin: x.public_origin,
            github_client_id: x.github_client_id,
            github_secret_file: x.github_secret_file,
            google_client_id: x.google_client_id,
            google_secret_file: x.google_secret_file,
            signing_key_file: x.signing_key_file,
            signing_key_id: x.signing_key_id,
            retiring_jwks_file: x.retiring_jwks_file,
            trusted_proxy_loopback: x.trusted_proxy_loopback,
            github_authorize_url,
            github_token_url,
            github_user_url,
            provider_ca_pem,
            now: unix_now,
        },
        store,
    )?;
    let listener = tokio::net::TcpListener::bind(x.listen).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

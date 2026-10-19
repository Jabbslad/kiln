use clap::Parser;
use kiln_identity::{Config, Store, router, unix_now};
use std::{net::SocketAddr, path::PathBuf};

#[derive(Parser)]
#[command(version, about = "Kiln central identity service")]
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
    #[arg(long, default_value = kiln_identity::GITHUB_AUTHORIZE_URL)]
    github_authorize_url: String,
    #[arg(long, default_value = kiln_identity::GITHUB_TOKEN_URL)]
    github_token_url: String,
    #[arg(long, default_value = kiln_identity::GITHUB_USER_URL)]
    github_user_url: String,
    /// Extra CA for a lab provider. Refused when the public origin is dark-forge.dev.
    #[arg(long)]
    provider_ca_file: Option<PathBuf>,
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let x = Args::parse();
    anyhow::ensure!(
        x.listen.ip().is_loopback(),
        "identity service must bind to loopback"
    );
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
            github_authorize_url: x.github_authorize_url,
            github_token_url: x.github_token_url,
            github_user_url: x.github_user_url,
            provider_ca_pem: x.provider_ca_file.map(std::fs::read).transpose()?,
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

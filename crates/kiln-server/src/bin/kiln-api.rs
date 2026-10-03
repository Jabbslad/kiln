use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use std::{net::SocketAddr, path::PathBuf};

#[derive(Parser)]
#[command(
    version,
    about = "Unprivileged HTTPS gateway for kiln",
    subcommand_negates_reqs = true
)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(long, default_value = "127.0.0.1:8443")]
    listen: SocketAddr,
    #[arg(long, required = true)]
    host_socket: Option<PathBuf>,
    #[arg(long, required = true)]
    token_file: Option<PathBuf>,
    #[arg(long, required = true)]
    tls_cert: Option<PathBuf>,
    #[arg(long, required = true)]
    tls_key: Option<PathBuf>,
    #[arg(long, default_value = "/etc/kiln/identity.json")]
    enrollment_file: PathBuf,
    #[arg(long, default_value = "/var/lib/kiln-api/jwks.json")]
    jwks_cache: PathBuf,
}

#[derive(Subcommand)]
enum Command {
    /// Enroll this installed host. Restarts only the API and disconnects API/SSH sessions.
    Enroll(kiln_server::enrollment::Options),
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    if let Some(Command::Enroll(options)) = args.command {
        return kiln_server::enrollment::run(options).await;
    }
    ensure!(
        unsafe { libc::geteuid() } != 0,
        "HTTPS gateway must run as an unprivileged user"
    );
    let token = kiln_api::read_token(&args.token_file.context("token file required")?)?;
    let enrollment = kiln_server::auth::Enrollment::load(&args.enrollment_file)?;
    let auth =
        kiln_server::auth::Authenticator::new(&token, enrollment.map(|e| (e, args.jwks_cache)))?;
    let router = kiln_server::gateway::router_with_auth(
        &args.host_socket.context("host socket required")?,
        auth,
    )?;
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(
        args.tls_cert.context("TLS certificate required")?,
        args.tls_key.context("TLS key required")?,
    )
    .await?;
    eprintln!("HTTPS gateway listening on {}", args.listen);
    axum_server::bind_rustls(args.listen, tls)
        .serve(router.into_make_service())
        .await?;
    Ok(())
}

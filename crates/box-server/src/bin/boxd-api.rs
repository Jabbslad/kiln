use anyhow::{Result, ensure};
use clap::Parser;
use std::{net::SocketAddr, path::PathBuf};

#[derive(Parser)]
#[command(version, about = "Unprivileged HTTPS gateway for boxd")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:8443")]
    listen: SocketAddr,
    #[arg(long)]
    host_socket: PathBuf,
    #[arg(long)]
    token_file: PathBuf,
    #[arg(long)]
    tls_cert: PathBuf,
    #[arg(long)]
    tls_key: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    ensure!(
        unsafe { libc::geteuid() } != 0,
        "HTTPS gateway must run as an unprivileged user"
    );
    let token = box_api::read_token(&args.token_file)?;
    let router = box_server::gateway::router(&args.host_socket, &token)?;
    let tls =
        axum_server::tls_rustls::RustlsConfig::from_pem_file(&args.tls_cert, &args.tls_key).await?;
    eprintln!("HTTPS gateway listening on {}", args.listen);
    axum_server::bind_rustls(args.listen, tls)
        .serve(router.into_make_service())
        .await?;
    Ok(())
}

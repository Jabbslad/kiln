use anyhow::Result;
use box_server::host::{Config, Host, bind_socket};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "Local boxd host service; Unix socket only")]
struct Args {
    #[arg(long)]
    config: PathBuf,
    /// Validate configuration, templates and runtime policy without serving.
    #[arg(long)]
    check: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    unsafe {
        libc::umask(0o077);
    }
    let args = Args::parse();
    if unsafe { libc::geteuid() } == 0 {
        box_runtime::isolation::trusted_path(&args.config)?;
    }
    let config: Config = box_runtime::storage::read_json(&args.config)?;
    let host = Host::open(&config)?;
    if args.check {
        println!("host configuration and catalog validated");
        return Ok(());
    }
    let listener = bind_socket(&config.socket).await?;
    eprintln!("host service listening on {}", config.socket.display());
    axum::serve(listener, host.router()).await?;
    Ok(())
}

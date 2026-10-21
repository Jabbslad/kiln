use anyhow::Result;
use clap::{CommandFactory, FromArgMatches, Parser};
use kiln_server::host::{Config, Host, bind_socket};
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Local kiln host service; Unix socket only")]
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
    let matches = Args::command()
        .version(kiln_api::release_version())
        .get_matches();
    let args = Args::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
    if unsafe { libc::geteuid() } == 0 {
        kiln_runtime::isolation::trusted_path(&args.config)?;
    }
    let config: Config = kiln_runtime::storage::read_json(&args.config)?;
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

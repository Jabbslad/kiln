use anyhow::{Context, Result, ensure};
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use std::{net::SocketAddr, path::PathBuf};

#[derive(Parser)]
#[command(
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
    #[arg(long, default_value = "/etc/kiln/login.token")]
    login_token_file: PathBuf,
}

#[derive(Subcommand)]
enum Command {
    /// Enroll this installed host. Restarts only the API and disconnects API/SSH sessions.
    Enroll(kiln_server::enrollment::Options),
    /// Serve the public door. Identity JWTs are checked before loopback kiln-api is dialed.
    Door {
        #[arg(long)]
        listen: SocketAddr,
        #[arg(long)]
        upstream: String,
        #[arg(long)]
        tls_cert: PathBuf,
        #[arg(long)]
        tls_key: PathBuf,
        #[arg(long)]
        upstream_ca: PathBuf,
        #[arg(long, default_value = "/etc/kiln/identity.json")]
        enrollment_file: PathBuf,
        #[arg(long, default_value = "/var/lib/kiln-api/jwks.json")]
        jwks_cache: PathBuf,
        /// Directory of the unpacked dashboard archive. Missing directories serve no HTML.
        #[arg(long)]
        dashboard_root: Option<PathBuf>,
        #[arg(long, default_value = "/var/lib/kiln-api/dashboard-session")]
        session_dir: PathBuf,
        #[arg(long, default_value = "/etc/kiln/login.token")]
        login_token_file: PathBuf,
    },
    /// Create /etc/kiln/login.token when that path is absent.
    #[command(hide = true)]
    PublishLoginToken,
    /// Validate live host dashboard metrics over loopback kiln-api.
    SelfCheck {
        #[arg(long)]
        address: String,
        #[arg(long, default_value = "/etc/kiln/admin.token")]
        token_file: PathBuf,
        #[arg(long, default_value = "/etc/kiln/ca.crt")]
        ca_file: PathBuf,
        #[arg(long)]
        require_launch_timing: Option<String>,
    },
    /// Serve the dashboard archive and the read API on 127.0.0.1:8093.
    App {
        #[arg(long, default_value = "127.0.0.1:8093")]
        listen: SocketAddr,
        #[arg(long, default_value = "https://127.0.0.1:8443")]
        upstream: String,
        #[arg(long)]
        upstream_ca: PathBuf,
        #[arg(long, default_value = "/etc/kiln/identity.json")]
        enrollment_file: PathBuf,
        #[arg(long, default_value = "/var/lib/kiln-api/jwks.json")]
        jwks_cache: PathBuf,
        /// Directory of the unpacked dashboard archive. Missing directories serve no HTML.
        #[arg(long)]
        dashboard_root: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let matches = Args::command()
        .version(kiln_api::release_version())
        .get_matches();
    let args = Args::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
    if let Some(command) = args.command {
        return match command {
            Command::Enroll(options) => kiln_server::enrollment::run(options).await,
            Command::PublishLoginToken => match kiln_server::enrollment::publish_login().await {
                Ok(kiln_server::enrollment::LoginPublish::Created) => {
                    println!("created");
                    Ok(())
                }
                Ok(kiln_server::enrollment::LoginPublish::Unchanged) => {
                    println!("unchanged");
                    Ok(())
                }
                Err(error) => {
                    eprintln!("login-token: {error}");
                    std::process::exit(1);
                }
            },
            Command::Door {
                listen,
                upstream,
                tls_cert,
                tls_key,
                upstream_ca,
                enrollment_file,
                jwks_cache,
                dashboard_root,
                session_dir,
                login_token_file,
            } => {
                let enrollment = kiln_server::auth::Enrollment::load(&enrollment_file)?
                    .context("server is not enrolled")?;
                let auth =
                    kiln_server::auth::Authenticator::central_only(enrollment.clone(), jwks_cache)?
                        .attach_login_secret_file(&login_token_file)?;
                let ca = std::fs::read(&upstream_ca).context("upstream CA required")?;
                kiln_server::door::serve(
                    listen,
                    &tls_cert,
                    &tls_key,
                    auth,
                    &upstream,
                    &ca,
                    kiln_server::door::Dashboard {
                        enrollment,
                        session_dir,
                        root: dashboard_root,
                        identity_ca: None,
                    },
                )
                .await
            }
            Command::App {
                listen,
                upstream,
                upstream_ca,
                enrollment_file,
                jwks_cache,
                dashboard_root,
            } => {
                kiln_server::app_listener::run(kiln_server::app_listener::Options {
                    listen,
                    upstream,
                    upstream_ca,
                    enrollment_file,
                    jwks_cache,
                    dashboard_root,
                })
                .await
            }
            Command::SelfCheck {
                address,
                token_file,
                ca_file,
                require_launch_timing,
            } => {
                if let Err(error) = kiln_server::dashboard_check::run_self_check(
                    kiln_server::dashboard_check::SelfCheckOptions {
                        address,
                        token_file,
                        ca_file,
                        require_launch_timing,
                    },
                )
                .await
                {
                    eprintln!("dashboard-metrics: {error:#}");
                    std::process::exit(1);
                }
                Ok(())
            }
        };
    }
    ensure!(
        unsafe { libc::geteuid() } != 0,
        "HTTPS gateway must run as an unprivileged user"
    );
    let token = kiln_api::read_token(&args.token_file.context("token file required")?)?;
    let enrollment = kiln_server::auth::Enrollment::load(&args.enrollment_file)?;
    let workspace = enrollment
        .as_ref()
        .map(|enrollment| enrollment.server_id.clone())
        .unwrap_or_default();
    let auth = kiln_server::auth::Authenticator::new(
        &token,
        enrollment
            .as_ref()
            .map(|e| (e.clone(), args.jwks_cache.clone())),
    )?
    .attach_login_secret_file(&args.login_token_file)?;
    let router = kiln_server::gateway::router_with_workspace(
        &args.host_socket.context("host socket required")?,
        auth,
        workspace,
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

mod ssh;
use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use kiln_api::{Action, BoxView, ExecRequest, Operation, OperationState, Outcome, Submit, new_id};
use kiln_client::Client;
use kiln_client::auth::{IdentityClient, LoginPoll, Session, credential_from_login};
use kiln_client::profile;
use reqwest::Method;
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Parser)]
#[command(
    name = "kiln",
    version,
    about = "Manage Kiln microVMs over authenticated HTTPS"
)]
struct Cli {
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[arg(long, global = true)]
    profile: Option<String>,
    #[arg(long, global = true)]
    issuer: Option<String>,
    #[arg(long, global = true)]
    auth_token_file: Option<PathBuf>,
    #[arg(long, global = true)]
    json: bool,
    /// Reuse only for an identical request; never generates a duplicate operation.
    #[arg(long, global = true)]
    request_id: Option<String>,
    /// Return accepted operation immediately; the server continues working.
    #[arg(long, global = true)]
    no_wait: bool,
    #[arg(long, global = true, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..=3605))]
    wait_seconds: u64,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Login {
        #[arg(long)]
        server: Option<String>,
    },
    Logout,
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    Ssh {
        id: String,
        #[arg(last = true)]
        command: Vec<String>,
    },
    Cp {
        source: String,
        destination: String,
    },
    SshConfig {
        id: String,
    },
    #[command(hide = true)]
    SshProxy {
        id: String,
    },
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },
    Templates,
    List,
    Inspect {
        id: String,
    },
    Create {
        #[arg(long)]
        template: String,
        #[arg(long, default_value = "box")]
        name: String,
    },
    Pause {
        id: String,
    },
    Resume {
        id: String,
    },
    Stop {
        id: String,
        #[arg(long)]
        force: bool,
    },
    Start {
        id: String,
    },
    Delete {
        id: String,
    },
    Exec {
        id: String,
        #[arg(long, default_value_t = 10000)]
        timeout_ms: u64,
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long = "env")]
        env: Vec<String>,
        #[arg(last = true, required = true)]
        argv: Vec<String>,
    },
    Operation {
        id: String,
        #[arg(long)]
        wait: bool,
    },
}

#[derive(Subcommand)]
enum AuthCommand {
    Status,
    Devices {
        #[command(subcommand)]
        command: DeviceCommand,
    },
    Keys {
        #[command(subcommand)]
        command: KeyCommand,
    },
}
#[derive(Subcommand)]
enum DeviceCommand {
    List,
    Revoke { id: String },
}
#[derive(Subcommand)]
enum KeyCommand {
    Create {
        #[arg(long)]
        server: String,
        #[arg(long, default_value = "operate")]
        scope: String,
        #[arg(long)]
        output: PathBuf,
    },
    List,
    Revoke {
        id: String,
    },
}

#[derive(Subcommand)]
enum ProfileCommand {
    Add {
        name: String,
        #[arg(long)]
        url: String,
        #[arg(long)]
        token_file: PathBuf,
        #[arg(long)]
        ca_file: Option<PathBuf>,
    },
    List,
}

fn json(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn show_box(record: &BoxView) {
    println!(
        "{}\t{}\t{}\t{} MiB\t{} vCPU",
        record.id, record.name, record.state, record.memory_mib, record.vcpus
    );
}

fn epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn credential_path(config: &Path, profile: &str) -> PathBuf {
    config
        .parent()
        .unwrap_or(Path::new("."))
        .join("credentials")
        .join(format!("{profile}-{}.json", kiln_api::new_id()))
}
fn identity_origin(explicit: Option<&str>) -> Result<String> {
    explicit
        .map(str::to_owned)
        .or_else(|| std::env::var("KILN_IDENTITY_URL").ok())
        .or_else(|| option_env!("KILN_DEFAULT_IDENTITY_URL").map(str::to_owned))
        .context("identity origin is not configured; pass --issuer or set KILN_IDENTITY_URL")
}
fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = std::process::Command::new("xdg-open");
    #[cfg(windows)]
    let mut command = {
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    command.arg(url);
    let status = command.status().context("could not launch browser")?;
    anyhow::ensure!(status.success(), "browser launch failed");
    Ok(())
}
fn show_operation(op: &Operation, as_json: bool) -> Result<i32> {
    if as_json {
        json(op)?;
    }
    if op.state == OperationState::Unknown || op.state == OperationState::Failed {
        if !as_json {
            eprintln!(
                "operation {} is {:?}: {}",
                op.id,
                op.state,
                op.error
                    .as_ref()
                    .map(|e| e.message.as_str())
                    .unwrap_or("no confirmed result")
            );
        }
        return Ok(1);
    }
    match &op.result {
        Some(Outcome::Exec(result)) => {
            if !as_json {
                if result.truncated {
                    eprintln!("warning: guest output was truncated");
                }
                std::io::stdout().write_all(&result.stdout)?;
                std::io::stderr().write_all(&result.stderr)?;
            }
            return Ok(if result.timed_out {
                124
            } else {
                result
                    .exit_code
                    .filter(|code| (0..=255).contains(code))
                    .unwrap_or(125)
            });
        }
        Some(Outcome::Box(record)) if !as_json => show_box(record),
        Some(Outcome::Deleted { id }) if !as_json => println!("deleted {id}"),
        None if !as_json => println!("{}\t{:?}\tbox {}", op.id, op.state, op.box_id),
        _ => {}
    }
    Ok(0)
}

async fn wait(client: &Client, mut op: Operation, seconds: u64) -> Result<Operation> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
    while op.state == OperationState::Running {
        if tokio::time::Instant::now() >= deadline {
            bail!(
                "operation {} still running; reconnect with `kiln operation {} --wait`",
                op.id,
                op.id
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        op = client.operation(&op.id).await.with_context(|| format!("poll interrupted; reconnect with `kiln operation {} --wait`; do not submit a new request", op.id))?;
    }
    Ok(op)
}

async fn run(cli: Cli) -> Result<i32> {
    use std::io::IsTerminal;
    let path = match cli.config {
        Some(p) => p,
        None => profile::default_path()?,
    };
    let profile_name = cli.profile.as_deref().unwrap_or("default").to_owned();
    anyhow::ensure!(kiln_api::valid_alias(&profile_name), "invalid profile name");
    let interactive =
        std::io::stdin().is_terminal() && std::io::stderr().is_terminal() && !cli.json;
    let explicit_login = matches!(cli.command, Command::Login { .. });
    anyhow::ensure!(
        cli.auth_token_file.is_none() || explicit_login,
        "use `kiln login --auth-token-file PATH` to configure an automation profile first"
    );
    let before = profile::load(&path)?;
    let auto_login = cli.profile.is_none()
        && interactive
        && cli.auth_token_file.is_none()
        && !matches!(
            cli.command,
            Command::SshProxy { .. }
                | Command::Profile { .. }
                | Command::Auth { .. }
                | Command::Logout
        )
        && !before.profiles.contains_key(&profile_name);
    if explicit_login || auto_login {
        let server = match &cli.command {
            Command::Login { server } => server.as_ref(),
            _ => None,
        };
        anyhow::ensure!(
            !matches!(
                before.profiles.get(&profile_name),
                Some(profile::Profile::Direct(_))
            ),
            "profile is a direct administrator profile; choose another --profile for login"
        );
        let existing = match before.profiles.get(&profile_name) {
            Some(profile::Profile::Central(p)) => Some(p),
            _ => None,
        };
        let issuer = identity_origin(
            cli.issuer
                .as_deref()
                .or(existing.map(|p| p.issuer.as_str())),
        )?;
        let identity = IdentityClient::new(&issuer)?;
        let issuer = reqwest::Url::parse(&issuer)?.origin().ascii_serialization();
        if let Some(existing) = existing {
            anyhow::ensure!(
                existing.issuer == issuer,
                "refusing changed identity issuer; use a separate profile"
            );
            let store =
                kiln_client::credentials::CredentialStore::new(existing.credential_file.clone());
            if cli.auth_token_file.is_none() && store.load()?.is_some() {
                let session = Session::new(existing.clone())?;
                let me = session.identity_json(Method::GET, "v1/me", None).await
                    .context("existing session is unavailable; use `kiln logout` before changing accounts")?;
                if let Some(id) = server {
                    let servers: Vec<kiln_api::auth::ServerDescriptor> = serde_json::from_value(
                        session
                            .identity_json(Method::GET, "v1/servers", None)
                            .await?,
                    )?;
                    let selected = servers
                        .into_iter()
                        .find(|s| &s.id == id)
                        .context("server ID not found")?;
                    profile::set(
                        &path,
                        &profile_name,
                        profile::Profile::Central(profile::CentralProfile {
                            server: selected,
                            ..existing.clone()
                        }),
                    )?;
                }
                if cli.json {
                    json(&me)?;
                } else {
                    eprintln!("Already signed in. Use `kiln logout` to change accounts.");
                }
                return Ok(0);
            }
        }
        if let Some(token_file) = &cli.auth_token_file {
            let key = identity.token_file(token_file).await?;
            let credential_file = std::path::absolute(credential_path(&path, &profile_name))?;
            let credentials = kiln_client::credentials::Credentials {
                issuer: issuer.clone(),
                access_token: String::new(),
                access_expires_at: 0,
                refresh_token: String::new(),
                automation_key_file: Some(fs::canonicalize(token_file)?),
                server_tokens: [(
                    format!("{}:credential", key.server.id),
                    kiln_client::credentials::CachedToken {
                        token: key.token.access_token,
                        expires_at: epoch() + key.token.expires_in,
                    },
                )]
                .into_iter()
                .collect(),
            };
            profile::save_login(
                &path,
                &profile_name,
                profile::CentralProfile {
                    issuer,
                    server: key.server,
                    credential_file,
                },
                &credentials,
            )?;
            return Ok(0);
        }
        anyhow::ensure!(
            interactive,
            "browser login requires an interactive terminal"
        );
        let flow = identity.begin_login("kiln CLI").await?;
        eprintln!(
            "Open {} and enter code {}",
            flow.verification_uri, flow.user_code
        );
        if let Err(error) = open_browser(&flow.verification_uri_complete) {
            eprintln!("Browser could not be opened ({error}); use the displayed URL and code.");
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(flow.expires_in);
        let mut interval = flow.interval;
        let tokens = loop {
            anyhow::ensure!(
                tokio::time::Instant::now() < deadline,
                "login approval expired"
            );
            tokio::select! {
                _ = tokio::signal::ctrl_c() => bail!("login cancelled"),
                _ = tokio::time::sleep_until(std::cmp::min(deadline, tokio::time::Instant::now()+Duration::from_secs(interval))) => {},
            }
            anyhow::ensure!(
                tokio::time::Instant::now() < deadline,
                "login approval expired"
            );
            match identity.poll_login(&flow.device_code).await? {
                LoginPoll::Ready(t) => break t,
                LoginPoll::Pending => {}
                LoginPoll::SlowDown => interval = interval.saturating_add(5),
            }
        };
        let servers = identity.servers(&tokens.access_token).await?;
        let selected = match server {
            Some(id) => servers
                .into_iter()
                .find(|s| &s.id == id)
                .context("selected server not found")?,
            None if servers.len() == 1 => servers.into_iter().next().unwrap(),
            None if servers.is_empty() => bail!(
                "no active servers are available; run `kiln-api enroll` on your server, then log in again"
            ),
            None => {
                for (n, s) in servers.iter().enumerate() {
                    eprintln!("{}: {} ({})", n + 1, s.name, s.id);
                }
                eprint!("Select a server number: ");
                std::io::stderr().flush()?;
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                let n: usize = input.trim().parse().context("invalid selection")?;
                servers
                    .into_iter()
                    .nth(n.checked_sub(1).context("invalid selection")?)
                    .context("invalid selection")?
            }
        };
        let credential_file = std::path::absolute(credential_path(&path, &profile_name))?;
        let credentials = credential_from_login(&issuer, tokens);
        profile::save_login(
            &path,
            &profile_name,
            profile::CentralProfile {
                issuer,
                server: selected,
                credential_file,
            },
            &credentials,
        )?;
        if explicit_login {
            return Ok(0);
        }
    }
    if let Command::Profile { command } = cli.command {
        match command {
            ProfileCommand::Add {
                name,
                url,
                token_file,
                ca_file,
            } => {
                profile::add(
                    &path,
                    &name,
                    profile::Profile::Direct(profile::DirectProfile {
                        url,
                        token_file: fs::canonicalize(token_file)?,
                        ca_file: ca_file.map(fs::canonicalize).transpose()?,
                    }),
                )?;
                if cli.json {
                    json(&serde_json::json!({"profile": name}))?;
                } else {
                    println!("added profile {name}");
                }
            }
            ProfileCommand::List => {
                let profiles = profile::load(&path)?;
                if cli.json {
                    json(&profiles)?;
                } else {
                    for (name, entry) in profiles.profiles {
                        let origin = match entry {
                            profile::Profile::Direct(p) => p.url,
                            profile::Profile::Central(p) => p.server.origin,
                        };
                        println!("{name}\t{origin}");
                    }
                }
            }
        }
        return Ok(0);
    }
    let profiles = profile::load(&path)?;
    let selected = profiles
        .profiles
        .get(&profile_name)
        .context("profile not found; use `kiln profile add`")?;
    if matches!(cli.command, Command::Logout | Command::Auth { .. }) {
        let profile::Profile::Central(central) = selected else {
            bail!("command requires a central profile")
        };
        let session = Session::new(central.clone())?;
        match &cli.command {
            Command::Logout => {
                let remote = async {
                    let me = session.identity_json(Method::GET, "v1/me", None).await?;
                    let id = me["device_id"]
                        .as_str()
                        .context("identity omitted device ID")?;
                    session
                        .identity_json(Method::DELETE, &format!("v1/devices/{id}"), None)
                        .await
                }
                .await;
                if let Err(e) = remote {
                    eprintln!("warning: remote revocation was not confirmed: {e}")
                }
                session.clear().await?;
                return Ok(0);
            }
            Command::Auth { command } => {
                let value = match command {
                    AuthCommand::Status => {
                        session.identity_json(Method::GET, "v1/me", None).await?
                    }
                    AuthCommand::Devices {
                        command: DeviceCommand::List,
                    } => {
                        session
                            .identity_json(Method::GET, "v1/devices", None)
                            .await?
                    }
                    AuthCommand::Devices {
                        command: DeviceCommand::Revoke { id },
                    } => {
                        session
                            .identity_json(Method::DELETE, &format!("v1/devices/{id}"), None)
                            .await?
                    }
                    AuthCommand::Keys {
                        command: KeyCommand::List,
                    } => session.identity_json(Method::GET, "v1/keys", None).await?,
                    AuthCommand::Keys {
                        command: KeyCommand::Revoke { id },
                    } => {
                        session
                            .identity_json(Method::DELETE, &format!("v1/keys/{id}"), None)
                            .await?
                    }
                    AuthCommand::Keys {
                        command:
                            KeyCommand::Create {
                                server,
                                scope,
                                output,
                            },
                    } => {
                        let value=session.identity_json(Method::POST,"v1/keys",Some(serde_json::json!({"server_id":server,"scope":kiln_client::auth::scope(scope)?}))).await?;
                        let secret = value["key"].as_str().context("identity omitted key")?;
                        kiln_client::credentials::write_secret(output, secret)?;
                        let mut redacted = value;
                        redacted["key"] = serde_json::Value::String("<written to file>".into());
                        redacted
                    }
                };
                json(&value)?;
                return Ok(0);
            }
            _ => unreachable!(),
        }
    }
    let client = selected.client()?;
    match &cli.command {
        Command::SshProxy { id } => {
            let (_, public) = ssh::ensure_key(&path, &profile_name)?;
            let tunnel = client.ssh_tunnel(id, &public).await?;
            let (mut read, mut write) = tokio::io::split(tunnel);
            let upload = tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                tokio::io::copy(&mut tokio::io::stdin(), &mut write).await?;
                write.shutdown().await
            });
            tokio::io::copy(&mut read, &mut tokio::io::stdout()).await?;
            upload.abort();
            return Ok(0);
        }
        Command::Ssh { id, command } => {
            let details = ssh_details(&path, &profile_name, id, &client).await?;
            let mut process = tokio::process::Command::new("ssh");
            process.args(&details.options).arg(format!("root@{id}"));
            if !command.is_empty() {
                process.args(command);
            }
            let status = process
                .status()
                .await
                .context("could not run OpenSSH ssh")?;
            return Ok(status.code().unwrap_or(255));
        }
        Command::Cp {
            source,
            destination,
        } => {
            let source_remote = ssh::parse_copy_endpoint(source)?;
            let destination_remote = ssh::parse_copy_endpoint(destination)?;
            anyhow::ensure!(
                source_remote.is_some() ^ destination_remote.is_some(),
                "exactly one cp endpoint must be a box ID and absolute path"
            );
            let (id, _) = source_remote
                .as_ref()
                .or(destination_remote.as_ref())
                .unwrap();
            let details = ssh_details(&path, &profile_name, id, &client).await?;
            let render = |original: &str, remote: &Option<(String, String)>| {
                remote.as_ref().map_or_else(
                    || original.to_owned(),
                    |(id, path)| format!("root@{id}:{path}"),
                )
            };
            let status = tokio::process::Command::new("scp")
                .args(&details.options)
                .arg("--")
                .arg(render(source, &source_remote))
                .arg(render(destination, &destination_remote))
                .status()
                .await
                .context("could not run OpenSSH scp")?;
            return Ok(status.code().unwrap_or(255));
        }
        Command::SshConfig { id } => {
            let details = ssh_details(&path, &profile_name, id, &client).await?;
            println!(
                "Host {}\n  HostName {}\n  User root\n  IdentityFile \"{}\"\n  IdentitiesOnly yes\n  UserKnownHostsFile \"{}\"\n  GlobalKnownHostsFile /dev/null\n  StrictHostKeyChecking yes\n  HostKeyAlias {}\n  ProxyCommand {} '%h'\n  ForwardAgent no\n  BatchMode yes",
                details.alias,
                id,
                details.key.display(),
                details.known_hosts.display(),
                details.alias,
                details.proxy
            );
            return Ok(0);
        }
        _ => {}
    }
    let action = match cli.command {
        Command::Templates => {
            let templates = client.templates().await?;
            if cli.json {
                json(&templates)?;
            } else {
                for t in templates {
                    println!("{}\t{} MiB\t{} vCPU", t.name, t.memory_mib, t.vcpus);
                }
            }
            return Ok(0);
        }
        Command::List => {
            let boxes = client.list().await?;
            if cli.json {
                json(&boxes)?;
            } else {
                for b in boxes {
                    show_box(&b);
                }
            }
            return Ok(0);
        }
        Command::Inspect { id } => {
            let record = client.inspect(&id).await?;
            if cli.json {
                json(&record)?;
            } else {
                show_box(&record);
            }
            return Ok(0);
        }
        Command::Operation {
            id,
            wait: should_wait,
        } => {
            let mut op = client.operation(&id).await?;
            if should_wait {
                op = wait(&client, op, cli.wait_seconds).await?;
            }
            return show_operation(&op, cli.json);
        }
        Command::Create { template, name } => Action::Create { template, name },
        Command::Pause { id } => Action::Pause { id },
        Command::Resume { id } => Action::Resume { id },
        Command::Stop { id, force } => Action::Stop { id, force },
        Command::Start { id } => Action::Start { id },
        Command::Delete { id } => Action::Delete { id },
        Command::Exec {
            id,
            timeout_ms,
            cwd,
            env,
            argv,
        } => {
            let mut environment = BTreeMap::new();
            for value in env {
                let (key, value) = value.split_once('=').context("--env requires KEY=VALUE")?;
                environment.insert(key.into(), value.into());
            }
            Action::Exec {
                id,
                request: ExecRequest {
                    argv,
                    cwd,
                    env: environment,
                    timeout_ms,
                },
            }
        }
        Command::Profile { .. } => unreachable!(),
        Command::Login { .. } | Command::Logout | Command::Auth { .. } => unreachable!(),
        Command::Ssh { .. }
        | Command::Cp { .. }
        | Command::SshConfig { .. }
        | Command::SshProxy { .. } => unreachable!(),
    };
    action.validate().map_err(anyhow::Error::msg)?;
    let request = Submit {
        id: cli.request_id.unwrap_or_else(new_id),
        action,
    };
    anyhow::ensure!(kiln_api::valid_id(&request.id), "invalid request ID");
    eprintln!("request {}", request.id);
    let mut op = client.submit(&request).await.with_context(|| format!("request {} may have been accepted; query `kiln operation {}` before retrying; any retry must use --request-id {} and the same arguments", request.id, request.id, request.id))?;
    if !cli.no_wait {
        op = wait(&client, op, cli.wait_seconds).await?;
    }
    show_operation(&op, cli.json)
}

struct SshDetails {
    alias: String,
    key: PathBuf,
    known_hosts: PathBuf,
    proxy: String,
    options: Vec<String>,
}

async fn ssh_details(path: &Path, profile: &str, id: &str, client: &Client) -> Result<SshDetails> {
    anyhow::ensure!(kiln_api::valid_id(id), "invalid box ID");
    let absolute = fs::canonicalize(path)?;
    let path = absolute.as_path();
    ssh::safe_config_value(profile)?;
    let alias = format!("kiln-{profile}-{id}");
    let (key, _) = ssh::ensure_key(path, profile)?;
    let host_key = client.ssh_host_key(id).await?;
    let known_hosts = ssh::write_known_hosts(path, profile, &alias, &host_key)?;
    let executable = std::env::current_exe()?
        .to_str()
        .context("kiln path is not UTF-8")?
        .to_owned();
    let proxy = ssh::proxy_command(path, profile, &executable)?;
    let options = [
        format!("IdentityFile=\"{}\"", key.display()),
        "IdentitiesOnly=yes".into(),
        format!("UserKnownHostsFile=\"{}\"", known_hosts.display()),
        "GlobalKnownHostsFile=/dev/null".into(),
        "BatchMode=yes".into(),
        "StrictHostKeyChecking=yes".into(),
        format!("HostKeyAlias={alias}"),
        format!("ProxyCommand={proxy} '{id}'"),
        "ForwardAgent=no".into(),
    ]
    .into_iter()
    .flat_map(|v| ["-o".to_owned(), v])
    .collect();
    Ok(SshDetails {
        alias,
        key,
        known_hosts,
        proxy,
        options,
    })
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let as_json = cli.json;
    match run(cli).await {
        Ok(exit) => std::process::exit(exit),
        Err(error) => {
            if as_json {
                eprintln!(
                    "{}",
                    serde_json::json!({"error":{"code":"client_error","message":format!("{error:#}")}})
                );
            } else {
                eprintln!("error: {error:#}");
            }
            std::process::exit(1);
        }
    }
}

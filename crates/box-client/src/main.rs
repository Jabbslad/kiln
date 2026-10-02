mod profile;
use anyhow::{Context, Result, bail};
use box_api::{Action, BoxView, ExecRequest, Operation, OperationState, Outcome, Submit, new_id};
use box_client::Client;
use clap::{Parser, Subcommand};
use std::{collections::BTreeMap, fs, io::Write, path::PathBuf, time::Duration};

#[derive(Parser)]
#[command(version, about = "Manage boxd microVMs over authenticated HTTPS")]
struct Cli {
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[arg(long, global = true, default_value = "default")]
    profile: String,
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
                "operation {} still running; reconnect with `boxctl operation {} --wait`",
                op.id,
                op.id
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        op = client.operation(&op.id).await.with_context(|| format!("poll interrupted; reconnect with `boxctl operation {} --wait`; do not submit a new request", op.id))?;
    }
    Ok(op)
}

async fn run(cli: Cli) -> Result<i32> {
    let path = match cli.config {
        Some(p) => p,
        None => profile::default_path()?,
    };
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
                    profile::Profile {
                        url,
                        token_file: fs::canonicalize(token_file)?,
                        ca_file: ca_file.map(fs::canonicalize).transpose()?,
                    },
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
                        println!("{name}\t{}", entry.url);
                    }
                }
            }
        }
        return Ok(0);
    }
    let profiles = profile::load(&path)?;
    let client = profiles
        .profiles
        .get(&cli.profile)
        .context("profile not found; use `boxctl profile add`")?
        .client()?;
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
    };
    action.validate().map_err(anyhow::Error::msg)?;
    let request = Submit {
        id: cli.request_id.unwrap_or_else(new_id),
        action,
    };
    anyhow::ensure!(box_api::valid_id(&request.id), "invalid request ID");
    eprintln!("request {}", request.id);
    let mut op = client.submit(&request).await.with_context(|| format!("request {} may have been accepted; query `boxctl operation {}` before retrying; any retry must use --request-id {} and the same arguments", request.id, request.id, request.id))?;
    if !cli.no_wait {
        op = wait(&client, op, cli.wait_seconds).await?;
    }
    show_operation(&op, cli.json)
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

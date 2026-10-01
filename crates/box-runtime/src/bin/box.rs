use box_runtime::{Error, Result, runtime::Runtime};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Parser)]
#[command(version, about = "Persistent Linux microVMs — local runtime")]
struct Cli {
    #[arg(long, global = true, default_value = ".boxd")]
    state_dir: PathBuf,
    #[arg(long, global = true)]
    json: bool,
    /// Initialize an empty, root-owned state store with a trusted jailer policy.
    #[arg(long, global = true)]
    isolation_config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Inspect host capabilities without changing the host.
    Doctor,
    /// Launch a trusted Linux image. Not an untrusted-code sandbox.
    Create {
        #[arg(long)]
        image: PathBuf,
        #[arg(long, default_value = "box")]
        name: String,
        #[arg(long, default_value_t = 256)]
        memory_mib: u32,
        #[arg(long, default_value_t = 1)]
        vcpus: u8,
        #[arg(long, default_value = "development")]
        profile: String,
        #[arg(long)]
        allow_unsafe_development: bool,
    },
    List,
    Inspect {
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
    Pause {
        id: String,
    },
    Resume {
        id: String,
    },
    Stop {
        id: String,
        /// Skip guest flush; works even if the guest or VMM API is unresponsive.
        #[arg(long)]
        force: bool,
    },
    Start {
        id: String,
    },
    Delete {
        id: String,
    },
    Checkpoint {
        #[command(subcommand)]
        command: Checkpoint,
    },
    Template {
        #[command(subcommand)]
        command: Template,
    },
    Clone {
        template: String,
        #[arg(long, default_value = "box")]
        name: String,
        #[arg(long, default_value = "development")]
        profile: String,
        #[arg(long)]
        allow_unsafe_development: bool,
    },
    /// Measure real launch through first successful command; removes its boxes.
    Benchmark {
        #[arg(long)]
        image: PathBuf,
        #[arg(long)]
        template: String,
        #[arg(long, default_value_t = 30)]
        samples: usize,
        #[arg(long, default_value_t = 1)]
        concurrency: usize,
        #[arg(long, default_value = "development")]
        profile: String,
        #[arg(long)]
        allow_unsafe_development: bool,
    },
}

#[derive(Subcommand)]
enum Template {
    Build {
        #[arg(long)]
        image: PathBuf,
        #[arg(long, default_value_t = 256)]
        memory_mib: u32,
        #[arg(long, default_value_t = 1)]
        vcpus: u8,
        #[arg(long, default_value = "development")]
        profile: String,
        #[arg(long)]
        allow_unsafe_development: bool,
    },
    List,
}

#[derive(Subcommand)]
enum Checkpoint {
    List,
    Delete {
        checkpoint: String,
    },
    Save {
        id: String,
    },
    Restore {
        id: String,
        checkpoint: String,
        #[arg(long)]
        acknowledge_external_state_replay: bool,
    },
}

async fn run(cli: Cli) -> Result<(Value, i32)> {
    if matches!(cli.command, Command::Doctor) {
        if let Some(path) = &cli.isolation_config {
            box_runtime::isolation::Config::load(path)?.preflight()?;
        }
        let mut report = box_runtime::host::check();
        report.isolated_ready = cli.isolation_config.is_some();
        let exit = if report.development_ready { 0 } else { 1 };
        return Ok((serde_json::to_value(report)?, exit));
    }
    let runtime = Runtime::open_with_isolation(&cli.state_dir, cli.isolation_config.as_deref())?;
    let value = match cli.command {
        Command::Doctor => unreachable!(),
        Command::Create {
            image,
            name,
            memory_mib,
            vcpus,
            profile,
            allow_unsafe_development,
        } => {
            launch_profile(&runtime, &profile, allow_unsafe_development)?;
            serde_json::to_value(
                runtime
                    .create(&image, &name, memory_mib, vcpus, false)
                    .await?,
            )?
        }
        Command::List => json!({"schema_version":1,"boxes":runtime.list().await?}),
        Command::Inspect { id } => serde_json::to_value(runtime.inspect(&id).await?)?,
        Command::Pause { id } => serde_json::to_value(runtime.set_paused(&id, true).await?)?,
        Command::Resume { id } => serde_json::to_value(runtime.set_paused(&id, false).await?)?,
        Command::Stop { id, force } => serde_json::to_value(runtime.stop(&id, force).await?)?,
        Command::Start { id } => serde_json::to_value(runtime.start(&id).await?)?,
        Command::Delete { id } => {
            runtime.delete(&id).await?;
            json!({"schema_version":1,"deleted":id})
        }
        Command::Exec {
            id,
            timeout_ms,
            cwd,
            env,
            argv,
        } => {
            if !(1..=3_600_000).contains(&timeout_ms) {
                return Err(Error::Invalid("timeout must be 1..3600000 ms".into()));
            }
            let mut environment = BTreeMap::new();
            for item in env {
                let (key, value) = item
                    .split_once('=')
                    .ok_or_else(|| Error::Invalid("--env requires KEY=VALUE".into()))?;
                environment.insert(key.into(), value.into());
            }
            let result = runtime
                .exec(
                    &id,
                    box_protocol::ExecRequest {
                        argv,
                        cwd,
                        env: environment,
                        timeout_ms,
                    },
                )
                .await?;
            let exit = if result.timed_out {
                124
            } else {
                result.exit_code.unwrap_or(125)
            };
            return Ok((
                json!({"schema_version":1,"stdout":String::from_utf8_lossy(&result.stdout),"stderr":String::from_utf8_lossy(&result.stderr),"exit_code":result.exit_code,"truncated":result.truncated,"timed_out":result.timed_out}),
                exit,
            ));
        }
        Command::Checkpoint {
            command: Checkpoint::Save { id },
        } => serde_json::to_value(runtime.checkpoint(&id).await?)?,
        Command::Checkpoint {
            command: Checkpoint::List,
        } => json!({"schema_version":1,"snapshots":runtime.snapshots()?}),
        Command::Checkpoint {
            command: Checkpoint::Delete { checkpoint },
        } => {
            runtime.delete_snapshot(&checkpoint)?;
            json!({"schema_version":1,"deleted":checkpoint})
        }
        Command::Checkpoint {
            command:
                Checkpoint::Restore {
                    id,
                    checkpoint,
                    acknowledge_external_state_replay,
                },
        } => {
            if !acknowledge_external_state_replay {
                return Err(Error::Invalid("restore rewinds memory and disk, not external systems; pass --acknowledge-external-state-replay".into()));
            }
            serde_json::to_value(runtime.restore(&id, &checkpoint).await?)?
        }
        Command::Template {
            command:
                Template::Build {
                    image,
                    memory_mib,
                    vcpus,
                    profile,
                    allow_unsafe_development,
                },
        } => {
            launch_profile(&runtime, &profile, allow_unsafe_development)?;
            serde_json::to_value(runtime.build_template(&image, memory_mib, vcpus).await?)?
        }
        Command::Template {
            command: Template::List,
        } => {
            json!({"schema_version":1,"templates":runtime.snapshots()?.into_iter().filter(|s|s.template).collect::<Vec<_>>()})
        }
        Command::Clone {
            template,
            name,
            profile,
            allow_unsafe_development,
        } => {
            launch_profile(&runtime, &profile, allow_unsafe_development)?;
            serde_json::to_value(runtime.clone_template(&template, &name).await?)?
        }
        Command::Benchmark {
            image,
            template,
            samples,
            concurrency,
            profile,
            allow_unsafe_development,
        } => {
            launch_profile(&runtime, &profile, allow_unsafe_development)?;
            let report = runtime
                .benchmark(&image, &template, samples, concurrency)
                .await?;
            let failed = report["results"]
                .as_object()
                .unwrap()
                .values()
                .any(|v| v["summary"]["failures"].as_u64().unwrap_or(1) != 0);
            return Ok((report, i32::from(failed)));
        }
    };
    Ok((value, 0))
}

fn launch_profile(runtime: &Runtime, profile: &str, allowed: bool) -> Result<()> {
    if profile == "isolated" && runtime.is_isolated() {
        return Ok(());
    }
    if profile != "development" || runtime.is_isolated() {
        return Err(Error::Invalid(
            "profile does not match state store; isolated launches require --isolation-config on an empty store; refusing unjailed fallback".into(),
        ));
    }
    if !allowed {
        return Err(Error::Invalid(
            "trusted fixtures only: pass --allow-unsafe-development".into(),
        ));
    }
    eprintln!(
        "WARNING: development profile; unjailed VM, no guest network. Trusted workloads only."
    );
    Ok(())
}

#[tokio::main]
async fn main() {
    match run(Cli::parse()).await {
        Ok((value, exit)) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&value).expect("serializable response")
            );
            std::process::exit(exit);
        }
        Err(error) => {
            eprintln!(
                "{}",
                json!({"schema_version":1,"error":{"code":"operation_failed","message":error.to_string()}})
            );
            std::process::exit(1);
        }
    }
}

use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

pub fn cli(args: &[String]) -> i32 {
    match run(args) {
        Ok(report) => {
            println!("{report}");
            0
        }
        Err(error) => {
            eprintln!("{error}");
            2
        }
    }
}

fn run(args: &[String]) -> Result<String, String> {
    let mut binary = "kiln".to_string();
    let mut config: Option<PathBuf> = None;
    let mut profile = "default".to_string();
    let mut box_id: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("missing value for {arg}"))?;
        match arg.as_str() {
            "--binary" => binary = value.clone(),
            "--config" => config = Some(PathBuf::from(value)),
            "--profile" => profile = value.clone(),
            "--box" => box_id = Some(value.clone()),
            other => return Err(format!("unknown argument {other}")),
        }
        index += 2;
    }
    let config = config.ok_or("acceptance requires --config")?;
    let box_id = box_id.ok_or("acceptance requires --box")?;
    if box_id.len() != 32
        || !box_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("box must be a 32-character hexadecimal ID".into());
    }
    let document: Value = serde_json::from_str(&fs_read(&config)?)
        .map_err(|error| format!("config is not JSON: {error}"))?;
    let profile_value = document
        .pointer(&format!("/profiles/{profile}"))
        .ok_or("acceptance requires a central profile, not administrator credentials")?;
    let profile_object = profile_value
        .as_object()
        .ok_or("acceptance requires a central profile, not administrator credentials")?;
    if !profile_object.contains_key("issuer")
        || !profile_object.contains_key("credential_file")
        || profile_object.contains_key("token_file")
    {
        return Err("acceptance requires a central profile, not administrator credentials".into());
    }
    let base = [
        binary.as_str(),
        "--config",
        config.to_str().ok_or("config path is not utf-8")?,
        "--profile",
        profile.as_str(),
    ];
    let status = command_json(&base, &["auth", "status"], 0, None, None)?;
    if status
        .get("device_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .is_empty()
        || status
            .get("account_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .is_empty()
    {
        return Err("device identity was not returned".into());
    }
    let templates = command_json(&base, &["--json", "templates"], 0, None, None)?;
    if templates.as_array().map(Vec::is_empty).unwrap_or(true) {
        return Err("no templates discovered".into());
    }
    let inventory = command_json(&base, &["--json", "list"], 0, None, None)?;
    let inspected = command_json(&base, &["--json", "inspect", &box_id], 0, None, None)?;
    if inspected.get("id").and_then(Value::as_str) != Some(box_id.as_str())
        || inspected.get("state").and_then(Value::as_str) != Some("running")
    {
        return Err("acceptance box is not running".into());
    }
    let listed = inventory
        .as_array()
        .map(|rows| {
            rows.iter()
                .any(|row| row.get("id").and_then(Value::as_str) == Some(box_id.as_str()))
        })
        .unwrap_or(false);
    if !listed {
        return Err("acceptance box missing from inventory".into());
    }
    command_json(
        &base,
        &[
            "exec",
            &box_id,
            "--",
            "python3",
            "-c",
            "import os; os.write(1,bytes([0,255,128,65])); os.write(2,b'guest-error\\n'); raise SystemExit(23)",
        ],
        23,
        Some(&[0, 255, 128, 65]),
        Some(b"guest-error\n"),
    )?;
    command_json(
        &base,
        &[
            "ssh",
            &box_id,
            "--",
            "sh",
            "-c",
            "'printf auth-ssh-ok; exit 37'",
        ],
        37,
        Some(b"auth-ssh-ok"),
        None,
    )?;
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "result": "pass",
        "checks": ["central-device-status", "templates", "list", "inspect", "binary-exec-exit23", "ssh-exit37"],
        "limitations": [
            "assumes completed real browser consent and enrollment",
            "SFTP, interactive PTY, concurrent refresh, revocation and outages require separate acceptance",
        ],
    }))
    .unwrap())
}

fn command_json(
    base: &[&str],
    arguments: &[&str],
    expected: i32,
    stdout: Option<&[u8]>,
    stderr: Option<&[u8]>,
) -> Result<Value, String> {
    let mut command = Command::new(base[0]);
    command.args(&base[1..]).args(arguments);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("{} failed to start: {error}", arguments[0]))?;
    let finished = wait_timeout(&mut child, Duration::from_secs(120))?;
    let code = finished.status.code().unwrap_or(-1);
    if code != expected {
        return Err(format!(
            "{} returned {code}, expected {expected}",
            arguments[0]
        ));
    }
    if let Some(expected_stdout) = stdout {
        if finished.stdout != expected_stdout {
            return Err(format!("{} stdout differed", arguments[0]));
        }
    }
    if let Some(expected_stderr) = stderr {
        let guest: Vec<u8> = finished
            .stderr
            .split_inclusive(|byte| *byte == b'\n')
            .filter(|line| !line.starts_with(b"request "))
            .flatten()
            .copied()
            .collect();
        if guest != expected_stderr {
            return Err(format!("{} stderr differed", arguments[0]));
        }
    }
    if stdout.is_some() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&finished.stdout)
        .map_err(|error| format!("{} stdout is not JSON: {error}", arguments[0]))
}

struct Output {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn wait_timeout(child: &mut std::process::Child, limit: Duration) -> Result<Output, String> {
    let start = std::time::Instant::now();
    loop {
        match child.try_wait().map_err(|error| error.to_string())? {
            Some(status) => {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                if let Some(mut pipe) = child.stdout.take() {
                    use std::io::Read;
                    pipe.read_to_end(&mut stdout)
                        .map_err(|error| error.to_string())?;
                }
                if let Some(mut pipe) = child.stderr.take() {
                    use std::io::Read;
                    pipe.read_to_end(&mut stderr)
                        .map_err(|error| error.to_string())?;
                }
                return Ok(Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            None if start.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("command timed out".into());
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

fn fs_read(path: &std::path::Path) -> Result<String, String> {
    std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))
}

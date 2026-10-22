use crate::register_cases;
use crate::release_version::{
    ReleaseVersion, attach_bootstrap, check_archives, committed_version, parse_release_tag,
};
use flate2::Compression;
use flate2::write::GzEncoder;
use regex::Regex;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const WORKFLOW_PATH: &str = ".github/workflows/build.yml";
pub const ATTEST_ACTION: &str = "actions/attest-build-provenance@";
pub const PACKAGE_ARTIFACTS: &[&str] = &[
    "package-aarch64-apple-darwin",
    "package-dashboard",
    "package-server",
    "package-x86_64-apple-darwin",
    "package-x86_64-unknown-linux-gnu",
];

const GH_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug)]
pub struct ReuseRefused(pub String);

impl fmt::Display for ReuseRefused {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ReuseRefused {}

#[derive(Debug)]
pub enum ReleaseError {
    Refused(ReuseRefused),
    Invalid(String),
}

impl fmt::Display for ReleaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(err) => formatter.write_str(&err.0),
            Self::Invalid(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ReleaseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Refused(err) => Some(err),
            Self::Invalid(_) => None,
        }
    }
}

impl From<ReuseRefused> for ReleaseError {
    fn from(err: ReuseRefused) -> Self {
        Self::Refused(err)
    }
}

impl From<String> for ReleaseError {
    fn from(message: String) -> Self {
        Self::Invalid(message)
    }
}

impl From<io::Error> for ReleaseError {
    fn from(err: io::Error) -> Self {
        Self::Invalid(err.to_string())
    }
}

#[derive(Clone, Debug)]
pub struct WorkflowRun {
    pub run_id: i64,
    pub head_sha: String,
    pub head_branch: String,
    pub event: String,
    pub path: String,
    pub conclusion: String,
    pub status: String,
}

#[derive(Clone, Debug)]
pub struct PackageArtifact {
    pub name: String,
    pub expired: bool,
    pub expires_at: DateTime,
}

#[derive(Clone, Copy, Debug)]
pub struct DateTime {
    micros: i128,
    aware: bool,
}

impl DateTime {
    fn utc(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> Self {
        Self {
            micros: civil_micros(year, month, day, hour, minute, second, 0),
            aware: true,
        }
    }

    fn plus_seconds(self, seconds: i64) -> Self {
        Self {
            micros: self.micros + i128::from(seconds) * 1_000_000,
            aware: self.aware,
        }
    }
}

pub fn select_run(runs: &[WorkflowRun], sha: &str) -> Result<WorkflowRun, ReuseRefused> {
    if !sha_ok(sha) {
        return Err(ReuseRefused("commit sha is not 40 hex characters".into()));
    }
    let mut chosen: Option<&WorkflowRun> = None;
    for run in runs {
        let matches = run.head_sha == sha
            && run.head_branch == "main"
            && run.event == "push"
            && run.path == WORKFLOW_PATH
            && run.conclusion == "success"
            && run.status == "completed";
        let newer = chosen.as_ref().is_none_or(|best| run.run_id > best.run_id);
        if matches && newer {
            chosen = Some(run);
        }
    }
    chosen.cloned().ok_or_else(|| {
        ReuseRefused("no successful main push of this workflow for that commit".into())
    })
}

pub fn require_artifacts(
    artifacts: &[PackageArtifact],
    now: &DateTime,
) -> Result<(), ReuseRefused> {
    if !now.aware {
        return Err(ReuseRefused("clock must be timezone-aware".into()));
    }
    let mut seen = BTreeSet::new();
    for artifact in artifacts {
        if !seen.insert(artifact.name.as_str()) {
            return Err(ReuseRefused("duplicate artifact name".into()));
        }
    }
    for name in PACKAGE_ARTIFACTS {
        if !seen.contains(name) {
            return Err(ReuseRefused(
                "artifact set does not match the package jobs".into(),
            ));
        }
    }
    for artifact in artifacts {
        if !artifact.expires_at.aware {
            panic!("can't compare offset-naive and offset-aware datetimes");
        }
        if artifact.expired || artifact.expires_at.micros <= now.micros {
            let name = &artifact.name;
            return Err(ReuseRefused(format!("{name} is expired")));
        }
    }
    Ok(())
}

pub fn workflow_attests(workflow_text: &str) -> bool {
    workflow_text
        .lines()
        .any(|line| line.trim_start().starts_with("uses:") && line.contains(ATTEST_ACTION))
}

pub fn attestation_gate(
    workflow_text: &str,
    results: &[&str],
) -> Result<&'static str, ReuseRefused> {
    let usable = |result: &&str| matches!(*result, "verified" | "absent" | "rejected");
    if results.is_empty() || results.iter().any(|result| !usable(result)) {
        return Err(ReuseRefused("attestation result is not usable".into()));
    }
    if results.contains(&"rejected") {
        return Err(ReuseRefused("attestation verification failed".into()));
    }
    if results.iter().all(|result| *result == "verified") {
        return Ok("verified");
    }
    if results.iter().all(|result| *result == "absent") {
        if workflow_attests(workflow_text) {
            return Err(ReuseRefused(
                "this commit's workflow attests builds, but the archives have no attestation"
                    .into(),
            ));
        }
        return Ok("not-produced");
    }
    Err(ReuseRefused("archives disagree on attestation".into()))
}

pub fn classify_verify(returncode: i32, stderr: &str) -> &'static str {
    if returncode == 0 {
        return "verified";
    }
    if stderr.contains("attestations/sha256:") && stderr.contains("404") {
        return "absent";
    }
    "rejected"
}

pub fn expected_archives(version: &ReleaseVersion) -> BTreeSet<String> {
    let tag = version.tag();
    let mut names = BTreeSet::new();
    names.insert(format!("kiln-dashboard-{tag}.tar.gz"));
    names.insert(format!(
        "kiln-identity-{tag}-x86_64-unknown-linux-gnu.tar.gz"
    ));
    names.insert(format!("kiln-server-{tag}-x86_64-unknown-linux-gnu.tar.gz"));
    for target in [
        "x86_64-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "aarch64-apple-darwin",
    ] {
        names.insert(format!("kiln-{tag}-{target}.tar.gz"));
    }
    names
}

pub fn expected_release_files(version: &ReleaseVersion) -> BTreeSet<String> {
    let archives = expected_archives(version);
    let mut names = archives.clone();
    for name in &archives {
        names.insert(format!("{name}.sha256"));
    }
    names.insert("install.sh".to_string());
    names.insert("install.sh.sha256".to_string());
    names
}

pub fn verify_and_refresh_sidecars(
    dist: &Path,
    version: &ReleaseVersion,
) -> Result<(), ReleaseError> {
    let archives = expected_archives(version);
    let paths = regular_files(dist)?;
    let names: BTreeSet<String> = paths.iter().map(|path| entry_name(path)).collect();
    let mut expected = archives.clone();
    for name in &archives {
        expected.insert(format!("{name}.sha256"));
    }
    if names != expected {
        return Err(refused("downloaded files are not the expected archives"));
    }
    let mut by_name: HashMap<String, &Path> = HashMap::new();
    for path in &paths {
        by_name.insert(entry_name(path), path.as_path());
    }
    for name in &archives {
        let archive = by_name[name.as_str()];
        let sidecar_name = format!("{name}.sha256");
        let sidecar = by_name[sidecar_name.as_str()];
        let digest = sha256_file(archive)?;
        let expected_bytes = format!("{digest}  {name}\n").into_bytes();
        let actual = fs::read(sidecar)?;
        if actual != expected_bytes && actual != crlf_bytes(&expected_bytes) {
            return Err(refused(format!("checksum does not match {name}")));
        }
        fs::write(sidecar, expected_bytes)?;
    }
    Ok(())
}

pub fn finish_dist(repo: &Path, dist: &Path, version: &ReleaseVersion) -> Result<(), ReleaseError> {
    check_archives(version, dist)?;
    attach_bootstrap(repo, dist, version)?;
    let paths = regular_files(dist)?;
    let names: BTreeSet<String> = paths.iter().map(|path| entry_name(path)).collect();
    if names != expected_release_files(version) {
        return Err(refused("dist is not the 14-file release set"));
    }
    Ok(())
}

pub fn parse_runs(items: &Value) -> Result<Vec<WorkflowRun>, ReuseRefused> {
    let Some(items) = items.as_array() else {
        return Err(ReuseRefused("run list is not a list".into()));
    };
    let mut runs = Vec::new();
    for item in items {
        let Some(obj) = item.as_object() else {
            return Err(ReuseRefused("run list entry is not an object".into()));
        };
        runs.push(WorkflowRun {
            run_id: json_i64_field(obj, "databaseId")?,
            head_sha: json_sha_field(obj, "headSha")?,
            head_branch: json_str_field(obj, "headBranch")?,
            event: json_str_field(obj, "event")?,
            path: json_str_field(obj, "path")?,
            conclusion: json_str_field(obj, "conclusion")?,
            status: json_str_field(obj, "status")?,
        });
    }
    Ok(runs)
}

pub fn parse_artifacts(
    payload: &Value,
    now: &DateTime,
) -> Result<Vec<PackageArtifact>, ReuseRefused> {
    let Some(obj) = payload.as_object() else {
        return Err(ReuseRefused("artifact list is not an object".into()));
    };
    let artifacts = obj.get("artifacts").and_then(Value::as_array);
    let total = obj.get("total_count").and_then(json_i64);
    let Some(artifacts) = artifacts else {
        return Err(ReuseRefused(
            "artifact list was paginated or malformed".into(),
        ));
    };
    let Some(total) = total else {
        return Err(ReuseRefused(
            "artifact list was paginated or malformed".into(),
        ));
    };
    if total != i64::try_from(artifacts.len()).unwrap_or(i64::MAX) {
        return Err(ReuseRefused(
            "artifact list was paginated or malformed".into(),
        ));
    }
    let mut parsed = Vec::new();
    for item in artifacts {
        let Some(entry) = item.as_object() else {
            return Err(ReuseRefused("artifact entry is not an object".into()));
        };
        let expires = json_str_field(entry, "expires_at")?;
        let expires_at = parse_expiry(&expires)?;
        let expired = match entry.get("expired") {
            Some(Value::Bool(value)) => *value,
            _ => {
                return Err(ReuseRefused(
                    "artifact expired flag is not a boolean".into(),
                ));
            }
        };
        parsed.push(PackageArtifact {
            name: json_str_field(entry, "name")?,
            expired,
            expires_at,
        });
    }
    if !now.aware {
        return Err(ReuseRefused("clock must be timezone-aware".into()));
    }
    Ok(parsed)
}

pub fn fetch(
    repo: &str,
    sha: &str,
    version_text: &str,
    dist: &Path,
    env: &HashMap<String, String>,
) -> Result<(String, ReleaseVersion), ReleaseError> {
    require_repo(repo)?;
    prepare_dist(dist)?;
    let listed = run_list(repo, sha, env)?;
    let enriched = with_paths(repo, listed, env)?;
    let runs = parse_runs(&Value::Array(enriched))?;
    let chosen = select_run(&runs, sha)?;
    let now = now_utc();
    let artifacts = parse_artifacts(&artifact_page(repo, chosen.run_id, env)?, &now)?;
    require_artifacts(&artifacts, &now)?;
    let mut packages: Vec<&str> = PACKAGE_ARTIFACTS.to_vec();
    packages.sort_unstable();
    for name in packages {
        gh(
            &[
                "run".to_string(),
                "download".to_string(),
                chosen.run_id.to_string(),
                "--repo".to_string(),
                repo.to_string(),
                "--name".to_string(),
                name.to_string(),
                "--dir".to_string(),
                path_arg(dist),
            ],
            env,
            &format!("download of {name} failed"),
        )?;
    }
    let version = parse_release_tag(version_text)?;
    verify_and_refresh_sidecars(dist, &version)?;
    let workflow_text = workflow_text(repo, sha, env)?;
    let mut results = Vec::new();
    for path in archive_paths(dist)? {
        let (code, stderr) = verify_attestation(repo, sha, &path, env)?;
        results.push(classify_verify(code, &stderr));
    }
    let produced = attestation_gate(&workflow_text, &results)?;
    let run_id = chosen.run_id;
    Ok((format!("run {run_id} attestation={produced}"), version))
}

pub fn run(args: &[String], env: &HashMap<String, String>) -> (i32, String, String) {
    let request = match parse_request(args) {
        Ok(request) => request,
        Err(message) => return (2, String::new(), format!("{message}\n")),
    };
    match request {
        Request::Fetch { dist } => match fetch_command(&dist, env) {
            Ok(stdout) => (0, stdout, String::new()),
            Err(ReleaseError::Refused(err)) => match fallback_stdout(env, &err) {
                Ok(stdout) => (0, stdout, format!("reuse refused: {err}\n")),
                Err(error) => (1, String::new(), format!("{error}\n")),
            },
            Err(error) => (1, String::new(), format!("{error}\n")),
        },
        Request::Finish {
            version,
            dist,
            repo,
        } => {
            let repo = repo.unwrap_or_else(crate::repo_root);
            match finish_with(&version, &dist, &repo) {
                Ok(()) => (0, String::new(), String::new()),
                Err(error) => (1, String::new(), format!("{error}\n")),
            }
        }
    }
}

pub fn cli(args: &[String]) -> i32 {
    let env = std::env::vars().collect();
    let (code, stdout, stderr) = run(args, &env);
    print!("{stdout}");
    eprint!("{stderr}");
    code
}

enum Request {
    Fetch {
        dist: PathBuf,
    },
    Finish {
        version: String,
        dist: PathBuf,
        repo: Option<PathBuf>,
    },
}

fn finish_with(version: &str, dist: &Path, repo: &Path) -> Result<(), ReleaseError> {
    let version = parse_release_tag(version)?;
    finish_dist(repo, dist, &version)
}

fn fetch_command(dist: &Path, env: &HashMap<String, String>) -> Result<String, ReleaseError> {
    let event = env_var(env, "REUSE_EVENT");
    let repo = env_var(env, "GH_REPO");
    let (sha, mut version_text) = match event.as_str() {
        "workflow_dispatch" => (env_var(env, "REUSE_SHA_INPUT"), String::new()),
        "push" => (env_var(env, "REUSE_SHA"), env_var(env, "REUSE_VERSION")),
        _ => return Err(refused("reuse runs only for a tag push or a dry run")),
    };
    if version_text.is_empty() {
        version_text = committed_tag(&repo, &sha, env)?;
    }
    let (reason, version) = fetch(&repo, &sha, &version_text, dist, env)?;
    accept_stdout(env, &reason, &version)
}

fn accept_stdout(
    env: &HashMap<String, String>,
    reason: &str,
    version: &ReleaseVersion,
) -> Result<String, ReleaseError> {
    let mut stdout = String::new();
    let tag = version.tag();
    emit(env, &mut stdout, "mode", "reuse")?;
    emit(env, &mut stdout, "version", &tag)?;
    emit(env, &mut stdout, "reason", &one_line(reason))?;
    stdout.push_str("reuse accepted: ");
    stdout.push_str(reason);
    stdout.push('\n');
    Ok(stdout)
}

fn fallback_stdout(
    env: &HashMap<String, String>,
    err: &ReuseRefused,
) -> Result<String, ReleaseError> {
    let mut stdout = String::new();
    emit(env, &mut stdout, "mode", "fallback")?;
    emit(env, &mut stdout, "version", "")?;
    emit(env, &mut stdout, "reason", &one_line(&err.0))?;
    Ok(stdout)
}

fn parse_request(args: &[String]) -> Result<Request, String> {
    let Some((command, rest)) = args.split_first() else {
        return Err(usage());
    };
    match command.as_str() {
        "fetch" => parse_fetch(rest),
        "finish" => parse_finish(rest),
        other => Err(format!("unrecognized command {other}")),
    }
}

fn usage() -> String {
    "usage: reuse-release fetch --dist DIR | reuse-release finish --version VER --dist DIR [--repo DIR]".to_string()
}

fn parse_fetch(args: &[String]) -> Result<Request, String> {
    let mut dist = None;
    let mut index = 0;
    while index < args.len() {
        let (flag, value, step) = next_flag(args, index)?;
        match flag.as_str() {
            "--dist" => dist = Some(PathBuf::from(value)),
            other => return Err(format!("unrecognized arguments: {other}")),
        }
        index += step;
    }
    let dist = dist.ok_or_else(|| "--dist is required".to_string())?;
    Ok(Request::Fetch { dist })
}

fn parse_finish(args: &[String]) -> Result<Request, String> {
    let mut version = None;
    let mut dist = None;
    let mut repo = None;
    let mut index = 0;
    while index < args.len() {
        let (flag, value, step) = next_flag(args, index)?;
        match flag.as_str() {
            "--version" => version = Some(value),
            "--dist" => dist = Some(PathBuf::from(value)),
            "--repo" => repo = Some(PathBuf::from(value)),
            other => return Err(format!("unrecognized arguments: {other}")),
        }
        index += step;
    }
    let version = version.ok_or_else(|| "--version is required".to_string())?;
    let dist = dist.ok_or_else(|| "--dist is required".to_string())?;
    Ok(Request::Finish {
        version,
        dist,
        repo,
    })
}

fn next_flag(args: &[String], index: usize) -> Result<(String, String, usize), String> {
    let arg = args
        .get(index)
        .ok_or_else(|| "missing argument".to_string())?;
    if let Some((key, value)) = arg.split_once('=')
        && key.starts_with("--")
    {
        if value.is_empty() {
            return Err(format!("missing value for {key}"));
        }
        return Ok((key.to_string(), value.to_string(), 1));
    }
    if !arg.starts_with("--") {
        return Err(format!("unrecognized arguments: {arg}"));
    }
    let Some(value) = args.get(index + 1) else {
        return Err(format!("missing value for {arg}"));
    };
    if value.starts_with("--") {
        return Err(format!("missing value for {arg}"));
    }
    Ok((arg.clone(), value.clone(), 2))
}

fn require_repo(repo: &str) -> Result<(), ReuseRefused> {
    if repo_ok(repo) {
        Ok(())
    } else {
        Err(ReuseRefused("repository is not owner/name".into()))
    }
}

fn committed_tag(
    repo: &str,
    sha: &str,
    env: &HashMap<String, String>,
) -> Result<String, ReleaseError> {
    require_repo(repo)?;
    if !sha_ok(sha) {
        return Err(refused("commit sha is not 40 hex characters"));
    }
    let text = gh_text(
        &[
            "api".to_string(),
            format!("repos/{repo}/contents/Cargo.toml?ref={sha}"),
            "-H".to_string(),
            "Accept: application/vnd.github.raw".to_string(),
        ],
        env,
        "could not read Cargo.toml at that commit",
    )?;
    Ok(committed_version(&text)?.tag())
}

fn with_paths(
    repo: &str,
    items: Vec<Value>,
    env: &HashMap<String, String>,
) -> Result<Vec<Value>, ReleaseError> {
    let mut enriched = Vec::new();
    for item in items {
        let Value::Object(mut obj) = item else {
            return Err(refused("run list entry is not an object"));
        };
        let run_id = json_i64_field(&obj, "databaseId")?;
        let path = gh_text(
            &[
                "api".to_string(),
                format!("repos/{repo}/actions/runs/{run_id}"),
                "--jq".to_string(),
                ".path".to_string(),
            ],
            env,
            "could not read the workflow path",
        )?;
        obj.insert("path".to_string(), Value::String(path.trim().to_string()));
        enriched.push(Value::Object(obj));
    }
    Ok(enriched)
}

fn run_list(
    repo: &str,
    sha: &str,
    env: &HashMap<String, String>,
) -> Result<Vec<Value>, ReleaseError> {
    if !sha_ok(sha) {
        return Err(refused("commit sha is not 40 hex characters"));
    }
    let text = gh_text(
        &[
            "run".to_string(),
            "list".to_string(),
            "--repo".to_string(),
            repo.to_string(),
            "--workflow".to_string(),
            "build.yml".to_string(),
            "--commit".to_string(),
            sha.to_string(),
            "--branch".to_string(),
            "main".to_string(),
            "--limit".to_string(),
            "20".to_string(),
            "--json".to_string(),
            "databaseId,headSha,headBranch,event,conclusion,status".to_string(),
        ],
        env,
        "could not list workflow runs",
    )?;
    let payload = parse_json(&text, "run list was not json")?;
    match payload {
        Value::Array(items) => Ok(items),
        _ => Err(refused("run list is not a list")),
    }
}

fn artifact_page(
    repo: &str,
    run_id: i64,
    env: &HashMap<String, String>,
) -> Result<Value, ReleaseError> {
    let text = gh_text(
        &[
            "api".to_string(),
            format!("repos/{repo}/actions/runs/{run_id}/artifacts"),
        ],
        env,
        "could not list artifacts",
    )?;
    parse_json(&text, "artifact list was not json")
}

fn workflow_text(
    repo: &str,
    sha: &str,
    env: &HashMap<String, String>,
) -> Result<String, ReleaseError> {
    gh_text(
        &[
            "api".to_string(),
            format!("repos/{repo}/contents/{WORKFLOW_PATH}?ref={sha}"),
            "-H".to_string(),
            "Accept: application/vnd.github.raw".to_string(),
        ],
        env,
        "could not read the workflow at that commit",
    )
}

fn verify_attestation(
    repo: &str,
    sha: &str,
    archive: &Path,
    env: &HashMap<String, String>,
) -> Result<(i32, String), ReleaseError> {
    let completed = run_process(
        &[
            "gh".to_string(),
            "attestation".to_string(),
            "verify".to_string(),
            path_arg(archive),
            "--repo".to_string(),
            repo.to_string(),
            "--signer-workflow".to_string(),
            format!("{repo}/{WORKFLOW_PATH}"),
            "--source-ref".to_string(),
            "refs/heads/main".to_string(),
            "--source-digest".to_string(),
            sha.to_string(),
            "--deny-self-hosted-runners".to_string(),
        ],
        env,
    )?;
    Ok((completed.code, completed.stderr))
}

fn gh(args: &[String], env: &HashMap<String, String>, refuse: &str) -> Result<(), ReleaseError> {
    gh_text(args, env, refuse)?;
    Ok(())
}

fn gh_text(
    args: &[String],
    env: &HashMap<String, String>,
    refuse: &str,
) -> Result<String, ReleaseError> {
    let mut full = Vec::with_capacity(args.len() + 1);
    full.push("gh".to_string());
    full.extend(args.iter().cloned());
    let completed = run_process(&full, env)?;
    if completed.code != 0 {
        let detail = public_error(&completed.stderr);
        return Err(refused(format!("{refuse}: {detail}")));
    }
    Ok(completed.stdout)
}

struct Completed {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run_process(args: &[String], env: &HashMap<String, String>) -> Result<Completed, ReuseRefused> {
    let Some((program, rest)) = args.split_first() else {
        return Err(ReuseRefused("gh could not be run".into()));
    };
    let mut command = Command::new(program);
    command.args(rest);
    command.env_clear();
    command.envs(env);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|_| ReuseRefused("gh could not be run".into()))?;
    let mut stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| ReuseRefused("gh could not be run".into()))?;
    let mut stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| ReuseRefused("gh could not be run".into()))?;
    let stdout_thread = thread::spawn(move || read_pipe(&mut stdout_pipe));
    let stderr_thread = thread::spawn(move || read_pipe(&mut stderr_pipe));
    let status = match wait_child(&mut child, GH_TIMEOUT) {
        Ok(status) => status,
        Err(err) => {
            let _ = stdout_thread.join();
            let _ = stderr_thread.join();
            return Err(err);
        }
    };
    let stdout = stdout_thread.join().unwrap_or_default();
    let stderr = stderr_thread.join().unwrap_or_default();
    Ok(Completed {
        code: status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

fn read_pipe(pipe: &mut impl Read) -> Vec<u8> {
    let mut buf = Vec::new();
    let _ = pipe.read_to_end(&mut buf);
    buf
}

fn wait_child(child: &mut Child, timeout: Duration) -> Result<ExitStatus, ReuseRefused> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ReuseRefused("gh could not be run".into()));
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(_) => return Err(ReuseRefused("gh could not be run".into())),
        }
    }
}

fn prepare_dist(dist: &Path) -> Result<(), ReleaseError> {
    if dist.exists() {
        if !dist.is_dir() {
            let display = dist.display();
            return Err(ReleaseError::Invalid(format!(
                "{display} exists and is not a directory"
            )));
        }
        let mut entries = fs::read_dir(dist)?;
        if entries.next().transpose()?.is_some() {
            return Err(refused("dist is not empty"));
        }
    }
    fs::create_dir_all(dist)?;
    Ok(())
}

fn archive_paths(dist: &Path) -> Result<Vec<PathBuf>, ReleaseError> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(dist)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name.ends_with(".sha256") {
            continue;
        }
        let path = entry.path();
        if path.is_file() {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

fn regular_files(dist: &Path) -> Result<Vec<PathBuf>, ReleaseError> {
    if !dist.is_dir() {
        return Err(refused("dist is not a directory"));
    }
    let mut paths = Vec::new();
    for entry in fs::read_dir(dist)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry_name(&path);
        let metadata = fs::symlink_metadata(&path)?;
        let kind = metadata.file_type();
        if kind.is_symlink() || !kind.is_file() {
            return Err(refused(format!("unexpected dist entry {name}")));
        }
        paths.push(path);
    }
    Ok(paths)
}

fn sha256_file(path: &Path) -> Result<String, ReleaseError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher)?;
    let digest = hasher.finalize();
    Ok(format!("{digest:x}"))
}

fn entry_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn emit(
    env: &HashMap<String, String>,
    stdout: &mut String,
    name: &str,
    value: &str,
) -> Result<(), ReleaseError> {
    let line = format!("{name}={value}");
    match env
        .get("GITHUB_OUTPUT")
        .map(String::as_str)
        .filter(|path| !path.is_empty())
    {
        Some(path) => {
            let mut file = OpenOptions::new().create(true).append(true).open(path)?;
            writeln!(file, "{line}")?;
        }
        None => {
            stdout.push_str(&line);
            stdout.push('\n');
        }
    }
    Ok(())
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn public_error(stderr: &str) -> String {
    let trimmed = stderr.trim();
    if trimmed.is_empty() {
        return "gh failed".to_string();
    }
    let line = trimmed.lines().next().unwrap_or("gh failed");
    if line.contains("ghp_")
        || line.contains("ghs_")
        || line.contains("github_pat")
        || line.contains("Bearer ")
    {
        return "gh failed".to_string();
    }
    line.chars().take(200).collect()
}

fn env_var(env: &HashMap<String, String>, key: &str) -> String {
    env.get(key).cloned().unwrap_or_default()
}

fn refused(message: impl Into<String>) -> ReleaseError {
    ReleaseError::Refused(ReuseRefused(message.into()))
}

fn parse_json(text: &str, refuse: &str) -> Result<Value, ReleaseError> {
    serde_json::from_str(text).map_err(|_| refused(refuse))
}

fn sha_ok(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn repo_ok(value: &str) -> bool {
    let Some((owner, name)) = value.split_once('/') else {
        return false;
    };
    !name.contains('/') && ident_ok(owner) && ident_ok(name)
}

fn ident_ok(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

fn json_i64(value: &Value) -> Option<i64> {
    let number = value.as_number()?;
    if number.is_f64() {
        return None;
    }
    number.as_i64()
}

fn json_i64_field(item: &serde_json::Map<String, Value>, key: &str) -> Result<i64, ReuseRefused> {
    item.get(key)
        .and_then(json_i64)
        .ok_or_else(|| ReuseRefused(format!("{key} is not an integer")))
}

fn json_str_field(
    item: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<String, ReuseRefused> {
    match item.get(key) {
        Some(Value::String(value)) if !value.is_empty() => Ok(value.clone()),
        _ => Err(ReuseRefused(format!("{key} is not a string"))),
    }
}

fn json_sha_field(
    item: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<String, ReuseRefused> {
    let value = json_str_field(item, key)?;
    if sha_ok(&value) {
        Ok(value)
    } else {
        Err(ReuseRefused(format!("{key} is not a commit sha")))
    }
}

fn crlf_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 1);
    for byte in bytes {
        if *byte == b'\n' {
            out.extend_from_slice(b"\r\n");
        } else {
            out.push(*byte);
        }
    }
    out
}

fn now_utc() -> DateTime {
    let micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i128::try_from(duration.as_micros()).unwrap_or(i128::MAX))
        .unwrap_or(0);
    DateTime {
        micros,
        aware: true,
    }
}

fn parse_expiry(raw: &str) -> Result<DateTime, ReuseRefused> {
    let text = raw.replace('Z', "+00:00");
    match parse_iso(&text) {
        Ok(stamp) if stamp.aware => Ok(stamp),
        Ok(_) => Err(ReuseRefused("artifact expiry is not timezone-aware".into())),
        Err(()) => Err(ReuseRefused("artifact expiry is not a timestamp".into())),
    }
}

fn parse_iso(text: &str) -> Result<DateTime, ()> {
    let bytes = text.as_bytes();
    if bytes.len() < 10 {
        return Err(());
    }
    let year = ascii_int(&bytes[0..4])?;
    if bytes.get(4) != Some(&b'-') || bytes.get(7) != Some(&b'-') {
        return Err(());
    }
    let month = u32::try_from(ascii_int(&bytes[5..7])?).map_err(|_| ())?;
    let day = u32::try_from(ascii_int(&bytes[8..10])?).map_err(|_| ())?;
    if month == 0 || month > 12 || day == 0 || day > month_days(year, month) {
        return Err(());
    }
    if bytes.len() == 10 {
        return Ok(DateTime {
            micros: civil_micros(year, month, day, 0, 0, 0, 0),
            aware: false,
        });
    }
    let separator = bytes[10];
    if separator != b'T' && separator != b't' && separator != b' ' {
        return Err(());
    }
    let mut index = 11;
    let hour = u32::try_from(take_n(bytes, &mut index, 2)?).map_err(|_| ())?;
    colon(bytes, &mut index)?;
    let minute = u32::try_from(take_n(bytes, &mut index, 2)?).map_err(|_| ())?;
    let mut second = 0u32;
    if index < bytes.len() && bytes[index] == b':' {
        index += 1;
        second = u32::try_from(take_n(bytes, &mut index, 2)?).map_err(|_| ())?;
    }
    if hour > 23 || minute > 59 || second > 59 {
        return Err(());
    }
    let mut micros = 0i128;
    if index < bytes.len() && bytes[index] == b'.' {
        index += 1;
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if index == start {
            return Err(());
        }
        micros = frac_micros(&bytes[start..index])?;
    }
    let local = civil_micros(year, month, day, hour, minute, second, micros);
    if index == bytes.len() {
        return Ok(DateTime {
            micros: local,
            aware: false,
        });
    }
    let sign = match bytes[index] {
        b'+' => 1i32,
        b'-' => -1,
        _ => return Err(()),
    };
    index += 1;
    let off_hour = take_n(bytes, &mut index, 2)?;
    let mut off_minute = 0i32;
    let mut off_second = 0i32;
    if index < bytes.len() && bytes[index] == b':' {
        index += 1;
        off_minute = take_n(bytes, &mut index, 2)?;
        if index < bytes.len() && bytes[index] == b':' {
            index += 1;
            off_second = take_n(bytes, &mut index, 2)?;
        }
    } else if index < bytes.len() && bytes[index].is_ascii_digit() {
        off_minute = take_n(bytes, &mut index, 2)?;
        if index < bytes.len() && bytes[index].is_ascii_digit() {
            off_second = take_n(bytes, &mut index, 2)?;
        }
    }
    if off_hour > 23 || off_minute > 59 || off_second > 59 || index != bytes.len() {
        return Err(());
    }
    let offset = sign * (off_hour * 3600 + off_minute * 60 + off_second);
    Ok(DateTime {
        micros: local - i128::from(offset) * 1_000_000,
        aware: true,
    })
}

fn take_n(bytes: &[u8], index: &mut usize, width: usize) -> Result<i32, ()> {
    let end = index.checked_add(width).ok_or(())?;
    if end > bytes.len() {
        return Err(());
    }
    let value = ascii_int(&bytes[*index..end])?;
    *index = end;
    Ok(value)
}

fn colon(bytes: &[u8], index: &mut usize) -> Result<(), ()> {
    if *index < bytes.len() && bytes[*index] == b':' {
        *index += 1;
        Ok(())
    } else {
        Err(())
    }
}

fn ascii_int(bytes: &[u8]) -> Result<i32, ()> {
    let text = std::str::from_utf8(bytes).map_err(|_| ())?;
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(());
    }
    text.parse().map_err(|_| ())
}

fn frac_micros(digits: &[u8]) -> Result<i128, ()> {
    if digits.is_empty() {
        return Err(());
    }
    let mut padded = [b'0'; 6];
    let take = digits.len().min(6);
    padded[..take].copy_from_slice(&digits[..take]);
    let text = std::str::from_utf8(&padded).map_err(|_| ())?;
    text.parse().map_err(|_| ())
}

fn month_days(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn leap(year: i32) -> bool {
    let extra = year % 4 == 0;
    extra && (year % 100 != 0 || year % 400 == 0)
}

fn civil_micros(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    micros: i128,
) -> i128 {
    let days = days_from_civil(year, month, day);
    let seconds =
        days * 86_400 + i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second);
    i128::from(seconds) * 1_000_000 + micros
}

fn days_from_civil(mut year: i32, month: u32, day: u32) -> i64 {
    year -= i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = u64::try_from(year - era * 400).unwrap_or(0);
    let month_prime = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era =
        year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + u64::from(day_of_year);
    i64::from(era) * 146_097 + i64::try_from(day_of_era).unwrap_or(0) - 719_468
}

const SHA: &str = "5c5799909437f794c79f3de4ab29ec8954eceefc";
const ABSENT: &str = "Error: HTTP 404: Not Found (https://api.github.com/repos/Jabbslad/kiln/attestations/sha256:abc?per_page=30)\n";

type Job = (&'static str, bool, bool, &'static str);

fn package_jobs(event: &str, reuse_result: &str, mode: &str, docs_only: bool) -> (bool, bool) {
    if reuse_result == "cancelled" {
        return (false, false);
    }
    let verified = reuse_result == "success" && mode == "reuse";
    let build = reuse_result == "skipped" || (event == "push" && !verified);
    let server = event != "pull_request" && build;
    let clients = (event != "pull_request" || !docs_only) && (event == "pull_request" || build);
    (server, clients)
}

fn draft_place(
    event: &str,
    tag: bool,
    reuse_result: &str,
    _mode: &str,
    packages_ok: bool,
) -> &'static str {
    if event != "push" || tag || reuse_result == "cancelled" || !packages_ok {
        return "";
    }
    "auto"
}

fn job_graph() -> Vec<Job> {
    let rows = [
        ("tag reuse", "push", true, "success", "reuse", false, true),
        (
            "tag no match",
            "push",
            true,
            "success",
            "fallback",
            false,
            true,
        ),
        ("tag error", "push", true, "failure", "", false, true),
        (
            "tag upload error",
            "push",
            true,
            "failure",
            "reuse",
            false,
            true,
        ),
        ("tag timeout", "push", true, "timed_out", "", false, true),
        ("main", "push", false, "skipped", "", false, true),
        ("code pr", "pull_request", false, "skipped", "", false, true),
        ("docs pr", "pull_request", false, "skipped", "", true, true),
        (
            "plain dispatch",
            "workflow_dispatch",
            false,
            "skipped",
            "",
            false,
            true,
        ),
        (
            "dry reuse",
            "workflow_dispatch",
            false,
            "success",
            "reuse",
            false,
            false,
        ),
        (
            "dry no match",
            "workflow_dispatch",
            false,
            "success",
            "fallback",
            false,
            false,
        ),
        (
            "dry error",
            "workflow_dispatch",
            false,
            "failure",
            "",
            false,
            false,
        ),
        ("cancelled tag", "push", true, "cancelled", "", false, false),
    ];
    rows.into_iter()
        .map(|(name, event, tag, result, mode, docs_only, packages_ok)| {
            let (server, clients) = package_jobs(event, result, mode, docs_only);
            let place = draft_place(event, tag, result, mode, packages_ok && server && clients);
            (name, server, clients, place)
        })
        .collect()
}

fn expected_jobs() -> Vec<Job> {
    vec![
        ("tag reuse", false, false, ""),
        ("tag no match", true, true, ""),
        ("tag error", true, true, ""),
        ("tag upload error", true, true, ""),
        ("tag timeout", true, true, ""),
        ("main", true, true, "auto"),
        ("code pr", false, true, ""),
        ("docs pr", false, false, ""),
        ("plain dispatch", true, true, ""),
        ("dry reuse", false, false, ""),
        ("dry no match", false, false, ""),
        ("dry error", false, false, ""),
        ("cancelled tag", false, false, ""),
    ]
}

fn job_body(text: &str, name: &str) -> Vec<String> {
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let marker = format!("  {name}:");
    let Some(start) = lines.iter().position(|line| line == &marker) else {
        panic!("missing job {name}");
    };
    let mut body = Vec::new();
    for line in &lines[start + 1..] {
        if is_job_key(line) {
            break;
        }
        body.push(line.clone());
    }
    body
}

fn is_job_key(line: &str) -> bool {
    let mut chars = line.chars();
    chars.next() == Some(' ')
        && chars.next() == Some(' ')
        && matches!(chars.next(), Some(ch) if ch != ' ')
}

fn unfold_if(lines: &[String], index: usize, continued: &str) -> String {
    let folded = lines[index]
        .split_once("if:")
        .map(|(_, rest)| rest.trim())
        .unwrap_or("");
    if !matches!(folded, ">-" | ">" | "|-" | "|") {
        return folded.to_string();
    }
    let mut parts = Vec::new();
    for line in &lines[index + 1..] {
        if line.starts_with(continued) {
            parts.push(line.trim().to_string());
        } else {
            break;
        }
    }
    parts.join(" ")
}

fn job_if_expression(text: &str, name: &str) -> String {
    let body = job_body(text, name);
    for (index, line) in body.iter().enumerate() {
        if line.starts_with("    if:") {
            return unfold_if(&body, index, "      ");
        }
    }
    panic!("{name} has no job if");
}

fn step_if_expression(text: &str, job: &str, marker: &str) -> String {
    let body = job_body(text, job);
    let mut steps = Vec::new();
    let mut current: Option<Vec<String>> = None;
    for line in &body {
        if line.starts_with("      - ") {
            if let Some(step) = current.take() {
                steps.push(step);
            }
            current = Some(vec![line.clone()]);
            continue;
        }
        if current.is_none() {
            continue;
        }
        if line.starts_with("        ") || line.trim().is_empty() {
            current.as_mut().unwrap().push(line.clone());
            continue;
        }
        steps.push(current.take().unwrap());
    }
    if let Some(step) = current {
        steps.push(step);
    }
    let matches: Vec<Vec<String>> = steps
        .into_iter()
        .filter(|step| step.iter().any(|line| line.contains(marker)))
        .collect();
    if matches.len() != 1 {
        let count = matches.len();
        panic!("{job} has {count} steps matching {marker}");
    }
    let step = &matches[0];
    for (index, line) in step.iter().enumerate() {
        if line.trim().starts_with("if:") {
            return unfold_if(step, index, "          ");
        }
    }
    panic!("{job} step {marker} has no if");
}

fn attest_runs(expr: &str, event: &str, reference: &str) -> bool {
    let mut text = expr.replace("!cancelled()", "True");
    let starts = if reference.starts_with("refs/tags/v") {
        "True"
    } else {
        "False"
    };
    text = text.replace("startsWith(github.ref, 'refs/tags/v')", starts);
    text = text.replace("github.event_name", &py_repr(event));
    text = text.replace("github.ref", &py_repr(reference));
    for job in ["server-package", "client", "dashboard"] {
        text = text.replace(&format!("needs.{job}.result"), &py_repr("success"));
    }
    text = text.replace("&&", " and ").replace("||", " or ");
    truthy(&eval_py(&text))
}

fn py_repr(value: &str) -> String {
    if !value.contains('\'') {
        format!("'{value}'")
    } else if !value.contains('"') {
        format!("\"{value}\"")
    } else {
        let escaped = value.replace('\\', "\\\\").replace('\'', "\\'");
        format!("'{escaped}'")
    }
}

enum PyVal {
    Bool(bool),
    Str(String),
}

fn truthy(value: &PyVal) -> bool {
    match value {
        PyVal::Bool(flag) => *flag,
        PyVal::Str(text) => !text.is_empty(),
    }
}

fn py_eq(left: &PyVal, right: &PyVal) -> bool {
    match (left, right) {
        (PyVal::Bool(left), PyVal::Bool(right)) => left == right,
        (PyVal::Str(left), PyVal::Str(right)) => left == right,
        _ => false,
    }
}

struct PyParser<'a> {
    text: &'a str,
    index: usize,
}

impl PyParser<'_> {
    fn skip(&mut self) {
        while let Some(ch) = self.text[self.index..].chars().next() {
            if ch.is_whitespace() {
                self.index += ch.len_utf8();
            } else {
                break;
            }
        }
    }

    fn starts_word(&mut self, word: &str) -> bool {
        self.skip();
        let rest = &self.text[self.index..];
        if !rest.starts_with(word) {
            return false;
        }
        !matches!(
            rest[word.len()..].chars().next(),
            Some(ch) if ch.is_ascii_alphanumeric() || ch == '_'
        )
    }

    fn eat_word(&mut self, word: &str) -> bool {
        if !self.starts_word(word) {
            return false;
        }
        self.index += word.len();
        true
    }

    fn eat(&mut self, token: &str) -> bool {
        self.skip();
        if self.text[self.index..].starts_with(token) {
            self.index += token.len();
            true
        } else {
            false
        }
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.text[self.index..].chars().next()?;
        self.index += ch.len_utf8();
        Some(ch)
    }

    fn parse_or(&mut self) -> PyVal {
        let mut left = self.parse_and();
        while self.eat_word("or") {
            let right = self.parse_and();
            left = PyVal::Bool(truthy(&left) || truthy(&right));
        }
        left
    }

    fn parse_and(&mut self) -> PyVal {
        let mut left = self.parse_not();
        while self.eat_word("and") {
            let right = self.parse_not();
            left = PyVal::Bool(truthy(&left) && truthy(&right));
        }
        left
    }

    fn parse_not(&mut self) -> PyVal {
        if self.eat_word("not") {
            let value = self.parse_not();
            return PyVal::Bool(!truthy(&value));
        }
        self.parse_cmp()
    }

    fn parse_cmp(&mut self) -> PyVal {
        let left = self.parse_atom();
        if self.eat("==") {
            let right = self.parse_atom();
            return PyVal::Bool(py_eq(&left, &right));
        }
        if self.eat("!=") {
            let right = self.parse_atom();
            return PyVal::Bool(!py_eq(&left, &right));
        }
        left
    }

    fn parse_atom(&mut self) -> PyVal {
        if self.eat("(") {
            let value = self.parse_or();
            if !self.eat(")") {
                let expression = self.text;
                panic!("missing ) in {expression}");
            }
            return value;
        }
        if self.eat_word("True") {
            return PyVal::Bool(true);
        }
        if self.eat_word("False") {
            return PyVal::Bool(false);
        }
        self.skip();
        let quote = self.text[self.index..].chars().next();
        if let Some(quote @ ('\'' | '"')) = quote {
            return PyVal::Str(self.parse_string(quote));
        }
        let expression = self.text;
        let tail = &expression[self.index..];
        panic!("bad atom at {tail:?} in {expression}");
    }

    fn parse_string(&mut self, quote: char) -> String {
        self.bump();
        let mut out = String::new();
        while let Some(ch) = self.bump() {
            if ch == '\\' {
                let escaped = self.bump().unwrap_or('\\');
                out.push(match escaped {
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    other => other,
                });
                continue;
            }
            if ch == quote {
                return out;
            }
            out.push(ch);
        }
        let expression = self.text;
        panic!("unterminated string in {expression}");
    }
}

fn eval_py(text: &str) -> PyVal {
    let mut parser = PyParser { text, index: 0 };
    let value = parser.parse_or();
    parser.skip();
    if parser.index != parser.text.len() {
        let tail = &parser.text[parser.index..];
        panic!("unparsed {tail:?} in {text}");
    }
    value
}

fn sample_run(run_id: i64) -> WorkflowRun {
    WorkflowRun {
        run_id,
        head_sha: SHA.to_string(),
        head_branch: "main".to_string(),
        event: "push".to_string(),
        path: WORKFLOW_PATH.to_string(),
        conclusion: "success".to_string(),
        status: "completed".to_string(),
    }
}

fn fixed_now() -> DateTime {
    DateTime::utc(2026, 10, 20, 12, 0, 0)
}

fn later() -> DateTime {
    fixed_now().plus_seconds(30 * 24 * 60 * 60)
}

fn sample_artifacts(
    expired: bool,
    expires_at: DateTime,
    names: Option<&[&str]>,
) -> Vec<PackageArtifact> {
    let mut chosen: Vec<&str> = match names {
        Some(names) => names.to_vec(),
        None => PACKAGE_ARTIFACTS.to_vec(),
    };
    chosen.sort_unstable();
    chosen
        .into_iter()
        .map(|name| PackageArtifact {
            name: name.to_string(),
            expired,
            expires_at,
        })
        .collect()
}

fn release_tag(tag: &str) -> ReleaseVersion {
    parse_release_tag(tag).unwrap()
}

fn fill_archives(dist: &Path, version: &ReleaseVersion) {
    fs::create_dir_all(dist).unwrap();
    let tag = version.tag();
    let payload = format!(r#"{{"version":"{tag}","target":"web"}}"#).into_bytes();
    for name in expected_archives(version) {
        let path = dist.join(&name);
        let stamped = name.starts_with("kiln-server-")
            || name.starts_with("kiln-identity-")
            || name.starts_with("kiln-dashboard-");
        if stamped {
            write_tar_gz(&path, "release.json", &payload);
        } else {
            write_tar_gz(&path, "kiln", b"kiln");
        }
        let digest = sha256_file(&path).unwrap();
        fs::write(
            dist.join(format!("{name}.sha256")),
            format!("{digest}  {name}\n"),
        )
        .unwrap();
    }
}

fn write_tar_gz(path: &Path, name: &str, data: &[u8]) {
    let file = File::create(path).unwrap();
    let encoder = GzEncoder::new(file, Compression::default());
    let mut archive = tar::Builder::new(encoder);
    let mut header = tar::Header::new_gnu();
    header.set_path(name).unwrap();
    header.set_size(data.len() as u64);
    header.set_mode(0o644);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_username("").unwrap();
    header.set_groupname("").unwrap();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_cksum();
    archive.append(&header, data).unwrap();
    let encoder = archive.into_inner().unwrap();
    encoder.finish().unwrap();
}

fn test_selects_the_latest_successful_main_push() {
    let older = sample_run(3);
    let newer = sample_run(9);
    let mut dispatch = sample_run(4);
    dispatch.event = "workflow_dispatch".to_string();
    let chosen = select_run(&[older, newer, dispatch], SHA).unwrap();
    assert_eq!(chosen.run_id, 9);
}

fn test_refuses_a_run_that_is_not_this_workflow_on_main() {
    let mut wrong_sha = sample_run(10);
    wrong_sha.head_sha = "0".repeat(40);
    let mut feature = sample_run(10);
    feature.head_branch = "feature".to_string();
    let mut dispatch = sample_run(10);
    dispatch.event = "workflow_dispatch".to_string();
    let mut pull = sample_run(10);
    pull.event = "pull_request".to_string();
    let mut other_workflow = sample_run(10);
    other_workflow.path = ".github/workflows/deploy.yml".to_string();
    let mut failed = sample_run(10);
    failed.conclusion = "failure".to_string();
    let mut running = sample_run(10);
    running.status = "in_progress".to_string();
    for run in [
        wrong_sha,
        feature,
        dispatch,
        pull,
        other_workflow,
        failed,
        running,
    ] {
        assert!(select_run(std::slice::from_ref(&run), SHA).is_err());
    }
    assert!(select_run(&[], SHA).is_err());
    assert!(select_run(&[sample_run(10)], "not-a-sha").is_err());
}

fn test_refuses_missing_extra_or_expired_artifacts() {
    let now = fixed_now();
    let future = later();
    require_artifacts(&sample_artifacts(false, future, None), &now).unwrap();
    let missing: Vec<&str> = PACKAGE_ARTIFACTS
        .iter()
        .copied()
        .filter(|name| *name != "package-server")
        .collect();
    assert!(require_artifacts(&sample_artifacts(false, future, Some(&missing)), &now).is_err());
    let mut stamped: Vec<&str> = PACKAGE_ARTIFACTS.to_vec();
    stamped.push("stamped-dist");
    require_artifacts(&sample_artifacts(false, future, Some(&stamped)), &now).unwrap();
    let mut extra: Vec<&str> = PACKAGE_ARTIFACTS.to_vec();
    extra.push("package-extra");
    require_artifacts(&sample_artifacts(false, future, Some(&extra)), &now).unwrap();
    assert!(require_artifacts(&sample_artifacts(true, future, None), &now).is_err());
    assert!(require_artifacts(&sample_artifacts(false, now, None), &now).is_err());
}

fn test_absent_attestation_is_allowed_only_before_the_workflow_attests() {
    let old = "uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4\n";
    let new = format!(
        "{old}        uses: {ATTEST_ACTION}e8998f949152b193b063cb0ec769d69d929409be # v2.4.0\n"
    );
    assert_eq!(
        attestation_gate(old, &["absent"; 6]).unwrap(),
        "not-produced"
    );
    assert_eq!(
        attestation_gate(&new, &["verified"; 6]).unwrap(),
        "verified"
    );
    assert!(attestation_gate(&new, &["absent"; 6]).is_err());
    assert!(attestation_gate(old, &["verified", "absent"]).is_err());
    assert!(attestation_gate(&new, &["rejected"]).is_err());
    assert!(!workflow_attests(
        "# actions/attest-build-provenance@mentioned\n"
    ));
}

fn test_classifies_the_real_missing_attestation_error() {
    assert_eq!(classify_verify(1, ABSENT), "absent");
    assert_eq!(classify_verify(0, ""), "verified");
    assert_eq!(
        classify_verify(1, "certificate identity did not match"),
        "rejected"
    );
}

fn test_checksum_mismatch_refuses_and_a_match_keeps_the_bytes() {
    let version = release_tag("v0.4.10");
    let tmp = tempfile::tempdir().unwrap();
    let dist = tmp.path();
    fill_archives(dist, &version);
    let tag = version.tag();
    let dashboard = dist.join(format!("kiln-dashboard-{tag}.tar.gz"));
    let before = fs::read(&dashboard).unwrap();
    let macos = dist.join(format!("kiln-{tag}-aarch64-apple-darwin.tar.gz.sha256"));
    let crlf = crlf_bytes(&fs::read(&macos).unwrap());
    fs::write(&macos, crlf).unwrap();
    verify_and_refresh_sidecars(dist, &version).unwrap();
    assert_eq!(fs::read(&dashboard).unwrap(), before);
    let refreshed = fs::read(&macos).unwrap();
    assert!(refreshed.ends_with(b"\n"));
    assert!(!refreshed.ends_with(b"\r\n"));
    let sidecar = dist.join(format!("kiln-dashboard-{tag}.tar.gz.sha256"));
    let zeros = "0".repeat(64);
    fs::write(&sidecar, format!("{zeros}  kiln-dashboard-{tag}.tar.gz\n")).unwrap();
    match verify_and_refresh_sidecars(dist, &version) {
        Err(ReleaseError::Refused(_)) => {}
        other => panic!("expected ReuseRefused, got {other:?}"),
    }
}

fn test_finish_writes_the_fourteen_asset_release_set() {
    let version = release_tag("v0.4.10");
    let tmp = tempfile::tempdir().unwrap();
    let dist = tmp.path();
    fill_archives(dist, &version);
    finish_dist(&crate::repo_root(), dist, &version).unwrap();
    let mut names = BTreeSet::new();
    let mut count = 0usize;
    for entry in fs::read_dir(dist).unwrap() {
        let entry = entry.unwrap();
        names.insert(entry.file_name().to_string_lossy().into_owned());
        count += 1;
    }
    assert_eq!(names, expected_release_files(&version));
    assert_eq!(count, 14);
    let mut sidecars: Vec<PathBuf> = fs::read_dir(dist)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".sha256") && !name.starts_with('.'))
        })
        .collect();
    sidecars.sort();
    let mut command = Command::new("sha256sum");
    command.arg("-c").current_dir(dist);
    for path in &sidecars {
        command.arg(path.file_name().unwrap());
    }
    let output = command.output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    let script = fs::read_to_string(dist.join("install.sh")).unwrap();
    assert!(script.contains("version=0.4.10"));
}

fn test_parser_rejects_a_paginated_artifact_list() {
    let page = serde_json::json!({
        "total_count": 2,
        "artifacts": [{
            "name": "package-server",
            "expired": false,
            "expires_at": "2026-11-19T00:00:00Z",
        }],
    });
    assert!(parse_artifacts(&page, &fixed_now()).is_err());
}

fn test_attest_signs_the_archives_a_tag_publishes() {
    let root = crate::repo_root();
    let text = fs::read_to_string(root.join(".github/workflows/build.yml")).unwrap();
    let expr = job_if_expression(&text, "attest");
    assert!(
        expr.contains("!cancelled()"),
        "a skipped reuse ancestor makes implicit success() false"
    );
    for result in [
        "needs.server-package.result == 'success'",
        "needs.client.result == 'success'",
        "needs.dashboard.result == 'success'",
    ] {
        assert!(expr.contains(result), "{result}");
    }
    assert!(expr.contains("github.ref == 'refs/heads/main'"));
    assert!(!expr.contains("startsWith(github.ref, 'refs/tags/v')"));
    assert!(attest_runs(&expr, "push", "refs/heads/main"));
    assert!(!attest_runs(&expr, "push", "refs/tags/v0.4.11"));
    assert!(attest_runs(&expr, "workflow_dispatch", "refs/heads/main"));
    assert!(
        !attest_runs(&expr, "workflow_dispatch", "refs/heads/feature"),
        "a non-main dispatch must not attest"
    );
    assert!(!attest_runs(&expr, "push", "refs/heads/feature"));
    let docs = fs::read_to_string(root.join("docs/releases.md")).unwrap();
    let documented = Regex::new(r"--cert-identity-regex '([^']+)'").unwrap();
    let caps = documented
        .captures(&docs)
        .unwrap_or_else(|| panic!("missing cert identity"));
    let identity = Regex::new(&caps[1]).unwrap();
    let subject = |reference: &str| {
        format!("https://github.com/Jabbslad/kiln/.github/workflows/build.yml@{reference}")
    };
    assert!(identity.is_match(&subject("refs/heads/main")));
    assert!(identity.is_match(&subject("refs/tags/v0.4.11")));
    assert!(!identity.is_match(&subject("refs/heads/feature")));
    assert!(!identity.is_match(&subject("refs/heads/main-backup")));
}

fn test_workflow_keeps_drafts_and_does_not_move_tags() {
    let text = fs::read_to_string(crate::repo_root().join(".github/workflows/build.yml")).unwrap();
    assert_eq!(text.matches("gh release create").count(), 1);
    assert!(!text.contains("--draft"));
    assert!(text.contains("verify-release-attestations.sh"));
    let verify =
        fs::read_to_string(crate::repo_root().join("scripts/verify-release-attestations.sh"))
            .unwrap();
    assert!(verify.contains("gh attestation verify"));
    assert!(verify.contains("--cert-identity-regex"));
    assert!(verify.contains("--source-ref"));
    assert!(text.contains("needs.attest.result == 'success'"));
    assert!(!text.contains("git push"));
    assert!(!text.contains("git tag"));
    let granted: Vec<&str> = text
        .lines()
        .filter(|line| line.trim() == "actions: write")
        .collect();
    assert_eq!(granted, vec!["      actions: write"]);
    assert!(
        job_body(&text, "cancel-on-required-failure")
            .iter()
            .any(|line| line.trim() == "actions: write"),
        "actions: write must stay scoped to cancel-on-required-failure"
    );
    assert_eq!(text.matches("retention-days: 30").count(), 5);
    assert!(
        text.contains("actions/attest-build-provenance@e8998f949152b193b063cb0ec769d69d929409be")
    );
    assert!(text.contains("dry_run:"));
    assert!(text.contains("needs.reuse.outputs.mode == 'fallback'"));
    let draft_if = job_if_expression(&text, "draft-release");
    assert!(draft_if.contains("needs.release-plan.outputs.release == 'true'"));
    assert!(draft_if.contains("github.ref == 'refs/heads/main'"));
    // A trailing `\` in a string literal drops the next line's leading
    // whitespace, so the fixture is joined from explicit lines.
    let step_only = [
        "  other:",
        "    if: needs.reuse.result == 'success' && needs.reuse.outputs.mode == 'reuse'",
        "  draft-release:",
        "    if: >-",
        "      ((needs.reuse.outputs.mode == 'reuse') ||",
        "        (needs.server-package.result == 'success'))",
        "    steps:",
        "      - if: needs.reuse.result == 'success' && needs.reuse.outputs.mode == 'reuse'",
        "",
    ]
    .join("\n");
    assert!(
        !job_if_expression(&step_only, "draft-release").contains("needs.reuse.result == 'success'")
    );
    let reuse_guard = "needs.reuse.result == 'success' && needs.reuse.outputs.mode == 'reuse'";
    assert!(
        !job_body(&text, "draft-release")
            .join("\n")
            .contains("name: reused-dist")
    );
    assert!(job_if_expression(&text, "dry-run-assets").contains(reuse_guard));
    let sibling = [
        "  draft-release:",
        "    steps:",
        "      - uses: actions/download-artifact",
        "        if: needs.reuse.outputs.mode == 'reuse'",
        "        with:",
        "          name: reused-dist",
        "      - uses: actions/download-artifact",
        "        if: needs.reuse.result == 'success' && needs.reuse.outputs.mode == 'reuse'",
        "        with:",
        "          name: package",
        "",
    ]
    .join("\n");
    assert_eq!(
        step_if_expression(&sibling, "draft-release", "name: reused-dist"),
        "needs.reuse.outputs.mode == 'reuse'"
    );
    assert_eq!(
        text.matches("!(needs.reuse.result == 'success' && needs.reuse.outputs.mode == 'reuse')")
            .count(),
        3
    );
    assert!(text.contains("needs.version-guard.outputs.stamped == 'true'"));
    assert!(text.contains("needs.version-guard.outputs.stamped != 'true'"));
    assert_eq!(
        text.matches("!startsWith(github.ref, 'refs/tags/v')")
            .count(),
        6
    );
    assert!(!text.contains("--auto"));
    assert_eq!(job_graph(), expected_jobs());
}

fn test_fetch_refusal_sets_fallback_without_publishing() {
    let tmp = tempfile::tempdir().unwrap();
    let output = tmp.path().join("out");
    let mut env: HashMap<String, String> = std::env::vars().collect();
    env.insert("REUSE_EVENT".to_string(), "workflow_dispatch".to_string());
    env.insert("REUSE_SHA_INPUT".to_string(), "not-a-sha".to_string());
    env.insert("REUSE_VERSION".to_string(), String::new());
    env.insert("GH_REPO".to_string(), "Jabbslad/kiln".to_string());
    env.insert("GITHUB_OUTPUT".to_string(), output.display().to_string());
    let args = vec![
        "fetch".to_string(),
        "--dist".to_string(),
        tmp.path().join("dist").display().to_string(),
    ];
    let (code, _stdout, stderr) = run(&args, &env);
    assert_eq!(code, 0, "{stderr}");
    let text = fs::read_to_string(&output).unwrap();
    assert!(text.contains("mode=fallback"));
    assert!(!text.contains("gh release"));
    let combined = format!("{text}{stderr}");
    assert!(!combined.contains("ghp_"));
}

register_cases! {
    test_selects_the_latest_successful_main_push,
    test_refuses_a_run_that_is_not_this_workflow_on_main,
    test_refuses_missing_extra_or_expired_artifacts,
    test_absent_attestation_is_allowed_only_before_the_workflow_attests,
    test_classifies_the_real_missing_attestation_error,
    test_checksum_mismatch_refuses_and_a_match_keeps_the_bytes,
    test_finish_writes_the_fourteen_asset_release_set,
    test_parser_rejects_a_paginated_artifact_list,
    test_attest_signs_the_archives_a_tag_publishes,
    test_workflow_keeps_drafts_and_does_not_move_tags,
    test_fetch_refusal_sets_fallback_without_publishing,
}

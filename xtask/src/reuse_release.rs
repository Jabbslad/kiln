//! Reuse main's packages for a tag, or say why that run cannot be published.

use crate::release_version::{self, ReleaseVersion};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const WORKFLOW_PATH: &str = ".github/workflows/build.yml";
pub const ATTEST_ACTION: &str = "actions/attest-build-provenance@";
pub const PACKAGE_ARTIFACTS: &[&str] = &[
    "package-server",
    "package-dashboard",
    "package-x86_64-unknown-linux-gnu",
    "package-aarch64-apple-darwin",
    "package-x86_64-apple-darwin",
    "package-x86_64-pc-windows-msvc",
];

const GH_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReuseRefused(pub String);

impl std::fmt::Display for ReuseRefused {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ReuseRefused {}

#[derive(Debug)]
pub enum FetchError {
    Refused(ReuseRefused),
    Failed(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(error) => write!(formatter, "{error}"),
            Self::Failed(message) => write!(formatter, "{message}"),
        }
    }
}

impl std::error::Error for FetchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Refused(error) => Some(error),
            Self::Failed(_) => None,
        }
    }
}

impl From<ReuseRefused> for FetchError {
    fn from(error: ReuseRefused) -> Self {
        Self::Refused(error)
    }
}

impl FetchError {
    fn message(&self) -> String {
        match self {
            Self::Refused(error) => error.0.clone(),
            Self::Failed(message) => message.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct GhOutput {
    pub returncode: i32,
    pub stdout: String,
    pub stderr: String,
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

#[derive(Clone, Copy, Debug)]
pub struct Timestamp {
    utc_micros: i128,
    aware: bool,
}

impl Timestamp {
    pub fn utc(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> Self {
        let days = days_from_civil(year, month, day);
        Self {
            utc_micros: i128::from(days) * 86_400_000_000
                + i128::from(hour) * 3_600_000_000
                + i128::from(minute) * 60_000_000
                + i128::from(second) * 1_000_000,
            aware: true,
        }
    }

    pub fn now_utc() -> Self {
        let micros = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_micros())
            .unwrap_or(0);
        Self {
            utc_micros: i128::try_from(micros).unwrap_or(i128::MAX),
            aware: true,
        }
    }

    fn plus_days(self, days: i64) -> Self {
        Self {
            utc_micros: self.utc_micros + i128::from(days) * 86_400_000_000,
            aware: self.aware,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PackageArtifact {
    pub name: String,
    pub expired: bool,
    pub expires_at: Timestamp,
}

trait GhRunner {
    fn execute(&mut self, args: &[String]) -> Result<GhOutput, ReuseRefused>;
}

impl<F> GhRunner for F
where
    F: FnMut(&[String]) -> Result<GhOutput, ReuseRefused>,
{
    fn execute(&mut self, args: &[String]) -> Result<GhOutput, ReuseRefused> {
        self(args)
    }
}

pub fn select_run(runs: &[WorkflowRun], sha: &str) -> Result<WorkflowRun, ReuseRefused> {
    if !is_sha(sha) {
        return Err(ReuseRefused("commit sha is not 40 hex characters".into()));
    }
    let mut chosen: Option<&WorkflowRun> = None;
    for run in runs {
        if run.head_sha == sha
            && run.head_branch == "main"
            && run.event == "push"
            && run.path == WORKFLOW_PATH
            && run.conclusion == "success"
            && run.status == "completed"
            && chosen.is_none_or(|current| run.run_id > current.run_id)
        {
            chosen = Some(run);
        }
    }
    chosen.cloned().ok_or_else(|| {
        ReuseRefused("no successful main push of this workflow for that commit".into())
    })
}

pub fn require_artifacts(
    artifacts: &[PackageArtifact],
    now: Timestamp,
) -> Result<(), ReuseRefused> {
    if !now.aware {
        return Err(ReuseRefused("clock must be timezone-aware".into()));
    }
    let names: Vec<&str> = artifacts
        .iter()
        .map(|artifact| artifact.name.as_str())
        .collect();
    if names.len() != names.iter().collect::<HashSet<_>>().len() {
        return Err(ReuseRefused("duplicate artifact name".into()));
    }
    let expected: BTreeSet<&str> = PACKAGE_ARTIFACTS.iter().copied().collect();
    let got: BTreeSet<&str> = names.into_iter().collect();
    if got != expected {
        return Err(ReuseRefused(
            "artifact set does not match the package jobs".into(),
        ));
    }
    for artifact in artifacts {
        if artifact.expired || artifact.expires_at.utc_micros <= now.utc_micros {
            return Err(ReuseRefused(format!("{} is expired", artifact.name)));
        }
    }
    Ok(())
}

pub fn workflow_attests(workflow_text: &str) -> bool {
    workflow_text
        .lines()
        .any(|line| line.trim_start().starts_with("uses:") && line.contains(ATTEST_ACTION))
}

pub fn attestation_gate(workflow_text: &str, results: &[&str]) -> Result<String, ReuseRefused> {
    if results.is_empty() || results.iter().any(|result| !known_attestation(result)) {
        return Err(ReuseRefused("attestation result is not usable".into()));
    }
    if results.iter().any(|result| *result == "rejected") {
        return Err(ReuseRefused("attestation verification failed".into()));
    }
    if results.iter().all(|result| *result == "verified") {
        return Ok("verified".into());
    }
    if results.iter().all(|result| *result == "absent") {
        if workflow_attests(workflow_text) {
            return Err(ReuseRefused(
                "this commit's workflow attests builds, but the archives have no attestation"
                    .into(),
            ));
        }
        return Ok("not-produced".into());
    }
    Err(ReuseRefused("archives disagree on attestation".into()))
}

fn known_attestation(result: &str) -> bool {
    matches!(result, "verified" | "absent" | "rejected")
}

pub fn classify_verify(returncode: i32, stderr: &str) -> String {
    if returncode == 0 {
        return "verified".into();
    }
    if stderr.contains("attestations/sha256:") && stderr.contains("404") {
        return "absent".into();
    }
    "rejected".into()
}

pub fn expected_archives(version: &ReleaseVersion) -> BTreeSet<String> {
    let tag = version.tag();
    let mut names = BTreeSet::from([
        format!("kiln-dashboard-{tag}.tar.gz"),
        format!("kiln-identity-{tag}-x86_64-unknown-linux-gnu.tar.gz"),
        format!("kiln-server-{tag}-x86_64-unknown-linux-gnu.tar.gz"),
        format!("kiln-{tag}-x86_64-pc-windows-msvc.zip"),
    ]);
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
    let mut names = archive_and_sidecar_names(version);
    names.insert("install.sh".into());
    names.insert("install.sh.sha256".into());
    names
}

fn archive_and_sidecar_names(version: &ReleaseVersion) -> BTreeSet<String> {
    let archives = expected_archives(version);
    let mut names = archives.clone();
    for name in &archives {
        names.insert(format!("{name}.sha256"));
    }
    names
}

pub fn verify_and_refresh_sidecars(
    dist: &Path,
    version: &ReleaseVersion,
) -> Result<(), FetchError> {
    let archives = expected_archives(version);
    let paths = regular_files(dist)?;
    let names: BTreeSet<String> = paths.iter().map(|path| entry_name(path)).collect();
    if names != archive_and_sidecar_names(version) {
        return Err(ReuseRefused("downloaded files are not the expected archives".into()).into());
    }
    let by_name: HashMap<String, PathBuf> = paths
        .into_iter()
        .map(|path| (entry_name(&path), path))
        .collect();
    for name in archives {
        let archive = &by_name[&name];
        let sidecar = &by_name[&format!("{name}.sha256")];
        let expected = sidecar_line(&sha256_hex(archive)?, &name);
        let actual = fs::read(sidecar).map_err(|error| FetchError::Failed(error.to_string()))?;
        let crlf = expected.replace('\n', "\r\n");
        if actual.as_slice() != expected.as_bytes() && actual.as_slice() != crlf.as_bytes() {
            return Err(ReuseRefused(format!("checksum does not match {name}")).into());
        }
        fs::write(sidecar, expected.as_bytes())
            .map_err(|error| FetchError::Failed(error.to_string()))?;
    }
    Ok(())
}

pub fn finish_dist(repo: &Path, dist: &Path, version: &ReleaseVersion) -> Result<(), FetchError> {
    release_version::check_archives(version, dist).map_err(FetchError::Failed)?;
    release_version::attach_bootstrap(repo, dist, version).map_err(FetchError::Failed)?;
    let names: BTreeSet<String> = regular_files(dist)?
        .iter()
        .map(|path| entry_name(path))
        .collect();
    if names != expected_release_files(version) {
        return Err(ReuseRefused("dist is not the 16-file release set".into()).into());
    }
    Ok(())
}

pub fn parse_runs(items: &Value) -> Result<Vec<WorkflowRun>, ReuseRefused> {
    let Some(items) = items.as_array() else {
        return Err(ReuseRefused("run list is not a list".into()));
    };
    let mut runs = Vec::with_capacity(items.len());
    for item in items {
        let Some(object) = item.as_object() else {
            return Err(ReuseRefused("run list entry is not an object".into()));
        };
        runs.push(WorkflowRun {
            run_id: json_i64(object, "databaseId")?,
            head_sha: json_sha(object, "headSha")?,
            head_branch: json_str(object, "headBranch")?,
            event: json_str(object, "event")?,
            path: json_str(object, "path")?,
            conclusion: json_str(object, "conclusion")?,
            status: json_str(object, "status")?,
        });
    }
    Ok(runs)
}

pub fn parse_artifacts(
    payload: &Value,
    now: Timestamp,
) -> Result<Vec<PackageArtifact>, ReuseRefused> {
    let Some(object) = payload.as_object() else {
        return Err(ReuseRefused("artifact list is not an object".into()));
    };
    let malformed = ReuseRefused("artifact list was paginated or malformed".into());
    let Some(artifacts) = object.get("artifacts").and_then(Value::as_array) else {
        return Err(malformed);
    };
    let Some(total) = json_exact_i64(object.get("total_count")) else {
        return Err(ReuseRefused(
            "artifact list was paginated or malformed".into(),
        ));
    };
    if total != artifacts.len() as i64 {
        return Err(ReuseRefused(
            "artifact list was paginated or malformed".into(),
        ));
    }
    let mut parsed = Vec::with_capacity(artifacts.len());
    for item in artifacts {
        parsed.push(parse_artifact(item)?);
    }
    if !now.aware {
        return Err(ReuseRefused("clock must be timezone-aware".into()));
    }
    Ok(parsed)
}

fn parse_artifact(item: &Value) -> Result<PackageArtifact, ReuseRefused> {
    let Some(object) = item.as_object() else {
        return Err(ReuseRefused("artifact entry is not an object".into()));
    };
    let expires_at = parse_expires(&json_str(object, "expires_at")?)?;
    let expired = match object.get("expired") {
        Some(Value::Bool(value)) => *value,
        _ => {
            return Err(ReuseRefused(
                "artifact expired flag is not a boolean".into(),
            ));
        }
    };
    Ok(PackageArtifact {
        name: json_str(object, "name")?,
        expired,
        expires_at,
    })
}

pub fn fetch<R>(
    repo: &str,
    sha: &str,
    version_text: &str,
    dist: &Path,
    gh: &mut R,
) -> Result<(String, ReleaseVersion), FetchError>
where
    R: FnMut(&[String]) -> Result<GhOutput, ReuseRefused>,
{
    fetch_inner(repo, sha, version_text, dist, gh)
}

fn fetch_inner(
    repo: &str,
    sha: &str,
    version_text: &str,
    dist: &Path,
    gh: &mut dyn GhRunner,
) -> Result<(String, ReleaseVersion), FetchError> {
    require_repo(repo)?;
    prepare_dist(dist)?;
    let chosen = select_run(&listed_runs(repo, sha, gh)?, sha)?;
    let now = Timestamp::now_utc();
    let page = artifact_page(repo, chosen.run_id, gh)?;
    require_artifacts(&parse_artifacts(&page, now)?, now)?;
    download_packages(repo, chosen.run_id, dist, gh)?;
    let version = release_version::parse_release_tag(version_text).map_err(FetchError::Failed)?;
    verify_and_refresh_sidecars(dist, &version)?;
    let workflow = workflow_text(repo, sha, gh)?;
    let classified = attest_results(repo, sha, dist, gh)?;
    let refs: Vec<&str> = classified.iter().map(String::as_str).collect();
    let produced = attestation_gate(&workflow, &refs)?;
    Ok((
        format!("run {} attestation={produced}", chosen.run_id),
        version,
    ))
}

pub fn run(args: &[String], env: &HashMap<String, String>) -> (i32, String, String) {
    let output = run_with(args, env, &mut |command| gh_command(command));
    (output.code, output.stdout, output.stderr)
}

pub fn cli(args: &[String]) -> i32 {
    let env = std::env::vars().collect();
    let (code, stdout, stderr) = run(args, &env);
    print!("{stdout}");
    eprint!("{stderr}");
    code
}

struct Output {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Output {
    fn ok(stdout: String, stderr: String) -> Self {
        Self {
            code: 0,
            stdout,
            stderr,
        }
    }

    fn fail(stderr: String) -> Self {
        Self {
            code: 1,
            stdout: String::new(),
            stderr,
        }
    }

    fn usage(stderr: String) -> Self {
        Self {
            code: 2,
            stdout: String::new(),
            stderr,
        }
    }
}

enum Command {
    Fetch {
        dist: PathBuf,
    },
    Finish {
        version: String,
        dist: PathBuf,
        repo: PathBuf,
    },
}

fn run_with(args: &[String], env: &HashMap<String, String>, gh: &mut dyn GhRunner) -> Output {
    match parse_command(args) {
        Ok(Command::Fetch { dist }) => fetch_command(&dist, env, gh),
        Ok(Command::Finish {
            version,
            dist,
            repo,
        }) => finish_command(&version, &dist, &repo),
        Err(message) => Output::usage(message),
    }
}

fn fetch_command(dist: &Path, env: &HashMap<String, String>, gh: &mut dyn GhRunner) -> Output {
    let event = env_get(env, "REUSE_EVENT");
    let repo = env_get(env, "GH_REPO").to_string();
    let (sha, version_text) = if event == "workflow_dispatch" {
        (env_get(env, "REUSE_SHA_INPUT").to_string(), String::new())
    } else if event == "push" {
        (
            env_get(env, "REUSE_SHA").to_string(),
            env_get(env, "REUSE_VERSION").to_string(),
        )
    } else {
        return fallback(env, "reuse runs only for a tag push or a dry run");
    };
    let version_text = if version_text.is_empty() {
        match committed_tag(&repo, &sha, gh) {
            Ok(tag) => tag,
            Err(error) => return fetch_output(env, Err(error)),
        }
    } else {
        version_text
    };
    fetch_output(env, fetch_inner(&repo, &sha, &version_text, dist, gh))
}

fn fetch_output(
    env: &HashMap<String, String>,
    result: Result<(String, ReleaseVersion), FetchError>,
) -> Output {
    match result {
        Ok((reason, version)) => accept(env, &reason, &version.tag()),
        Err(FetchError::Refused(error)) => fallback(env, &error.0),
        Err(FetchError::Failed(message)) => Output::fail(format!("{message}\n")),
    }
}

fn finish_command(version: &str, dist: &Path, repo: &Path) -> Output {
    let version = match release_version::parse_release_tag(version) {
        Ok(version) => version,
        Err(error) => return Output::fail(format!("{error}\n")),
    };
    match finish_dist(repo, dist, &version) {
        Ok(()) => Output::ok(String::new(), String::new()),
        Err(error) => Output::fail(format!("{}\n", error.message())),
    }
}

fn accept(env: &HashMap<String, String>, reason: &str, version: &str) -> Output {
    let mut stdout = String::new();
    if let Err(error) = write_outputs(
        env,
        &mut stdout,
        &[
            ("mode", "reuse"),
            ("version", version),
            ("reason", &one_line(reason)),
        ],
    ) {
        return Output {
            code: 1,
            stdout,
            stderr: format!("{error}\n"),
        };
    }
    stdout.push_str("reuse accepted: ");
    stdout.push_str(reason);
    stdout.push('\n');
    Output::ok(stdout, String::new())
}

fn fallback(env: &HashMap<String, String>, message: &str) -> Output {
    let mut stdout = String::new();
    let reason = one_line(message);
    if let Err(error) = write_outputs(
        env,
        &mut stdout,
        &[
            ("mode", "fallback"),
            ("version", ""),
            ("reason", reason.as_str()),
        ],
    ) {
        return Output {
            code: 1,
            stdout,
            stderr: format!("{error}\n"),
        };
    }
    Output {
        code: 0,
        stdout,
        stderr: format!("reuse refused: {message}\n"),
    }
}

fn write_outputs(
    env: &HashMap<String, String>,
    stdout: &mut String,
    pairs: &[(&str, &str)],
) -> Result<(), String> {
    for (name, value) in pairs {
        emit(name, value, env, stdout)?;
    }
    Ok(())
}

fn parse_command(args: &[String]) -> Result<Command, String> {
    if args.is_empty() {
        return Err("a command is required\n".into());
    }
    let command = args[0].as_str();
    if command != "fetch" && command != "finish" {
        return Err(format!("unknown command {command}\n"));
    }
    let mut dist = None;
    let mut version = None;
    let mut repo = None;
    let mut index = 1usize;
    while index < args.len() {
        let arg = &args[index];
        let Some(body) = arg.strip_prefix("--") else {
            return Err(format!("unrecognized arguments: {arg}\n"));
        };
        if body.is_empty() {
            return Err("unrecognized arguments: --\n".into());
        }
        let (key, inline) = split_flag(body);
        let value = if let Some(value) = inline {
            value.to_string()
        } else {
            index += 1;
            let Some(value) = args.get(index) else {
                return Err(format!("argument --{key}: expected one argument\n"));
            };
            value.clone()
        };
        match key {
            "dist" => dist = Some(PathBuf::from(value)),
            "version" => version = Some(value),
            "repo" => repo = Some(PathBuf::from(value)),
            _ => return Err(format!("unrecognized arguments: --{key}\n")),
        }
        index += 1;
    }
    if command == "fetch" {
        if version.is_some() || repo.is_some() {
            return Err("unrecognized arguments\n".into());
        }
        let Some(dist) = dist else {
            return Err("the following arguments are required: --dist\n".into());
        };
        return Ok(Command::Fetch { dist });
    }
    let Some(version) = version else {
        return Err("the following arguments are required: --version\n".into());
    };
    let Some(dist) = dist else {
        return Err("the following arguments are required: --dist\n".into());
    };
    Ok(Command::Finish {
        version,
        dist,
        repo: repo.unwrap_or_else(crate::repo_root),
    })
}

fn split_flag(body: &str) -> (&str, Option<&str>) {
    if let Some((key, value)) = body.split_once('=') {
        (key, Some(value))
    } else {
        (body, None)
    }
}

fn listed_runs(
    repo: &str,
    sha: &str,
    gh: &mut dyn GhRunner,
) -> Result<Vec<WorkflowRun>, FetchError> {
    let items = run_list(repo, sha, gh)?;
    let enriched = with_paths(repo, &items, gh)?;
    parse_runs(&Value::Array(enriched)).map_err(FetchError::from)
}

fn run_list(repo: &str, sha: &str, gh: &mut dyn GhRunner) -> Result<Vec<Value>, FetchError> {
    if !is_sha(sha) {
        return Err(ReuseRefused("commit sha is not 40 hex characters".into()).into());
    }
    let text = gh_text(
        gh,
        &[
            "run",
            "list",
            "--repo",
            repo,
            "--workflow",
            "build.yml",
            "--commit",
            sha,
            "--branch",
            "main",
            "--limit",
            "20",
            "--json",
            "databaseId,headSha,headBranch,event,conclusion,status",
        ],
        "could not list workflow runs",
    )?;
    let payload: Value = serde_json::from_str(&text)
        .map_err(|_| FetchError::from(ReuseRefused("run list was not json".into())))?;
    let Some(items) = payload.as_array() else {
        return Err(ReuseRefused("run list is not a list".into()).into());
    };
    Ok(items.clone())
}

fn with_paths(
    repo: &str,
    items: &[Value],
    gh: &mut dyn GhRunner,
) -> Result<Vec<Value>, FetchError> {
    let mut enriched = Vec::with_capacity(items.len());
    for item in items {
        let Some(object) = item.as_object() else {
            return Err(ReuseRefused("run list entry is not an object".into()).into());
        };
        let run_id = json_i64(object, "databaseId")?;
        let api = format!("repos/{repo}/actions/runs/{run_id}");
        let path = gh_text(
            gh,
            &["api", &api, "--jq", ".path"],
            "could not read the workflow path",
        )?;
        let mut copy = object.clone();
        copy.insert("path".into(), Value::String(path.trim().to_string()));
        enriched.push(Value::Object(copy));
    }
    Ok(enriched)
}

fn artifact_page(repo: &str, run_id: i64, gh: &mut dyn GhRunner) -> Result<Value, FetchError> {
    let api = format!("repos/{repo}/actions/runs/{run_id}/artifacts");
    let text = gh_text(gh, &["api", &api], "could not list artifacts")?;
    serde_json::from_str(&text)
        .map_err(|_| FetchError::from(ReuseRefused("artifact list was not json".into())))
}

fn workflow_text(repo: &str, sha: &str, gh: &mut dyn GhRunner) -> Result<String, FetchError> {
    let api = format!("repos/{repo}/contents/{WORKFLOW_PATH}?ref={sha}");
    gh_text(
        gh,
        &["api", &api, "-H", "Accept: application/vnd.github.raw"],
        "could not read the workflow at that commit",
    )
}

fn download_packages(
    repo: &str,
    run_id: i64,
    dist: &Path,
    gh: &mut dyn GhRunner,
) -> Result<(), FetchError> {
    let dir = path_arg(dist);
    let run = run_id.to_string();
    let mut names: Vec<&str> = PACKAGE_ARTIFACTS.to_vec();
    names.sort_unstable();
    for name in names {
        let refuse = format!("download of {name} failed");
        gh_text(
            gh,
            &[
                "run", "download", &run, "--repo", repo, "--name", name, "--dir", &dir,
            ],
            &refuse,
        )?;
    }
    Ok(())
}

fn attest_results(
    repo: &str,
    sha: &str,
    dist: &Path,
    gh: &mut dyn GhRunner,
) -> Result<Vec<String>, FetchError> {
    let mut files = archive_files(dist)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut results = Vec::with_capacity(files.len());
    for (_, path) in files {
        let (code, stderr) = verify_attestation(gh, repo, sha, &path)?;
        results.push(classify_verify(code, &stderr));
    }
    Ok(results)
}

fn verify_attestation(
    gh: &mut dyn GhRunner,
    repo: &str,
    sha: &str,
    archive: &Path,
) -> Result<(i32, String), FetchError> {
    let args = vec![
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
    ];
    let completed = gh.execute(&args)?;
    Ok((completed.returncode, completed.stderr))
}

fn committed_tag(repo: &str, sha: &str, gh: &mut dyn GhRunner) -> Result<String, FetchError> {
    require_repo(repo)?;
    if !is_sha(sha) {
        return Err(ReuseRefused("commit sha is not 40 hex characters".into()).into());
    }
    let api = format!("repos/{repo}/contents/Cargo.toml?ref={sha}");
    let text = gh_text(
        gh,
        &["api", &api, "-H", "Accept: application/vnd.github.raw"],
        "could not read Cargo.toml at that commit",
    )?;
    Ok(release_version::committed_version(&text)
        .map_err(FetchError::Failed)?
        .tag())
}

fn gh_text(gh: &mut dyn GhRunner, args: &[&str], refuse: &str) -> Result<String, FetchError> {
    let mut full = Vec::with_capacity(args.len() + 1);
    full.push("gh".to_string());
    full.extend(args.iter().map(|arg| (*arg).to_string()));
    let completed = gh.execute(&full)?;
    if completed.returncode != 0 {
        return Err(ReuseRefused(format!("{refuse}: {}", public_error(&completed.stderr))).into());
    }
    Ok(completed.stdout)
}

fn gh_command(args: &[String]) -> Result<GhOutput, ReuseRefused> {
    if args.is_empty() {
        return Err(ReuseRefused("gh could not be run".into()));
    }
    let mut child = match Command::new(&args[0])
        .args(&args[1..])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return Err(ReuseRefused("gh could not be run".into())),
    };
    let stdout = match child.stdout.take() {
        Some(pipe) => pipe,
        None => return stop_child(child),
    };
    let stderr = match child.stderr.take() {
        Some(pipe) => pipe,
        None => return stop_child(child),
    };
    let stdout_thread = std::thread::spawn(move || read_to_string(stdout));
    let stderr_thread = std::thread::spawn(move || read_to_string(stderr));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= GH_TIMEOUT => {
                return stop_timed_out(child, stdout_thread, stderr_thread);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(30)),
            Err(_) => return stop_timed_out(child, stdout_thread, stderr_thread),
        }
    };
    Ok(GhOutput {
        returncode: status.code().unwrap_or(1),
        stdout: stdout_thread.join().unwrap_or_default(),
        stderr: stderr_thread.join().unwrap_or_default(),
    })
}

fn stop_child(mut child: std::process::Child) -> Result<GhOutput, ReuseRefused> {
    let _ = child.kill();
    let _ = child.wait();
    Err(ReuseRefused("gh could not be run".into()))
}

fn stop_timed_out<T>(
    mut child: std::process::Child,
    stdout_thread: std::thread::JoinHandle<T>,
    stderr_thread: std::thread::JoinHandle<T>,
) -> Result<GhOutput, ReuseRefused> {
    let _ = child.kill();
    let _ = child.wait();
    let _ = stdout_thread.join();
    let _ = stderr_thread.join();
    Err(ReuseRefused("gh could not be run".into()))
}

fn read_to_string<R: Read>(mut pipe: R) -> String {
    let mut buffer = Vec::new();
    let _ = pipe.read_to_end(&mut buffer);
    String::from_utf8_lossy(&buffer).into_owned()
}

fn prepare_dist(dist: &Path) -> Result<(), FetchError> {
    if dist.exists() {
        let mut entries =
            fs::read_dir(dist).map_err(|error| FetchError::Failed(error.to_string()))?;
        match entries.next() {
            Some(Ok(_)) => return Err(ReuseRefused("dist is not empty".into()).into()),
            Some(Err(error)) => return Err(FetchError::Failed(error.to_string())),
            None => {}
        }
    }
    fs::create_dir_all(dist).map_err(|error| FetchError::Failed(error.to_string()))
}

fn archive_files(dist: &Path) -> Result<Vec<(String, PathBuf)>, FetchError> {
    let mut files = Vec::new();
    let entries = fs::read_dir(dist).map_err(|error| FetchError::Failed(error.to_string()))?;
    for entry in entries {
        let entry = entry.map_err(|error| FetchError::Failed(error.to_string()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if follows_file(&path) && !name.ends_with(".sha256") {
            files.push((name, path));
        }
    }
    Ok(files)
}

fn regular_files(dist: &Path) -> Result<Vec<PathBuf>, ReuseRefused> {
    if !fs::metadata(dist)
        .map(|meta| meta.is_dir())
        .unwrap_or(false)
    {
        return Err(ReuseRefused("dist is not a directory".into()));
    }
    let entries = fs::read_dir(dist).map_err(|_| ReuseRefused("dist is not a directory".into()))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| ReuseRefused("dist is not a directory".into()))?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let meta = fs::symlink_metadata(&path)
            .map_err(|_| ReuseRefused(format!("unexpected dist entry {name}")))?;
        let kind = meta.file_type();
        if kind.is_symlink() || !kind.is_file() {
            return Err(ReuseRefused(format!("unexpected dist entry {name}")));
        }
        paths.push(path);
    }
    Ok(paths)
}

fn follows_file(path: &Path) -> bool {
    fs::metadata(path)
        .map(|meta| meta.is_file())
        .unwrap_or(false)
}

fn sha256_hex(path: &Path) -> Result<String, FetchError> {
    let mut file = File::open(path).map_err(|error| FetchError::Failed(error.to_string()))?;
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher).map_err(|error| FetchError::Failed(error.to_string()))?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn sidecar_line(digest: &str, name: &str) -> String {
    let mut line = String::with_capacity(digest.len() + name.len() + 3);
    line.push_str(digest);
    line.push(' ');
    line.push(' ');
    line.push_str(name);
    line.push('\n');
    line
}

fn require_repo(repo: &str) -> Result<(), ReuseRefused> {
    if is_repo(repo) {
        Ok(())
    } else {
        Err(ReuseRefused("repository is not owner/name".into()))
    }
}

fn is_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn is_repo(value: &str) -> bool {
    let Some((owner, name)) = value.split_once('/') else {
        return false;
    };
    !owner.is_empty()
        && !name.is_empty()
        && !name.contains('/')
        && owner.chars().all(repo_char)
        && name.chars().all(repo_char)
}

fn repo_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-')
}

fn json_exact_i64(value: Option<&Value>) -> Option<i64> {
    let number = value?.as_number()?;
    if number.is_f64() {
        return None;
    }
    number.as_i64()
}

fn json_i64(object: &Map<String, Value>, key: &str) -> Result<i64, ReuseRefused> {
    json_exact_i64(object.get(key)).ok_or_else(|| ReuseRefused(format!("{key} is not an integer")))
}

fn json_str(object: &Map<String, Value>, key: &str) -> Result<String, ReuseRefused> {
    match object.get(key) {
        Some(Value::String(value)) if !value.is_empty() => Ok(value.clone()),
        _ => Err(ReuseRefused(format!("{key} is not a string"))),
    }
}

fn json_sha(object: &Map<String, Value>, key: &str) -> Result<String, ReuseRefused> {
    let value = json_str(object, key)?;
    if is_sha(&value) {
        Ok(value)
    } else {
        Err(ReuseRefused(format!("{key} is not a commit sha")))
    }
}

fn parse_expires(text: &str) -> Result<Timestamp, ReuseRefused> {
    let normalized = text.replace('Z', "+00:00");
    match parse_iso(&normalized) {
        Ok(timestamp) if timestamp.aware => Ok(timestamp),
        Ok(_) => Err(ReuseRefused("artifact expiry is not timezone-aware".into())),
        Err(()) => Err(ReuseRefused("artifact expiry is not a timestamp".into())),
    }
}

fn parse_iso(text: &str) -> Result<Timestamp, ()> {
    let bytes = text.as_bytes();
    if bytes.len() < 19 {
        return Err(());
    }
    let year = i32::try_from(parse_digits(&bytes[0..4])?).map_err(|_| ())?;
    if bytes[4] != b'-' || bytes[7] != b'-' || (bytes[10] != b'T' && bytes[10] != b' ') {
        return Err(());
    }
    let month = parse_digits(&bytes[5..7])?;
    let day = parse_digits(&bytes[8..10])?;
    if bytes[13] != b':' || bytes[16] != b':' {
        return Err(());
    }
    let hour = parse_digits(&bytes[11..13])?;
    let minute = parse_digits(&bytes[14..16])?;
    let second = parse_digits(&bytes[17..19])?;
    let (micros, next) = parse_fraction(bytes, 19)?;
    if !valid_date(year, month, day) || hour > 23 || minute > 59 || second > 59 {
        return Err(());
    }
    let local = Timestamp::utc(year, month, day, hour, minute, second).utc_micros + micros;
    if next == bytes.len() {
        return Ok(Timestamp {
            utc_micros: local,
            aware: false,
        });
    }
    let offset = parse_offset(&bytes[next..])?;
    Ok(Timestamp {
        utc_micros: local - i128::from(offset) * 1_000_000,
        aware: true,
    })
}

fn parse_fraction(bytes: &[u8], start: usize) -> Result<(i128, usize), ()> {
    if start >= bytes.len() || bytes[start] != b'.' {
        return Ok((0, start));
    }
    let mut end = start + 1;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    let digits = &bytes[start + 1..end];
    if digits.is_empty() || digits.len() > 6 {
        return Err(());
    }
    let mut padded = [b'0'; 6];
    padded[..digits.len()].copy_from_slice(digits);
    let micros = parse_digits(&padded)? as i128;
    Ok((micros, end))
}

fn parse_offset(bytes: &[u8]) -> Result<i64, ()> {
    if bytes.is_empty() {
        return Err(());
    }
    let sign: i64 = match bytes[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return Err(()),
    };
    if bytes.len() < 6 || bytes[3] != b':' {
        return Err(());
    }
    let hour = i64::from(parse_digits(&bytes[1..3])?);
    let minute = i64::from(parse_digits(&bytes[4..6])?);
    let (second, consumed) = if bytes.len() == 6 {
        (0, 6)
    } else if bytes.len() == 9 && bytes[6] == b':' {
        (i64::from(parse_digits(&bytes[7..9])?), 9)
    } else {
        return Err(());
    };
    if consumed != bytes.len() || hour > 23 || minute > 59 || second > 59 {
        return Err(());
    }
    Ok(sign * (hour * 3600 + minute * 60 + second))
}

fn parse_digits(bytes: &[u8]) -> Result<u32, ()> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return Err(());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| ())?;
    text.parse().map_err(|_| ())
}

fn valid_date(year: i32, month: u32, day: u32) -> bool {
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&day)
}

fn days_from_civil(mut year: i32, month: u32, day: u32) -> i64 {
    if month <= 2 {
        year -= 1;
    }
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = u64::try_from(year - era * 400).unwrap_or(0);
    let month_adj = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * u64::from(month_adj) + 2) / 5 + u64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    i64::from(era) * 146097 + i64::try_from(day_of_era).unwrap_or(0) - 719468
}

fn emit(
    name: &str,
    value: &str,
    env: &HashMap<String, String>,
    stdout: &mut String,
) -> Result<(), String> {
    let line = format!("{name}={value}");
    match env
        .get("GITHUB_OUTPUT")
        .map(String::as_str)
        .filter(|path| !path.is_empty())
    {
        None => {
            stdout.push_str(&line);
            stdout.push('\n');
            Ok(())
        }
        Some(path) => {
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|error| error.to_string())?;
            writeln!(file, "{line}").map_err(|error| error.to_string())
        }
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn public_error(stderr: &str) -> String {
    let trimmed = stderr.trim();
    if trimmed.is_empty() {
        return "gh failed".to_string();
    }
    let line = trimmed.split(['\n', '\r']).next().unwrap_or("");
    const MARKERS: [&str; 4] = ["ghp_", "ghs_", "github_pat", "Bearer "];
    if MARKERS.iter().any(|marker| line.contains(marker)) {
        return "gh failed".to_string();
    }
    line.chars().take(200).collect()
}

fn env_get<'a>(env: &'a HashMap<String, String>, key: &str) -> &'a str {
    env.get(key).map(String::as_str).unwrap_or("")
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn entry_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

const SHA: &str = "5c5799909437f794c79f3de4ab29ec8954eceefc";
const ABSENT: &str = "Error: HTTP 404: Not Found (https://api.github.com/repos/Jabbslad/kiln/attestations/sha256:abc?per_page=30)\n";

fn fixed_now() -> Timestamp {
    Timestamp::utc(2026, 10, 20, 12, 0, 0)
}

fn sample_run(run_id: i64, sha: &str) -> WorkflowRun {
    WorkflowRun {
        run_id,
        head_sha: sha.to_string(),
        head_branch: "main".into(),
        event: "push".into(),
        path: WORKFLOW_PATH.into(),
        conclusion: "success".into(),
        status: "completed".into(),
    }
}

fn package_set(expired: bool, expires_at: Timestamp, names: &[&str]) -> Vec<PackageArtifact> {
    let mut names = names.to_vec();
    names.sort_unstable();
    names
        .into_iter()
        .map(|name| PackageArtifact {
            name: name.to_string(),
            expired,
            expires_at,
        })
        .collect()
}

fn release_tag() -> ReleaseVersion {
    release_version::parse_release_tag("v0.4.10").unwrap()
}

fn fill_archives(dist: &Path, version: &ReleaseVersion) {
    fs::create_dir_all(dist).unwrap();
    let tag = version.tag();
    for name in expected_archives(version) {
        let path = dist.join(&name);
        let stamped = name.starts_with("kiln-server-")
            || name.starts_with("kiln-identity-")
            || name.starts_with("kiln-dashboard-");
        let payload = format!(r#"{{"version": "{tag}", "target": "web"}}"#);
        if name.ends_with(".zip") {
            if stamped {
                write_zip(&path, "release.json", payload.as_bytes());
            } else {
                write_zip(&path, "kiln.exe", b"kiln");
            }
        } else if stamped {
            write_tar_gz(&path, "release.json", payload.as_bytes());
        } else {
            write_tar_gz(&path, "kiln", b"kiln");
        }
        let digest = sha256_hex(&path).unwrap();
        fs::write(
            dist.join(format!("{name}.sha256")),
            sidecar_line(&digest, &name),
        )
        .unwrap();
    }
}

fn write_zip(path: &Path, name: &str, data: &[u8]) {
    let file = File::create(path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    archive.start_file(name, options).unwrap();
    archive.write_all(data).unwrap();
    archive.finish().unwrap();
}

fn write_tar_gz(path: &Path, name: &str, data: &[u8]) {
    let file = File::create(path).unwrap();
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
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
    header.set_cksum();
    header.set_entry_type(tar::EntryType::Regular);
    archive.append(&header, data).unwrap();
    let encoder = archive.into_inner().unwrap();
    encoder.finish().unwrap();
}

fn test_selects_the_latest_successful_main_push() {
    let older = sample_run(3, SHA);
    let newer = sample_run(9, SHA);
    let mut dispatch = sample_run(4, SHA);
    dispatch.event = "workflow_dispatch".into();
    let chosen = select_run(&[older, newer, dispatch], SHA).unwrap();
    assert_eq!(chosen.run_id, 9);
}

fn test_refuses_a_run_that_is_not_this_workflow_on_main() {
    let base = sample_run(10, SHA);
    let cases = [
        WorkflowRun {
            head_sha: "0".repeat(40),
            ..base.clone()
        },
        WorkflowRun {
            head_branch: "feature".into(),
            ..base.clone()
        },
        WorkflowRun {
            event: "workflow_dispatch".into(),
            ..base.clone()
        },
        WorkflowRun {
            event: "pull_request".into(),
            ..base.clone()
        },
        WorkflowRun {
            path: ".github/workflows/deploy.yml".into(),
            ..base.clone()
        },
        WorkflowRun {
            conclusion: "failure".into(),
            ..base.clone()
        },
        WorkflowRun {
            status: "in_progress".into(),
            ..base
        },
    ];
    for run in cases {
        assert!(select_run(&[run], SHA).is_err());
    }
    assert!(select_run(&[], SHA).is_err());
    assert!(select_run(&[sample_run(10, SHA)], "not-a-sha").is_err());
}

fn test_refuses_missing_extra_or_expired_artifacts() {
    let now = fixed_now();
    let later = now.plus_days(30);
    require_artifacts(&package_set(false, later, PACKAGE_ARTIFACTS), now).unwrap();
    let missing: Vec<&str> = PACKAGE_ARTIFACTS
        .iter()
        .copied()
        .filter(|name| *name != "package-server")
        .collect();
    assert!(require_artifacts(&package_set(false, later, &missing), now).is_err());
    let mut extra: Vec<&str> = PACKAGE_ARTIFACTS.to_vec();
    extra.push("package-extra");
    assert!(require_artifacts(&package_set(false, later, &extra), now).is_err());
    assert!(require_artifacts(&package_set(true, later, PACKAGE_ARTIFACTS), now).is_err());
    assert!(require_artifacts(&package_set(false, now, PACKAGE_ARTIFACTS), now).is_err());
}

fn test_absent_attestation_is_allowed_only_before_the_workflow_attests() {
    let old = "uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4\n";
    let new = format!(
        "{old}        uses: {ATTEST_ACTION}e8998f949152b193b063cb0ec769d69d929409be # v2.4.0\n"
    );
    let absent = ["absent"; 7];
    let verified = ["verified"; 7];
    assert_eq!(attestation_gate(old, &absent).unwrap(), "not-produced");
    assert_eq!(attestation_gate(&new, &verified).unwrap(), "verified");
    assert!(attestation_gate(&new, &absent).is_err());
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
    let version = release_tag();
    let tmp = tempfile::tempdir().unwrap();
    let dist = tmp.path();
    fill_archives(dist, &version);
    let archive = dist.join("kiln-dashboard-v0.4.10.tar.gz");
    let before = fs::read(&archive).unwrap();
    let windows = dist.join("kiln-v0.4.10-x86_64-pc-windows-msvc.zip.sha256");
    let text = fs::read_to_string(&windows).unwrap().replace('\n', "\r\n");
    fs::write(&windows, text).unwrap();
    verify_and_refresh_sidecars(dist, &version).unwrap();
    assert_eq!(fs::read(&archive).unwrap(), before);
    let refreshed = fs::read(&windows).unwrap();
    assert!(refreshed.ends_with(b"\n"));
    assert!(!refreshed.ends_with(b"\r\n"));
    let sidecar = dist.join("kiln-dashboard-v0.4.10.tar.gz.sha256");
    fs::write(
        &sidecar,
        format!("{}  kiln-dashboard-v0.4.10.tar.gz\n", "0".repeat(64)),
    )
    .unwrap();
    match verify_and_refresh_sidecars(dist, &version) {
        Err(FetchError::Refused(_)) => {}
        other => panic!("expected refusal, got {other:?}"),
    }
}

fn test_finish_writes_the_sixteen_asset_release_set() {
    let version = release_tag();
    let tmp = tempfile::tempdir().unwrap();
    let dist = tmp.path();
    fill_archives(dist, &version);
    finish_dist(&crate::repo_root(), dist, &version).unwrap();
    let names: BTreeSet<String> = fs::read_dir(dist)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, expected_release_files(&version));
    assert_eq!(fs::read_dir(dist).unwrap().count(), 16);
    let mut checksums: Vec<String> = names
        .iter()
        .filter(|name| name.ends_with(".sha256"))
        .cloned()
        .collect();
    checksums.sort();
    let completed = Command::new("sha256sum")
        .arg("-c")
        .args(&checksums)
        .current_dir(dist)
        .output()
        .unwrap();
    assert!(
        completed.status.success(),
        "{}",
        String::from_utf8_lossy(&completed.stderr)
    );
    let script = fs::read_to_string(dist.join("install.sh")).unwrap();
    assert!(script.contains("version=0.4.10"), "{script}");
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
    assert!(parse_artifacts(&page, fixed_now()).is_err());
}

fn test_attest_signs_the_archives_a_tag_publishes() {
    let text = fs::read_to_string(crate::repo_root().join(".github/workflows/build.yml")).unwrap();
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
    assert!(expr.contains("startsWith(github.ref, 'refs/tags/v')"));
    assert!(attest_runs(&expr, "push", "refs/heads/main"));
    assert!(attest_runs(&expr, "push", "refs/tags/v0.4.11"));
    assert!(attest_runs(&expr, "workflow_dispatch", "refs/heads/main"));
    assert!(
        !attest_runs(&expr, "workflow_dispatch", "refs/heads/feature"),
        "a non-main dispatch must not attest"
    );
    assert!(!attest_runs(&expr, "push", "refs/heads/feature"));
    let docs = fs::read_to_string(crate::repo_root().join("docs/releases.md")).unwrap();
    let documented = regex::Regex::new(r"--cert-identity-regex '([^']+)'").unwrap();
    let found = documented.captures(&docs).expect("cert identity regex");
    let identity = regex::Regex::new(&found[1]).unwrap();
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
    assert!(text.contains("gh attestation verify"));
    assert!(text.contains("--cert-identity-regex"));
    assert!(text.contains("--source-ref"));
    assert!(text.contains("needs.attest.result == 'success'"));
    assert!(!text.contains("git push"));
    assert!(!text.contains("git tag"));
    let granted: Vec<&str> = text
        .lines()
        .filter(|line| line.trim() == "actions: write")
        .collect();
    assert!(granted.is_empty());
    assert_eq!(text.matches("retention-days: 30").count(), 4);
    assert!(
        text.contains("actions/attest-build-provenance@e8998f949152b193b063cb0ec769d69d929409be")
    );
    assert!(text.contains("dry_run:"));
    assert!(text.contains("needs.reuse.outputs.mode == 'fallback'"));
    let draft_if = job_if_expression(&text, "draft-release");
    assert!(
        draft_if
            .contains("(needs.reuse.result == 'success' && needs.reuse.outputs.mode == 'reuse')")
    );
    let step_only = "\
  other:
    if: needs.reuse.result == 'success' && needs.reuse.outputs.mode == 'reuse'
  draft-release:
    if: >-
      ((needs.reuse.outputs.mode == 'reuse') ||
        (needs.server-package.result == 'success'))
    steps:
      - if: needs.reuse.result == 'success' && needs.reuse.outputs.mode == 'reuse'
";
    assert!(
        !job_if_expression(step_only, "draft-release").contains("needs.reuse.result == 'success'")
    );
    let reuse_guard = "needs.reuse.result == 'success' && needs.reuse.outputs.mode == 'reuse'";
    assert!(step_if_expression(&text, "draft-release", "name: reused-dist").contains(reuse_guard));
    assert!(job_if_expression(&text, "dry-run-assets").contains(reuse_guard));
    let sibling = "\
  draft-release:
    steps:
      - uses: actions/download-artifact
        if: needs.reuse.outputs.mode == 'reuse'
        with:
          name: reused-dist
      - uses: actions/download-artifact
        if: needs.reuse.result == 'success' && needs.reuse.outputs.mode == 'reuse'
        with:
          name: package
";
    assert_eq!(
        step_if_expression(sibling, "draft-release", "name: reused-dist"),
        "needs.reuse.outputs.mode == 'reuse'"
    );
    assert_eq!(
        text.matches("!(needs.reuse.result == 'success' && needs.reuse.outputs.mode == 'reuse')")
            .count(),
        3
    );
    assert!(
        text.contains("needs.reuse.result != 'success' || needs.reuse.outputs.mode != 'reuse'")
    );
    assert_eq!(
        text.matches("!startsWith(github.ref, 'refs/tags/v')")
            .count(),
        6
    );
    assert_eq!(text.matches("id: queue").count(), 3);
    assert!(!text.contains("--auto"));
    assert_eq!(job_graph().as_slice(), EXPECTED_JOBS);
}

fn test_fetch_refusal_sets_fallback_without_publishing() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out");
    let dist = tmp.path().join("dist");
    let mut env = HashMap::new();
    env.insert("REUSE_EVENT".into(), "workflow_dispatch".into());
    env.insert("REUSE_SHA_INPUT".into(), "not-a-sha".into());
    env.insert("REUSE_VERSION".into(), String::new());
    env.insert("GH_REPO".into(), "Jabbslad/kiln".into());
    env.insert("GITHUB_OUTPUT".into(), out.display().to_string());
    let args = vec!["fetch".into(), "--dist".into(), dist.display().to_string()];
    let (code, _stdout, stderr) = run(&args, &env);
    assert_eq!(code, 0, "{stderr}");
    let output = fs::read_to_string(&out).unwrap();
    assert!(output.contains("mode=fallback"), "{output}");
    assert!(!output.contains("gh release"));
    assert!(!format!("{output}{stderr}").contains("ghp_"));
}

const EXPECTED_JOBS: &[(&str, bool, bool, &str)] = &[
    ("tag reuse", false, false, "reused"),
    ("tag no match", true, true, "packages"),
    ("tag error", true, true, "packages"),
    ("tag upload error", true, true, "packages"),
    ("tag timeout", true, true, "packages"),
    ("main", true, true, ""),
    ("code pr", false, true, ""),
    ("docs pr", false, false, ""),
    ("plain dispatch", true, true, ""),
    ("dry reuse", false, false, ""),
    ("dry no match", false, false, ""),
    ("dry error", false, false, ""),
    ("cancelled tag", false, false, ""),
];

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

fn draft_source(
    event: &str,
    tag: bool,
    reuse_result: &str,
    mode: &str,
    packages_ok: bool,
) -> &'static str {
    if event != "push" || !tag || reuse_result == "cancelled" {
        return "";
    }
    if reuse_result == "success" && mode == "reuse" {
        return "reused";
    }
    if packages_ok { "packages" } else { "" }
}

fn job_graph() -> Vec<(&'static str, bool, bool, &'static str)> {
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
            let draft = draft_source(event, tag, result, mode, packages_ok && server && clients);
            (name, server, clients, draft)
        })
        .collect()
}

fn job_body(text: &str, name: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let marker = format!("  {name}:");
    let start = lines
        .iter()
        .position(|line| *line == marker)
        .unwrap_or_else(|| panic!("missing job {name}"));
    let mut body = Vec::new();
    for line in &lines[start + 1..] {
        if next_job_line(line) {
            break;
        }
        body.push((*line).to_string());
    }
    body
}

fn next_job_line(line: &str) -> bool {
    let bytes = line.as_bytes();
    bytes.len() > 2 && bytes[0] == b' ' && bytes[1] == b' ' && bytes[2] != b' '
}

fn unfold_if(lines: &[String], index: usize, continued: &str) -> String {
    let folded = lines[index]
        .split_once("if:")
        .expect("if:")
        .1
        .trim()
        .to_string();
    if !matches!(folded.as_str(), ">-" | ">" | "|-" | "|") {
        return folded;
    }
    let mut parts = Vec::new();
    for line in &lines[index + 1..] {
        if !line.starts_with(continued) {
            break;
        }
        parts.push(line.trim());
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
    let mut steps: Vec<Vec<String>> = Vec::new();
    let mut current: Option<Vec<String>> = None;
    for line in body {
        if line.starts_with("      - ") {
            if let Some(step) = current.take() {
                steps.push(step);
            }
            current = Some(vec![line]);
            continue;
        }
        let Some(step) = current.as_mut() else {
            continue;
        };
        if line.starts_with("        ") || line.trim().is_empty() {
            step.push(line);
            continue;
        }
        steps.push(current.take().expect("step"));
    }
    if let Some(step) = current {
        steps.push(step);
    }
    let matches: Vec<Vec<String>> = steps
        .into_iter()
        .filter(|step| step.iter().any(|line| line.contains(marker)))
        .collect();
    if matches.len() != 1 {
        panic!("{job} has {} steps matching {marker}", matches.len());
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
    text = text.replace("github.event_name", &format!("'{event}'"));
    text = text.replace("github.ref", &format!("'{reference}'"));
    for job in ["server-package", "client", "dashboard"] {
        text = text.replace(&format!("needs.{job}.result"), "'success'");
    }
    text = text.replace("&&", " and ").replace("||", " or ");
    eval_bool(&text)
}

#[derive(PartialEq, Eq)]
enum BoolValue {
    Bool(bool),
    Str(String),
}

impl BoolValue {
    fn as_bool(&self) -> bool {
        match self {
            Self::Bool(value) => *value,
            Self::Str(text) => panic!("expected bool, got {text}"),
        }
    }
}

struct BoolParser<'a> {
    bytes: &'a [u8],
    index: usize,
}

fn eval_bool(text: &str) -> bool {
    let mut parser = BoolParser {
        bytes: text.as_bytes(),
        index: 0,
    };
    let value = parser.parse_or();
    parser.skip();
    if parser.index != parser.bytes.len() {
        panic!("leftover in {text:?}");
    }
    value.as_bool()
}

impl BoolParser<'_> {
    fn parse_or(&mut self) -> BoolValue {
        let mut value = self.parse_and();
        while self.consume_word("or") {
            let right = self.parse_and();
            value = BoolValue::Bool(value.as_bool() || right.as_bool());
        }
        value
    }

    fn parse_and(&mut self) -> BoolValue {
        let mut value = self.parse_cmp();
        while self.consume_word("and") {
            let right = self.parse_cmp();
            value = BoolValue::Bool(value.as_bool() && right.as_bool());
        }
        value
    }

    fn parse_cmp(&mut self) -> BoolValue {
        let left = self.parse_primary();
        self.skip();
        if self.rest().starts_with(b"==") {
            self.index += 2;
            let right = self.parse_primary();
            return BoolValue::Bool(left == right);
        }
        left
    }

    fn parse_primary(&mut self) -> BoolValue {
        self.skip();
        if self.consume_word("True") {
            return BoolValue::Bool(true);
        }
        if self.consume_word("False") {
            return BoolValue::Bool(false);
        }
        if self.peek() == Some(b'\'') {
            self.index += 1;
            let start = self.index;
            while self.index < self.bytes.len() && self.bytes[self.index] != b'\'' {
                self.index += 1;
            }
            let text = String::from_utf8(self.bytes[start..self.index].to_vec()).unwrap();
            self.index += 1;
            return BoolValue::Str(text);
        }
        if self.peek() == Some(b'(') {
            self.index += 1;
            let value = self.parse_or();
            self.skip();
            if self.peek() != Some(b')') {
                panic!("expected )");
            }
            self.index += 1;
            return value;
        }
        panic!(
            "bad token at {} in {:?}",
            self.index,
            String::from_utf8_lossy(self.bytes)
        );
    }

    fn consume_word(&mut self, word: &str) -> bool {
        self.skip();
        let bytes = word.as_bytes();
        if !self.rest().starts_with(bytes) {
            return false;
        }
        let end = self.index + bytes.len();
        if end < self.bytes.len() {
            let next = self.bytes[end];
            if next.is_ascii_alphanumeric() || next == b'_' {
                return false;
            }
        }
        self.index = end;
        true
    }

    fn skip(&mut self) {
        while self.peek().is_some_and(u8::is_ascii_whitespace) {
            self.index += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.index).copied()
    }

    fn rest(&self) -> &[u8] {
        &self.bytes[self.index..]
    }
}

register_cases! {
    test_selects_the_latest_successful_main_push,
    test_refuses_a_run_that_is_not_this_workflow_on_main,
    test_refuses_missing_extra_or_expired_artifacts,
    test_absent_attestation_is_allowed_only_before_the_workflow_attests,
    test_classifies_the_real_missing_attestation_error,
    test_checksum_mismatch_refuses_and_a_match_keeps_the_bytes,
    test_finish_writes_the_sixteen_asset_release_set,
    test_parser_rejects_a_paginated_artifact_list,
    test_attest_signs_the_archives_a_tag_publishes,
    test_workflow_keeps_drafts_and_does_not_move_tags,
    test_fetch_refusal_sets_fallback_without_publishing,
}

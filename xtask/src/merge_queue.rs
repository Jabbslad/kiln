use crate::register_cases;
use crate::repo_root;
use crate::required_gate::{self, Blocker};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::process::Command;

const WORKFLOW: &str = "build.yml";
const PAGE: u32 = 20;
const NEEDED: &[&str] = &[
    "changes",
    "docs-check",
    "feature-map-guard",
    "server-check",
    "client",
    "dashboard",
];
const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TREE_OTHER: &str = "cccccccccccccccccccccccccccccccccccccccc";

pub fn should_skip(
    event_name: &str,
    reference: &str,
    head_tree: &str,
    tested_trees: &HashSet<String>,
) -> bool {
    event_name == "push"
        && reference == "refs/heads/main"
        && sha_ok(head_tree)
        && tested_trees.contains(head_tree)
}

pub fn head_tree(cwd: Option<&Path>) -> Result<String, String> {
    let mut command = Command::new("git");
    command.args(["rev-parse", "HEAD^{tree}"]);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command
        .output()
        .map_err(|error| format!("git rev-parse failed to start: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn tested_trees<F>(
    repo: &str,
    token: &str,
    api: &str,
    fetch: &mut F,
) -> Result<HashSet<String>, String>
where
    F: FnMut(&str, &str) -> Result<Value, String>,
{
    if !repo_ok(repo) {
        return Err("GITHUB_REPOSITORY is not owner/name".into());
    }
    if token.is_empty() {
        return Err("GITHUB_TOKEN is missing".into());
    }
    let base = api.trim_end_matches('/');
    let listed = fetch(
        &format!(
            "{base}/repos/{repo}/actions/workflows/{WORKFLOW}/runs?event=merge_group&status=success&per_page={PAGE}"
        ),
        token,
    )?;
    let runs = listed
        .get("workflow_runs")
        .and_then(Value::as_array)
        .ok_or_else(|| "workflow run list has no workflow_runs".to_string())?;
    let mut trees = HashSet::new();
    for run in runs {
        let Some(run) = run.as_object() else {
            continue;
        };
        if run.get("event").and_then(Value::as_str) != Some("merge_group")
            || run.get("conclusion").and_then(Value::as_str) != Some("success")
        {
            continue;
        }
        let Some(sha) = run.get("head_sha").and_then(Value::as_str) else {
            continue;
        };
        if !sha_ok(sha) {
            continue;
        }
        let commit = fetch(&format!("{base}/repos/{repo}/commits/{sha}"), token)?;
        if let Some(tree) = commit_tree(&commit) {
            trees.insert(tree);
        }
    }
    Ok(trees)
}

pub fn decide<F>(
    env: &HashMap<String, String>,
    fetch: &mut F,
    cwd: Option<&Path>,
) -> Result<bool, String>
where
    F: FnMut(&str, &str) -> Result<Value, String>,
{
    let event_name = env
        .get("GITHUB_EVENT_NAME")
        .map(String::as_str)
        .unwrap_or("");
    let reference = env.get("GITHUB_REF").map(String::as_str).unwrap_or("");
    if event_name != "push" || reference != "refs/heads/main" {
        return Ok(false);
    }
    let trees = tested_trees(
        env.get("GITHUB_REPOSITORY")
            .map(String::as_str)
            .unwrap_or(""),
        env.get("GITHUB_TOKEN").map(String::as_str).unwrap_or(""),
        env.get("GITHUB_API_URL")
            .map(String::as_str)
            .unwrap_or("https://api.github.com"),
        fetch,
    )?;
    Ok(should_skip(event_name, reference, &head_tree(cwd)?, &trees))
}

fn github_fetch(url: &str, token: &str) -> Result<Value, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("Authorization", format!("Bearer {token}"))
        .header("User-Agent", "kiln-merge-queue-tested")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .map_err(|error| error.to_string())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("GitHub API {} for {url}", status.as_u16()));
    }
    let payload: Value = response
        .json()
        .map_err(|error| format!("GitHub API returned a non-object: {error}"))?;
    if !payload.is_object() {
        return Err("GitHub API returned a non-object".into());
    }
    Ok(payload)
}

pub fn run(env: &HashMap<String, String>, cwd: Option<&Path>) -> (i32, String, String) {
    match decide(env, &mut github_fetch, cwd) {
        Ok(skip) => {
            write_output(skip, env.get("GITHUB_OUTPUT"));
            (
                0,
                format!("skip_tests={}\n", bool_word(skip)),
                String::new(),
            )
        }
        Err(error) => {
            write_output(false, env.get("GITHUB_OUTPUT"));
            (
                0,
                "skip_tests=false\n".to_string(),
                format!("merge queue lookup failed, running tests: {error}\n"),
            )
        }
    }
}

pub fn cli(_args: &[String]) -> i32 {
    let env = std::env::vars().collect();
    let (code, stdout, stderr) = run(&env, None);
    print!("{stdout}");
    eprint!("{stderr}");
    code
}

fn write_output(skip: bool, path: Option<&String>) {
    let Some(path) = path.filter(|path| !path.is_empty()) else {
        return;
    };
    if let Ok(mut handle) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(handle, "skip_tests={}", bool_word(skip));
    }
}

fn bool_word(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

fn commit_tree(commit: &Value) -> Option<String> {
    let sha = commit.pointer("/commit/tree/sha")?.as_str()?;
    sha_ok(sha).then(|| sha.to_string())
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
    !name.contains('/') && ident(owner) && ident(name)
}

fn ident(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

fn test_same_tree_on_a_new_commit_skips_main_tests() {
    let repo = tempfile::tempdir().unwrap();
    let (first, second, tree) = make_repo(repo.path());
    assert_ne!(first, second);
    assert_eq!(
        git(repo.path(), &["rev-parse", &format!("{first}^{{tree}}")]).trim(),
        tree
    );
    let mut fetch = runs(&tree, "merge_group", "success", SHA_A);
    assert!(decide(&main_push(), &mut fetch, Some(repo.path())).unwrap());
}

fn test_a_different_tree_still_runs_tests() {
    let repo = tempfile::tempdir().unwrap();
    make_repo(repo.path());
    let mut fetch = runs(TREE_OTHER, "merge_group", "success", SHA_A);
    assert!(!decide(&main_push(), &mut fetch, Some(repo.path())).unwrap());
}

fn test_pull_request_tag_and_dispatch_ignore_a_matching_tree() {
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let repo = tempfile::tempdir().unwrap();
    make_repo(repo.path());
    for (event, reference) in [
        ("pull_request", "refs/pull/1/merge"),
        ("push", "refs/tags/v1.2.3"),
        ("workflow_dispatch", "refs/heads/main"),
        ("merge_group", "refs/heads/gh-readonly-queue/main/pr-1"),
    ] {
        let seen = std::sync::Arc::clone(&calls);
        let mut fetch = move |url: &str, _token: &str| {
            seen.lock().unwrap().push(url.to_string());
            Err(url.to_string())
        };
        let mut env = HashMap::new();
        env.insert("GITHUB_EVENT_NAME".into(), event.into());
        env.insert("GITHUB_REF".into(), reference.into());
        assert!(!decide(&env, &mut fetch, Some(repo.path())).unwrap());
    }
    assert!(calls.lock().unwrap().is_empty());
}

fn test_failed_or_non_queue_runs_do_not_count() {
    let repo = tempfile::tempdir().unwrap();
    let (_first, _second, tree) = make_repo(repo.path());
    for (event, conclusion) in [
        ("merge_group", "failure"),
        ("pull_request", "success"),
        ("push", "success"),
    ] {
        let mut fetch = runs(&tree, event, conclusion, SHA_A);
        assert!(!decide(&main_push(), &mut fetch, Some(repo.path())).unwrap());
    }
}

fn test_lookup_failure_runs_tests_and_exits_zero() {
    let repo = tempfile::tempdir().unwrap();
    make_repo(repo.path());
    let output = repo.path().join("output");
    let mut env = main_push();
    env.insert("GITHUB_OUTPUT".into(), output.display().to_string());
    env.insert("GITHUB_API_URL".into(), "https://127.0.0.1:9".into());
    let (code, stdout, stderr) = run(&env, Some(repo.path()));
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("skip_tests=false"));
    assert_eq!(fs::read_to_string(&output).unwrap(), "skip_tests=false\n");
    assert!(!stderr.contains("test-token"));
}

fn test_pull_request_process_does_not_need_a_token() {
    let output = tempfile::NamedTempFile::new().unwrap();
    let mut env = HashMap::new();
    env.insert("GITHUB_EVENT_NAME".into(), "pull_request".into());
    env.insert("GITHUB_REF".into(), "refs/pull/8/merge".into());
    env.insert("GITHUB_OUTPUT".into(), output.path().display().to_string());
    let (code, _stdout, stderr) = run(&env, Some(&repo_root()));
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        fs::read_to_string(output.path()).unwrap(),
        "skip_tests=false\n"
    );
}

fn test_queue_runs_checks_and_only_push_or_dispatch_packages() {
    let text = fs::read_to_string(repo_root().join(".github/workflows/build.yml")).unwrap();
    assert!(text.contains("merge_group:"));
    assert!(text.contains("types: [checks_requested]"));
    assert!(text.contains("branches: [main]"));
    assert!(!text.contains("if: github.event_name != 'pull_request'\n"));
    assert!(!text.contains("if: github.event_name != 'pull_request' &&"));
    assert!(
        !text
            .contains("github.event_name == 'push' || github.event_name == 'workflow_dispatch' &&")
    );
    assert!(text.contains(
        "(github.event_name == 'push' || github.event_name == 'workflow_dispatch') && matrix.target == 'aarch64-apple-darwin'"
    ));
    assert!(text.contains(
        "if: github.event_name == 'push' || github.event_name == 'workflow_dispatch'\n        run: cargo xtask test-release\n"
    ));
    assert!(!text.contains("windows-2025"));
    assert!(!text.contains("x86_64-pc-windows-msvc"));
    assert!(!text.contains("runner.os == 'Windows'"));
    assert!(!text.contains("runner.os != 'Windows'"));
    let package = text
        .split("      - name: Package client\n")
        .nth(1)
        .unwrap()
        .split("      - uses:")
        .next()
        .unwrap();
    assert!(
        package.contains(
            "if: github.event_name == 'push' || github.event_name == 'workflow_dispatch'\n"
        )
    );
    assert!(!package.contains("test-release"));
    assert!(package.contains("cargo xtask release client"));
    assert!(text.contains("github.event_name == 'push' && github.ref == 'refs/heads/main'"));
    assert!(!text.contains("github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')"));
    assert!(text.contains("github.event.merge_group.base_sha"));
    assert!(text.contains("github.event.merge_group.head_sha"));
    assert!(text.contains("github.event_name != 'merge_group'"));
    assert_eq!(text.matches("id: queue").count(), 3);
    assert_eq!(text.matches("cargo xtask merge-queue-tested").count(), 3);
    assert!(text.contains("cargo xtask test-merge-queue-tested"));
    for name in ["deploy.yml", "diagnose.yml"] {
        let other = fs::read_to_string(repo_root().join(".github/workflows").join(name)).unwrap();
        assert!(!other.contains("merge_group"), "{name}");
        assert!(!other.contains("pull_request"), "{name}");
    }
}

fn test_required_is_the_only_stable_check() {
    let text = fs::read_to_string(repo_root().join(".github/workflows/build.yml")).unwrap();
    let block = text
        .split("  required:\n")
        .nth(1)
        .unwrap()
        .split("\n  draft-release:")
        .next()
        .unwrap();
    let needs = format!("needs: [{}]", NEEDED.join(", "));
    assert!(block.contains(&needs));
    assert!(block.contains("if: always()"));
    assert!(block.contains("NEEDS_JSON: ${{ toJSON(needs) }}"));
    assert!(block.contains("cargo xtask required-gate"));
    assert!(!block.lines().any(|line| line.starts_with("    name:")));
    assert!(!block.contains("server-package"));
    assert!(text.split("  draft-release:\n").nth(1).unwrap().contains(
        "needs: [version-guard, release-plan, server-check, server-package, client, dashboard, attest]"
    ));
}

fn test_docs_only_and_heavy_success_pass() {
    let docs = job_needs(
        "success", "success", "success", "skipped", "skipped", "skipped", "false",
    );
    let heavy = job_needs(
        "success", "skipped", "success", "success", "success", "success", "true",
    );
    assert!(required_gate::evaluate(&docs).ok());
    assert!(required_gate::evaluate(&docs).blockers.is_empty());
    assert!(required_gate::evaluate(&heavy).ok());
}

fn test_failure_and_cancelled_block_each_job() {
    for job_id in NEEDED {
        for result in ["failure", "cancelled"] {
            let mut payload = job_needs(
                "success", "skipped", "success", "success", "success", "success", "true",
            );
            payload[job_id]["result"] = Value::String(result.into());
            let decision = required_gate::evaluate(&payload);
            assert!(!decision.ok());
            assert_eq!(
                decision
                    .blockers
                    .iter()
                    .map(|item| (item.job_id.as_str(), item.result.as_str()))
                    .collect::<Vec<_>>(),
                vec![(*job_id, result)]
            );
        }
    }
}

fn test_unknown_result_blocks_and_outputs_are_ignored() {
    let mut payload = job_needs(
        "success", "skipped", "success", "success", "success", "success", "true",
    );
    payload["server-check"]["outputs"] = serde_json::json!({"note": "failure"});
    assert!(required_gate::evaluate(&payload).ok());
    payload["client"]["result"] = Value::String("SUCCESS".into());
    let decision = required_gate::evaluate(&payload);
    assert_eq!(
        decision.blockers,
        vec![Blocker {
            job_id: "client".into(),
            result: "SUCCESS".into(),
        }]
    );
}

fn test_malformed_needs_block() {
    assert_eq!(
        required_gate::evaluate(&Value::Array(vec![])).blockers[0].result,
        "malformed"
    );
    assert_eq!(
        required_gate::evaluate(&serde_json::json!({})).blockers[0].result,
        "empty"
    );
    assert_eq!(
        required_gate::evaluate(&serde_json::json!({"client": "failure"})).blockers,
        vec![Blocker {
            job_id: "client".into(),
            result: "malformed".into(),
        }]
    );
}

fn test_process_exits_with_the_blocker() {
    let payload = job_needs(
        "success",
        "skipped",
        "success",
        "success",
        "failure",
        "cancelled",
        "true",
    );
    let (code, stdout) = required_gate::run_gate(Some(&payload.to_string()));
    assert_eq!(code, 1);
    assert_eq!(stdout, "client failure\ndashboard cancelled\n");
    let (code, stdout) = required_gate::run_gate(None);
    assert_eq!(code, 1);
    assert_eq!(stdout, "needs malformed\n");
}

fn job_needs(
    changes: &str,
    docs_check: &str,
    feature_map_guard: &str,
    server_check: &str,
    client: &str,
    dashboard: &str,
    heavy: &str,
) -> Value {
    serde_json::json!({
        "changes": {"result": changes, "outputs": {"heavy": heavy}},
        "docs-check": {"result": docs_check, "outputs": {}},
        "feature-map-guard": {"result": feature_map_guard, "outputs": {}},
        "server-check": {"result": server_check, "outputs": {}},
        "client": {"result": client, "outputs": {}},
        "dashboard": {"result": dashboard, "outputs": {}},
    })
}

fn main_push() -> HashMap<String, String> {
    HashMap::from([
        ("GITHUB_EVENT_NAME".into(), "push".into()),
        ("GITHUB_REF".into(), "refs/heads/main".into()),
        ("GITHUB_REPOSITORY".into(), "Jabbslad/kiln".into()),
        ("GITHUB_TOKEN".into(), "test-token".into()),
        ("GITHUB_API_URL".into(), "https://api.github.com".into()),
    ])
}

fn runs(
    tree: &str,
    event: &'static str,
    conclusion: &'static str,
    sha: &'static str,
) -> impl FnMut(&str, &str) -> Result<Value, String> {
    let tree = tree.to_string();
    move |url: &str, token: &str| {
        assert_eq!(token, "test-token");
        if url.ends_with(&format!("/commits/{sha}")) {
            Ok(serde_json::json!({"commit": {"tree": {"sha": tree}}}))
        } else {
            Ok(serde_json::json!({
                "workflow_runs": [{"event": event, "conclusion": conclusion, "head_sha": sha}]
            }))
        }
    }
}

fn make_repo(repo: &Path) -> (String, String, String) {
    git(repo, &["init", "-q"]);
    git(repo, &["config", "user.email", "test@example.com"]);
    git(repo, &["config", "user.name", "Test"]);
    fs::write(repo.join("README"), "kiln\n").unwrap();
    git(repo, &["add", "README"]);
    git(repo, &["commit", "-q", "-m", "first"]);
    let first = git(repo, &["rev-parse", "HEAD"]);
    git(repo, &["commit", "-q", "--allow-empty", "-m", "second"]);
    let second = git(repo, &["rev-parse", "HEAD"]);
    let tree = git(repo, &["rev-parse", "HEAD^{tree}"]);
    (
        first.trim().into(),
        second.trim().into(),
        tree.trim().into(),
    )
}

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap_or_else(|error| panic!("git {args:?}: {error}"));
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

register_cases! {
    test_same_tree_on_a_new_commit_skips_main_tests,
    test_a_different_tree_still_runs_tests,
    test_pull_request_tag_and_dispatch_ignore_a_matching_tree,
    test_failed_or_non_queue_runs_do_not_count,
    test_lookup_failure_runs_tests_and_exits_zero,
    test_pull_request_process_does_not_need_a_token,
    test_queue_runs_checks_and_only_push_or_dispatch_packages,
    test_required_is_the_only_stable_check,
    test_docs_only_and_heavy_success_pass,
    test_failure_and_cancelled_block_each_job,
    test_unknown_result_blocks_and_outputs_are_ignored,
    test_malformed_needs_block,
    test_process_exits_with_the_blocker,
}

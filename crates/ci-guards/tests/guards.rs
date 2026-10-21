use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(name: &str) -> String {
    fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join(name),
    )
    .unwrap_or_else(|error| panic!("read {name}: {error}"))
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn workflows(root: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(root.join(".github/workflows"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("yml"))
        .collect();
    paths.sort();
    paths
}

fn shell_files(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for dir in ["scripts", "images", "deploy"] {
        walk_sh(&root.join(dir), &mut paths);
    }
    paths.sort();
    paths
}

fn walk_sh(dir: &Path, paths: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            walk_sh(&path, paths);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("sh") {
            paths.push(path);
        }
    }
}

#[test]
fn attest_if_allows_only_main_push_main_dispatch_and_version_tags() {
    let workflow = read(&repo_root().join(".github/workflows/build.yml"));
    let violations = ci_guards::attest_violations(&workflow);
    assert!(violations.is_empty(), "{violations:?}");

    let violations = ci_guards::attest_violations(&fixture("attest-allows-pull-request.yml"));
    assert!(
        violations
            .iter()
            .any(|violation| violation.contains("pull_request")),
        "{violations:?}"
    );
}

#[test]
fn secrets_stay_in_env_and_out_of_run_scripts() {
    let root = repo_root();
    for path in workflows(&root) {
        let violations = ci_guards::secret_violations(&read(&path));
        assert!(violations.is_empty(), "{} {violations:?}", path.display());
    }

    let mapped = "\
jobs:
  keep:
    steps:
      - env:
          GITHUB_TOKEN: ${{ github.token }}
        run: echo \"$GITHUB_TOKEN\"
";
    assert!(ci_guards::secret_violations(mapped).is_empty());

    let violations = ci_guards::secret_violations(&fixture("secret-in-run.yml"));
    assert!(
        violations
            .iter()
            .any(|violation| violation.contains("secrets.DARK_FORGE_SSH_KEY")),
        "{violations:?}"
    );
}

#[test]
fn downloaded_tools_are_checksummed_before_use() {
    let root = repo_root();
    for path in workflows(&root) {
        for script in ci_guards::run_scripts(&read(&path)) {
            let violations = ci_guards::download_violations(&script);
            assert!(violations.is_empty(), "{} {violations:?}", path.display());
        }
    }
    for path in shell_files(&root) {
        let violations = ci_guards::download_violations(&read(&path));
        assert!(violations.is_empty(), "{} {violations:?}", path.display());
    }

    let violations = ci_guards::download_violations(&fixture("unverified-download.sh"));
    assert!(
        violations
            .iter()
            .any(|violation| violation.contains("used before a checksum")),
        "{violations:?}"
    );
}

fn tag_guard(tag: &str, cargo: &str) -> std::process::Output {
    Command::new("sh")
        .arg(repo_root().join("scripts/check-tag-version.sh"))
        .arg(tag)
        .arg(cargo)
        .output()
        .unwrap_or_else(|error| panic!("run check-tag-version.sh: {error}"))
}

#[test]
fn a_tag_that_differs_from_cargo_toml_cannot_publish() {
    let mismatch = tag_guard("v0.4.14", "0.4.10");
    let mismatch_err = String::from_utf8_lossy(&mismatch.stderr);
    assert!(
        !mismatch.status.success(),
        "v0.4.14 against Cargo.toml 0.4.10 must be rejected: {mismatch_err}"
    );
    assert!(mismatch_err.contains("0.4.10"), "{mismatch_err}");

    let matched = tag_guard("v0.4.15", "0.4.15");
    assert!(
        matched.status.success(),
        "a matching tag and Cargo.toml version must be accepted: {}",
        String::from_utf8_lossy(&matched.stderr)
    );

    let root = repo_root();
    let workflow = read(&root.join(".github/workflows/build.yml"));
    let violations = ci_guards::version_guard_violations(&workflow);
    assert!(violations.is_empty(), "{violations:?}");

    let manifest = read(&root.join("Cargo.toml"));
    assert!(
        manifest.contains("version = \"0.4.15\""),
        "workspace version must be 0.4.15 so a v0.4.15 tag reuses main's archives"
    );

    let missing = "\
jobs:
  draft-release:
    needs: [reuse]
    if: startsWith(github.ref, 'refs/tags/v')
    steps:
      - run: echo publish
";
    let missing_hits = ci_guards::version_guard_violations(missing);
    assert!(
        missing_hits
            .iter()
            .any(|violation| violation.contains("does not depend on version-guard")),
        "{missing_hits:?}"
    );
}

#[test]
fn published_v0_4_14_linux_binary_fails_the_tag_version_check() {
    let root = repo_root();
    let dir = std::env::temp_dir().join(format!("kiln-v0414-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap_or_else(|error| panic!("mkdir {}: {error}", dir.display()));
    let archive = dir.join("kiln.tar.gz");
    let url = "https://github.com/Jabbslad/kiln/releases/download/v0.4.14/kiln-v0.4.14-x86_64-unknown-linux-gnu.tar.gz";
    let download = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--output",
        ])
        .arg(&archive)
        .arg(url)
        .status()
        .unwrap_or_else(|error| panic!("curl: {error}"));
    assert!(download.success(), "download {url}");
    let extract = Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&dir)
        .arg("kiln")
        .status()
        .unwrap_or_else(|error| panic!("tar: {error}"));
    assert!(
        extract.success(),
        "extract kiln from the v0.4.14 Linux archive"
    );
    let binary = dir.join("kiln");
    let output = Command::new("python3")
        .arg(root.join("scripts/release_version.py"))
        .args(["check-binary", "--version", "v0.4.14", "--"])
        .arg(&binary)
        .output()
        .unwrap_or_else(|error| panic!("check-binary: {error}"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("v0.4.14 check-binary: {stderr}");
    assert!(
        !output.status.success(),
        "published v0.4.14 kiln must fail check-binary for v0.4.14: {stderr}"
    );
    assert!(
        stderr.contains("0.4.10"),
        "published binary should report kiln 0.4.10, got {stderr}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn reuse_publish_does_not_need_server_check_and_rebuild_does() {
    let workflow = read(&repo_root().join(".github/workflows/build.yml"));
    let violations = ci_guards::publish_server_check_violations(&workflow);
    assert!(violations.is_empty(), "{violations:?}");
    assert!(
        workflow.contains("sh scripts/reuse-server-check.sh"),
        "reuse must prove main's server-check before mode stays reuse"
    );
    assert!(
        workflow.contains("server_check: ${{ steps.decide.outputs.server_check }}"),
        "publish reads the server-check proof from the reuse job"
    );

    let stuck =
        ci_guards::publish_server_check_violations(&fixture("publish-always-server-check.yml"));
    assert!(
        stuck
            .iter()
            .any(|violation| violation.contains("reuse path: publish is false")),
        "{stuck:?}"
    );

    let open = ci_guards::publish_server_check_violations(&fixture(
        "publish-skips-server-check-on-rebuild.yml",
    ));
    assert!(
        open.iter()
            .any(|violation| { violation.contains("rebuild path: publish is true") }),
        "{open:?}"
    );
}

#[test]
fn reuse_looks_up_main_archives_by_the_committed_version() {
    let workflow = read(&repo_root().join(".github/workflows/build.yml"));
    let violations = ci_guards::reuse_lookup_violations(&workflow);
    assert!(violations.is_empty(), "{violations:?}");

    let violations = ci_guards::reuse_lookup_violations(&fixture("reuse-uses-tag-version.yml"));
    assert!(
        violations
            .iter()
            .any(|violation| violation.contains("git tag")),
        "{violations:?}"
    );
    assert!(
        violations
            .iter()
            .any(|violation| violation.contains("empty reuse_sha")),
        "{violations:?}"
    );
    assert!(
        violations
            .iter()
            .any(|violation| violation.contains("draft-release finish")),
        "{violations:?}"
    );
}

#[test]
fn tag_push_deploys_from_build_without_a_release_event_or_a_main_push() {
    let root = repo_root();
    let build = read(&root.join(".github/workflows/build.yml"));
    let caller = ci_guards::deploy_caller_violations(&build);
    assert!(caller.is_empty(), "{caller:?}");
    // A reusable workflow sees the caller event. A tag push is `push`, not
    // `workflow_call`, so the deploy gate has to allow that event on a stable tag.
    let guard = read(&root.join("deploy/github/guard.sh"));
    assert!(
        guard.contains("deploy:release|deploy:workflow_call|deploy:push"),
        "{guard}"
    );

    for path in workflows(&root) {
        let text = read(&path);
        let release = ci_guards::release_event_violations(&text);
        assert!(release.is_empty(), "{} {release:?}", path.display());
        let pushes = ci_guards::main_push_violations(&text);
        assert!(pushes.is_empty(), "{} {pushes:?}", path.display());
    }

    let caller = ci_guards::deploy_caller_violations(&fixture("deploy-without-needs.yml"));
    assert!(
        caller
            .iter()
            .any(|violation| violation.contains("without needing draft-release")),
        "{caller:?}"
    );

    let release = ci_guards::release_event_violations(&fixture("release-published.yml"));
    assert!(
        release
            .iter()
            .any(|violation| violation.contains("release")),
        "{release:?}"
    );

    let pushes = ci_guards::main_push_violations(&fixture("push-to-main.yml"));
    assert!(
        pushes
            .iter()
            .filter(|violation| violation.contains("pushes to main"))
            .count()
            >= 2,
        "{pushes:?}"
    );
    assert!(
        pushes
            .iter()
            .any(|violation| violation.contains("HEAD:main")),
        "{pushes:?}"
    );
    assert!(
        pushes
            .iter()
            .any(|violation| violation.contains("contents API")),
        "{pushes:?}"
    );

    let plain_main = "\
jobs:
  pin:
    steps:
      - run: git push origin main
";
    let plain_hits = ci_guards::main_push_violations(plain_main);
    assert!(
        plain_hits
            .iter()
            .any(|violation| violation.contains("pushes to main")),
        "{plain_hits:?}"
    );

    let other_branch = "\
jobs:
  pin:
    steps:
      - run: git push origin feature:next
      - run: gh api --method PUT /repos/Jabbslad/kiln/contents/file -f branch=feature
";
    let other_hits = ci_guards::main_push_violations(other_branch);
    assert!(other_hits.is_empty(), "{other_hits:?}");

    let wired = "\
jobs:
  deploy:
    needs: [draft-release]
    if: needs.draft-release.result == 'success'
    permissions:
      contents: read
    uses: ./.github/workflows/deploy.yml
    with:
      tag: ${{ github.ref_name }}
";
    assert!(ci_guards::deploy_caller_violations(wired).is_empty());
}

#[test]
fn downstream_of_a_reuse_skipped_job_guards_its_if() {
    let planted =
        ci_guards::reuse_skip_downstream_violations(&fixture("reuse-skip-downstream.yml"));
    let mut flagged: Vec<&str> = planted
        .iter()
        .filter_map(|violation| violation.split_once(' ').map(|(name, _)| name))
        .collect();
    flagged.sort();
    flagged.dedup();
    assert_eq!(flagged, ["deploy", "publish", "success-fn"], "{planted:?}");
    assert!(
        planted.iter().any(|violation| {
            violation.starts_with("deploy ") && violation.contains("!cancelled()")
        }),
        "{planted:?}"
    );
    assert!(
        planted
            .iter()
            .any(|violation| violation.starts_with("publish ")
                && violation.contains("needs.package.result")),
        "{planted:?}"
    );
    assert!(
        planted
            .iter()
            .any(|violation| violation.starts_with("success-fn ") && violation.contains("always()")),
        "{planted:?}"
    );

    let workflow = read(&repo_root().join(".github/workflows/build.yml"));
    let violations = ci_guards::reuse_skip_downstream_violations(&workflow);
    assert!(violations.is_empty(), "{violations:?}");
}

#[test]
fn dark_forge_jobs_declare_the_environment_and_callers_pass_secrets() {
    let root = repo_root();
    let files = workflow_texts(&root);
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(name, text)| (name.as_str(), text.as_str()))
        .collect();
    let live = ci_guards::dark_forge_secret_violations(&borrowed);
    assert!(live.is_empty(), "{live:?}");

    let called = fixture("dark-forge-called.yml");
    let caller = fixture("dark-forge-caller-omits-secrets.yml");
    let omitted = ci_guards::dark_forge_secret_violations(&[
        ("deploy.yml", called.as_str()),
        ("build.yml", caller.as_str()),
    ]);
    assert!(
        omitted
            .iter()
            .any(|violation| violation.contains("without passing secrets")),
        "{omitted:?}"
    );
    assert!(
        omitted
            .iter()
            .all(|violation| !violation.contains("without environment")),
        "{omitted:?}"
    );

    let bare = fixture("dark-forge-without-environment.yml");
    let unscoped = ci_guards::dark_forge_secret_violations(&[("deploy.yml", bare.as_str())]);
    assert!(
        unscoped.iter().any(|violation| {
            violation.contains("reads DARK_FORGE_SSH_KEY without environment: dark-forge")
        }),
        "{unscoped:?}"
    );

    let inherited = "\
jobs:
  deploy:
    needs: [draft-release]
    if: >-
      !cancelled() &&
      needs.draft-release.result == 'success'
    uses: ./.github/workflows/deploy.yml
    secrets: inherit
";
    let inherited_hits = ci_guards::dark_forge_secret_violations(&[
        ("deploy.yml", called.as_str()),
        ("build.yml", inherited),
    ]);
    assert!(inherited_hits.is_empty(), "{inherited_hits:?}");

    let named = "\
jobs:
  deploy:
    uses: ./.github/workflows/deploy.yml
    secrets:
      DARK_FORGE_SSH_KEY: ${{ secrets.DARK_FORGE_SSH_KEY }}
      DARK_FORGE_KNOWN_HOSTS: ${{ secrets.DARK_FORGE_KNOWN_HOSTS }}
";
    let named_hits = ci_guards::dark_forge_secret_violations(&[
        ("deploy.yml", called.as_str()),
        ("build.yml", named),
    ]);
    assert!(named_hits.is_empty(), "{named_hits:?}");

    let partial = "\
jobs:
  deploy:
    uses: ./.github/workflows/deploy.yml
    secrets:
      DARK_FORGE_SSH_KEY: ${{ secrets.DARK_FORGE_SSH_KEY }}
";
    let partial_hits = ci_guards::dark_forge_secret_violations(&[
        ("deploy.yml", called.as_str()),
        ("build.yml", partial),
    ]);
    assert!(
        partial_hits
            .iter()
            .any(|violation| { violation.contains("without passing DARK_FORGE_KNOWN_HOSTS") }),
        "{partial_hits:?}"
    );

    let missing = "\
jobs:
  deploy:
    uses: ./.github/workflows/deploy.yml
";
    let missing_hits = ci_guards::dark_forge_secret_violations(&[("build.yml", missing)]);
    assert!(
        missing_hits
            .iter()
            .any(|violation| violation.contains("was not provided")),
        "{missing_hits:?}"
    );

    let build = read(&root.join(".github/workflows/build.yml"));
    let deploy_job = build
        .split_once("  deploy:\n")
        .and_then(|(_, rest)| rest.split_once("\n  dry-run-assets:"))
        .map(|(job, _)| job)
        .expect("deploy job");
    assert!(deploy_job.contains("!cancelled()"), "{deploy_job}");
    assert!(
        deploy_job.contains("needs.draft-release.result == 'success'"),
        "{deploy_job}"
    );
    assert!(deploy_job.contains("secrets: inherit"), "{deploy_job}");
    assert!(!deploy_job.contains("environment:"), "{deploy_job}");
    assert!(!deploy_job.contains("secrets.DARK_FORGE_"), "{deploy_job}");
}

#[test]
fn unpublished_tag_is_refused_before_install() {
    let root = repo_root();
    let deploy = read(&root.join(".github/workflows/deploy.yml"));
    let require_at = deploy
        .find("bash deploy/github/require-release.sh")
        .expect("require step");
    let install_at = deploy
        .find("bash deploy/github/deploy.sh")
        .expect("install step");
    assert!(require_at < install_at, "{deploy}");
    assert!(deploy.contains("workflow_dispatch:"), "{deploy}");
    assert!(deploy.contains("default: latest"), "{deploy}");
    assert!(!deploy.contains("pull_request"), "{deploy}");

    let script = read(&root.join("deploy/github/require-release.sh"));
    assert!(script.contains("guard_github_event deploy"), "{script}");
    assert!(script.contains("repo=Jabbslad/kiln"), "{script}");
    assert!(script.contains("releases/tags/"), "{script}");
    assert!(script.contains("releases/latest"), "{script}");

    let fork = run_require(
        &root,
        "v0.4.15",
        "200",
        r#"{"draft":false}"#,
        0,
        &[
            ("GITHUB_EVENT_NAME", "workflow_dispatch"),
            ("GITHUB_REPOSITORY", "example/kiln"),
            ("GITHUB_REF", "refs/heads/main"),
        ],
    );
    assert!(!fork.status.success(), "{fork:?}");
    assert!(
        String::from_utf8_lossy(&fork.stderr).contains("refusing a fork"),
        "{fork:?}"
    );
    assert!(
        fork.log.is_empty(),
        "fork must not call the release API: {fork:?}"
    );

    let missing = run_require(
        &root,
        "v9.9.9",
        "404",
        r#"{"message":"Not Found"}"#,
        0,
        &[
            ("GITHUB_EVENT_NAME", "workflow_dispatch"),
            ("GITHUB_REPOSITORY", "Jabbslad/kiln"),
            ("GITHUB_REF", "refs/heads/main"),
        ],
    );
    assert!(!missing.status.success(), "{missing:?}");
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("no published release for v9.9.9"),
        "{missing:?}"
    );

    let draft = run_require(
        &root,
        "v0.4.15",
        "200",
        r#"{"tag_name":"v0.4.15","draft": true,"prerelease":false}"#,
        0,
        &[
            ("GITHUB_EVENT_NAME", "workflow_dispatch"),
            ("GITHUB_REPOSITORY", "Jabbslad/kiln"),
            ("GITHUB_REF", "refs/heads/main"),
            ("GH_TOKEN", "token-must-stay-out-of-the-log"),
        ],
    );
    assert!(!draft.status.success(), "{draft:?}");
    assert!(
        String::from_utf8_lossy(&draft.stderr).contains("no published release for v0.4.15"),
        "{draft:?}"
    );
    let shown = format!(
        "{}{}",
        String::from_utf8_lossy(&draft.stdout),
        String::from_utf8_lossy(&draft.stderr)
    );
    assert!(!shown.contains("token-must-stay-out-of-the-log"), "{shown}");
    assert!(
        draft
            .log
            .contains("Authorization: Bearer token-must-stay-out-of-the-log"),
        "{draft:?}"
    );

    let published = run_require(
        &root,
        "v0.4.15",
        "200",
        r#"{"tag_name":"v0.4.15","draft": false,"prerelease":false}"#,
        0,
        &[
            ("GITHUB_EVENT_NAME", "workflow_dispatch"),
            ("GITHUB_REPOSITORY", "Jabbslad/kiln"),
            ("GITHUB_REF", "refs/heads/main"),
        ],
    );
    assert!(published.status.success(), "{published:?}");
    assert!(
        published
            .log
            .contains("/repos/Jabbslad/kiln/releases/tags/v0.4.15"),
        "{published:?}"
    );

    let latest = run_require(
        &root,
        "latest",
        "200",
        r#"{"tag_name":"v0.4.15","draft":false}"#,
        0,
        &[
            ("GITHUB_EVENT_NAME", "workflow_call"),
            ("GITHUB_REPOSITORY", "Jabbslad/kiln"),
            ("GITHUB_REF", "refs/tags/v0.4.15"),
        ],
    );
    assert!(latest.status.success(), "{latest:?}");
    assert!(
        latest.log.contains("/repos/Jabbslad/kiln/releases/latest"),
        "{latest:?}"
    );

    let invalid = run_require(
        &root,
        "not-a-tag",
        "200",
        r#"{"draft":false}"#,
        0,
        &[
            ("GITHUB_EVENT_NAME", "workflow_dispatch"),
            ("GITHUB_REPOSITORY", "Jabbslad/kiln"),
            ("GITHUB_REF", "refs/heads/main"),
        ],
    );
    assert!(!invalid.status.success(), "{invalid:?}");
    assert!(
        String::from_utf8_lossy(&invalid.stderr).contains("invalid release tag"),
        "{invalid:?}"
    );
    assert!(invalid.log.is_empty(), "{invalid:?}");
}

#[derive(Debug)]
struct RequireRun {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    log: String,
}

fn run_require(
    root: &Path,
    tag: &str,
    code: &str,
    body: &str,
    curl_exit: i32,
    env: &[(&str, &str)],
) -> RequireRun {
    let dir = std::env::temp_dir().join(format!(
        "kiln-require-{}-{}",
        std::process::id(),
        tag.replace('.', "-")
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap_or_else(|error| panic!("mkdir {}: {error}", dir.display()));
    let curl = dir.join("curl");
    let log = dir.join("curl.log");
    fs::write(
        &curl,
        "#!/bin/bash\nprintf '%s\\n' \"$*\" >> \"$CURL_LOG\"\nif [ \"${CURL_EXIT:-0}\" != 0 ]; then exit \"$CURL_EXIT\"; fi\nprintf '%s\\n%s' \"$CURL_BODY\" \"$CURL_CODE\"\n",
    )
    .unwrap();
    let mut perms = fs::metadata(&curl).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&curl, perms).unwrap();
    let mut command = Command::new("bash");
    command
        .arg(root.join("deploy/github/require-release.sh"))
        .env_clear()
        .env("PATH", format!("{}:/usr/bin:/bin", dir.display()))
        .env("HOME", dir.as_os_str())
        .env("TAG", tag)
        .env("CURL_LOG", &log)
        .env("CURL_CODE", code)
        .env("CURL_BODY", body)
        .env("CURL_EXIT", curl_exit.to_string());
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("require-release: {error}"));
    let recorded = fs::read_to_string(&log).unwrap_or_default();
    let _ = fs::remove_dir_all(&dir);
    RequireRun {
        status: output.status,
        stdout: output.stdout,
        stderr: output.stderr,
        log: recorded,
    }
}

fn workflow_texts(root: &Path) -> Vec<(String, String)> {
    workflows(root)
        .into_iter()
        .map(|path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("workflow.yml")
                .to_string();
            (name, read(&path))
        })
        .collect()
}

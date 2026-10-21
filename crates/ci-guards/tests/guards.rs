use std::fs;
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

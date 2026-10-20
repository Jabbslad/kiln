use std::fs;
use std::path::{Path, PathBuf};

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

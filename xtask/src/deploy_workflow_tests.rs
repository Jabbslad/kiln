use crate::register_cases;
use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

const CREATE_JSON: &str = concat!(
    r#"{"id":"op","box_id":"0123456789abcdef0123456789abcdef","state":"succeeded","#,
    r#""result":{"type":"box","id":"0123456789abcdef0123456789abcdef","name":"kiln-smoke","#,
    r#""state":"running","memory_mib":4096,"vcpus":1},"error":null}"#
);

fn test_workflows_have_no_pull_request_trigger() {
    for name in ["deploy.yml", "diagnose.yml"] {
        let text = workflow(name);
        assert!(!text.contains("pull_request"), "{name}");
        assert!(!text.contains("accept-new"), "{name}");
        assert!(!text.contains("ssh-keyscan"), "{name}");
    }
    for name in ["deploy.yml", "diagnose.yml"] {
        let text = workflow(name);
        assert!(text.contains("environment: dark-forge"), "{name}");
        assert!(text.contains("secrets.DARK_FORGE_SSH_KEY"), "{name}");
        assert!(text.contains("secrets.DARK_FORGE_KNOWN_HOSTS"), "{name}");
    }
    let caller = workflow("build.yml");
    assert!(!caller.contains("secrets.DARK_FORGE_SSH_KEY"));
    assert!(!caller.contains("secrets.DARK_FORGE_KNOWN_HOSTS"));
    assert!(!caller.contains("environment:"));
    assert!(caller.contains("uses: ./.github/workflows/deploy.yml"));
}

fn test_deploy_is_manual_from_main() {
    let text = workflow("deploy.yml");
    assert!(text.contains("workflow_dispatch:"));
    assert!(text.contains("workflow_call:"));
    assert!(!text.contains("\n  release:"));
    assert!(!text.contains("types: [published]"));
    assert!(text.contains("group: dark-forge-deploy"));
    assert!(text.contains("cancel-in-progress: false"));
    assert!(text.contains("default: latest"));
    assert_eq!(count(&text, "secrets.DARK_FORGE_SSH_KEY"), 1);
    assert!(text.contains("name: Install the release\n"));
    let caller = workflow("build.yml");
    assert!(!caller.contains("types: [published]"));
    assert!(caller.contains("tag: ${{ needs.release-plan.outputs.tag }}"));
    let docs = read_repo("docs/releases.md");
    assert!(docs.contains("Publishing a release runs"));
    assert!(!docs.contains("Publishing a release does not start it."));
    assert!(docs.contains("Leave required reviewers"));
}

fn test_diagnose_is_manual_and_read_only() {
    let text = workflow("diagnose.yml");
    assert!(text.contains("workflow_dispatch:"));
    assert!(!text.contains("release:"));
    assert!(!text.contains("inputs:"));
    assert!(text.contains("group: dark-forge-diagnose"));
    let remote = read_repo("deploy/github/diagnose.sh");
    for forbidden in [
        "apt-get",
        "systemctl restart",
        "systemctl enable",
        "install.py",
        "rm ",
        "eval ",
    ] {
        assert!(!remote.contains(forbidden), "{forbidden}");
    }
    let ssh = read_repo("deploy/github/ssh.sh");
    assert!(ssh.contains("StrictHostKeyChecking=yes"));
    assert!(ssh.contains("ControlMaster=auto"));
    assert!(ssh.contains("ControlPath"));
    assert!(!ssh.contains("accept-new"));
    assert!(!ssh.contains("ssh-keyscan"));
}

fn test_guard_refuses_pull_requests_forks_and_branch_dispatch() {
    let refused = run_guard(
        "deploy",
        &[
            ("GITHUB_EVENT_NAME", "pull_request"),
            ("GITHUB_REPOSITORY", "Jabbslad/kiln"),
            ("GITHUB_REF", "refs/heads/main"),
        ],
    );
    assert!(!refused.status.success(), "{}", refused.stderr);
    assert!(
        refused.stderr.contains("refusing event"),
        "{}",
        refused.stderr
    );
    let target = run_guard(
        "deploy",
        &[
            ("GITHUB_EVENT_NAME", "pull_request_target"),
            ("GITHUB_REPOSITORY", "Jabbslad/kiln"),
            ("GITHUB_REF", "refs/heads/main"),
        ],
    );
    assert!(!target.status.success(), "{}", target.stderr);
    let fork = run_guard(
        "diagnose",
        &[
            ("GITHUB_EVENT_NAME", "workflow_dispatch"),
            ("GITHUB_REPOSITORY", "example/kiln"),
            ("GITHUB_REF", "refs/heads/main"),
        ],
    );
    assert!(!fork.status.success(), "{}", fork.stderr);
    assert!(fork.stderr.contains("refusing a fork"), "{}", fork.stderr);
    let branch = run_guard(
        "deploy",
        &[
            ("GITHUB_EVENT_NAME", "workflow_dispatch"),
            ("GITHUB_REPOSITORY", "Jabbslad/kiln"),
            ("GITHUB_REF", "refs/heads/feature"),
        ],
    );
    assert!(!branch.status.success(), "{}", branch.stderr);
    assert!(
        branch.stderr.contains("only from main"),
        "{}",
        branch.stderr
    );
}

fn test_guard_allows_main_dispatch_and_a_tag_release() {
    let dispatch = run_guard(
        "deploy",
        &[
            ("GITHUB_EVENT_NAME", "workflow_dispatch"),
            ("GITHUB_REPOSITORY", "Jabbslad/kiln"),
            ("GITHUB_REF", "refs/heads/main"),
        ],
    );
    assert!(dispatch.status.success(), "{}", dispatch.stderr);
    let release = run_guard(
        "deploy",
        &[
            ("GITHUB_EVENT_NAME", "release"),
            ("GITHUB_REPOSITORY", "Jabbslad/kiln"),
            ("GITHUB_REF", "refs/tags/v0.4.12"),
        ],
    );
    assert!(release.status.success(), "{}", release.stderr);
    let called = run_guard(
        "deploy",
        &[
            ("GITHUB_EVENT_NAME", "workflow_call"),
            ("GITHUB_REPOSITORY", "Jabbslad/kiln"),
            ("GITHUB_REF", "refs/tags/v0.4.12"),
        ],
    );
    assert!(called.status.success(), "{}", called.stderr);
    let main_push = run_guard(
        "deploy",
        &[
            ("GITHUB_EVENT_NAME", "push"),
            ("GITHUB_REPOSITORY", "Jabbslad/kiln"),
            ("GITHUB_REF", "refs/heads/main"),
            ("KILN_RELEASE_TAG", "v0.4.20"),
        ],
    );
    assert!(main_push.status.success(), "{}", main_push.stderr);
    let main_call = run_guard(
        "deploy",
        &[
            ("GITHUB_EVENT_NAME", "workflow_call"),
            ("GITHUB_REPOSITORY", "Jabbslad/kiln"),
            ("GITHUB_REF", "refs/heads/main"),
            ("TAG", "v0.4.20"),
        ],
    );
    assert!(main_call.status.success(), "{}", main_call.stderr);
    let bare_main = run_guard(
        "deploy",
        &[
            ("GITHUB_EVENT_NAME", "push"),
            ("GITHUB_REPOSITORY", "Jabbslad/kiln"),
            ("GITHUB_REF", "refs/heads/main"),
        ],
    );
    assert!(!bare_main.status.success(), "{}", bare_main.stderr);
    assert!(
        bare_main.stderr.contains("stable tag"),
        "{}",
        bare_main.stderr
    );
    let branch = run_guard(
        "deploy",
        &[
            ("GITHUB_EVENT_NAME", "release"),
            ("GITHUB_REPOSITORY", "Jabbslad/kiln"),
            ("GITHUB_REF", "refs/heads/main"),
        ],
    );
    assert!(!branch.status.success(), "{}", branch.stderr);
    assert!(branch.stderr.contains("stable tag"), "{}", branch.stderr);
    let diagnose_release = run_guard(
        "diagnose",
        &[
            ("GITHUB_EVENT_NAME", "release"),
            ("GITHUB_REPOSITORY", "Jabbslad/kiln"),
            ("GITHUB_REF", "refs/tags/v0.4.4"),
        ],
    );
    assert!(
        !diagnose_release.status.success(),
        "{}",
        diagnose_release.stderr
    );
    assert!(
        diagnose_release.stderr.contains("refusing event release"),
        "{}",
        diagnose_release.stderr
    );
    let local = run_guard("deploy", &[]);
    assert!(local.status.success(), "{}", local.stderr);
}

fn test_secrets_are_read_only_by_the_dark_forge_environment() {
    for name in ["deploy.yml", "diagnose.yml"] {
        let text = workflow(name);
        assert_eq!(count(&text, "environment: dark-forge"), 1, "{name}");
        assert!(
            index_of(&text, "environment: dark-forge")
                < index_of(&text, "secrets.DARK_FORGE_SSH_KEY"),
            "{name}"
        );
        assert!(
            index_of(&text, "environment: dark-forge")
                < index_of(&text, "secrets.DARK_FORGE_KNOWN_HOSTS"),
            "{name}"
        );
        assert_eq!(count(&text, "jobs:"), 1, "{name}");
    }
    let docs = read_repo("docs/releases.md");
    assert!(docs.contains("environment secrets"));
    assert!(docs.contains("Jamie"));
    assert!(docs.contains("deployment branches to `main`"));
    assert!(docs.contains("deployment tags to `v*`"));
    assert!(!docs.contains("Leave the key out of the environment"));
    assert!(!docs.to_lowercase().contains("repository secret"));
}

fn test_older_tags_and_prereleases_are_refused_before_ssh() {
    for tag in ["v0.4.3", "v0.4.4-rc.1", "v0.5.0-rc.1"] {
        let result = run_deploy(tag, "[]", None, false);
        assert!(
            !result.captured.status.success(),
            "{}",
            result.captured.stderr
        );
        assert!(
            result.captured.stderr.contains(&format!("refusing {tag}")),
            "{}",
            result.captured.stderr
        );
        assert!(
            result
                .captured
                .stderr
                .contains("v0.4.4 and newer stable releases"),
            "{}",
            result.captured.stderr
        );
        assert!(result.calls.is_empty(), "{}", result.captured.stderr);
        assert!(result.ssh_calls.is_empty(), "{}", result.captured.stderr);
    }
    let latest = run_deploy(
        "latest",
        r#"[{"tag_name":"v0.4.3","draft":false}]"#,
        None,
        false,
    );
    assert!(
        !latest.captured.status.success(),
        "{}",
        latest.captured.stderr
    );
    assert!(
        latest.captured.stderr.contains("refusing v0.4.3"),
        "{}",
        latest.captured.stderr
    );
    assert!(
        latest
            .calls
            .iter()
            .any(|call| call.contains("api.github.com")),
        "{:?}",
        latest.calls
    );
    assert!(
        !latest
            .calls
            .iter()
            .any(|call| call.contains("releases/download")),
        "{:?}",
        latest.calls
    );
    assert!(latest.ssh_calls.is_empty(), "{}", latest.captured.stderr);
}

fn test_deploy_ssh_and_scp_run_while_multiplex_master_is_open() {
    let result = run_deploy("v0.4.5", "[]", Some("printf 'archive-v045\\n'\n"), false);
    assert!(
        result.captured.status.success(),
        "{}{}",
        result.captured.stdout,
        result.captured.stderr
    );
    assert!(
        result.captured.stdout.contains("deployed v0.4.5"),
        "{}",
        result.captured.stdout
    );
    assert!(
        result.captured.stdout.contains("ssh session opened\n"),
        "{}",
        result.captured.stdout
    );
    assert!(
        result.captured.stdout.contains("ssh session closed\n"),
        "{}",
        result.captured.stdout
    );
    let remote_rm = result
        .ssh_calls
        .iter()
        .position(|call| call.contains(" rm ") && call.contains("rm -rf"));
    let master_close = result
        .ssh_calls
        .iter()
        .position(|call| call.contains("-O exit"));
    assert!(
        remote_rm.is_some(),
        "missing remote staging cleanup: {:?}",
        result.ssh_calls
    );
    assert!(
        master_close.is_some(),
        "missing master close: {:?}",
        result.ssh_calls
    );
    assert!(
        remote_rm < master_close,
        "remote cleanup must run before master close: {:?}",
        result.ssh_calls
    );
}

fn test_v044_uses_this_tree_and_later_tags_use_the_archive() {
    let tree = repo().join("deploy/install.sh").display().to_string();
    let current = run_deploy("v0.4.4", "[]", Some("printf 'archive\\n'\n"), false);
    assert!(
        current.captured.status.success(),
        "{}",
        current.captured.stderr
    );
    assert!(current.captured.stdout.contains("deployed v0.4.4"));
    assert!(
        current.ssh_calls.iter().any(|call| call.contains(&tree)),
        "{:?}",
        current.ssh_calls
    );
    assert!(
        current
            .ssh_calls
            .iter()
            .any(|call| call.starts_with("scp ") && call.ends_with("installer.sh")),
        "{:?}",
        current.ssh_calls
    );
    assert!(
        current
            .calls
            .iter()
            .any(|call| call.contains("kiln-server-v0.4.4-")),
        "{:?}",
        current.calls
    );

    let later = run_deploy("v0.4.5", "[]", Some("printf 'archive-v045\\n'\n"), false);
    assert!(later.captured.status.success(), "{}", later.captured.stderr);
    assert!(later.captured.stdout.contains("deployed v0.4.5"));
    assert!(
        !later.ssh_calls.iter().any(|call| {
            call.contains("install.py")
                || call.contains("installer.py")
                || call.contains("installer.sh")
        }),
        "{:?}",
        later.ssh_calls
    );
    assert!(
        later
            .ssh_calls
            .iter()
            .any(|call| call.contains("kiln-server-v0.4.5-") && call.starts_with("scp ")),
        "{:?}",
        later.ssh_calls
    );

    let numeric = run_deploy("v0.4.10", "[]", Some("printf 'archive-v0410\\n'\n"), false);
    assert!(
        numeric.captured.status.success(),
        "{}",
        numeric.captured.stderr
    );
    assert!(
        !numeric
            .ssh_calls
            .iter()
            .any(|call| call.contains("installer.sh")),
        "{:?}",
        numeric.ssh_calls
    );

    for tag in ["v0.4.4", "v0.4.5"] {
        let failed = run_deploy(tag, "[]", Some("printf 'must-not-run\\n'\n"), true);
        assert!(
            !failed.captured.status.success(),
            "{}{}",
            failed.captured.stdout,
            failed.captured.stderr
        );
        assert!(
            failed.captured.stderr.contains("NOT match"),
            "{}",
            failed.captured.stderr
        );
        assert!(failed.ssh_calls.is_empty(), "{}", failed.captured.stderr);
    }

    let tree_run = run_remote(
        "printf 'installer=archive\\n'\n",
        Some("printf 'installer=tree\\n'\n"),
        false,
    );
    assert!(
        tree_run.stdout.contains("installer=tree"),
        "{}",
        tree_run.stdout
    );
    assert!(
        !tree_run.stdout.contains("installer=archive"),
        "{}",
        tree_run.stdout
    );
    let archive_run = run_remote("printf 'installer=archive\\n'\n", None, false);
    assert!(
        archive_run.stdout.contains("installer=archive"),
        "{}",
        archive_run.stdout
    );
    assert!(
        !archive_run.stdout.contains("installer=tree"),
        "{}",
        archive_run.stdout
    );
    let mismatch = run_remote(
        "printf 'installer=archive\\n'\n",
        Some("printf 'installer=tree\\n'\n"),
        true,
    );
    assert!(
        !mismatch.stdout.contains("installer="),
        "{}",
        mismatch.stdout
    );
    assert!(mismatch.stderr.contains("NOT match"), "{}", mismatch.stderr);
}

fn test_apt_installs_missing_packages_without_upgrading() {
    let script = read_repo("deploy/github/install-remote.sh");
    assert!(script.contains("NEEDRESTART_MODE=l"));
    assert!(script.contains("DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l apt-get update"));
    assert!(script.contains(
        "DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l apt-get install -y --no-install-recommends --no-upgrade sqlite3"
    ));
    assert!(!script.contains("apt-get upgrade"));
    assert!(!script.contains("dist-upgrade"));
    assert!(script.contains("apt-get install --no-install-recommends --no-upgrade -y"));
    let pending = Command::new("apt-get")
        .args(["-s", "upgrade"])
        .output()
        .unwrap_or_else(|error| panic!("apt-get: {error}"));
    let pending_out = String::from_utf8_lossy(&pending.stdout);
    let name = pending_out
        .lines()
        .find(|line| line.starts_with("Inst "))
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("openssl")
        .to_string();
    let plain = Command::new("apt-get")
        .args(["install", "-s", "--no-install-recommends", &name])
        .output()
        .unwrap_or_else(|error| panic!("apt-get: {error}"));
    let held = Command::new("apt-get")
        .args([
            "install",
            "-s",
            "--no-install-recommends",
            "--no-upgrade",
            &name,
        ])
        .output()
        .unwrap_or_else(|error| panic!("apt-get: {error}"));
    let held_out = String::from_utf8_lossy(&held.stdout);
    let held_err = String::from_utf8_lossy(&held.stderr);
    assert!(held.status.success(), "{held_err}");
    assert!(!held_out.contains(&format!("Inst {name} ")), "{held_out}");
    assert!(held_out.contains(&format!("Skipping {name}")), "{held_out}");
    let plain_out = String::from_utf8_lossy(&plain.stdout);
    assert!(plain_out.contains(&name), "{plain_out}");
}

fn test_sqlite3_install_is_exact_and_skipped_when_present() {
    let update = "DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l apt-get update";
    let exact = "DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l apt-get install -y --no-install-recommends --no-upgrade sqlite3";
    let script = read_repo("deploy/github/install-remote.sh");
    assert_eq!(count(&script, exact), 1);
    assert!(!script.contains("apt-get upgrade"));
    assert!(!script.contains("dist-upgrade"));
    for line in script.lines() {
        if line.contains("sqlite3") && line.contains("apt-get") {
            assert!(line.contains(exact), "{line}");
            assert!(!line.contains("openssl"), "{line}");
        }
    }
    let (present, _, _) = apt_lines(false, false, "");
    assert!(!present.iter().any(|line| line == exact));
    assert!(!upgrade_lines(&present));
    let (missing, _, _) = apt_lines(true, false, "");
    assert_eq!(count_eq(&missing, exact), 1);
    let update_at = position(&missing, update);
    let exact_at = position(&missing, exact);
    assert!(update_at < exact_at);
    assert_eq!(missing[exact_at - 1], update);
    assert!(!upgrade_lines(&missing));
    let (existing_present, present_out, present_err) = apt_lines(false, true, "");
    assert!(existing_present.is_empty(), "{present_err}");
    assert!(present_out.contains("installer-ran"), "{present_out}");
    let (existing_missing, missing_out, missing_err) = apt_lines(true, true, "");
    assert_eq!(
        existing_missing,
        vec![update.to_string(), exact.to_string()],
        "{missing_out}{missing_err}"
    );
    assert!(missing_out.contains("installer-ran"), "{missing_out}");
    assert!(!upgrade_lines(&existing_missing));
    let (failed, failed_out, failed_err) = apt_lines(true, true, "update");
    assert_eq!(failed, vec![update.to_string()], "{failed_out}{failed_err}");
    assert!(!failed_out.contains("installer-ran"), "{failed_out}");
}

fn test_deploy_installs_dashboard_and_checks_the_live_bundle() {
    let deploy = read_repo("deploy/github/deploy.sh");
    let remote = read_repo("deploy/github/install-remote.sh");
    assert!(deploy.contains("kiln-dashboard-"));
    assert!(deploy.contains("check-dashboard-bundle.sh"));
    assert!(remote.contains("--dashboard-archive"));
    assert!(remote.contains("check-dashboard-bundle.sh"));
    assert!(remote.contains("kiln-api self-check"));
    assert!(deploy.contains("check-installed-versions.sh"));
    assert!(remote.contains("check-installed-versions.sh"));
    assert!(remote.contains("dashboard inputs"));
    let metrics_at = index_of(&remote, "kiln-api self-check");
    let smoke_at = index_of(&remote, "bash \"$dir/smoke.sh\"");
    assert!(
        metrics_at < smoke_at,
        "metrics must run right after restart, before guest smoke"
    );
    assert!(
        remote.contains("[ \"$metrics_rc\" -eq 0 ]")
            && index_of(&remote, "[ \"$metrics_rc\" -eq 0 ]") < smoke_at,
        "smoke must wait for host metrics"
    );
    assert!(
        !deploy.contains("check-dashboard-metrics.sh"),
        "deploy must not ship shell JSON metrics scripts"
    );
}

fn test_deploy_host_scripts_do_not_invoke_jq() {
    let dir = crate::repo_root().join("deploy/github");
    for entry in fs::read_dir(&dir).expect("deploy/github") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("sh") {
            continue;
        }
        let name = path
            .file_name()
            .expect("script name")
            .to_string_lossy()
            .into_owned();
        let text = fs::read_to_string(&path).unwrap_or_else(|err| panic!("{name}: {err}"));
        for line in text.lines() {
            assert!(
                !line.contains(" jq ") && !line.contains("|jq") && !line.contains("command -v jq"),
                "{name}: {line}"
            );
        }
        assert!(
            !text.contains("check-dashboard-metrics.sh"),
            "{name} still references check-dashboard-metrics.sh"
        );
    }
}

fn test_install_door_caddy_validates_staging_and_skips_unchanged_bytes() {
    let script = read_repo("deploy/github/install-door-caddy.sh");
    let deploy = read_repo("deploy/github/deploy.sh");
    let remote = read_repo("deploy/github/install-remote.sh");
    assert!(deploy.contains("install-door-caddy.sh"));
    assert!(remote.contains("install-door-caddy.sh"));
    assert!(remote.contains("deploy/door.Caddyfile"));
    assert!(remote.contains("\"$door_caddy_rc\" -eq 0"));
    assert!(script.contains("caddy validate"));
    assert!(script.contains("caddy reload"));
    assert!(!script.contains("compose up"));
    assert!(!script.contains("restart caddy"));

    let root = std::env::temp_dir().join(format!("kiln-door-caddy-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("bin")).unwrap();
    let live = root.join("Caddyfile");
    let door = root.join("door.Caddyfile");
    let ca = root.join("ca.crt");
    let backup = root.join("Caddyfile.bak");
    let staging = root.join("Caddyfile.new");
    let log = root.join("docker.log");
    let base_site = read_repo("deploy/door.Caddyfile");
    let legacy = base_site.replace("versions 1.1\n", "");
    let prefix = "dark-forge.dev {\n\trespond \"login ok\" 200\n}\n\n";
    fs::write(&live, format!("{prefix}{legacy}")).unwrap();
    fs::write(&door, &base_site).unwrap();
    fs::write(&ca, read_repo("deploy/host.example.json")).unwrap();
    write_executable(
        &root.join("bin/docker"),
        &format!(
            r#"#!/bin/bash
printf '%s\n' "$*" >> "{log}"
case "$1" in
  inspect)
    if [ "$2" = '-f' ] && [ "$3" = '{{{{.State.Running}}}}' ]; then
      printf 'true\n'
      exit 0
    fi
    if [ "$2" = '-f' ] && [ "$3" = '{{{{.Config.Image}}}}' ]; then
      printf 'caddy:test\n'
      exit 0
    fi
    ;;
  run)
    config=
    for arg in "$@"; do
      case "$arg" in
        type=bind,source=*,target=/etc/caddy/Caddyfile,*)
          config=${{arg#type=bind,source=}}
          config=${{config%%,target=/etc/caddy/Caddyfile*}}
          ;;
      esac
    done
    if printf '%s\n' "$*" | grep -q 'caddy validate'; then
      if grep -q 'not_a_directive' "$config"; then
        printf 'invalid config\n' >&2
        exit 17
      fi
      printf 'Valid configuration\n'
      exit 0
    fi
    exit 99
    ;;
  exec)
    if printf '%s\n' "$*" | grep -q 'caddy reload'; then
      printf 'reloaded\n'
      exit 0
    fi
    exit 98
    ;;
esac
exit 97
"#,
            log = log.display()
        ),
    );
    let run = |door_site: &Path| {
        let mut command = Command::new("bash");
        command.arg(repo().join("deploy/github/install-door-caddy.sh"));
        command.arg(door_site);
        command.env("PATH", prepend_path(&root.join("bin")));
        command.env("KILN_PASEO_CADDYFILE", &live);
        command.env("KILN_PASEO_CADDYFILE_BAK", &backup);
        command.env("KILN_PASEO_CADDYFILE_NEW", &staging);
        command.env("KILN_KILN_CA_CERT", &ca);
        command.env("KILN_DOCKER", root.join("bin/docker"));
        capture(&mut command)
    };

    let before = fs::read(&live).unwrap();
    let ok = run(&door);
    assert!(ok.status.success(), "{}{}", ok.stderr, ok.stdout);
    assert!(ok.stdout.contains("door caddy installed and reloaded"));
    let logged = fs::read_to_string(&log).unwrap();
    assert!(logged.contains("validate"));
    assert!(logged.contains("reload"));
    assert!(String::from_utf8_lossy(&fs::read(&live).unwrap()).contains("versions 1.1"));
    assert_ne!(fs::read(&live).unwrap(), before);

    let log2 = root.join("docker2.log");
    fs::write(&log, "").unwrap();
    fs::write(&log2, "").unwrap();
    let unchanged = run(&door);
    assert!(
        unchanged.status.success(),
        "{}{}",
        unchanged.stderr,
        unchanged.stdout
    );
    assert!(unchanged.stdout.contains("door caddy unchanged"));
    let logged2 = fs::read_to_string(&log).unwrap();
    assert!(!logged2.contains("validate"), "{logged2}");
    assert!(!logged2.contains("reload"), "{logged2}");

    let bad = root.join("bad.Caddyfile");
    fs::write(&bad, "cli.dark-forge.dev {\n\tnot_a_directive\n}\n").unwrap();
    let live_before = fs::read(&live).unwrap();
    let bad_run = run(&bad);
    assert!(!bad_run.status.success(), "{}", bad_run.stdout);
    assert!(bad_run.stderr.contains("validate failed"));
    assert!(bad_run.stderr.contains("exit 17"));
    assert_eq!(fs::read(&live).unwrap(), live_before);
    assert!(!staging.exists());

    let _ = fs::remove_dir_all(&root);
}

fn test_door_ssh_probe_rejects_an_unauthenticated_request() {
    let deploy = read_repo("deploy/github/deploy.sh");
    let remote = read_repo("deploy/github/install-remote.sh");
    let probe = read_repo("deploy/github/check-door-ssh.sh");
    assert!(deploy.contains("check-door-ssh.sh"));
    assert!(deploy.contains("install-door-caddy.sh"));
    assert!(remote.contains("bash \"$dir/check-door-ssh.sh\""));
    assert!(remote.contains("bash \"$dir/install-door-caddy.sh\""));
    assert!(remote.contains("\"$door_rc\" -eq 0"));
    assert!(probe.contains("Upgrade: kiln-ssh"));
    assert!(probe.contains("--http1.1"));
    assert!(probe.contains("journalctl -u kiln-door.service"));
    assert!(probe.contains("/v1/boxes/${box_id}/ssh"));
    assert!(!probe.contains(":8444/v1/boxes"));
    assert!(!probe.contains("Authorization:"));
    assert!(!probe.contains("admin.token"));

    let root = std::env::temp_dir().join(format!("kiln-door-ssh-probe-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("bin")).unwrap();
    fs::write(root.join("ca.crt"), "not-a-certificate\n").unwrap();
    write_executable(
        &root.join("bin/curl"),
        r#"#!/bin/bash
printf '%s\n' "$*" >> "$CURL_LOG"
if [ "${CURL_FAIL:-}" = 1 ]; then
  printf 'curl: (7) Failed to connect\n' >&2
  exit 7
fi
out=
while [ $# -gt 0 ]; do
  case $1 in
    --output) out=$2; shift 2 ;;
    *) shift ;;
  esac
done
printf '%s' '{"code":"unauthorized","message":"A valid credential is required."}' > "$out"
printf '%s' "${CURL_CODE:-401}"
"#,
    );
    write_executable(
        &root.join("bin/journalctl"),
        r#"#!/bin/bash
printf '%s\n' "audit ssh outcome=unauthorized box_id=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
"#,
    );
    write_executable(
        &root.join("bin/systemctl"),
        r#"#!/bin/bash
if [ "$1" = is-active ] && [ "$2" = --quiet ] && [ "$3" = kiln-door.service ]; then
  exit 0
fi
exit 1
"#,
    );
    let script = repo().join("deploy/github/check-door-ssh.sh");
    let run = |code: &str, fail: bool, log_name: &str| {
        let log = root.join(log_name);
        let mut command = Command::new("bash");
        command.arg(&script);
        command.env("PATH", prepend_path(&root.join("bin")));
        command.env(
            "KILN_DOOR_SSH_URL",
            "https://cli.dark-forge.dev/v1/boxes/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/ssh",
        );
        command.env("KILN_DOOR_RESOLVE", "cli.dark-forge.dev:443:127.0.0.1");
        command.env("KILN_DOOR_CA", root.join("ca.crt"));
        command.env("CURL_LOG", &log);
        command.env("CURL_CODE", code);
        if fail {
            command.env("CURL_FAIL", "1");
        }
        let captured = capture(&mut command);
        let logged = fs::read_to_string(&log).unwrap_or_default();
        (captured, logged)
    };
    let (ok, logged) = run("401", false, "ok.log");
    assert!(ok.status.success(), "{}{}", ok.stderr, ok.stdout);
    assert!(ok.stdout.contains("door ssh unauthenticated: 401"));
    assert!(logged.contains("/v1/boxes/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/ssh"));
    assert!(logged.contains("Upgrade: kiln-ssh"));
    assert!(!logged.contains("Authorization"));
    assert!(!logged.contains("Bearer"));
    let (rejected, _) = run("200", false, "open.log");
    assert!(!rejected.status.success(), "{}", rejected.stdout);
    assert!(rejected.stderr.contains("returned 200"));
    let (down, _) = run("401", true, "down.log");
    assert!(!down.status.success(), "{}", down.stdout);
    assert!(down.stderr.contains("not reachable"), "{}", down.stderr);
    let _ = fs::remove_dir_all(&root);
}

fn test_smoke_deletes_the_box_when_exec_or_output_fails() {
    let remote = read_repo("deploy/github/install-remote.sh");
    let deploy = read_repo("deploy/github/deploy.sh");
    assert!(remote.contains("bash \"$dir/smoke.sh\" \"$address\""));
    assert!(remote.contains("\"$smoke_rc\" -eq 0"));
    assert!(deploy.contains("\"$script_dir/smoke.sh\""));

    let ok = run_smoke(&[]);
    assert!(
        ok.captured.status.success(),
        "{}{}",
        ok.captured.stderr,
        ok.captured.stdout
    );
    assert_eq!(command_names(&ok.log), ["add", "create", "exec", "delete"]);
    let flat = ok.log.join("\n").replace('\0', " ");
    assert!(
        flat.contains("--template ubuntu-4g --name kiln-smoke"),
        "{flat}"
    );
    assert!(
        flat.contains("--token-file /etc/kiln/admin.token"),
        "{flat}"
    );
    assert!(flat.contains("--ca-file /etc/kiln/ca.crt"), "{flat}");
    assert!(flat.contains("https://127.0.0.1:8443"), "{flat}");
    assert!(
        flat.contains("exec 0123456789abcdef0123456789abcdef -- /bin/sh -c printf kiln-smoke-ok"),
        "{flat}"
    );

    let exec_failed = run_smoke(&[("KILN_EXEC_RC", "1")]);
    assert!(
        !exec_failed.captured.status.success(),
        "{}",
        exec_failed.captured.stderr
    );
    assert!(exec_failed.captured.stderr.contains("box exec failed"));
    assert!(!exec_failed.captured.stderr.contains("box delete failed"));
    assert_eq!(
        command_names(&exec_failed.log),
        ["add", "create", "exec", "delete"]
    );

    let mismatch = run_smoke(&[("KILN_EXEC_OUT", "nope")]);
    assert!(
        !mismatch.captured.status.success(),
        "{}",
        mismatch.captured.stderr
    );
    assert!(
        mismatch
            .captured
            .stderr
            .contains("box exec output mismatch")
    );
    assert_eq!(
        command_names(&mismatch.log),
        ["add", "create", "exec", "delete"]
    );

    let deleted = run_smoke(&[("KILN_DELETE_RC", "1")]);
    assert!(
        !deleted.captured.status.success(),
        "{}",
        deleted.captured.stderr
    );
    assert!(deleted.captured.stderr.contains("box delete failed"));

    let created = run_smoke(&[("KILN_CREATE_RC", "1"), ("KILN_CREATE_BODY", "")]);
    assert!(
        !created.captured.status.success(),
        "{}",
        created.captured.stderr
    );
    assert!(created.captured.stderr.contains("box create failed"));
    assert!(!command_names(&created.log).contains(&"delete"));

    let leaked = run_smoke(&[("KILN_CREATE_RC", "1"), ("KILN_CREATE_BODY", "box")]);
    assert!(
        !leaked.captured.status.success(),
        "{}",
        leaked.captured.stderr
    );
    assert!(leaked.captured.stderr.contains("box create failed"));
    assert!(command_names(&leaked.log).contains(&"delete"));

    let smoke = read_repo("deploy/github/smoke.sh");
    assert!(
        smoke.find("self-check").unwrap() < smoke.find("delete_box || true").unwrap(),
        "launch timing check must run before the smoke box is deleted"
    );
    assert!(
        ok.captured
            .stdout
            .contains("smoke: launch timing kiln-smoke"),
        "stdout: {}",
        ok.captured.stdout
    );

    let metrics_failed = run_smoke(&[("KILN_SELF_CHECK_RC", "1")]);
    assert!(
        !metrics_failed.captured.status.success(),
        "{}",
        metrics_failed.captured.stderr
    );
    assert!(
        metrics_failed
            .captured
            .stderr
            .contains("kiln-api self-check failed (exit 1)"),
        "{}",
        metrics_failed.captured.stderr
    );
}

struct Captured {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

struct DeployRun {
    captured: Captured,
    calls: Vec<String>,
    ssh_calls: Vec<String>,
}

struct SmokeRun {
    captured: Captured,
    log: Vec<String>,
}

struct RmDir(PathBuf);

impl Drop for RmDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn repo() -> PathBuf {
    crate::repo_root()
}

fn read_repo(relative: &str) -> String {
    fs::read_to_string(repo().join(relative))
        .unwrap_or_else(|error| panic!("read {relative}: {error}"))
}

fn workflow(name: &str) -> String {
    read_repo(&format!(".github/workflows/{name}"))
}

fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

fn count_eq(lines: &[String], needle: &str) -> usize {
    lines.iter().filter(|line| line.as_str() == needle).count()
}

fn index_of(haystack: &str, needle: &str) -> usize {
    haystack
        .find(needle)
        .unwrap_or_else(|| panic!("missing {needle}"))
}

fn position(lines: &[String], needle: &str) -> usize {
    lines
        .iter()
        .position(|line| line == needle)
        .unwrap_or_else(|| panic!("missing {needle}"))
}

fn run_guard(mode: &str, env: &[(&str, &str)]) -> Captured {
    let guard = repo().join("deploy/github/guard.sh");
    let mut command = Command::new("bash");
    command.args(["-c", "source \"$1\"; guard_github_event \"$2\"", "guard"]);
    command.arg(&guard);
    command.arg(mode);
    command.current_dir(repo());
    command.env_remove("GITHUB_EVENT_NAME");
    command.env_remove("GITHUB_REF");
    command.env_remove("GITHUB_REPOSITORY");
    command.env_remove("KILN_RELEASE_TAG");
    command.env_remove("TAG");
    for (key, value) in env {
        command.env(key, value);
    }
    capture(&mut command)
}

fn write_min_dashboard(remote: &Path, tag: &str) {
    let name = format!("kiln-dashboard-{tag}.tar.gz");
    let blob = tar_bytes(&[
        (
            "index.html",
            r#"<script type="module" crossorigin src="/assets/index-remote.js"></script>"#,
        ),
        ("assets/index-remote.js", "console.log('ok');\n"),
    ]);
    write_release_asset(remote, &name, &blob, false);
}

fn write_release_asset(dir: &Path, name: &str, blob: &[u8], bad_checksum: bool) {
    let path = dir.join(name);
    fs::write(&path, blob).unwrap();
    let digest = if bad_checksum {
        "0".repeat(64)
    } else {
        sha256_hex(blob)
    };
    fs::write(
        dir.join(format!("{name}.sha256")),
        format!("{digest}  {name}\n"),
    )
    .unwrap();
}

fn run_deploy(
    tag: &str,
    releases: &str,
    archive_installer: Option<&str>,
    bad_checksum: bool,
) -> DeployRun {
    let tmp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let log = tmp.path().join("curl.log");
    let ssh_log = tmp.path().join("ssh.log");
    let assets = tmp.path().join("assets");
    if let Some(installer) = archive_installer {
        fs::create_dir_all(&assets).unwrap();
        let server_name = format!("kiln-server-{tag}-x86_64-unknown-linux-gnu.tar.gz");
        let server_blob = tar_bytes(&[("install.sh", installer)]);
        write_release_asset(&assets, &server_name, &server_blob, bad_checksum);
        let identity_name = format!("kiln-identity-{tag}-x86_64-unknown-linux-gnu.tar.gz");
        let ver = tag.strip_prefix('v').unwrap_or(tag);
        let identity_json = format!(r#"{{"target":"x86_64-unknown-linux-gnu","version":"{ver}"}}"#);
        let identity_script = format!("#!/bin/sh\nprintf 'kiln-identity {ver}\n'\n");
        let identity_blob = tar_bytes(&[
            ("release.json", &identity_json),
            ("bin/kiln-identity", &identity_script),
        ]);
        write_release_asset(&assets, &identity_name, &identity_blob, bad_checksum);
        let dashboard_name = format!("kiln-dashboard-{tag}.tar.gz");
        let dashboard_blob = tar_bytes(&[
            (
                "index.html",
                r#"<script type="module" crossorigin src="/assets/index-deploytest.js"></script>"#,
            ),
            ("assets/index-deploytest.js", "console.log('ok');\n"),
        ]);
        write_release_asset(&assets, &dashboard_name, &dashboard_blob, bad_checksum);
        let master_state = tmp.path().join("ssh-master.state");
        write_executable(&tmp.path().join("ssh"), &ssh_stub(&ssh_log, &master_state));
        write_executable(&tmp.path().join("scp"), &scp_stub(&ssh_log, &master_state));
    }
    write_executable(
        &tmp.path().join("curl"),
        &curl_stub(&log, releases, archive_installer.map(|_| assets.as_path())),
    );
    let mut command = Command::new("bash");
    command.arg("deploy/github/deploy.sh");
    command.current_dir(repo());
    command.env_remove("GITHUB_EVENT_NAME");
    command.env_remove("GITHUB_REF");
    command.env_remove("GITHUB_REPOSITORY");
    command.env_remove("KILN_SSH_SH");
    command.env("PATH", prepend_path(tmp.path()));
    command.env("KILN_RELEASE_TAG", tag);
    if archive_installer.is_some() {
        command.env(
            "DARK_FORGE_SSH_KEY",
            "-----BEGIN PRIVATE KEY-----\nfake\n-----END PRIVATE KEY-----",
        );
        command.env(
            "DARK_FORGE_KNOWN_HOSTS",
            "dark-forge.dev ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHRlc3Q=",
        );
        command.env(
            "KILN_SSH_CONTROL_PATH",
            tmp.path().join("kiln-ssh-control").display().to_string(),
        );
    }
    let captured = capture(&mut command);
    DeployRun {
        captured,
        calls: read_lines(&log),
        ssh_calls: read_lines(&ssh_log),
    }
}

fn curl_stub(log: &Path, releases: &str, assets_dir: Option<&Path>) -> String {
    let assets = assets_dir
        .map(|path| sh_quote(&path.display().to_string()))
        .unwrap_or_else(|| "''".to_string());
    format!(
        r#"#!/bin/bash
log={log}
assets={assets}
releases={releases}
printf '%s\n' "$*" >> "$log"
out=
prev=
url=
for arg in "$@"; do
  url=$arg
  if [ "$prev" = --output ]; then
    out=$arg
  fi
  prev=$arg
done
case "$url" in
  *api.github.com/repos/Jabbslad/kiln/releases*)
    case "$url" in
      *download*) ;;
      *)
        printf '%s\n' "$releases"
        exit 0
        ;;
    esac
    ;;
esac
if [ -n "$assets" ] && [ -n "$out" ]; then
  base=$(basename "$url")
  if [ -f "$assets/$base" ]; then
    cp -- "$assets/$base" "$out"
    exit 0
  fi
fi
exit 1
"#,
        log = sh_quote(&log.display().to_string()),
        assets = assets,
        releases = sh_quote(releases),
    )
}

fn ssh_stub(log: &Path, master_state: &Path) -> String {
    r#"#!/bin/bash
printf '%s\n' "ssh $*" >> __LOG__
case " $* " in
  *" -M "*|*ControlMaster=yes*)
    printf '1\n' > __STATE__
    exit 0
    ;;
  *" -O exit "*)
    printf '0\n' > __STATE__
    exit 0
    ;;
esac
if [ "$(cat __STATE__ 2>/dev/null || printf 0)" != 1 ]; then
  printf 'ssh stub: multiplex master is not open\n' >&2
  exit 97
fi
while [ $# -gt 0 ]; do
  if [ "$1" = -- ]; then
    shift
    if [ "${1:-}" = mktemp ]; then
      shift
      mktemp "$@"
      exit 0
    fi
    if [ "${1:-}" = rm ]; then
      shift
      rm "$@"
      exit 0
    fi
    exit 0
  fi
  shift
done
exit 0
"#
    .replace("__LOG__", &sh_quote(&log.display().to_string()))
    .replace("__STATE__", &sh_quote(&master_state.display().to_string()))
}

fn scp_stub(log: &Path, master_state: &Path) -> String {
    r#"#!/bin/bash
printf '%s\n' "scp $*" >> __LOG__
if [ "$(cat __STATE__ 2>/dev/null || printf 0)" != 1 ]; then
  printf 'scp stub: multiplex master is not open\n' >&2
  exit 96
fi
local=
remote=
for arg in "$@"; do
  case "$arg" in
    -*) ;;
    *:*)
      remote=${arg#*:}
      ;;
    *)
      local=$arg
      ;;
  esac
done
mkdir -p -- "$(dirname -- "$remote")"
cp -- "$local" "$remote"
exit 0
"#
    .replace("__LOG__", &sh_quote(&log.display().to_string()))
    .replace("__STATE__", &sh_quote(&master_state.display().to_string()))
}

fn run_remote(archive_body: &str, overlay: Option<&str>, bad_checksum: bool) -> Captured {
    let remote = mktemp_deploy();
    let _remote = RmDir(remote.clone());
    let fake = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let name = "kiln-server-v0.4.4-x86_64-unknown-linux-gnu.tar.gz";
    let blob = tar_bytes(&[("install.sh", archive_body)]);
    fs::write(remote.join(name), &blob).unwrap();
    let digest = if bad_checksum {
        "0".repeat(64)
    } else {
        sha256_hex(&blob)
    };
    fs::write(
        remote.join(format!("{name}.sha256")),
        format!("{digest}  {name}\n"),
    )
    .unwrap();
    if let Some(text) = overlay {
        fs::write(remote.join("installer.sh"), text).unwrap();
    }
    write_min_dashboard(&remote, "v0.4.4");
    write_executable(&fake.path().join("apt-get"), "#!/bin/bash\nexit 0\n");
    let path = prepend_path(fake.path());
    let mut command = Command::new("bash");
    command.arg(repo().join("deploy/github/install-remote.sh"));
    command.arg(&remote);
    command.arg("127.0.0.1");
    command.env("PATH", path);
    capture(&mut command)
}

fn apt_lines(hide_sqlite3: bool, existing: bool, apt_fail: &str) -> (Vec<String>, String, String) {
    let remote = mktemp_deploy();
    let _remote = RmDir(remote.clone());
    let fake = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let name = "kiln-server-v0.4.4-x86_64-unknown-linux-gnu.tar.gz";
    let blob = tar_bytes(&[("install.sh", "#!/bin/bash\nprintf 'installer-ran\\n'\n")]);
    fs::write(remote.join(name), &blob).unwrap();
    let digest = sha256_hex(&blob);
    fs::write(
        remote.join(format!("{name}.sha256")),
        format!("{digest}  {name}\n"),
    )
    .unwrap();
    write_min_dashboard(&remote, "v0.4.4");
    let log = fake.path().join("apt.log");
    let apt_body = r#"#!/bin/bash
umask 022
log=__LOG__
printf 'DEBIAN_FRONTEND=%s NEEDRESTART_MODE=%s apt-get %s\n' "${DEBIAN_FRONTEND-}" "${NEEDRESTART_MODE-}" "$*" >> "$log"
chmod a+r "$log"
case " ${APT_FAIL-} " in *" $1 "*) exit 100;; esac
exit 0
"#
    .replace("__LOG__", &sh_quote(&log.display().to_string()));
    write_executable(&fake.path().join("apt-get"), &apt_body);
    let path = if hide_sqlite3 {
        path_without("sqlite3", fake.path(), &[fake.path()])
    } else {
        write_executable(&fake.path().join("sqlite3"), "#!/bin/bash\nexit 0\n");
        prepend_path(fake.path())
    };
    set_mode(fake.path(), 0o755);
    set_mode(&remote, 0o755);
    let hidden = fake.path().join("hidden");
    if hidden.is_dir() {
        set_mode(&hidden, 0o755);
    }
    let inner = [
        "bash".to_string(),
        repo()
            .join("deploy/github/install-remote.sh")
            .display()
            .to_string(),
        remote.display().to_string(),
        "127.0.0.1".to_string(),
    ];
    let overlay = existing.then(|| {
        let dir = tempfile::Builder::new()
            .prefix("kiln-ov-")
            .tempdir()
            .unwrap_or_else(|error| panic!("tempdir: {error}"));
        set_mode(dir.path(), 0o755);
        dir
    });
    let joined = inner
        .iter()
        .map(|part| sh_quote(part))
        .collect::<Vec<_>>()
        .join(" ");
    let program = format!(
        "mkdir -p \"$OV/upper\" \"$OV/work\" \"$OV/merged\" && mount -t overlay overlay -o lowerdir=/etc,upperdir=\"$OV/upper\",workdir=\"$OV/work\" \"$OV/merged\" && mount --bind \"$OV/merged\" /etc && mkdir -p /etc/kiln && exec {joined}"
    );
    let mut command = if let Some(overlay) = overlay.as_ref() {
        let mut command = Command::new("sudo");
        command.args([
            "-n",
            "env",
            &format!("PATH={path}"),
            &format!("APT_FAIL={apt_fail}"),
            &format!("OV={}", overlay.path().display()),
            "unshare",
            "--mount",
            "bash",
            "-c",
            &program,
        ]);
        command
    } else {
        let mut command = Command::new(&inner[0]);
        command.args(&inner[1..]);
        command
    };
    command.env("PATH", &path);
    command.env("APT_FAIL", apt_fail);
    let captured = capture(&mut command);
    let lines = read_lines(&log);
    (lines, captured.stdout, captured.stderr)
}

fn path_without(name: &str, root: &Path, front: &[&Path]) -> String {
    let hidden = root.join("hidden");
    fs::create_dir_all(&hidden).unwrap();
    if let Ok(path_var) = std::env::var("PATH") {
        for directory in std::env::split_paths(&path_var) {
            let Ok(entries) = fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let dest = hidden.join(entry.file_name());
                if entry.file_name() == std::ffi::OsStr::new(name) || dest.exists() {
                    continue;
                }
                if symlink(entry.path(), &dest).is_err() {
                    continue;
                }
            }
        }
    }
    let mut parts: Vec<String> = front
        .iter()
        .map(|path| path.display().to_string())
        .collect();
    parts.push(hidden.display().to_string());
    parts.join(":")
}

fn upgrade_lines(lines: &[String]) -> bool {
    lines.iter().any(|line| {
        let Some((_, after)) = line.split_once(" apt-get ") else {
            return false;
        };
        let word = after.split_whitespace().next().unwrap_or("");
        matches!(word, "upgrade" | "dist-upgrade" | "full-upgrade")
    })
}

fn run_smoke(extra: &[(&str, &str)]) -> SmokeRun {
    let tmp = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let log = tmp.path().join("kiln.log");
    let kiln = tmp.path().join("kiln");
    write_executable(&kiln, &kiln_stub(&log));
    let token = tmp.path().join("admin.token");
    fs::write(&token, "a".repeat(64)).unwrap();
    let ca = tmp.path().join("ca.crt");
    fs::write(&ca, "fixture-ca\n").unwrap();
    let kiln_api = tmp.path().join("kiln-api");
    write_executable(
        &kiln_api,
        r#"#!/bin/sh
case "$*" in
  *self-check*)
    printf 'smoke: launch timing kiln-smoke total_ms=1 storage_ms=1\n'
    exit "${KILN_SELF_CHECK_RC-0}" ;;
  *) exit 1 ;;
esac
"#,
    );
    let mut command = Command::new("bash");
    command.arg(repo().join("deploy/github/smoke.sh"));
    command.arg("127.0.0.1");
    command.current_dir(repo());
    command.env("KILN_BIN", &kiln);
    command.env("KILN_API_BIN", &kiln_api);
    command.env("KILN_ADMIN_TOKEN_FILE", &token);
    command.env("KILN_CA_FILE", &ca);
    for (key, value) in extra {
        command.env(key, value);
    }
    let captured = capture(&mut command);
    SmokeRun {
        captured,
        log: read_lines(&log),
    }
}

fn kiln_stub(log: &Path) -> String {
    r#"#!/bin/bash
log=__LOG__
first=1
for arg in "$@"; do
  if [ "$first" -eq 1 ]; then
    printf '%s' "$arg" >> "$log"
    first=0
  else
    printf '\0%s' "$arg" >> "$log"
  fi
done
printf '\n' >> "$log"
has() {
  needle=$1
  shift
  for arg in "$@"; do
    if [ "$arg" = "$needle" ]; then
      return 0
    fi
  done
  return 1
}
if has add "$@"; then
  exit 0
fi
if has create "$@"; then
  body=${KILN_CREATE_BODY-box}
  if [ "$body" = box ]; then
    printf '%s' '__JSON__'
  fi
  exit "${KILN_CREATE_RC-0}"
fi
if has exec "$@"; then
  if [ "${KILN_EXEC_RC-0}" = 0 ]; then
    printf '%s' "${KILN_EXEC_OUT-kiln-smoke-ok}"
  fi
  exit "${KILN_EXEC_RC-0}"
fi
if has delete "$@"; then
  exit "${KILN_DELETE_RC-0}"
fi
exit 99
"#
    .replace("__LOG__", &sh_quote(&log.display().to_string()))
    .replace("__JSON__", CREATE_JSON)
}

fn command_names(calls: &[String]) -> Vec<&'static str> {
    let mut names = Vec::new();
    for call in calls {
        let args: Vec<&str> = call.split('\0').collect();
        for name in ["add", "create", "exec", "delete"] {
            if args.contains(&name) {
                names.push(name);
                break;
            }
        }
    }
    names
}

fn tar_bytes(members: &[(&str, &str)]) -> Vec<u8> {
    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut archive = tar::Builder::new(encoder);
    for (name, text) in members {
        let data = text.as_bytes();
        let mut header = tar::Header::new_gnu();
        header
            .set_path(name)
            .unwrap_or_else(|error| panic!("{error}"));
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_username("").unwrap();
        header.set_groupname("").unwrap();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        archive
            .append(&header, data)
            .unwrap_or_else(|error| panic!("tar {name}: {error}"));
    }
    let encoder = archive.into_inner().unwrap();
    encoder.finish().unwrap()
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn write_executable(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, body).unwrap();
    set_mode(path, 0o755);
}

fn set_mode(path: &Path, mode: u32) {
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions).unwrap();
}

fn prepend_path(front: &Path) -> String {
    match std::env::var("PATH") {
        Ok(path) if !path.is_empty() => format!("{}:{path}", front.display()),
        _ => front.display().to_string(),
    }
}

fn read_lines(path: &Path) -> Vec<String> {
    match fs::read_to_string(path) {
        Ok(text) => text.lines().map(ToString::to_string).collect(),
        Err(_) => Vec::new(),
    }
}

fn capture(command: &mut Command) -> Captured {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("spawn: {error}"));
    Captured {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn mktemp_deploy() -> PathBuf {
    let output = Command::new("mktemp")
        .args(["-d", "/tmp/kiln-deploy.XXXXXX"])
        .output()
        .unwrap_or_else(|error| panic!("mktemp: {error}"));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    PathBuf::from(String::from_utf8_lossy(&output.stdout).trim())
}

pub fn cli(_args: &[String]) -> i32 {
    crate::run_cases(&cases())
}

register_cases! {
    test_workflows_have_no_pull_request_trigger,
    test_deploy_is_manual_from_main,
    test_diagnose_is_manual_and_read_only,
    test_guard_refuses_pull_requests_forks_and_branch_dispatch,
    test_guard_allows_main_dispatch_and_a_tag_release,
    test_secrets_are_read_only_by_the_dark_forge_environment,
    test_older_tags_and_prereleases_are_refused_before_ssh,
    test_deploy_ssh_and_scp_run_while_multiplex_master_is_open,
    test_v044_uses_this_tree_and_later_tags_use_the_archive,
    test_apt_installs_missing_packages_without_upgrading,
    test_sqlite3_install_is_exact_and_skipped_when_present,
    test_deploy_installs_dashboard_and_checks_the_live_bundle,
    test_deploy_host_scripts_do_not_invoke_jq,
    test_smoke_deletes_the_box_when_exec_or_output_fails,
    test_door_ssh_probe_rejects_an_unauthenticated_request,
    test_install_door_caddy_validates_staging_and_skips_unchanged_bytes,
}

use crate::register_cases;
use crate::repo_root;
use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;

const UPDATE: &str = "DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l apt-get update";
const SQLITE_EXACT: &str = "DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l apt-get install -y --no-install-recommends --no-upgrade sqlite3";

struct Captured {
    code: i32,
    stdout: String,
    stderr: String,
}

fn capture(cmd: &mut Command) -> Captured {
    let output = cmd
        .output()
        .unwrap_or_else(|error| panic!("failed to start command: {error}"));
    Captured {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|error| panic!("read {relative}: {error}"))
}

fn workflow(name: &str) -> String {
    repo_text(&format!(".github/workflows/{name}"))
}

fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn write_exe(path: &Path, body: &str) {
    fs::write(path, body).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn chmod(path: &Path, mode: u32) {
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions).unwrap();
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut hex, byte| {
            use std::fmt::Write as _;
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

fn tar_bytes(members: &[(&str, &str)]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut archive = tar::Builder::new(&mut encoder);
        for (name, text) in members {
            let payload = text.as_bytes();
            let mut header = tar::Header::new_gnu();
            header.set_path(name).unwrap();
            header.set_size(payload.len() as u64);
            header.set_mode(0o644);
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(0);
            header.set_username("").unwrap();
            header.set_groupname("").unwrap();
            header.set_cksum();
            header.set_entry_type(tar::EntryType::Regular);
            archive.append(&header, io::Cursor::new(payload)).unwrap();
        }
        archive.finish().unwrap();
    }
    encoder.finish().unwrap()
}

struct RmDir(PathBuf);

impl Drop for RmDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn mktemp_deploy() -> PathBuf {
    let captured = capture(Command::new("mktemp").args(["-d", "/tmp/kiln-deploy.XXXXXX"]));
    assert_eq!(captured.code, 0, "{}", captured.stderr);
    PathBuf::from(captured.stdout.trim())
}

fn lines_of(path: &Path) -> Vec<String> {
    if !path.exists() {
        return Vec::new();
    }
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(ToString::to_string)
        .collect()
}

fn run_guard(mode: &str, env: &[(&str, &str)]) -> Captured {
    let guard = repo_root().join("deploy/github/guard.sh");
    let mut command = Command::new("bash");
    command.args([
        "-c",
        "source \"$1\"; guard_github_event \"$2\"",
        "guard",
        guard.to_str().unwrap(),
        mode,
    ]);
    command.current_dir(repo_root());
    command.env_remove("GITHUB_EVENT_NAME");
    command.env_remove("GITHUB_REF");
    command.env_remove("GITHUB_REPOSITORY");
    for (key, value) in env {
        command.env(key, value);
    }
    capture(&mut command)
}

fn curl_stub() -> String {
    r#"#!/bin/bash
printf '%s\n' "$*" >> "$CURL_LOG"
url=${@: -1}
output=
prev=
for arg in "$@"; do
  if [ "$prev" = --output ]; then
    output=$arg
  fi
  prev=$arg
done
case "$url" in
  *api.github.com/repos/Jabbslad/kiln/releases*)
    case "$url" in
      *download*) ;;
      *)
        printf '%s\n' "$CURL_RELEASES"
        exit 0
        ;;
    esac
    ;;
esac
if [ -n "${CURL_ARCHIVE:-}" ] && [ -n "$output" ]; then
  case "$url" in
    *.tar.gz)
      cp -- "$CURL_ARCHIVE" "$output"
      exit 0
      ;;
  esac
fi
if [ -n "${CURL_CHECKSUM:-}" ] && [ -n "$output" ]; then
  case "$url" in
    *.sha256)
      printf '%s' "$CURL_CHECKSUM" > "$output"
      exit 0
      ;;
  esac
fi
exit 1
"#
    .to_string()
}

fn ssh_stub() -> String {
    r#"#!/bin/bash
printf '%s\n' "$*" >> "$SSH_LOG"
if [ "$1" = scp ]; then
  mkdir -p -- "$(dirname -- "$3")"
  cp -- "$2" "$3"
  exit 0
fi
if [ "$1" = ssh ]; then
  shift
  if [ "${1:-}" = -- ]; then
    shift
  fi
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
exit 1
"#
    .to_string()
}

fn run_deploy(
    tag: &str,
    releases: &str,
    archive_installer: Option<&str>,
    bad_checksum: bool,
) -> (Captured, Vec<String>, Vec<String>) {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("curl.log");
    let ssh_log = tmp.path().join("ssh.log");
    let archive_path = tmp.path().join("release.tar.gz");
    let asset_name = format!("kiln-server-{tag}-x86_64-unknown-linux-gnu.tar.gz");
    let mut checksum_text = String::new();
    if let Some(installer) = archive_installer {
        let blob = tar_bytes(&[("install.sh", installer)]);
        fs::write(&archive_path, &blob).unwrap();
        let digest = if bad_checksum {
            "0".repeat(64)
        } else {
            sha256_hex(&blob)
        };
        checksum_text = format!("{digest}  {asset_name}\n");
        write_exe(&tmp.path().join("ssh.sh"), &ssh_stub());
    }
    write_exe(&tmp.path().join("curl"), &curl_stub());
    let path = format!(
        "{}:{}",
        tmp.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut command = Command::new("bash");
    command.arg("deploy/github/deploy.sh");
    command.current_dir(repo_root());
    command.env("PATH", &path);
    command.env("KILN_RELEASE_TAG", tag);
    command.env("CURL_LOG", &log);
    command.env("CURL_RELEASES", releases);
    command.env_remove("GITHUB_EVENT_NAME");
    command.env_remove("GITHUB_REF");
    command.env_remove("GITHUB_REPOSITORY");
    command.env_remove("KILN_SSH_SH");
    if archive_installer.is_some() {
        command.env("KILN_SSH_SH", tmp.path().join("ssh.sh"));
        command.env("SSH_LOG", &ssh_log);
        command.env("CURL_ARCHIVE", &archive_path);
        command.env("CURL_CHECKSUM", &checksum_text);
    }
    let result = capture(&mut command);
    let calls = lines_of(&log);
    let ssh_calls = lines_of(&ssh_log);
    (result, calls, ssh_calls)
}

fn path_without(name: &str, root: &Path, front: &[PathBuf]) -> std::ffi::OsString {
    let hidden = root.join("hidden");
    fs::create_dir_all(&hidden).unwrap();
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            let Ok(entries) = fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let file_name = entry.file_name();
                if file_name.as_os_str() == std::ffi::OsStr::new(name) {
                    continue;
                }
                let dest = hidden.join(entry.file_name());
                if dest.symlink_metadata().is_ok() {
                    continue;
                }
                let _ = symlink(entry.path(), dest);
            }
        }
    }
    std::env::join_paths(front.iter().chain(std::iter::once(&hidden))).unwrap()
}

fn upgrade_lines(lines: &[String]) -> bool {
    lines.iter().any(|line| {
        let Some((_, rest)) = line.split_once(" apt-get ") else {
            return false;
        };
        matches!(
            rest.split(' ').next().unwrap_or(""),
            "upgrade" | "dist-upgrade" | "full-upgrade"
        )
    })
}

fn apt_stub(log: &Path) -> String {
    format!(
        "#!/bin/bash\n\
         umask 022\n\
         log={}\n\
         printf 'DEBIAN_FRONTEND=%s NEEDRESTART_MODE=%s apt-get %s\\n' \
         \"${{DEBIAN_FRONTEND-}}\" \"${{NEEDRESTART_MODE-}}\" \"$*\" >> \"$log\"\n\
         chmod a+r \"$log\"\n\
         case \" ${{APT_FAIL-}} \" in *\" $1 \"*) exit 100;; esac\n\
         exit 0\n",
        sh_quote(&log.display().to_string())
    )
}

fn apt_lines(hide_sqlite3: bool, existing: bool, apt_fail: &str) -> (Vec<String>, String, String) {
    let remote = mktemp_deploy();
    let _remote_guard = RmDir(remote.clone());
    let fake = tempfile::tempdir().unwrap();
    chmod(fake.path(), 0o755);
    let overlay = existing.then(|| tempfile::tempdir().unwrap());
    let name = "kiln-server-v0.4.4-x86_64-unknown-linux-gnu.tar.gz";
    let blob = tar_bytes(&[("install.sh", "#!/bin/bash\nprintf 'installer-ran\\n'\n")]);
    fs::write(remote.join(name), &blob).unwrap();
    fs::write(
        remote.join(format!("{name}.sha256")),
        format!("{}  {name}\n", sha256_hex(&blob)),
    )
    .unwrap();
    chmod(&remote, 0o755);
    let log = fake.path().join("apt.log");
    write_exe(&fake.path().join("apt-get"), &apt_stub(&log));
    let path = if hide_sqlite3 {
        path_without("sqlite3", fake.path(), &[fake.path().to_path_buf()])
    } else {
        write_exe(&fake.path().join("sqlite3"), "#!/bin/bash\nexit 0\n");
        let mut joined = fake.path().as_os_str().to_os_string();
        joined.push(":");
        if let Some(current) = std::env::var_os("PATH") {
            joined.push(current);
        }
        joined
    };
    let hidden = fake.path().join("hidden");
    if hidden.is_dir() {
        chmod(&hidden, 0o755);
    }
    let install_remote = repo_root().join("deploy/github/install-remote.sh");
    let inner = [
        "bash".to_string(),
        install_remote.display().to_string(),
        remote.display().to_string(),
        "127.0.0.1".to_string(),
    ];
    let script = if let Some(overlay) = overlay.as_ref() {
        chmod(overlay.path(), 0o755);
        let quoted = inner
            .iter()
            .map(|part| sh_quote(part))
            .collect::<Vec<_>>()
            .join(" ");
        Some(format!(
            "mkdir -p \"$OV/upper\" \"$OV/work\" \"$OV/merged\" && \
             mount -t overlay overlay -o lowerdir=/etc,upperdir=\"$OV/upper\",workdir=\"$OV/work\" \"$OV/merged\" && \
             mount --bind \"$OV/merged\" /etc && \
             mkdir -p /etc/kiln && \
             exec {quoted}"
        ))
    } else {
        None
    };
    let mut command = if let Some(overlay) = overlay.as_ref() {
        let mut command = Command::new("sudo");
        command.arg("-n");
        command.arg("env");
        command.arg(format!("PATH={}", path.to_string_lossy()));
        command.arg(format!("APT_FAIL={apt_fail}"));
        command.arg(format!("OV={}", overlay.path().display()));
        command.arg("unshare");
        command.arg("--mount");
        command.arg("bash");
        command.arg("-c");
        command.arg(script.as_deref().unwrap());
        command.env("PATH", &path);
        command.env("APT_FAIL", apt_fail);
        command
    } else {
        let mut command = Command::new("bash");
        command.arg(&install_remote);
        command.arg(&remote);
        command.arg("127.0.0.1");
        command.env("PATH", &path);
        command.env("APT_FAIL", apt_fail);
        command
    };
    let result = capture(&mut command);
    (lines_of(&log), result.stdout, result.stderr)
}

fn run_remote(archive_body: &str, overlay: Option<&str>, bad_checksum: bool) -> Captured {
    let remote = mktemp_deploy();
    let _remote_guard = RmDir(remote.clone());
    let fake = tempfile::tempdir().unwrap();
    chmod(fake.path(), 0o755);
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
    if let Some(body) = overlay {
        fs::write(remote.join("installer.sh"), body).unwrap();
    }
    write_exe(&fake.path().join("apt-get"), "#!/bin/bash\nexit 0\n");
    let path = format!(
        "{}:{}",
        fake.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut command = Command::new("bash");
    command.arg(repo_root().join("deploy/github/install-remote.sh"));
    command.arg(&remote);
    command.arg("127.0.0.1");
    command.env("PATH", path);
    capture(&mut command)
}

fn kiln_stub(log: &Path) -> String {
    format!(
        "#!/bin/bash\n\
         log={}\n\
         first=1\n\
         for arg in \"$@\"; do\n\
           if [ \"$first\" = 1 ]; then\n\
             printf '%s' \"$arg\" >> \"$log\"\n\
             first=0\n\
           else\n\
             printf '\\0%s' \"$arg\" >> \"$log\"\n\
           fi\n\
         done\n\
         printf '\\n' >> \"$log\"\n\
         contains() {{\n\
           want=$1\n\
           shift\n\
           for arg in \"$@\"; do\n\
             if [ \"$arg\" = \"$want\" ]; then\n\
               return 0\n\
             fi\n\
           done\n\
           return 1\n\
         }}\n\
         if contains add \"$@\"; then\n\
           exit 0\n\
         fi\n\
         if contains create \"$@\"; then\n\
           body=${{KILN_CREATE_BODY-box}}\n\
           if [ \"$body\" = box ]; then\n\
             printf '%s' '{{\"id\":\"op\",\"box_id\":\"0123456789abcdef0123456789abcdef\",\"state\":\"succeeded\",\"result\":{{\"type\":\"box\",\"id\":\"0123456789abcdef0123456789abcdef\",\"name\":\"kiln-smoke\",\"state\":\"running\",\"memory_mib\":4096,\"vcpus\":1}},\"error\":null}}'\n\
           fi\n\
           exit \"${{KILN_CREATE_RC-0}}\"\n\
         fi\n\
         if contains exec \"$@\"; then\n\
           if [ \"${{KILN_EXEC_RC-0}}\" = 0 ]; then\n\
             printf '%s' \"${{KILN_EXEC_OUT-kiln-smoke-ok}}\"\n\
           fi\n\
           exit \"${{KILN_EXEC_RC-0}}\"\n\
         fi\n\
         if contains delete \"$@\"; then\n\
           exit \"${{KILN_DELETE_RC-0}}\"\n\
         fi\n\
         exit 99\n",
        sh_quote(&log.display().to_string())
    )
}

fn run_smoke(extra: &[(&str, &str)]) -> (Captured, Vec<String>) {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("kiln.log");
    let kiln = tmp.path().join("kiln");
    write_exe(&kiln, &kiln_stub(&log));
    let mut command = Command::new("bash");
    command.arg(repo_root().join("deploy/github/smoke.sh"));
    command.arg("127.0.0.1");
    command.current_dir(repo_root());
    command.env("KILN_BIN", &kiln);
    for (key, value) in extra {
        command.env(key, value);
    }
    let result = capture(&mut command);
    (result, lines_of(&log))
}

fn command_names(calls: &[String]) -> Vec<String> {
    let mut names = Vec::new();
    for call in calls {
        let args: Vec<&str> = call.split('\0').collect();
        for name in ["add", "create", "exec", "delete"] {
            if args.contains(&name) {
                names.push(name.to_string());
                break;
            }
        }
    }
    names
}

fn test_workflows_have_no_pull_request_trigger() {
    for name in ["deploy.yml", "diagnose.yml", "deploy-release.yml"] {
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
    let caller = workflow("deploy-release.yml");
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
    assert_eq!(text.matches("secrets.DARK_FORGE_SSH_KEY").count(), 1);
    assert!(text.contains("name: Install the release\n"));
    let caller = workflow("deploy-release.yml");
    assert!(caller.contains("types: [published]"));
    assert!(caller.contains("tag: ${{ github.event.release.tag_name }}"));
    let docs = repo_text("docs/releases.md");
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
    let remote = repo_text("deploy/github/diagnose.sh");
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
    let ssh = repo_text("deploy/github/ssh.sh");
    assert!(ssh.contains("StrictHostKeyChecking=yes"));
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
    assert_ne!(refused.code, 0, "{}", refused.stderr);
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
    assert_ne!(target.code, 0, "{}", target.stderr);
    let fork = run_guard(
        "diagnose",
        &[
            ("GITHUB_EVENT_NAME", "workflow_dispatch"),
            ("GITHUB_REPOSITORY", "example/kiln"),
            ("GITHUB_REF", "refs/heads/main"),
        ],
    );
    assert_ne!(fork.code, 0, "{}", fork.stderr);
    assert!(fork.stderr.contains("refusing a fork"), "{}", fork.stderr);
    let branch = run_guard(
        "deploy",
        &[
            ("GITHUB_EVENT_NAME", "workflow_dispatch"),
            ("GITHUB_REPOSITORY", "Jabbslad/kiln"),
            ("GITHUB_REF", "refs/heads/feature"),
        ],
    );
    assert_ne!(branch.code, 0, "{}", branch.stderr);
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
    assert_eq!(dispatch.code, 0, "{}", dispatch.stderr);
    let release = run_guard(
        "deploy",
        &[
            ("GITHUB_EVENT_NAME", "release"),
            ("GITHUB_REPOSITORY", "Jabbslad/kiln"),
            ("GITHUB_REF", "refs/tags/v0.4.12"),
        ],
    );
    assert_eq!(release.code, 0, "{}", release.stderr);
    let called = run_guard(
        "deploy",
        &[
            ("GITHUB_EVENT_NAME", "workflow_call"),
            ("GITHUB_REPOSITORY", "Jabbslad/kiln"),
            ("GITHUB_REF", "refs/tags/v0.4.12"),
        ],
    );
    assert_eq!(called.code, 0, "{}", called.stderr);
    let branch = run_guard(
        "deploy",
        &[
            ("GITHUB_EVENT_NAME", "release"),
            ("GITHUB_REPOSITORY", "Jabbslad/kiln"),
            ("GITHUB_REF", "refs/heads/main"),
        ],
    );
    assert_ne!(branch.code, 0, "{}", branch.stderr);
    assert!(branch.stderr.contains("stable tag"), "{}", branch.stderr);
    let diagnose_release = run_guard(
        "diagnose",
        &[
            ("GITHUB_EVENT_NAME", "release"),
            ("GITHUB_REPOSITORY", "Jabbslad/kiln"),
            ("GITHUB_REF", "refs/tags/v0.4.4"),
        ],
    );
    assert_ne!(diagnose_release.code, 0, "{}", diagnose_release.stderr);
    assert!(
        diagnose_release.stderr.contains("refusing event release"),
        "{}",
        diagnose_release.stderr
    );
    let local = run_guard("deploy", &[]);
    assert_eq!(local.code, 0, "{}", local.stderr);
}

fn test_secrets_are_read_only_by_the_dark_forge_environment() {
    for name in ["deploy.yml", "diagnose.yml"] {
        let text = workflow(name);
        assert_eq!(text.matches("environment: dark-forge").count(), 1, "{name}");
        assert!(
            text.find("environment: dark-forge").unwrap()
                < text.find("secrets.DARK_FORGE_SSH_KEY").unwrap(),
            "{name}"
        );
        assert!(
            text.find("environment: dark-forge").unwrap()
                < text.find("secrets.DARK_FORGE_KNOWN_HOSTS").unwrap(),
            "{name}"
        );
        assert_eq!(text.matches("jobs:").count(), 1, "{name}");
    }
    let docs = repo_text("docs/releases.md");
    assert!(docs.contains("environment secrets"));
    assert!(docs.contains("Jamie"));
    assert!(docs.contains("deployment branches to `main`"));
    assert!(docs.contains("deployment tags to `v*`"));
    assert!(!docs.contains("Leave the key out of the environment"));
    assert!(!docs.to_lowercase().contains("repository secret"));
}

fn test_older_tags_and_prereleases_are_refused_before_ssh() {
    for tag in ["v0.4.3", "v0.4.4-rc.1", "v0.5.0-rc.1"] {
        let (result, calls, ssh_calls) = run_deploy(tag, "[]", None, false);
        assert_ne!(result.code, 0, "{}", result.stderr);
        assert!(
            result.stderr.contains(&format!("refusing {tag}")),
            "{}",
            result.stderr
        );
        assert!(
            result.stderr.contains("v0.4.4 and newer stable releases"),
            "{}",
            result.stderr
        );
        assert!(calls.is_empty(), "{}", result.stderr);
        assert!(ssh_calls.is_empty(), "{}", result.stderr);
    }
    let (latest, calls, ssh_calls) = run_deploy(
        "latest",
        "[{\"tag_name\":\"v0.4.3\",\"draft\":false}]",
        None,
        false,
    );
    assert_ne!(latest.code, 0, "{}", latest.stderr);
    assert!(
        latest.stderr.contains("refusing v0.4.3"),
        "{}",
        latest.stderr
    );
    assert!(
        calls.iter().any(|call| call.contains("api.github.com")),
        "{calls:?}"
    );
    assert!(
        calls.iter().all(|call| !call.contains("releases/download")),
        "{calls:?}"
    );
    assert!(ssh_calls.is_empty(), "{}", latest.stderr);
}

fn test_v044_uses_this_tree_and_later_tags_use_the_archive() {
    let tree = repo_root().join("deploy/install.sh").display().to_string();
    let (current, calls, ssh_calls) =
        run_deploy("v0.4.4", "[]", Some("printf 'archive\\n'\n"), false);
    assert_eq!(current.code, 0, "{}", current.stderr);
    assert!(
        current.stdout.contains("deployed v0.4.4"),
        "{}",
        current.stdout
    );
    assert!(
        ssh_calls.iter().any(|call| call.contains(&tree)),
        "{ssh_calls:?}"
    );
    assert!(
        ssh_calls
            .iter()
            .any(|call| call.starts_with("scp ") && call.ends_with("installer.sh")),
        "{ssh_calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|call| call.contains("kiln-server-v0.4.4-")),
        "{calls:?}"
    );

    let (later, _, later_ssh) =
        run_deploy("v0.4.5", "[]", Some("printf 'archive-v045\\n'\n"), false);
    assert_eq!(later.code, 0, "{}", later.stderr);
    assert!(later.stdout.contains("deployed v0.4.5"), "{}", later.stdout);
    assert!(
        later_ssh.iter().all(|call| {
            !call.contains("install.py")
                && !call.contains("installer.py")
                && !call.contains("installer.sh")
        }),
        "{later_ssh:?}"
    );
    assert!(
        later_ssh
            .iter()
            .any(|call| call.contains("kiln-server-v0.4.5-") && call.starts_with("scp ")),
        "{later_ssh:?}"
    );

    let (numeric, _, numeric_ssh) =
        run_deploy("v0.4.10", "[]", Some("printf 'archive-v0410\\n'\n"), false);
    assert_eq!(numeric.code, 0, "{}", numeric.stderr);
    assert!(
        numeric_ssh
            .iter()
            .all(|call| !call.contains("installer.sh")),
        "{numeric_ssh:?}"
    );

    for tag in ["v0.4.4", "v0.4.5"] {
        let (failed, _, failed_ssh) =
            run_deploy(tag, "[]", Some("printf 'must-not-run\\n'\n"), true);
        assert_ne!(failed.code, 0, "{}{}", failed.stdout, failed.stderr);
        assert!(failed.stderr.contains("NOT match"), "{}", failed.stderr);
        assert!(failed_ssh.is_empty(), "{}", failed.stderr);
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
    let script = repo_text("deploy/github/install-remote.sh");
    assert!(script.contains("NEEDRESTART_MODE=l"));
    assert!(script.contains("DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l apt-get update"));
    assert!(script.contains(
        "DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l apt-get install -y --no-install-recommends --no-upgrade sqlite3"
    ));
    assert!(!script.contains("apt-get upgrade"));
    assert!(!script.contains("dist-upgrade"));
    assert!(script.contains("apt-get install --no-install-recommends --no-upgrade -y"));
    let pending = capture(Command::new("apt-get").args(["-s", "upgrade"]));
    let name = pending
        .stdout
        .lines()
        .find_map(|line| {
            line.strip_prefix("Inst ")
                .and_then(|rest| rest.split_whitespace().next())
        })
        .unwrap_or("openssl")
        .to_string();
    let plain =
        capture(Command::new("apt-get").args(["install", "-s", "--no-install-recommends", &name]));
    let held = capture(Command::new("apt-get").args([
        "install",
        "-s",
        "--no-install-recommends",
        "--no-upgrade",
        &name,
    ]));
    assert_eq!(held.code, 0, "{}", held.stderr);
    assert!(
        !held.stdout.contains(&format!("Inst {name} ")),
        "{}",
        held.stdout
    );
    assert!(
        held.stdout.contains(&format!("Skipping {name}")),
        "{}",
        held.stdout
    );
    assert!(plain.stdout.contains(&name), "{}", plain.stdout);
}

fn test_sqlite3_install_is_exact_and_skipped_when_present() {
    let script = repo_text("deploy/github/install-remote.sh");
    assert_eq!(script.matches(SQLITE_EXACT).count(), 1);
    assert!(!script.contains("apt-get upgrade"));
    assert!(!script.contains("dist-upgrade"));
    for line in script.lines() {
        if line.contains("sqlite3") && line.contains("apt-get") {
            assert!(line.contains(SQLITE_EXACT), "{line}");
            assert!(!line.contains("openssl"), "{line}");
        }
    }
    let (present, _, _) = apt_lines(false, false, "");
    assert!(
        !present.iter().any(|line| line.contains(SQLITE_EXACT)),
        "{present:?}"
    );
    assert!(!upgrade_lines(&present));
    let (missing, _, _) = apt_lines(true, false, "");
    assert_eq!(
        missing
            .iter()
            .filter(|line| line.as_str() == SQLITE_EXACT)
            .count(),
        1,
        "{missing:?}"
    );
    let exact_at = missing
        .iter()
        .position(|line| line == SQLITE_EXACT)
        .unwrap();
    let update_at = missing.iter().position(|line| line == UPDATE).unwrap();
    assert!(update_at < exact_at, "{missing:?}");
    assert_eq!(missing[exact_at - 1], UPDATE, "{missing:?}");
    assert!(!upgrade_lines(&missing));
    let (existing_present, present_out, present_err) = apt_lines(false, true, "");
    assert!(existing_present.is_empty(), "{present_err}");
    assert!(present_out.contains("installer-ran"), "{present_out}");
    let (existing_missing, missing_out, missing_err) = apt_lines(true, true, "");
    assert_eq!(
        existing_missing,
        vec![UPDATE.to_string(), SQLITE_EXACT.to_string()],
        "{missing_out}{missing_err}"
    );
    assert!(missing_out.contains("installer-ran"), "{missing_out}");
    assert!(!upgrade_lines(&existing_missing));
    let (failed, failed_out, failed_err) = apt_lines(true, true, "update");
    assert_eq!(failed, vec![UPDATE.to_string()], "{failed_out}{failed_err}");
    assert!(!failed_out.contains("installer-ran"), "{failed_out}");
}

fn test_smoke_deletes_the_box_when_exec_or_output_fails() {
    let remote = repo_text("deploy/github/install-remote.sh");
    let deploy = repo_text("deploy/github/deploy.sh");
    assert!(remote.contains("bash \"$dir/smoke.sh\" \"$address\""));
    assert!(remote.contains("\"$smoke_rc\" -eq 0"));
    assert!(deploy.contains("\"$script_dir/smoke.sh\""));

    let (ok, ok_log) = run_smoke(&[]);
    assert_eq!(ok.code, 0, "{}{}", ok.stderr, ok.stdout);
    assert_eq!(command_names(&ok_log), ["add", "create", "exec", "delete"]);
    let flat = ok_log.join("\n").replace('\0', " ");
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

    let (exec_failed, exec_log) = run_smoke(&[("KILN_EXEC_RC", "1")]);
    assert_ne!(exec_failed.code, 0, "{}", exec_failed.stderr);
    assert!(
        exec_failed.stderr.contains("box exec failed"),
        "{}",
        exec_failed.stderr
    );
    assert!(
        !exec_failed.stderr.contains("box delete failed"),
        "{}",
        exec_failed.stderr
    );
    assert_eq!(
        command_names(&exec_log),
        ["add", "create", "exec", "delete"]
    );

    let (mismatch, mismatch_log) = run_smoke(&[("KILN_EXEC_OUT", "nope")]);
    assert_ne!(mismatch.code, 0, "{}", mismatch.stderr);
    assert!(
        mismatch.stderr.contains("box exec output mismatch"),
        "{}",
        mismatch.stderr
    );
    assert_eq!(
        command_names(&mismatch_log),
        ["add", "create", "exec", "delete"]
    );

    let (deleted, _) = run_smoke(&[("KILN_DELETE_RC", "1")]);
    assert_ne!(deleted.code, 0, "{}", deleted.stderr);
    assert!(
        deleted.stderr.contains("box delete failed"),
        "{}",
        deleted.stderr
    );

    let (created, created_log) = run_smoke(&[("KILN_CREATE_RC", "1"), ("KILN_CREATE_BODY", "")]);
    assert_ne!(created.code, 0, "{}", created.stderr);
    assert!(
        created.stderr.contains("box create failed"),
        "{}",
        created.stderr
    );
    assert!(
        !command_names(&created_log)
            .iter()
            .any(|name| name == "delete")
    );

    let (leaked, leaked_log) = run_smoke(&[("KILN_CREATE_RC", "1"), ("KILN_CREATE_BODY", "box")]);
    assert_ne!(leaked.code, 0, "{}", leaked.stderr);
    assert!(
        leaked.stderr.contains("box create failed"),
        "{}",
        leaked.stderr
    );
    assert!(
        command_names(&leaked_log)
            .iter()
            .any(|name| name == "delete")
    );
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
    test_v044_uses_this_tree_and_later_tags_use_the_archive,
    test_apt_installs_missing_packages_without_upgrading,
    test_sqlite3_install_is_exact_and_skipped_when_present,
    test_smoke_deletes_the_box_when_exec_or_output_fails,
}

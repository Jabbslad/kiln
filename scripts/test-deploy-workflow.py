#!/usr/bin/env python3
"""Gates for the dark-forge workflows. They do not open a connection."""

import hashlib
import io
import os
import shlex
import shutil
import stat
import subprocess
import tarfile
import tempfile
import textwrap
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GUARD = ROOT / "deploy/github/guard.sh"


def workflow(name):
    return (ROOT / ".github/workflows" / name).read_text()


def run_guard(mode, **env):
    merged = os.environ.copy()
    for key in ("GITHUB_EVENT_NAME", "GITHUB_REF", "GITHUB_REPOSITORY"):
        merged.pop(key, None)
    merged.update(env)
    return subprocess.run(
        [
            "bash",
            "-c",
            'source "$1"; guard_github_event "$2"',
            "guard",
            str(GUARD),
            mode,
        ],
        cwd=ROOT,
        env=merged,
        text=True,
        capture_output=True,
        check=False,
    )


class DeployWorkflowTests(unittest.TestCase):
    def test_workflows_have_no_pull_request_trigger(self):
        for name in ("deploy.yml", "diagnose.yml", "deploy-release.yml"):
            text = workflow(name)
            self.assertNotIn("pull_request", text, name)
            self.assertNotIn("accept-new", text, name)
            self.assertNotIn("ssh-keyscan", text, name)
        for name in ("deploy.yml", "diagnose.yml"):
            text = workflow(name)
            self.assertIn("environment: dark-forge", text, name)
            self.assertIn("secrets.DARK_FORGE_SSH_KEY", text, name)
            self.assertIn("secrets.DARK_FORGE_KNOWN_HOSTS", text, name)
        caller = workflow("deploy-release.yml")
        self.assertNotIn("secrets.DARK_FORGE_SSH_KEY", caller)
        self.assertNotIn("secrets.DARK_FORGE_KNOWN_HOSTS", caller)
        self.assertNotIn("environment:", caller)
        self.assertIn("uses: ./.github/workflows/deploy.yml", caller)

    def test_deploy_is_manual_from_main(self):
        text = workflow("deploy.yml")
        self.assertIn("workflow_dispatch:", text)
        self.assertIn("workflow_call:", text)
        self.assertNotIn("\n  release:", text)
        self.assertNotIn("types: [published]", text)
        self.assertIn("group: dark-forge-deploy", text)
        self.assertIn("cancel-in-progress: false", text)
        self.assertIn("default: latest", text)
        self.assertEqual(text.count("secrets.DARK_FORGE_SSH_KEY"), 1)
        self.assertIn("name: Install the release\n", text)
        caller = workflow("deploy-release.yml")
        self.assertIn("types: [published]", caller)
        self.assertIn("tag: ${{ github.event.release.tag_name }}", caller)
        docs = (ROOT / "docs/releases.md").read_text()
        self.assertIn("Publishing a release runs", docs)
        self.assertNotIn("Publishing a release does not start it.", docs)
        self.assertIn("Leave required reviewers", docs)

    def test_diagnose_is_manual_and_read_only(self):
        text = workflow("diagnose.yml")
        self.assertIn("workflow_dispatch:", text)
        self.assertNotIn("release:", text)
        self.assertNotIn("inputs:", text)
        self.assertIn("group: dark-forge-diagnose", text)
        remote = (ROOT / "deploy/github/diagnose.sh").read_text()
        for forbidden in ("apt-get", "systemctl restart", "systemctl enable", "install.py", "rm ", "eval "):
            self.assertNotIn(forbidden, remote)
        ssh = (ROOT / "deploy/github/ssh.sh").read_text()
        self.assertIn("StrictHostKeyChecking=yes", ssh)
        self.assertNotIn("accept-new", ssh)
        self.assertNotIn("ssh-keyscan", ssh)

    def test_guard_refuses_pull_requests_forks_and_branch_dispatch(self):
        refused = run_guard(
            "deploy",
            GITHUB_EVENT_NAME="pull_request",
            GITHUB_REPOSITORY="Jabbslad/kiln",
            GITHUB_REF="refs/heads/main",
        )
        self.assertNotEqual(refused.returncode, 0, refused.stderr)
        self.assertIn("refusing event", refused.stderr)
        target = run_guard(
            "deploy",
            GITHUB_EVENT_NAME="pull_request_target",
            GITHUB_REPOSITORY="Jabbslad/kiln",
            GITHUB_REF="refs/heads/main",
        )
        self.assertNotEqual(target.returncode, 0, target.stderr)
        fork = run_guard(
            "diagnose",
            GITHUB_EVENT_NAME="workflow_dispatch",
            GITHUB_REPOSITORY="example/kiln",
            GITHUB_REF="refs/heads/main",
        )
        self.assertNotEqual(fork.returncode, 0, fork.stderr)
        self.assertIn("refusing a fork", fork.stderr)
        branch = run_guard(
            "deploy",
            GITHUB_EVENT_NAME="workflow_dispatch",
            GITHUB_REPOSITORY="Jabbslad/kiln",
            GITHUB_REF="refs/heads/feature",
        )
        self.assertNotEqual(branch.returncode, 0, branch.stderr)
        self.assertIn("only from main", branch.stderr)

    def test_guard_allows_main_dispatch_and_a_tag_release(self):
        dispatch = run_guard(
            "deploy",
            GITHUB_EVENT_NAME="workflow_dispatch",
            GITHUB_REPOSITORY="Jabbslad/kiln",
            GITHUB_REF="refs/heads/main",
        )
        self.assertEqual(dispatch.returncode, 0, dispatch.stderr)
        release = run_guard(
            "deploy",
            GITHUB_EVENT_NAME="release",
            GITHUB_REPOSITORY="Jabbslad/kiln",
            GITHUB_REF="refs/tags/v0.4.12",
        )
        self.assertEqual(release.returncode, 0, release.stderr)
        called = run_guard(
            "deploy",
            GITHUB_EVENT_NAME="workflow_call",
            GITHUB_REPOSITORY="Jabbslad/kiln",
            GITHUB_REF="refs/tags/v0.4.12",
        )
        self.assertEqual(called.returncode, 0, called.stderr)
        branch = run_guard(
            "deploy",
            GITHUB_EVENT_NAME="release",
            GITHUB_REPOSITORY="Jabbslad/kiln",
            GITHUB_REF="refs/heads/main",
        )
        self.assertNotEqual(branch.returncode, 0, branch.stderr)
        self.assertIn("stable tag", branch.stderr)
        diagnose_release = run_guard(
            "diagnose",
            GITHUB_EVENT_NAME="release",
            GITHUB_REPOSITORY="Jabbslad/kiln",
            GITHUB_REF="refs/tags/v0.4.4",
        )
        self.assertNotEqual(diagnose_release.returncode, 0, diagnose_release.stderr)
        self.assertIn("refusing event release", diagnose_release.stderr)
        local = run_guard("deploy")
        self.assertEqual(local.returncode, 0, local.stderr)

    def test_secrets_are_read_only_by_the_dark_forge_environment(self):
        for name in ("deploy.yml", "diagnose.yml"):
            text = workflow(name)
            self.assertEqual(text.count("environment: dark-forge"), 1, name)
            self.assertLess(
                text.index("environment: dark-forge"),
                text.index("secrets.DARK_FORGE_SSH_KEY"),
                name,
            )
            self.assertLess(
                text.index("environment: dark-forge"),
                text.index("secrets.DARK_FORGE_KNOWN_HOSTS"),
                name,
            )
            self.assertEqual(text.count("jobs:"), 1, name)
        docs = (ROOT / "docs/releases.md").read_text()
        self.assertIn("environment secrets", docs)
        self.assertIn("Jamie", docs)
        self.assertIn("deployment branches to `main`", docs)
        self.assertIn("deployment tags to `v*`", docs)
        self.assertNotIn("Leave the key out of the environment", docs)
        self.assertNotIn("repository secret", docs.lower())

    def test_older_tags_and_prereleases_are_refused_before_ssh(self):
        for tag in ("v0.4.3", "v0.4.4-rc.1", "v0.5.0-rc.1"):
            result, calls, ssh_calls = run_deploy(tag)
            self.assertNotEqual(result.returncode, 0, result.stderr)
            self.assertIn(f"refusing {tag}", result.stderr)
            self.assertIn("v0.4.4 and newer stable releases", result.stderr)
            self.assertEqual(calls, [], result.stderr)
            self.assertEqual(ssh_calls, [], result.stderr)
        latest, calls, ssh_calls = run_deploy(
            "latest",
            releases='[{"tag_name":"v0.4.3","draft":false}]',
        )
        self.assertNotEqual(latest.returncode, 0, latest.stderr)
        self.assertIn("refusing v0.4.3", latest.stderr)
        self.assertTrue(any("api.github.com" in call for call in calls), calls)
        self.assertFalse(any("releases/download" in call for call in calls), calls)
        self.assertEqual(ssh_calls, [], latest.stderr)

    def test_v044_uses_this_tree_and_later_tags_use_the_archive(self):
        tree = str(ROOT / "deploy/install.sh")
        current, calls, ssh_calls = run_deploy("v0.4.4", archive_installer="printf 'archive\\n'\n")
        self.assertEqual(current.returncode, 0, current.stderr)
        self.assertIn("deployed v0.4.4", current.stdout)
        self.assertTrue(any(tree in call for call in ssh_calls), ssh_calls)
        self.assertTrue(any(call.startswith("scp ") and call.endswith("installer.sh") for call in ssh_calls), ssh_calls)
        self.assertTrue(any("kiln-server-v0.4.4-" in call for call in calls), calls)

        later, _, later_ssh = run_deploy("v0.4.5", archive_installer="printf 'archive-v045\\n'\n")
        self.assertEqual(later.returncode, 0, later.stderr)
        self.assertIn("deployed v0.4.5", later.stdout)
        self.assertFalse(any("install.py" in call or "installer.py" in call or "installer.sh" in call for call in later_ssh), later_ssh)
        self.assertTrue(any("kiln-server-v0.4.5-" in call and call.startswith("scp ") for call in later_ssh), later_ssh)

        numeric, _, numeric_ssh = run_deploy("v0.4.10", archive_installer="printf 'archive-v0410\\n'\n")
        self.assertEqual(numeric.returncode, 0, numeric.stderr)
        self.assertFalse(any("installer.sh" in call for call in numeric_ssh), numeric_ssh)

        for tag in ("v0.4.4", "v0.4.5"):
            failed, _, failed_ssh = run_deploy(
                tag,
                archive_installer="printf 'must-not-run\\n'\n",
                bad_checksum=True,
            )
            self.assertNotEqual(failed.returncode, 0, failed.stdout + failed.stderr)
            self.assertIn("NOT match", failed.stderr)
            self.assertEqual(failed_ssh, [], failed.stderr)

        tree_run = run_remote("printf 'installer=archive\\n'\n", overlay="printf 'installer=tree\\n'\n")
        self.assertIn("installer=tree", tree_run.stdout)
        self.assertNotIn("installer=archive", tree_run.stdout)
        archive_run = run_remote("printf 'installer=archive\\n'\n")
        self.assertIn("installer=archive", archive_run.stdout)
        self.assertNotIn("installer=tree", archive_run.stdout)
        mismatch = run_remote(
            "printf 'installer=archive\\n'\n",
            overlay="printf 'installer=tree\\n'\n",
            bad_checksum=True,
        )
        self.assertNotIn("installer=", mismatch.stdout)
        self.assertIn("NOT match", mismatch.stderr)

    def test_apt_installs_missing_packages_without_upgrading(self):
        script = (ROOT / "deploy/github/install-remote.sh").read_text()
        self.assertIn("NEEDRESTART_MODE=l", script)
        self.assertIn(
            "DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l apt-get update",
            script,
        )
        self.assertIn(
            "DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l apt-get install -y --no-install-recommends --no-upgrade sqlite3",
            script,
        )
        self.assertNotIn("apt-get upgrade", script)
        self.assertNotIn("dist-upgrade", script)
        self.assertIn("apt-get install --no-install-recommends --no-upgrade -y", script)
        pending = subprocess.run(
            ["apt-get", "-s", "upgrade"],
            text=True,
            capture_output=True,
            check=False,
        )
        name = next(
            (line.split()[1] for line in pending.stdout.splitlines() if line.startswith("Inst ")),
            None,
        )
        if name is None:
            name = "openssl"
        plain = subprocess.run(
            ["apt-get", "install", "-s", "--no-install-recommends", name],
            text=True,
            capture_output=True,
            check=False,
        )
        held = subprocess.run(
            ["apt-get", "install", "-s", "--no-install-recommends", "--no-upgrade", name],
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(held.returncode, 0, held.stderr)
        self.assertNotIn(f"Inst {name} ", held.stdout)
        self.assertIn(f"Skipping {name}", held.stdout)
        self.assertIn(name, plain.stdout)

    def test_sqlite3_install_is_exact_and_skipped_when_present(self):
        update = "DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l apt-get update"
        exact = "DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l apt-get install -y --no-install-recommends --no-upgrade sqlite3"
        script = (ROOT / "deploy/github/install-remote.sh").read_text()
        self.assertEqual(script.count(exact), 1)
        self.assertNotIn("apt-get upgrade", script)
        self.assertNotIn("dist-upgrade", script)
        for line in script.splitlines():
            if "sqlite3" in line and "apt-get" in line:
                self.assertIn(exact, line)
                self.assertNotIn("openssl", line)
        present, _, _ = apt_lines(hide_sqlite3=False)
        self.assertNotIn(exact, present)
        self.assertFalse(upgrade_lines(present))
        missing, _, _ = apt_lines(hide_sqlite3=True)
        self.assertEqual(missing.count(exact), 1)
        self.assertLess(missing.index(update), missing.index(exact))
        self.assertEqual(missing[missing.index(exact) - 1], update)
        self.assertFalse(upgrade_lines(missing))
        existing_present, present_out, present_err = apt_lines(hide_sqlite3=False, existing=True)
        self.assertEqual(existing_present, [], present_err)
        self.assertIn("installer-ran", present_out)
        existing_missing, missing_out, missing_err = apt_lines(hide_sqlite3=True, existing=True)
        self.assertEqual(existing_missing, [update, exact], missing_out + missing_err)
        self.assertIn("installer-ran", missing_out)
        self.assertFalse(upgrade_lines(existing_missing))
        failed, failed_out, failed_err = apt_lines(hide_sqlite3=True, existing=True, apt_fail="update")
        self.assertEqual(failed, [update], failed_out + failed_err)
        self.assertNotIn("installer-ran", failed_out)

    def test_smoke_deletes_the_box_when_exec_or_output_fails(self):
        remote = (ROOT / "deploy/github/install-remote.sh").read_text()
        deploy = (ROOT / "deploy/github/deploy.sh").read_text()
        self.assertIn('bash "$dir/smoke.sh" "$address"', remote)
        self.assertIn('"$smoke_rc" -eq 0', remote)
        self.assertIn('"$script_dir/smoke.sh"', deploy)

        ok, ok_log = run_smoke()
        self.assertEqual(ok.returncode, 0, ok.stderr + ok.stdout)
        self.assertEqual(command_names(ok_log), ["add", "create", "exec", "delete"])
        flat = "\n".join(ok_log).replace("\0", " ")
        self.assertIn("--template ubuntu-4g --name kiln-smoke", flat)
        self.assertIn("--token-file /etc/kiln/admin.token", flat)
        self.assertIn("--ca-file /etc/kiln/ca.crt", flat)
        self.assertIn("https://127.0.0.1:8443", flat)
        self.assertIn(
            "exec 0123456789abcdef0123456789abcdef -- /bin/sh -c printf kiln-smoke-ok",
            flat,
        )

        exec_failed, exec_log = run_smoke(KILN_EXEC_RC="1")
        self.assertNotEqual(exec_failed.returncode, 0, exec_failed.stderr)
        self.assertIn("box exec failed", exec_failed.stderr)
        self.assertNotIn("box delete failed", exec_failed.stderr)
        self.assertEqual(command_names(exec_log), ["add", "create", "exec", "delete"])

        mismatch, mismatch_log = run_smoke(KILN_EXEC_OUT="nope")
        self.assertNotEqual(mismatch.returncode, 0, mismatch.stderr)
        self.assertIn("box exec output mismatch", mismatch.stderr)
        self.assertEqual(command_names(mismatch_log), ["add", "create", "exec", "delete"])

        deleted = run_smoke(KILN_DELETE_RC="1")[0]
        self.assertNotEqual(deleted.returncode, 0, deleted.stderr)
        self.assertIn("box delete failed", deleted.stderr)

        created = run_smoke(KILN_CREATE_RC="1", KILN_CREATE_BODY="")
        self.assertNotEqual(created[0].returncode, 0, created[0].stderr)
        self.assertIn("box create failed", created[0].stderr)
        self.assertNotIn("delete", command_names(created[1]))

        leaked, leaked_log = run_smoke(KILN_CREATE_RC="1", KILN_CREATE_BODY="box")
        self.assertNotEqual(leaked.returncode, 0, leaked.stderr)
        self.assertIn("box create failed", leaked.stderr)
        self.assertIn("delete", command_names(leaked_log))


def run_deploy(tag, releases="[]", archive_installer=None, bad_checksum=False):
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        log = tmp_path / "curl.log"
        ssh_log = tmp_path / "ssh.log"
        archive_path = tmp_path / "release.tar.gz"
        asset_name = f"kiln-server-{tag}-x86_64-unknown-linux-gnu.tar.gz"
        checksum_text = ""
        if archive_installer is not None:
            blob = tar_bytes({"install.sh": archive_installer})
            archive_path.write_bytes(blob)
            digest = "0" * 64 if bad_checksum else hashlib.sha256(blob).hexdigest()
            checksum_text = f"{digest}  {asset_name}\n"
            ssh = tmp_path / "ssh.sh"
            ssh.write_text(ssh_stub(ssh_log))
            ssh.chmod(ssh.stat().st_mode | stat.S_IEXEC)
        curl = tmp_path / "curl"
        curl.write_text(curl_stub(log, releases, archive_path if archive_installer is not None else None, checksum_text))
        curl.chmod(curl.stat().st_mode | stat.S_IEXEC)
        env = os.environ.copy()
        for key in ("GITHUB_EVENT_NAME", "GITHUB_REF", "GITHUB_REPOSITORY", "KILN_SSH_SH"):
            env.pop(key, None)
        env["PATH"] = f"{tmp}{os.pathsep}{env.get('PATH', '')}"
        env["KILN_RELEASE_TAG"] = tag
        if archive_installer is not None:
            env["KILN_SSH_SH"] = str(tmp_path / "ssh.sh")
        result = subprocess.run(
            ["bash", "deploy/github/deploy.sh"],
            cwd=ROOT,
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )
        calls = log.read_text().splitlines() if log.exists() else []
        ssh_calls = ssh_log.read_text().splitlines() if ssh_log.exists() else []
        return result, calls, ssh_calls


def path_without(name, root, *front):
    hidden = root / "hidden"
    hidden.mkdir()
    for directory in os.environ["PATH"].split(os.pathsep):
        base = Path(directory)
        if not base.is_dir():
            continue
        for entry in base.iterdir():
            dest = hidden / entry.name
            if entry.name == name or dest.exists():
                continue
            try:
                dest.symlink_to(entry)
            except OSError:
                continue
    return os.pathsep.join([*front, str(hidden)])


def upgrade_lines(lines):
    return any(
        line.split(" apt-get ", 1)[-1].split(" ", 1)[0] in ("upgrade", "dist-upgrade", "full-upgrade")
        for line in lines
        if " apt-get " in line
    )


def apt_lines(hide_sqlite3, existing=False, apt_fail=""):
    remote = Path(subprocess.check_output(["mktemp", "-d", "/tmp/kiln-deploy.XXXXXX"], text=True).strip())
    fake = Path(tempfile.mkdtemp())
    overlay = None
    try:
        name = "kiln-server-v0.4.4-x86_64-unknown-linux-gnu.tar.gz"
        blob = tar_bytes({"install.sh": "#!/bin/bash\nprintf 'installer-ran\\n'\n"})
        (remote / name).write_bytes(blob)
        (remote / f"{name}.sha256").write_text(f"{hashlib.sha256(blob).hexdigest()}  {name}\n")
        log = fake / "apt.log"
        apt = fake / "apt-get"
        apt.write_text(
            "#!/bin/bash\n"
            "umask 022\n"
            f"log={shlex.quote(str(log))}\n"
            "printf 'DEBIAN_FRONTEND=%s NEEDRESTART_MODE=%s apt-get %s\\n' "
            "\"${DEBIAN_FRONTEND-}\" \"${NEEDRESTART_MODE-}\" \"$*\" >> \"$log\"\n"
            "chmod a+r \"$log\"\n"
            'case " ${APT_FAIL-} " in *" $1 "*) exit 100;; esac\n'
            "exit 0\n"
        )
        apt.chmod(0o755)
        if hide_sqlite3:
            path = path_without("sqlite3", fake, str(fake))
        else:
            sqlite = fake / "sqlite3"
            sqlite.write_text("#!/bin/bash\nexit 0\n")
            sqlite.chmod(0o755)
            path = str(fake) + os.pathsep + os.environ["PATH"]
        os.chmod(fake, 0o755)
        os.chmod(remote, 0o755)
        hidden = fake / "hidden"
        if hidden.is_dir():
            os.chmod(hidden, 0o755)
        env = os.environ.copy()
        env["PATH"] = path
        env["APT_FAIL"] = apt_fail
        inner = ["bash", str(ROOT / "deploy/github/install-remote.sh"), str(remote), "127.0.0.1"]
        if existing:
            # Root mount namespace. Unprivileged user namespaces cannot mount
            # overlay on the GitHub runner, so the script never started there.
            overlay = Path(tempfile.mkdtemp(prefix="kiln-ov-"))
            os.chmod(overlay, 0o755)
            command = [
                "sudo",
                "-n",
                "env",
                f"PATH={path}",
                f"APT_FAIL={apt_fail}",
                f"OV={overlay}",
                "unshare",
                "--mount",
                "bash",
                "-c",
                "mkdir -p \"$OV/upper\" \"$OV/work\" \"$OV/merged\" && "
                "mount -t overlay overlay -o lowerdir=/etc,upperdir=\"$OV/upper\",workdir=\"$OV/work\" \"$OV/merged\" && "
                "mount --bind \"$OV/merged\" /etc && "
                "mkdir -p /etc/kiln && "
                "exec "
                + " ".join(shlex.quote(part) for part in inner),
            ]
        else:
            command = inner
        result = subprocess.run(
            command,
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )
        lines = log.read_text().splitlines() if log.exists() else []
        return lines, result.stdout, result.stderr
    finally:
        shutil.rmtree(remote, ignore_errors=True)
        shutil.rmtree(fake, ignore_errors=True)
        if overlay is not None:
            shutil.rmtree(overlay, ignore_errors=True)


def run_remote(archive_body, overlay=None, bad_checksum=False):
    remote = Path(subprocess.check_output(["mktemp", "-d", "/tmp/kiln-deploy.XXXXXX"], text=True).strip())
    fake = Path(tempfile.mkdtemp())
    try:
        name = "kiln-server-v0.4.4-x86_64-unknown-linux-gnu.tar.gz"
        blob = tar_bytes({"install.sh": archive_body})
        (remote / name).write_bytes(blob)
        digest = "0" * 64 if bad_checksum else hashlib.sha256(blob).hexdigest()
        (remote / f"{name}.sha256").write_text(f"{digest}  {name}\n")
        if overlay is not None:
            (remote / "installer.sh").write_text(overlay)
        apt = fake / "apt-get"
        apt.write_text("#!/bin/bash\nexit 0\n")
        apt.chmod(apt.stat().st_mode | stat.S_IEXEC)
        env = os.environ.copy()
        env["PATH"] = f"{fake}{os.pathsep}{env.get('PATH', '')}"
        return subprocess.run(
            ["bash", str(ROOT / "deploy/github/install-remote.sh"), str(remote), "127.0.0.1"],
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )
    finally:
        shutil.rmtree(remote, ignore_errors=True)
        shutil.rmtree(fake, ignore_errors=True)


def tar_bytes(members):
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
        for name, text in members.items():
            payload = text.encode()
            info = tarfile.TarInfo(name)
            info.size = len(payload)
            archive.addfile(info, io.BytesIO(payload))
    return buffer.getvalue()


def curl_stub(log, releases, archive_path, checksum_text):
    archive_literal = repr(str(archive_path)) if archive_path is not None else '""'
    return textwrap.dedent(
        f"""\
        #!/usr/bin/env python3
        import sys
        from pathlib import Path
        args = sys.argv[1:]
        with open({str(log)!r}, "a", encoding="utf-8") as handle:
            handle.write(" ".join(args) + "\\n")
        url = args[-1]
        output = args[args.index("--output") + 1] if "--output" in args else None
        if "api.github.com/repos/Jabbslad/kiln/releases" in url and "download" not in url:
            sys.stdout.write({releases!r} + "\\n")
            raise SystemExit(0)
        archive = {archive_literal}
        if archive and output and url.endswith(".tar.gz"):
            Path(output).write_bytes(Path(archive).read_bytes())
            raise SystemExit(0)
        checksum = {checksum_text!r}
        if checksum and output and url.endswith(".sha256"):
            Path(output).write_text(checksum)
            raise SystemExit(0)
        raise SystemExit(1)
        """
    )


def ssh_stub(log):
    return textwrap.dedent(
        f"""\
        #!/bin/bash
        printf '%s\\n' "$*" >> {sh_quote(log)}
        if [ "$1" = scp ]; then
          mkdir -p -- "$(dirname -- "$3")"
          cp -- "$2" "$3"
          exit 0
        fi
        if [ "$1" = ssh ]; then
          shift
          if [ "${{1:-}}" = -- ]; then
            shift
          fi
          if [ "${{1:-}}" = mktemp ]; then
            shift
            mktemp "$@"
            exit 0
          fi
          if [ "${{1:-}}" = rm ]; then
            shift
            rm "$@"
            exit 0
          fi
          exit 0
        fi
        exit 1
        """
    )


def run_smoke(**env_extra):
    with tempfile.TemporaryDirectory() as tmp:
        log = Path(tmp) / "kiln.log"
        kiln = Path(tmp) / "kiln"
        kiln.write_text(
            textwrap.dedent(
                f"""\
                #!/usr/bin/env python3
                import os, sys
                log = {str(log)!r}
                with open(log, "a", encoding="utf-8") as handle:
                    handle.write("\\0".join(sys.argv[1:]) + "\\n")
                args = sys.argv[1:]
                if "add" in args:
                    sys.exit(0)
                if "create" in args:
                    body = os.environ.get("KILN_CREATE_BODY", "box")
                    if body == "box":
                        sys.stdout.write(
                            '{{"id":"op","box_id":"0123456789abcdef0123456789abcdef",'
                            '"state":"succeeded","result":{{"type":"box",'
                            '"id":"0123456789abcdef0123456789abcdef","name":"kiln-smoke",'
                            '"state":"running","memory_mib":4096,"vcpus":1}},"error":null}}'
                        )
                    sys.exit(int(os.environ.get("KILN_CREATE_RC", "0")))
                if "exec" in args:
                    if os.environ.get("KILN_EXEC_RC", "0") == "0":
                        sys.stdout.write(os.environ.get("KILN_EXEC_OUT", "kiln-smoke-ok"))
                    sys.exit(int(os.environ.get("KILN_EXEC_RC", "0")))
                if "delete" in args:
                    sys.exit(int(os.environ.get("KILN_DELETE_RC", "0")))
                sys.exit(99)
                """
            )
        )
        kiln.chmod(kiln.stat().st_mode | stat.S_IEXEC)
        env = os.environ.copy()
        env["KILN_BIN"] = str(kiln)
        env.update(env_extra)
        result = subprocess.run(
            ["bash", str(ROOT / "deploy/github/smoke.sh"), "127.0.0.1"],
            cwd=ROOT,
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )
        calls = log.read_text().splitlines() if log.exists() else []
        return result, calls


def command_names(calls):
    names = []
    for call in calls:
        args = call.split("\0")
        for name in ("add", "create", "exec", "delete"):
            if name in args:
                names.append(name)
                break
    return names


def sh_quote(value):
    return "'" + str(value).replace("'", "'\\''") + "'"


if __name__ == "__main__":
    unittest.main()

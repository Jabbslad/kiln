#!/usr/bin/env python3
"""Gates for the dark-forge workflows. They do not open a connection."""

import os
import re
import stat
import subprocess
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
        for name in ("deploy.yml", "diagnose.yml"):
            text = workflow(name)
            self.assertNotIn("pull_request", text, name)
            self.assertNotIn("accept-new", text, name)
            self.assertNotIn("ssh-keyscan", text, name)
            self.assertIn("environment: dark-forge", text, name)
            self.assertIn("secrets.DARK_FORGE_SSH_KEY", text, name)
            self.assertIn("secrets.DARK_FORGE_KNOWN_HOSTS", text, name)

    def test_deploy_is_manual_on_main_or_a_published_release(self):
        text = workflow("deploy.yml")
        self.assertIn("workflow_dispatch:", text)
        self.assertIn("types: [published]", text)
        self.assertIn("group: dark-forge-deploy", text)
        self.assertIn("cancel-in-progress: false", text)
        self.assertIn("default: latest", text)

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

    def test_guard_allows_main_dispatch_and_published_release(self):
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
            GITHUB_REF="refs/tags/v0.4.4",
        )
        self.assertEqual(release.returncode, 0, release.stderr)
        diagnose_release = run_guard(
            "diagnose",
            GITHUB_EVENT_NAME="release",
            GITHUB_REPOSITORY="Jabbslad/kiln",
            GITHUB_REF="refs/tags/v0.4.4",
        )
        self.assertNotEqual(diagnose_release.returncode, 0, diagnose_release.stderr)
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

    def test_unsupported_tag_is_refused_before_ssh(self):
        for tag in ("v0.4.3", "v0.4.5", "v0.4.4-rc.1"):
            result, calls = run_deploy(tag)
            self.assertNotEqual(result.returncode, 0, result.stderr)
            self.assertIn("this installer supports only v0.4.4", result.stderr)
            self.assertIn(f"refusing {tag}", result.stderr)
            self.assertEqual(calls, [], result.stderr)
        latest, calls = run_deploy(
            "latest",
            releases='[{"tag_name":"v0.4.3","draft":false}]',
        )
        self.assertNotEqual(latest.returncode, 0, latest.stderr)
        self.assertIn("refusing v0.4.3", latest.stderr)
        self.assertTrue(any("api.github.com" in call for call in calls), calls)
        self.assertFalse(any("releases/download" in call for call in calls), calls)

    def test_supported_tag_is_the_installer_package_and_reaches_download(self):
        install = (ROOT / "deploy/install.py").read_text()
        match = re.search(r'target != "(\d+\.\d+\.\d+)"', install)
        self.assertIsNotNone(match)
        deploy = (ROOT / "deploy/github/deploy.sh").read_text()
        self.assertIn(f"supported_tag=v{match.group(1)}", deploy)
        self.assertIn('"$repo_root/deploy/install.py"', deploy)
        result, calls = run_deploy("v0.4.4")
        self.assertNotEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("supports only", result.stderr)
        self.assertTrue(
            any("kiln-server-v0.4.4-x86_64-unknown-linux-gnu.tar.gz" in call for call in calls),
            calls,
        )

    def test_apt_installs_missing_packages_without_upgrading(self):
        script = (ROOT / "deploy/github/install-remote.sh").read_text()
        self.assertIn("NEEDRESTART_MODE=l", script)
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


def run_deploy(tag, releases="[]"):
    with tempfile.TemporaryDirectory() as tmp:
        log = Path(tmp) / "curl.log"
        curl = Path(tmp) / "curl"
        curl.write_text(
            textwrap.dedent(
                f"""\
                #!/bin/bash
                printf '%s\\n' "$*" >> {sh_quote(log)}
                for arg in "$@"; do
                  case "$arg" in
                    *api.github.com/repos/Jabbslad/kiln/releases*)
                      printf '%s\\n' {sh_quote(releases)}
                      exit 0
                      ;;
                  esac
                done
                exit 1
                """
            )
        )
        curl.chmod(curl.stat().st_mode | stat.S_IEXEC)
        env = os.environ.copy()
        for key in ("GITHUB_EVENT_NAME", "GITHUB_REF", "GITHUB_REPOSITORY"):
            env.pop(key, None)
        env["PATH"] = f"{tmp}{os.pathsep}{env.get('PATH', '')}"
        env["KILN_RELEASE_TAG"] = tag
        result = subprocess.run(
            ["bash", "deploy/github/deploy.sh"],
            cwd=ROOT,
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )
        calls = log.read_text().splitlines() if log.exists() else []
        return result, calls


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

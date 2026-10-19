#!/usr/bin/env python3
"""Gates for the dark-forge workflows. They do not open a connection."""

import os
import subprocess
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


if __name__ == "__main__":
    unittest.main()

#!/usr/bin/env python3

import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "merge_queue_tested.py"
GATE = ROOT / "scripts" / "required_gate.py"
NEEDED = ("changes", "docs-check", "server-check", "client", "dashboard")
SHA_A = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
SHA_B = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
TREE_OTHER = "cccccccccccccccccccccccccccccccccccccccc"


def load():
    spec = importlib.util.spec_from_file_location("merge_queue_tested", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def load_gate():
    spec = importlib.util.spec_from_file_location("required_gate", GATE)
    module = importlib.util.module_from_spec(spec)
    sys.modules["required_gate"] = module
    spec.loader.exec_module(module)
    return module


def git(repo, *args):
    return subprocess.run(
        ["git", *args],
        cwd=repo,
        check=True,
        capture_output=True,
        text=True,
    )


def make_repo(repo: Path):
    git(repo, "init", "-q")
    git(repo, "config", "user.email", "test@example.com")
    git(repo, "config", "user.name", "Test")
    (repo / "README").write_text("kiln\n")
    git(repo, "add", "README")
    git(repo, "commit", "-q", "-m", "first")
    first = git(repo, "rev-parse", "HEAD").stdout.strip()
    git(repo, "commit", "-q", "--allow-empty", "-m", "second")
    second = git(repo, "rev-parse", "HEAD").stdout.strip()
    tree = git(repo, "rev-parse", "HEAD^{tree}").stdout.strip()
    return first, second, tree


def runs(tree, event="merge_group", conclusion="success", sha=SHA_A):
    def fetch(url, token):
        assert token == "test-token"
        if url.endswith(f"/commits/{sha}"):
            return {"commit": {"tree": {"sha": tree}}}
        return {
            "workflow_runs": [
                {"event": event, "conclusion": conclusion, "head_sha": sha},
            ]
        }

    return fetch


class MergeQueueTestedTests(unittest.TestCase):
    def test_same_tree_on_a_new_commit_skips_main_tests(self):
        module = load()
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            first, second, tree = make_repo(repo)
            self.assertNotEqual(first, second)
            self.assertEqual(git(repo, "rev-parse", f"{first}^{{tree}}").stdout.strip(), tree)
            self.assertTrue(
                module.decide(
                    {
                        "GITHUB_EVENT_NAME": "push",
                        "GITHUB_REF": "refs/heads/main",
                        "GITHUB_REPOSITORY": "Jabbslad/kiln",
                        "GITHUB_TOKEN": "test-token",
                        "GITHUB_API_URL": "https://api.github.com",
                    },
                    runs(tree),
                    repo,
                )
            )

    def test_a_different_tree_still_runs_tests(self):
        module = load()
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            make_repo(repo)
            self.assertFalse(
                module.decide(
                    _main_push(),
                    runs(TREE_OTHER),
                    repo,
                )
            )

    def test_pull_request_tag_and_dispatch_ignore_a_matching_tree(self):
        module = load()
        calls = []

        def fetch(url, token):
            calls.append(url)
            raise AssertionError(url)

        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            make_repo(repo)
            for environ in (
                {"GITHUB_EVENT_NAME": "pull_request", "GITHUB_REF": "refs/pull/1/merge"},
                {"GITHUB_EVENT_NAME": "push", "GITHUB_REF": "refs/tags/v1.2.3"},
                {"GITHUB_EVENT_NAME": "workflow_dispatch", "GITHUB_REF": "refs/heads/main"},
                {"GITHUB_EVENT_NAME": "merge_group", "GITHUB_REF": "refs/heads/gh-readonly-queue/main/pr-1"},
            ):
                self.assertFalse(module.decide(environ, fetch, repo))
        self.assertEqual(calls, [])

    def test_failed_or_non_queue_runs_do_not_count(self):
        module = load()
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            _first, _second, tree = make_repo(repo)
            for event, conclusion in (
                ("merge_group", "failure"),
                ("pull_request", "success"),
                ("push", "success"),
            ):
                self.assertFalse(module.decide(_main_push(), runs(tree, event, conclusion), repo))

    def test_lookup_failure_runs_tests_and_exits_zero(self):
        env = os.environ.copy()
        env.update(_main_push())
        output = Path(tempfile.mkdtemp()) / "output"
        env["GITHUB_OUTPUT"] = str(output)
        env["GITHUB_API_URL"] = "https://127.0.0.1:9"
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            make_repo(repo)
            result = subprocess.run(
                [sys.executable, str(SCRIPT)],
                cwd=repo,
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("skip_tests=false", result.stdout)
        self.assertEqual(output.read_text(), "skip_tests=false\n")
        self.assertNotIn("test-token", result.stderr)

    def test_pull_request_process_does_not_need_a_token(self):
        env = os.environ.copy()
        env["GITHUB_EVENT_NAME"] = "pull_request"
        env["GITHUB_REF"] = "refs/pull/8/merge"
        env.pop("GITHUB_TOKEN", None)
        output = Path(tempfile.mkdtemp()) / "output"
        env["GITHUB_OUTPUT"] = str(output)
        result = subprocess.run(
            [sys.executable, str(SCRIPT)],
            cwd=ROOT,
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output.read_text(), "skip_tests=false\n")


class BuildWorkflowTests(unittest.TestCase):
    def test_queue_runs_checks_and_only_push_or_dispatch_packages(self):
        text = (ROOT / ".github/workflows/build.yml").read_text()
        self.assertIn("merge_group:", text)
        self.assertIn("types: [checks_requested]", text)
        self.assertIn("branches: [main]", text)
        self.assertNotIn("if: github.event_name != 'pull_request'\n", text)
        self.assertNotIn("if: github.event_name != 'pull_request' &&", text)
        self.assertNotIn(
            "github.event_name == 'push' || github.event_name == 'workflow_dispatch' &&",
            text,
        )
        self.assertIn(
            "(github.event_name == 'push' || github.event_name == 'workflow_dispatch') && matrix.target == 'aarch64-apple-darwin'",
            text,
        )
        self.assertIn("github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')", text)
        self.assertIn("github.event.merge_group.base_sha", text)
        self.assertIn("github.event.merge_group.head_sha", text)
        self.assertIn("github.event_name != 'merge_group'", text)
        self.assertEqual(text.count("id: queue"), 3)
        self.assertEqual(text.count("python scripts/merge_queue_tested.py"), 3)
        self.assertIn("python scripts/test-merge-queue-tested.py", text)
        for name in ("deploy.yml", "diagnose.yml"):
            other = (ROOT / ".github/workflows" / name).read_text()
            self.assertNotIn("merge_group", other, name)
            self.assertNotIn("pull_request", other, name)

    def test_required_is_the_only_stable_check(self):
        text = (ROOT / ".github/workflows/build.yml").read_text()
        block = text.split("  required:\n", 1)[1].split("\n  draft-release:", 1)[0]
        needs = "needs: [" + ", ".join(NEEDED) + "]"
        self.assertIn(needs, block)
        self.assertIn("if: always()", block)
        self.assertIn("NEEDS_JSON: ${{ toJSON(needs) }}", block)
        self.assertIn("python scripts/required_gate.py", block)
        self.assertNotRegex(block, r"(?m)^    name:")
        self.assertNotIn("server-package", block)
        self.assertIn(
            "needs: [server-check, reuse, server-package, client, dashboard, attest]",
            text.split("  draft-release:\n", 1)[1],
        )


class RequiredGateTests(unittest.TestCase):
    def setUp(self):
        self.gate = load_gate()

    def test_docs_only_and_heavy_success_pass(self):
        docs = self._needs(
            changes="success",
            docs_check="success",
            server_check="skipped",
            client="skipped",
            dashboard="skipped",
            heavy="false",
        )
        heavy = self._needs(
            changes="success",
            docs_check="skipped",
            server_check="success",
            client="success",
            dashboard="success",
            heavy="true",
        )
        self.assertTrue(self.gate.evaluate(docs).ok)
        self.assertEqual(self.gate.evaluate(docs).blockers, ())
        self.assertTrue(self.gate.evaluate(heavy).ok)

    def test_failure_and_cancelled_block_each_job(self):
        for job_id in NEEDED:
            for result in ("failure", "cancelled"):
                payload = self._needs(
                    changes="success",
                    docs_check="skipped",
                    server_check="success",
                    client="success",
                    dashboard="success",
                    heavy="true",
                )
                payload[job_id]["result"] = result
                decision = self.gate.evaluate(payload)
                self.assertFalse(decision.ok)
                self.assertEqual(
                    [(item.job_id, item.result) for item in decision.blockers],
                    [(job_id, result)],
                )

    def test_unknown_result_blocks_and_outputs_are_ignored(self):
        payload = self._needs(
            changes="success",
            docs_check="skipped",
            server_check="success",
            client="success",
            dashboard="success",
            heavy="true",
        )
        payload["server-check"]["outputs"] = {"note": "failure"}
        self.assertTrue(self.gate.evaluate(payload).ok)
        payload["client"]["result"] = "SUCCESS"
        decision = self.gate.evaluate(payload)
        self.assertEqual(
            [(item.job_id, item.result) for item in decision.blockers],
            [("client", "SUCCESS")],
        )

    def test_malformed_needs_block(self):
        self.assertEqual(self.gate.evaluate([]).blockers[0].result, "malformed")
        self.assertEqual(self.gate.evaluate({}).blockers[0].result, "empty")
        self.assertEqual(
            self.gate.evaluate({"client": "failure"}).blockers,
            (self.gate.Blocker("client", "malformed"),),
        )

    def test_process_exits_with_the_blocker(self):
        payload = self._needs(
            changes="success",
            docs_check="skipped",
            server_check="success",
            client="failure",
            dashboard="cancelled",
            heavy="true",
        )
        env = os.environ.copy()
        env["NEEDS_JSON"] = json.dumps(payload)
        result = subprocess.run(
            [sys.executable, str(GATE)],
            cwd=ROOT,
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, "client failure\ndashboard cancelled\n")
        missing = os.environ.copy()
        missing.pop("NEEDS_JSON", None)
        result = subprocess.run(
            [sys.executable, str(GATE)],
            cwd=ROOT,
            env=missing,
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, "needs malformed\n")

    def _needs(self, *, changes, docs_check, server_check, client, dashboard, heavy):
        results = {
            "changes": changes,
            "docs-check": docs_check,
            "server-check": server_check,
            "client": client,
            "dashboard": dashboard,
        }
        return {
            job_id: {"result": result, "outputs": {"heavy": heavy} if job_id == "changes" else {}}
            for job_id, result in results.items()
        }


def _main_push():
    return {
        "GITHUB_EVENT_NAME": "push",
        "GITHUB_REF": "refs/heads/main",
        "GITHUB_REPOSITORY": "Jabbslad/kiln",
        "GITHUB_TOKEN": "test-token",
        "GITHUB_API_URL": "https://api.github.com",
    }


if __name__ == "__main__":
    unittest.main()

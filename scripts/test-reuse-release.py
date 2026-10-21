#!/usr/bin/env python3

import hashlib
import importlib.util
import io
import json
import os
import re
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zipfile
from datetime import datetime, timedelta, timezone
from pathlib import Path


def _load(filename):
    path = Path(__file__).with_name(filename)
    spec = importlib.util.spec_from_file_location(path.stem, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    try:
        spec.loader.exec_module(module)
    except Exception:
        sys.modules.pop(spec.name, None)
        raise
    return module


reuse = _load("reuse_release.py")
rv = reuse.release_version
REPO = Path(__file__).resolve().parent.parent
SHA = "5c5799909437f794c79f3de4ab29ec8954eceefc"
NOW = datetime(2026, 10, 20, 12, 0, tzinfo=timezone.utc)
ABSENT = (
    "Error: HTTP 404: Not Found "
    "(https://api.github.com/repos/Jabbslad/kiln/attestations/sha256:abc?per_page=30)\n"
)


def _package_jobs(event, reuse_result, mode, docs_only):
    """Which package jobs start. A cancelled reuse starts none."""
    if reuse_result == "cancelled":
        return False, False
    verified = reuse_result == "success" and mode == "reuse"
    build = reuse_result == "skipped" or (event == "push" and not verified)
    server = event != "pull_request" and build
    clients = (event != "pull_request" or not docs_only) and (
        event == "pull_request" or build
    )
    return server, clients


def _draft(event, tag, reuse_result, mode, packages_ok):
    """Where the draft reads archives. Empty means the draft does not start."""
    if event != "push" or not tag or reuse_result == "cancelled":
        return ""
    if reuse_result == "success" and mode == "reuse":
        return "reused"
    if packages_ok:
        return "packages"
    return ""


def job_graph():
    """The tag, main, pull-request, and dry-run graphs for one reuse outcome."""
    rows = (
        ("tag reuse", "push", True, "success", "reuse", False, True),
        ("tag no match", "push", True, "success", "fallback", False, True),
        ("tag error", "push", True, "failure", "", False, True),
        ("tag upload error", "push", True, "failure", "reuse", False, True),
        ("tag timeout", "push", True, "timed_out", "", False, True),
        ("main", "push", False, "skipped", "", False, True),
        ("code pr", "pull_request", False, "skipped", "", False, True),
        ("docs pr", "pull_request", False, "skipped", "", True, True),
        ("plain dispatch", "workflow_dispatch", False, "skipped", "", False, True),
        ("dry reuse", "workflow_dispatch", False, "success", "reuse", False, False),
        ("dry no match", "workflow_dispatch", False, "success", "fallback", False, False),
        ("dry error", "workflow_dispatch", False, "failure", "", False, False),
        ("cancelled tag", "push", True, "cancelled", "", False, False),
    )
    graph = []
    for name, event, tag, result, mode, docs_only, packages_ok in rows:
        server, clients = _package_jobs(event, result, mode, docs_only)
        draft = _draft(event, tag, result, mode, packages_ok and server and clients)
        graph.append((name, server, clients, draft))
    return tuple(graph)


EXPECTED_JOBS = (
    ("tag reuse", False, False, "reused"),
    ("tag no match", True, True, "packages"),
    ("tag error", True, True, "packages"),
    ("tag upload error", True, True, "packages"),
    ("tag timeout", True, True, "packages"),
    ("main", True, True, ""),
    ("code pr", False, True, ""),
    ("docs pr", False, False, ""),
    ("plain dispatch", True, True, ""),
    ("dry reuse", False, False, ""),
    ("dry no match", False, False, ""),
    ("dry error", False, False, ""),
    ("cancelled tag", False, False, ""),
)


def _job_body(text, name):
    lines = text.splitlines()
    try:
        start = lines.index(f"  {name}:")
    except ValueError as exc:
        raise AssertionError(f"missing job {name}") from exc
    body = []
    for line in lines[start + 1 :]:
        if len(line) > 2 and line.startswith("  ") and line[2] != " ":
            break
        body.append(line)
    return body


def _unfold_if(lines, index, continued):
    folded = lines[index].split("if:", 1)[1].strip()
    if folded not in {">-", ">", "|-", "|"}:
        return folded
    parts = []
    for nxt in lines[index + 1 :]:
        if nxt.startswith(continued):
            parts.append(nxt.strip())
            continue
        break
    return " ".join(parts)


def _attest_runs(expr, event, ref, cancelled=False, result="success"):
    text = expr.replace("!cancelled()", "False" if cancelled else "True")
    text = text.replace(
        "startsWith(github.ref, 'refs/tags/v')",
        "True" if ref.startswith("refs/tags/v") else "False",
    )
    text = text.replace("github.event_name", repr(event))
    text = text.replace("github.ref", repr(ref))
    for job in ("server-package", "client", "dashboard"):
        text = text.replace(f"needs.{job}.result", repr(result))
    text = text.replace("&&", " and ").replace("||", " or ")
    return eval(text, {"__builtins__": {}}, {})


def job_if_expression(text, name):
    """Return one job's if, unfolded, without a later step's if."""
    body = _job_body(text, name)
    for index, line in enumerate(body):
        if line.startswith("    if:"):
            return _unfold_if(body, index, "      ")
    raise AssertionError(f"{name} has no job if")


def step_if_expression(text, job, marker):
    """Return the if of the one step in job whose block contains marker."""
    body = _job_body(text, job)
    steps = []
    current = None
    for line in body:
        if line.startswith("      - "):
            if current is not None:
                steps.append(current)
            current = [line]
            continue
        if current is None:
            continue
        if line.startswith("        ") or line.strip() == "":
            current.append(line)
            continue
        steps.append(current)
        current = None
    if current is not None:
        steps.append(current)
    matches = [step for step in steps if any(marker in line for line in step)]
    if len(matches) != 1:
        raise AssertionError(f"{job} has {len(matches)} steps matching {marker}")
    step = matches[0]
    for index, line in enumerate(step):
        if line.strip().startswith("if:"):
            return _unfold_if(step, index, "          ")
    raise AssertionError(f"{job} step {marker} has no if")


def _run(sha=SHA, **overrides):
    fields = dict(
        run_id=10,
        head_sha=sha,
        head_branch="main",
        event="push",
        path=reuse.WORKFLOW_PATH,
        conclusion="success",
        status="completed",
    )
    fields.update(overrides)
    return reuse.WorkflowRun(**fields)


def _artifacts(expired=False, expires_at=None, names=None):
    if expires_at is None:
        expires_at = NOW + timedelta(days=30)
    chosen = reuse.PACKAGE_ARTIFACTS if names is None else names
    return tuple(
        reuse.PackageArtifact(name=name, expired=expired, expires_at=expires_at)
        for name in sorted(chosen)
    )


def _version(tag="v0.4.10"):
    return rv.parse_release_tag(tag)


def _archive(path: Path, version: str, release_json: bool) -> None:
    payload = json.dumps({"version": version, "target": "web"}).encode()
    if path.name.endswith(".zip"):
        with zipfile.ZipFile(path, "w") as archive:
            if release_json:
                archive.writestr("release.json", payload)
            else:
                archive.writestr("kiln.exe", b"kiln")
        return
    with tarfile.open(path, "w:gz") as archive:
        if release_json:
            info = tarfile.TarInfo("release.json")
            info.size = len(payload)
            archive.addfile(info, io.BytesIO(payload))
        else:
            data = b"kiln"
            info = tarfile.TarInfo("kiln")
            info.size = len(data)
            archive.addfile(info, io.BytesIO(data))


def _fill_archives(dist: Path, version) -> None:
    dist.mkdir(parents=True, exist_ok=True)
    for name in reuse.expected_archives(version):
        path = dist / name
        stamped = name.startswith("kiln-server-") or name.startswith("kiln-identity-") or name.startswith(
            "kiln-dashboard-"
        )
        _archive(path, version.tag, stamped)
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        (dist / f"{name}.sha256").write_text(f"{digest}  {name}\n", encoding="ascii")


class ReuseDecisionTests(unittest.TestCase):
    def test_selects_the_latest_successful_main_push(self):
        older = _run(run_id=3)
        newer = _run(run_id=9)
        chosen = reuse.select_run((older, newer, _run(run_id=4, event="workflow_dispatch")), SHA)
        self.assertEqual(chosen.run_id, 9)

    def test_refuses_a_run_that_is_not_this_workflow_on_main(self):
        cases = (
            {"head_sha": "0" * 40},
            {"head_branch": "feature"},
            {"event": "workflow_dispatch"},
            {"event": "pull_request"},
            {"path": ".github/workflows/deploy.yml"},
            {"conclusion": "failure"},
            {"status": "in_progress"},
        )
        for overrides in cases:
            with self.subTest(overrides=overrides):
                with self.assertRaises(reuse.ReuseRefused):
                    reuse.select_run((_run(**overrides),), SHA)
        with self.assertRaises(reuse.ReuseRefused):
            reuse.select_run((), SHA)
        with self.assertRaises(reuse.ReuseRefused):
            reuse.select_run((_run(),), "not-a-sha")

    def test_refuses_missing_extra_or_expired_artifacts(self):
        reuse.require_artifacts(_artifacts(), NOW)
        with self.assertRaises(reuse.ReuseRefused):
            reuse.require_artifacts(_artifacts(names=reuse.PACKAGE_ARTIFACTS - {"package-server"}), NOW)
        with self.assertRaises(reuse.ReuseRefused):
            reuse.require_artifacts(
                _artifacts(names=reuse.PACKAGE_ARTIFACTS | {"package-extra"}),
                NOW,
            )
        with self.assertRaises(reuse.ReuseRefused):
            reuse.require_artifacts(_artifacts(expired=True), NOW)
        with self.assertRaises(reuse.ReuseRefused):
            reuse.require_artifacts(_artifacts(expires_at=NOW), NOW)

    def test_absent_attestation_is_allowed_only_before_the_workflow_attests(self):
        old = "uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4\n"
        new = old + f"        uses: {reuse.ATTEST_ACTION}e8998f949152b193b063cb0ec769d69d929409be # v2.4.0\n"
        self.assertEqual(reuse.attestation_gate(old, ("absent",) * 7), "not-produced")
        self.assertEqual(reuse.attestation_gate(new, ("verified",) * 7), "verified")
        with self.assertRaises(reuse.ReuseRefused):
            reuse.attestation_gate(new, ("absent",) * 7)
        with self.assertRaises(reuse.ReuseRefused):
            reuse.attestation_gate(old, ("verified", "absent"))
        with self.assertRaises(reuse.ReuseRefused):
            reuse.attestation_gate(new, ("rejected",))
        self.assertFalse(reuse.workflow_attests("# actions/attest-build-provenance@mentioned\n"))

    def test_classifies_the_real_missing_attestation_error(self):
        self.assertEqual(reuse.classify_verify(1, ABSENT), "absent")
        self.assertEqual(reuse.classify_verify(0, ""), "verified")
        self.assertEqual(reuse.classify_verify(1, "certificate identity did not match"), "rejected")

    def test_checksum_mismatch_refuses_and_a_match_keeps_the_bytes(self):
        version = _version()
        with tempfile.TemporaryDirectory() as tmp:
            dist = Path(tmp)
            _fill_archives(dist, version)
            before = (dist / "kiln-dashboard-v0.4.10.tar.gz").read_bytes()
            windows = dist / "kiln-v0.4.10-x86_64-pc-windows-msvc.zip.sha256"
            windows.write_bytes(windows.read_bytes().replace(b"\n", b"\r\n"))
            reuse.verify_and_refresh_sidecars(dist, version)
            self.assertEqual((dist / "kiln-dashboard-v0.4.10.tar.gz").read_bytes(), before)
            self.assertTrue(windows.read_bytes().endswith(b"\n"))
            self.assertFalse(windows.read_bytes().endswith(b"\r\n"))
            sidecar = dist / "kiln-dashboard-v0.4.10.tar.gz.sha256"
            sidecar.write_text("0" * 64 + "  kiln-dashboard-v0.4.10.tar.gz\n", encoding="ascii")
            with self.assertRaises(reuse.ReuseRefused):
                reuse.verify_and_refresh_sidecars(dist, version)

    def test_finish_writes_the_sixteen_asset_release_set(self):
        version = _version()
        with tempfile.TemporaryDirectory() as tmp:
            dist = Path(tmp)
            _fill_archives(dist, version)
            reuse.finish_dist(REPO, dist, version)
            self.assertEqual({path.name for path in dist.iterdir()}, reuse.expected_release_files(version))
            self.assertEqual(len(list(dist.iterdir())), 16)
            completed = subprocess.run(
                ["sha256sum", "-c", *[path.name for path in sorted(dist.glob("*.sha256"))]],
                cwd=dist,
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(completed.returncode, 0, completed.stderr)
            self.assertIn("version=0.4.10", (dist / "install.sh").read_text(encoding="utf-8"))

    def test_parser_rejects_a_paginated_artifact_list(self):
        page = {
            "total_count": 2,
            "artifacts": [
                {
                    "name": "package-server",
                    "expired": False,
                    "expires_at": "2026-11-19T00:00:00Z",
                }
            ],
        }
        with self.assertRaises(reuse.ReuseRefused):
            reuse.parse_artifacts(page, NOW)

    def test_attest_signs_the_archives_a_tag_publishes(self):
        text = (REPO / ".github/workflows/build.yml").read_text(encoding="utf-8")
        expr = job_if_expression(text, "attest")
        self.assertIn(
            "!cancelled()",
            expr,
            "a skipped reuse ancestor makes implicit success() false",
        )
        for result in (
            "needs.server-package.result == 'success'",
            "needs.client.result == 'success'",
            "needs.dashboard.result == 'success'",
        ):
            self.assertIn(result, expr)
        self.assertIn("github.ref == 'refs/heads/main'", expr)
        self.assertIn("startsWith(github.ref, 'refs/tags/v')", expr)
        self.assertTrue(_attest_runs(expr, "push", "refs/heads/main"))
        self.assertTrue(_attest_runs(expr, "push", "refs/tags/v0.4.11"))
        self.assertTrue(_attest_runs(expr, "workflow_dispatch", "refs/heads/main"))
        self.assertFalse(
            _attest_runs(expr, "workflow_dispatch", "refs/heads/feature"),
            "a non-main dispatch must not attest",
        )
        self.assertFalse(_attest_runs(expr, "push", "refs/heads/feature"))
        docs = (REPO / "docs/releases.md").read_text(encoding="utf-8")
        documented = re.search(r"--cert-identity-regex '([^']+)'", docs)
        self.assertIsNotNone(documented)
        identity = re.compile(documented.group(1))

        def subject(ref):
            return f"https://github.com/Jabbslad/kiln/.github/workflows/build.yml@{ref}"

        self.assertIsNotNone(identity.fullmatch(subject("refs/heads/main")))
        self.assertIsNotNone(identity.fullmatch(subject("refs/tags/v0.4.11")))
        self.assertIsNone(identity.fullmatch(subject("refs/heads/feature")))
        self.assertIsNone(identity.fullmatch(subject("refs/heads/main-backup")))

    def test_workflow_keeps_drafts_and_does_not_move_tags(self):
        text = (REPO / ".github/workflows/build.yml").read_text(encoding="utf-8")
        self.assertEqual(text.count("gh release create"), 1)
        self.assertNotIn("--draft", text)
        self.assertIn("gh attestation verify", text)
        self.assertIn("--cert-identity-regex", text)
        self.assertIn("--source-ref", text)
        self.assertIn("needs.attest.result == 'success'", text)
        self.assertNotIn("git push", text)
        self.assertNotIn("git tag", text)
        granted = [line for line in text.splitlines() if line.strip() == "actions: write"]
        self.assertEqual(granted, [])
        self.assertEqual(text.count("retention-days: 30"), 4)
        self.assertIn("actions/attest-build-provenance@e8998f949152b193b063cb0ec769d69d929409be", text)
        self.assertIn("dry_run:", text)
        self.assertIn("needs.reuse.outputs.mode == 'fallback'", text)
        draft_if = job_if_expression(text, "draft-release")
        self.assertIn(
            "(needs.reuse.result == 'success' && needs.reuse.outputs.mode == 'reuse')",
            draft_if,
        )
        step_only = (
            "  other:\n"
            "    if: needs.reuse.result == 'success' && needs.reuse.outputs.mode == 'reuse'\n"
            "  draft-release:\n"
            "    if: >-\n"
            "      ((needs.reuse.outputs.mode == 'reuse') ||\n"
            "        (needs.server-package.result == 'success'))\n"
            "    steps:\n"
            "      - if: needs.reuse.result == 'success' && needs.reuse.outputs.mode == 'reuse'\n"
        )
        self.assertNotIn(
            "needs.reuse.result == 'success'",
            job_if_expression(step_only, "draft-release"),
        )
        reuse_guard = "needs.reuse.result == 'success' && needs.reuse.outputs.mode == 'reuse'"
        self.assertIn(reuse_guard, step_if_expression(text, "draft-release", "name: reused-dist"))
        self.assertIn(reuse_guard, job_if_expression(text, "dry-run-assets"))
        sibling = (
            "  draft-release:\n"
            "    steps:\n"
            "      - uses: actions/download-artifact\n"
            "        if: needs.reuse.outputs.mode == 'reuse'\n"
            "        with:\n"
            "          name: reused-dist\n"
            "      - uses: actions/download-artifact\n"
            "        if: needs.reuse.result == 'success' && needs.reuse.outputs.mode == 'reuse'\n"
            "        with:\n"
            "          name: package\n"
        )
        self.assertEqual(
            step_if_expression(sibling, "draft-release", "name: reused-dist"),
            "needs.reuse.outputs.mode == 'reuse'",
        )
        self.assertEqual(
            text.count("!(needs.reuse.result == 'success' && needs.reuse.outputs.mode == 'reuse')"),
            3,
        )
        self.assertIn("needs.reuse.result != 'success' || needs.reuse.outputs.mode != 'reuse'", text)
        self.assertEqual(text.count("!startsWith(github.ref, 'refs/tags/v')"), 6)
        self.assertNotIn("--auto", text)
        self.assertEqual(job_graph(), EXPECTED_JOBS)

    def test_fetch_refusal_sets_fallback_without_publishing(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = os.environ.copy()
            env.update(
                REUSE_EVENT="workflow_dispatch",
                REUSE_SHA_INPUT="not-a-sha",
                REUSE_VERSION="",
                GH_REPO="Jabbslad/kiln",
                GITHUB_OUTPUT=str(Path(tmp) / "out"),
            )
            completed = subprocess.run(
                [sys.executable, str(Path(__file__).with_name("reuse_release.py")), "fetch", "--dist", str(Path(tmp) / "dist")],
                check=False,
                capture_output=True,
                text=True,
                env=env,
            )
            self.assertEqual(completed.returncode, 0, completed.stderr)
            output = (Path(tmp) / "out").read_text(encoding="utf-8")
            self.assertIn("mode=fallback", output)
            self.assertNotIn("gh release", output)
            self.assertNotIn("ghp_", output + completed.stderr)


if __name__ == "__main__":
    unittest.main()

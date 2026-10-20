#!/usr/bin/env python3

import hashlib
import importlib.util
import os
import re
import stat
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "check_bootstrap_pins.py"
WORKFLOW = ROOT / ".github" / "workflows" / "bootstrap-pins.yml"
BUILD = ROOT / ".github" / "workflows" / "build.yml"
DOCS = ROOT / "docs" / "releases.md"
INSTALL = ROOT / "deploy" / "bootstrap" / "install.sh"

LINUX = "a" * 64
SERVER = "b" * 64
PIN = (
    "    version=1.2.3\n"
    f"client:x86_64-unknown-linux-gnu {LINUX}\n"
    f"server:x86_64-unknown-linux-gnu {SERVER}\n"
)
DOCS_FIXTURE = (
    "verify with `gh attestation verify ARCHIVE --repo Jabbslad/kiln "
    "--cert-identity-regex "
    "'^https://github\\.com/Jabbslad/kiln/\\.github/workflows/build\\.yml@refs/"
    "(heads/main|tags/v[0-9]+\\.[0-9]+\\.[0-9]+)$'`\n"
)


def load():
    spec = importlib.util.spec_from_file_location("check_bootstrap_pins", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    sys.modules["check_bootstrap_pins"] = module
    spec.loader.exec_module(module)
    return module


class ParseTests(unittest.TestCase):
    def test_pin_rows_become_archive_names(self):
        module = load()
        pin = module.parse_pin(PIN)
        self.assertEqual(pin.version, "1.2.3")
        self.assertEqual(
            pin.digests,
            (
                ("kiln-v1.2.3-x86_64-unknown-linux-gnu.tar.gz", LINUX),
                ("kiln-server-v1.2.3-x86_64-unknown-linux-gnu.tar.gz", SERVER),
            ),
        )

    def test_two_versions_or_a_slash_in_the_target_are_rejected(self):
        module = load()
        doubled = PIN + "    version=1.2.4\n"
        slashed = f"    version=1.2.3\nclient:../secret {LINUX}\n"
        for text in (doubled, slashed, "    version=1.2.3\n"):
            with self.subTest(text=text):
                with self.assertRaises(module.PinError):
                    module.parse_pin(text)

    def test_the_real_pin_names_the_published_sentence(self):
        module = load()
        pin = module.parse_pin(INSTALL.read_text(encoding="utf-8"))
        docs = DOCS.read_text(encoding="utf-8")
        self.assertIn(f"The published bootstrap pin is v{pin.version}.", docs)
        self.assertEqual(len(pin.digests), 4)
        for name, digest in pin.digests:
            self.assertRegex(name, r"^kiln(-server)?-v[0-9]+\.[0-9]+\.[0-9]+-.+\.tar\.gz$")
            self.assertEqual(len(digest), 64)

    def test_documented_identity_accepts_main_and_tags(self):
        module = load()
        repo, identity = module.parse_verify(DOCS.read_text(encoding="utf-8"))
        self.assertEqual(repo, "Jabbslad/kiln")
        pattern = re.compile(identity)
        base = "https://github.com/Jabbslad/kiln/.github/workflows/build.yml@"
        self.assertIsNotNone(pattern.fullmatch(base + "refs/heads/main"))
        self.assertIsNotNone(pattern.fullmatch(base + "refs/tags/v0.4.12"))
        self.assertIsNone(pattern.fullmatch(base + "refs/heads/feature"))
        self.assertIsNone(pattern.fullmatch(base + "refs/tags/v0.4"))
        self.assertIsNone(pattern.fullmatch(base + "refs/heads/main-backup"))
        with self.assertRaises(module.PinError):
            module.parse_verify("no command here\n")


class CheckTests(unittest.TestCase):
    def setUp(self):
        self.module = load()
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)

    def pin_for(self, blob):
        digest = hashlib.sha256(blob).hexdigest()
        name = "kiln-v1.2.3-x86_64-unknown-linux-gnu.tar.gz"
        return self.module.Pin("1.2.3", ((name, digest),)), name

    def fetch_of(self, blob, name):
        def fetch(version, archive):
            self.assertEqual(version, "1.2.3")
            self.assertEqual(archive, name)
            path = self.root / archive
            path.write_bytes(blob)
            return path

        return fetch

    def test_matching_bytes_pass_when_attestation_passes(self):
        blob = b"archive-bytes"
        pin, name = self.pin_for(blob)
        seen = []

        def verify(path, repo, identity):
            seen.append((path.name, repo, identity, path.read_bytes()))
            return 0, ""

        self.module.check(
            pin,
            "Jabbslad/kiln",
            "^https://example$",
            self.fetch_of(blob, name),
            verify,
        )
        self.assertEqual(seen, [(name, "Jabbslad/kiln", "^https://example$", blob)])

    def test_a_wrong_digest_fails_before_attestation(self):
        pin, name = self.pin_for(b"expected")

        def verify(path, repo, identity):
            raise AssertionError("attested a mismatched archive")

        with self.assertRaises(self.module.PinError) as caught:
            self.module.check(
                pin,
                "Jabbslad/kiln",
                "^https://example$",
                self.fetch_of(b"other-bytes", name),
                verify,
            )
        self.assertIn(name, str(caught.exception))
        self.assertIn("sha256", str(caught.exception))

    def test_a_failed_attestation_rejects_matching_bytes(self):
        blob = b"archive-bytes"
        pin, name = self.pin_for(blob)

        def verify(path, repo, identity):
            return 1, "certificate identity rejected"

        with self.assertRaises(self.module.PinError) as caught:
            self.module.check(
                pin,
                "Jabbslad/kiln",
                "^https://example$",
                self.fetch_of(blob, name),
                verify,
            )
        message = str(caught.exception)
        self.assertIn(name, message)
        self.assertIn("attestation failed", message)
        self.assertIn("certificate identity rejected", message)

    def test_verify_runs_the_documented_command_and_hides_the_token(self):
        module = load()
        repo, identity = module.parse_verify(DOCS_FIXTURE)
        archive = self.root / "archive.tar.gz"
        archive.write_bytes(b"bytes")
        bindir = self.root / "bin"
        bindir.mkdir()
        log = self.root / "argv"
        token = "super-secret-token"
        gh = bindir / "gh"
        gh.write_text(
            "#!/bin/sh\n"
            f"printf '%s\\n' \"$@\" > {log}\n"
            "printf '%s\\n' \"$GH_TOKEN\" >&2\n"
            "exit 1\n"
        )
        gh.chmod(gh.stat().st_mode | stat.S_IEXEC)
        env = os.environ.copy()
        env["PATH"] = str(bindir) + os.pathsep + env.get("PATH", "")
        env["GH_TOKEN"] = token
        previous = os.environ.get("PATH"), os.environ.get("GH_TOKEN")
        os.environ["PATH"] = env["PATH"]
        os.environ["GH_TOKEN"] = token
        try:
            code, detail = module._verify(archive, repo, identity)
        finally:
            if previous[0] is None:
                os.environ.pop("PATH", None)
            else:
                os.environ["PATH"] = previous[0]
            if previous[1] is None:
                os.environ.pop("GH_TOKEN", None)
            else:
                os.environ["GH_TOKEN"] = previous[1]
        self.assertEqual(code, 1)
        self.assertNotIn(token, detail)
        recorded = log.read_text(encoding="utf-8").splitlines()
        self.assertEqual(
            recorded,
            [
                "attestation",
                "verify",
                str(archive),
                "--repo",
                repo,
                "--cert-identity-regex",
                identity,
            ],
        )
        pattern = re.compile(identity)
        base = "https://github.com/Jabbslad/kiln/.github/workflows/build.yml@"
        self.assertIsNotNone(pattern.fullmatch(base + "refs/heads/main"))
        self.assertIsNotNone(pattern.fullmatch(base + "refs/tags/v1.2.3"))
        self.assertIsNone(pattern.fullmatch(base + "refs/heads/feature"))


class WorkflowTests(unittest.TestCase):
    def test_the_job_runs_on_pin_files_and_a_schedule(self):
        text = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("\n  pull_request:\n", text)
        self.assertNotIn("pull_request_target", text)
        paths = text.split("pull_request:\n", 1)[1].split("\n  schedule:\n", 1)[0]
        for path in (
            "deploy/bootstrap/install.sh",
            "docs/releases.md",
            "scripts/check_bootstrap_pins.py",
            ".github/workflows/bootstrap-pins.yml",
        ):
            self.assertIn(f"- {path}\n", paths)
        schedule = text.split("\n  schedule:\n", 1)[1].split("\npermissions:\n", 1)[0]
        self.assertRegex(schedule, r"cron: '[0-9* ]+'")
        self.assertIn("python scripts/check_bootstrap_pins.py\n", text)
        self.assertIn("attestations: read\n", text)
        self.assertIn("contents: read\n", text)
        self.assertNotIn("contents: write", text)
        self.assertNotIn("secrets.", text)
        self.assertIn("GH_TOKEN: ${{ github.token }}\n", text)
        self.assertIn("persist-credentials: false", text)

    def test_build_runs_the_pin_check_unit_test_twice(self):
        text = BUILD.read_text(encoding="utf-8")
        self.assertEqual(text.count("python scripts/test-check-bootstrap-pins.py"), 2)
        before, after = text.split("\n  server-check:\n", 1)
        self.assertEqual(before.count("python scripts/test-check-bootstrap-pins.py"), 1)
        self.assertEqual(after.count("python scripts/test-check-bootstrap-pins.py"), 1)


if __name__ == "__main__":
    unittest.main()

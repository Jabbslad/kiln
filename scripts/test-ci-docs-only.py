#!/usr/bin/env python3
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import ci_docs_only


class DocsOnlyTests(unittest.TestCase):
    def test_runbooks_and_markdown_skip_the_compile_jobs(self):
        paths = [
            "docs/runbooks/2026-10-19-public-door.md",
            "docs/releases.md",
            "README.md",
            "web/README.md",
            "AGENTS.md",
            "docs/picture.png",
        ]
        self.assertFalse(ci_docs_only.needs_heavy(paths))
        self.assertEqual(self.classify(paths), "heavy=false\n")

    def test_code_scripts_workflows_deploy_and_locks_build(self):
        samples = [
            "crates/kiln-server/src/main.rs",
            "scripts/test-bootstrap.py",
            "scripts/README.md",
            ".github/workflows/build.yml",
            "deploy/bootstrap/README.md",
            "deploy/install.sh",
            "Cargo.toml",
            "crates/kiln-api/Cargo.toml",
            "Cargo.lock",
            "web/package-lock.json",
            "web/package.json",
            "images/build-ubuntu.sh",
            "docs/notes.rs",
            "docs/tool.py",
            "docs/extra.json",
        ]
        for path in samples:
            with self.subTest(path=path):
                self.assertTrue(ci_docs_only.needs_heavy([path, "docs/releases.md"]))
                self.assertEqual(self.classify([path]), "heavy=true\n")

    def test_empty_diff_builds(self):
        self.assertTrue(ci_docs_only.needs_heavy([]))
        self.assertTrue(ci_docs_only.needs_heavy(["", "  "]))
        self.assertEqual(self.classify([]), "heavy=true\n")

    def test_rename_out_of_rust_lists_both_sides(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.git(root, "init")
            self.git(root, "config", "user.email", "test@example.com")
            self.git(root, "config", "user.name", "test")
            source = root / "crates" / "kiln-server" / "src"
            source.mkdir(parents=True)
            (source / "main.rs").write_text("fn main() {}\n")
            self.git(root, "add", ".")
            self.git(root, "commit", "-m", "add rust")
            base = self.git(root, "rev-parse", "HEAD").stdout.strip()
            dest = root / "docs" / "runbooks"
            dest.mkdir(parents=True)
            self.git(
                root,
                "mv",
                "crates/kiln-server/src/main.rs",
                "docs/runbooks/retired.md",
            )
            self.git(root, "commit", "-m", "move rust under docs")
            head = self.git(root, "rev-parse", "HEAD").stdout.strip()
            paths = ci_docs_only.changed_paths(base, head, cwd=root)
            self.assertIn("crates/kiln-server/src/main.rs", paths)
            self.assertIn("docs/runbooks/retired.md", paths)
            self.assertTrue(ci_docs_only.needs_heavy(paths))
            self.assertFalse(ci_docs_only.needs_heavy(["docs/runbooks/retired.md"]))
            detected = self.git(root, "diff", "--name-only", base, head).stdout.splitlines()
            self.assertNotIn("crates/kiln-server/src/main.rs", detected)

    def classify(self, paths):
        completed = subprocess.run(
            [sys.executable, str(Path(ci_docs_only.__file__).resolve())],
            input="".join(path + "\n" for path in paths),
            text=True,
            capture_output=True,
            check=True,
        )
        self.assertEqual(completed.stderr, "")
        return completed.stdout

    def git(self, root, *args):
        return subprocess.run(
            ["git", *args],
            cwd=root,
            check=True,
            capture_output=True,
            text=True,
        )


if __name__ == "__main__":
    unittest.main()

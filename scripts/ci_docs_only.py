#!/usr/bin/env python3
"""Classify a diff as documentation or a full build."""

import subprocess
import sys

HEAVY_PREFIXES = ("deploy/", "scripts/", ".github/")

LOCKFILES = frozenset(
    {
        "Cargo.lock",
        "package-lock.json",
        "npm-shrinkwrap.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "bun.lock",
        "bun.lockb",
        "poetry.lock",
        "uv.lock",
        "composer.lock",
        "Gemfile.lock",
        "go.sum",
    }
)

CODE_SUFFIXES = (
    ".rs",
    ".py",
    ".sh",
    ".bash",
    ".js",
    ".mjs",
    ".cjs",
    ".ts",
    ".tsx",
    ".jsx",
    ".toml",
    ".yml",
    ".yaml",
    ".json",
)


def normalize(path):
    rel = path.strip().replace("\\", "/").lstrip("/")
    if rel.startswith("./"):
        rel = rel[2:]
    return rel


def is_documentation(path):
    rel = normalize(path)
    if not rel or rel.endswith("/"):
        return False
    if rel.startswith(HEAVY_PREFIXES):
        return False
    name = rel.rsplit("/", 1)[-1]
    if name == "Cargo.toml" or name in LOCKFILES or name.endswith(".lock"):
        return False
    if name.endswith(".md"):
        return True
    return rel.startswith("docs/") and not name.endswith(CODE_SUFFIXES)


def needs_heavy(paths):
    cleaned = [normalize(path) for path in paths if path.strip()]
    # An empty listing still builds. Silence is not a documentation diff.
    if not cleaned:
        return True
    return any(not is_documentation(path) for path in cleaned)


def changed_paths(base, head, cwd=None):
    result = subprocess.run(
        ["git", "diff", "--name-only", "--no-renames", base, head],
        cwd=cwd,
        check=True,
        capture_output=True,
        text=True,
    )
    return [line for line in result.stdout.splitlines() if line.strip()]


def main(argv):
    if len(argv) == 3:
        paths = changed_paths(argv[1], argv[2])
    else:
        paths = sys.stdin.read().splitlines()
    print("heavy=true" if needs_heavy(paths) else "heavy=false")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

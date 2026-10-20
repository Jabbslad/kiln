#!/usr/bin/env python3
"""Skip tests on a main push when the merge queue already passed this tree.

Squash creates a new commit. Its tree matches the merge group's tree when
the group contains one pull request. A different tree, a failed lookup, or
any other event still runs the suite.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import urllib.error
import urllib.request
from collections.abc import Callable, Mapping
from pathlib import Path

_SHA = re.compile(r"^[0-9a-f]{40}$")
_REPO = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
_WORKFLOW = "build.yml"
_PAGE = 20

Fetch = Callable[[str, str], Mapping[str, object]]


def should_skip(event_name: str, ref: str, head_tree: str, tested_trees: set[str]) -> bool:
    if event_name != "push" or ref != "refs/heads/main":
        return False
    if not _SHA.fullmatch(head_tree):
        return False
    return head_tree in tested_trees


def head_tree(cwd: Path | None = None) -> str:
    result = subprocess.run(
        ["git", "rev-parse", "HEAD^{tree}"],
        cwd=cwd,
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def tested_trees(repo: str, token: str, api: str, fetch: Fetch) -> set[str]:
    if not _REPO.fullmatch(repo):
        raise RuntimeError("GITHUB_REPOSITORY is not owner/name")
    if not token:
        raise RuntimeError("GITHUB_TOKEN is missing")
    base = api.rstrip("/")
    listed = fetch(
        f"{base}/repos/{repo}/actions/workflows/{_WORKFLOW}/runs"
        f"?event=merge_group&status=success&per_page={_PAGE}",
        token,
    )
    runs = listed.get("workflow_runs")
    if not isinstance(runs, list):
        raise RuntimeError("workflow run list has no workflow_runs")
    trees: set[str] = set()
    for run in runs:
        if not isinstance(run, dict):
            continue
        if run.get("event") != "merge_group" or run.get("conclusion") != "success":
            continue
        sha = run.get("head_sha")
        if not isinstance(sha, str) or not _SHA.fullmatch(sha):
            continue
        commit = fetch(f"{base}/repos/{repo}/commits/{sha}", token)
        tree = _commit_tree(commit)
        if tree:
            trees.add(tree)
    return trees


def decide(environ: Mapping[str, str], fetch: Fetch, cwd: Path | None = None) -> bool:
    event_name = environ.get("GITHUB_EVENT_NAME", "")
    ref = environ.get("GITHUB_REF", "")
    if event_name != "push" or ref != "refs/heads/main":
        return False
    trees = tested_trees(
        environ.get("GITHUB_REPOSITORY", ""),
        environ.get("GITHUB_TOKEN", ""),
        environ.get("GITHUB_API_URL", "https://api.github.com"),
        fetch,
    )
    return should_skip(event_name, ref, head_tree(cwd), trees)


def github_fetch(url: str, token: str) -> dict[str, object]:
    request = urllib.request.Request(
        url,
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "User-Agent": "kiln-merge-queue-tested",
            "X-GitHub-Api-Version": "2022-11-28",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            payload = json.load(response)
    except urllib.error.HTTPError as error:
        raise RuntimeError(f"GitHub API {error.code} for {url}") from error
    if not isinstance(payload, dict):
        raise RuntimeError("GitHub API returned a non-object")
    return payload


def write_output(skip: bool, path: str | None) -> None:
    if not path:
        return
    with open(path, "a", encoding="utf-8") as handle:
        handle.write(f"skip_tests={'true' if skip else 'false'}\n")


def main() -> int:
    try:
        skip = decide(os.environ, github_fetch)
    except Exception as error:
        print(f"merge queue lookup failed, running tests: {error}", file=sys.stderr)
        skip = False
    write_output(skip, os.environ.get("GITHUB_OUTPUT"))
    print(f"skip_tests={'true' if skip else 'false'}")
    return 0


def _commit_tree(commit: Mapping[str, object]) -> str:
    body = commit.get("commit")
    if not isinstance(body, dict):
        return ""
    tree = body.get("tree")
    if not isinstance(tree, dict):
        return ""
    sha = tree.get("sha")
    if not isinstance(sha, str) or not _SHA.fullmatch(sha):
        return ""
    return sha


if __name__ == "__main__":
    sys.exit(main())

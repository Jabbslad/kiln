#!/usr/bin/env python3
"""Reuse main's packages for a tag, or say why that run cannot be published.

A main run that is still in progress is polled for 15 minutes. Success reuses
that run. Failure, cancellation, and timeout fall back to a rebuild.
"""

import argparse
import hashlib
import os
import subprocess
import sys
import time
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path

import release_version

WORKFLOW_PATH = ".github/workflows/build.yml"
ATTEST_ACTION = "actions/attest-build-provenance@"
WAIT_LIMIT = timedelta(minutes=15)
POLL_INTERVAL = timedelta(seconds=15)
PACKAGE_ARTIFACTS = frozenset(
    {
        "package-server",
        "package-dashboard",
        "package-x86_64-unknown-linux-gnu",
        "package-aarch64-apple-darwin",
        "package-x86_64-apple-darwin",
        "package-x86_64-pc-windows-msvc",
    }
)


class ReuseRefused(ValueError):
    """The main run cannot be published. The tag build has to compile."""


@dataclass(frozen=True)
class WorkflowRun:
    run_id: int
    head_sha: str
    head_branch: str
    event: str
    path: str
    conclusion: str | None
    status: str


@dataclass(frozen=True)
class PackageArtifact:
    name: str
    expired: bool
    expires_at: datetime


def _sha_pattern():
    import re

    return re.compile(r"^[0-9a-f]{40}$")


def _repo_pattern():
    import re

    return re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")


_SHA_RE = _sha_pattern()
_REPO_RE = _repo_pattern()


def select_run(runs: tuple[WorkflowRun, ...], sha: str) -> WorkflowRun:
    """Return the latest successful main push of this workflow for that commit."""
    if _SHA_RE.fullmatch(sha) is None:
        raise ReuseRefused("commit sha is not 40 hex characters")
    matches = [
        run
        for run in runs
        if _matches(run, sha) and run.conclusion == "success" and run.status == "completed"
    ]
    if not matches:
        raise ReuseRefused("no successful main push of this workflow for that commit")
    return max(matches, key=lambda run: run.run_id)


def wait_for_main(runs, sha, *, clock, poll, sleep, log=None):
    """Wait out one in-progress main run, then reuse it or refuse.

    `poll` receives that run's id and returns the run again. `clock` returns
    an aware datetime. `sleep` waits one poll interval. `log` records one line
    per case; the default prints it.
    """
    started = clock()
    if started.tzinfo is None:
        raise ReuseRefused("clock must be timezone-aware")
    deadline = started + WAIT_LIMIT
    budget = int(WAIT_LIMIT / POLL_INTERVAL)
    current = tuple(runs)
    announced = None
    polls = 0
    while True:
        latest = _latest_match(current, sha)
        if latest is not None and latest.conclusion is None and latest.status != "completed":
            if announced != latest.run_id:
                _note(log, f"reuse waiting: main run {latest.run_id} is in progress")
                announced = latest.run_id
            if clock() >= deadline or polls >= budget:
                raise ReuseRefused(f"timed out waiting for main run {latest.run_id}")
            polls += 1
            sleep(POLL_INTERVAL.total_seconds())
            current = _replace_run(current, poll(latest.run_id))
            continue
        if announced is None:
            return select_run(current, sha)
        if (
            latest is not None
            and latest.run_id == announced
            and latest.conclusion == "success"
            and latest.status == "completed"
        ):
            _note(log, f"reuse reusing main run {latest.run_id}")
            return latest
        if latest is not None and latest.run_id == announced and latest.conclusion == "cancelled":
            raise ReuseRefused(f"main run {announced} was cancelled")
        if latest is not None and latest.run_id == announced:
            raise ReuseRefused(f"main run {announced} failed")
        raise ReuseRefused("no successful main push of this workflow for that commit")


def _matches(run: WorkflowRun, sha: str) -> bool:
    return (
        run.head_sha == sha
        and run.head_branch == "main"
        and run.event == "push"
        and run.path == WORKFLOW_PATH
    )


def _latest_match(runs, sha: str):
    matches = [run for run in runs if _matches(run, sha)]
    if not matches:
        return None
    return max(matches, key=lambda run: run.run_id)


def _replace_run(runs, refreshed: WorkflowRun):
    others = tuple(run for run in runs if run.run_id != refreshed.run_id)
    return others + (refreshed,)


def _note(log, line: str) -> None:
    if log is not None:
        log(line)
        return
    stream = sys.stdout if line.startswith("reuse reusing ") else sys.stderr
    print(line, file=stream)


def require_artifacts(
    artifacts: tuple[PackageArtifact, ...], now: datetime
) -> None:
    if now.tzinfo is None:
        raise ReuseRefused("clock must be timezone-aware")
    names = [artifact.name for artifact in artifacts]
    if len(names) != len(set(names)):
        raise ReuseRefused("duplicate artifact name")
    if set(names) != PACKAGE_ARTIFACTS:
        raise ReuseRefused("artifact set does not match the package jobs")
    for artifact in artifacts:
        if artifact.expired or artifact.expires_at <= now:
            raise ReuseRefused(f"{artifact.name} is expired")


def workflow_attests(workflow_text: str) -> bool:
    for line in workflow_text.splitlines():
        if line.lstrip().startswith("uses:") and ATTEST_ACTION in line:
            return True
    return False


def attestation_gate(workflow_text: str, results: tuple[str, ...]) -> str:
    """Return verified or not-produced.

    A main build from before this workflow attested has nothing to verify.
    Once the workflow file attests, a missing statement is a failed check.
    """
    if not results or any(result not in {"verified", "absent", "rejected"} for result in results):
        raise ReuseRefused("attestation result is not usable")
    if any(result == "rejected" for result in results):
        raise ReuseRefused("attestation verification failed")
    if all(result == "verified" for result in results):
        return "verified"
    if all(result == "absent" for result in results):
        if workflow_attests(workflow_text):
            raise ReuseRefused(
                "this commit's workflow attests builds, but the archives have no attestation"
            )
        return "not-produced"
    raise ReuseRefused("archives disagree on attestation")


def classify_verify(returncode: int, stderr: str) -> str:
    if returncode == 0:
        return "verified"
    if "attestations/sha256:" in stderr and "404" in stderr:
        return "absent"
    return "rejected"


def expected_archives(version: release_version.ReleaseVersion) -> frozenset[str]:
    if type(version) is not release_version.ReleaseVersion:
        raise TypeError("version must be a ReleaseVersion")
    tag = version.tag
    names = {
        f"kiln-dashboard-{tag}.tar.gz",
        f"kiln-identity-{tag}-x86_64-unknown-linux-gnu.tar.gz",
        f"kiln-server-{tag}-x86_64-unknown-linux-gnu.tar.gz",
        f"kiln-{tag}-x86_64-pc-windows-msvc.zip",
    }
    for target in (
        "x86_64-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "aarch64-apple-darwin",
    ):
        names.add(f"kiln-{tag}-{target}.tar.gz")
    return frozenset(names)


def expected_release_files(version: release_version.ReleaseVersion) -> frozenset[str]:
    archives = expected_archives(version)
    return archives | {name + ".sha256" for name in archives} | {
        "install.sh",
        "install.sh.sha256",
    }


def verify_and_refresh_sidecars(
    dist: Path, version: release_version.ReleaseVersion
) -> None:
    """Check archive bytes against their sidecars, then rewrite those sidecars."""
    archives = expected_archives(version)
    paths = _regular_files(dist)
    names = {path.name for path in paths}
    if names != archives | {name + ".sha256" for name in archives}:
        raise ReuseRefused("downloaded files are not the expected archives")
    by_name = {path.name: path for path in paths}
    for name in sorted(archives):
        archive = by_name[name]
        sidecar = by_name[name + ".sha256"]
        digest = _sha256(archive)
        expected = f"{digest}  {name}\n".encode("ascii")
        # Windows text mode wrote CRLF for this sidecar. The digest is the same.
        if sidecar.read_bytes() not in (expected, expected.replace(b"\n", b"\r\n")):
            raise ReuseRefused(f"checksum does not match {name}")
        sidecar.write_bytes(expected)


def finish_dist(repo: Path, dist: Path, version: release_version.ReleaseVersion) -> None:
    release_version.check_archives(version, dist)
    release_version.attach_bootstrap(repo, dist, version)
    names = {path.name for path in _regular_files(dist)}
    if names != expected_release_files(version):
        raise ReuseRefused("dist is not the 16-file release set")


def parse_runs(items: list) -> tuple[WorkflowRun, ...]:
    if not isinstance(items, list):
        raise ReuseRefused("run list is not a list")
    runs = []
    for item in items:
        if not isinstance(item, dict):
            raise ReuseRefused("run list entry is not an object")
        runs.append(
            WorkflowRun(
                run_id=_json_int(item, "databaseId"),
                head_sha=_json_sha(item, "headSha"),
                head_branch=_json_str(item, "headBranch"),
                event=_json_str(item, "event"),
                path=_json_str(item, "path"),
                conclusion=_json_conclusion(item),
                status=_json_str(item, "status"),
            )
        )
    return tuple(runs)


def parse_api_run(payload: dict) -> WorkflowRun:
    """Parse one Actions run object. A null conclusion is still in progress."""
    if not isinstance(payload, dict):
        raise ReuseRefused("workflow run is not an object")
    item = {
        "databaseId": payload.get("id"),
        "headSha": payload.get("head_sha"),
        "headBranch": payload.get("head_branch"),
        "event": payload.get("event"),
        "path": payload.get("path"),
        "status": payload.get("status"),
    }
    if "conclusion" in payload:
        item["conclusion"] = payload["conclusion"]
    return parse_runs([item])[0]


def parse_artifacts(payload: dict, now: datetime) -> tuple[PackageArtifact, ...]:
    if not isinstance(payload, dict):
        raise ReuseRefused("artifact list is not an object")
    artifacts = payload.get("artifacts")
    total = payload.get("total_count")
    if not isinstance(artifacts, list) or type(total) is not int or total != len(artifacts):
        raise ReuseRefused("artifact list was paginated or malformed")
    parsed = []
    for item in artifacts:
        if not isinstance(item, dict):
            raise ReuseRefused("artifact entry is not an object")
        expires = _json_str(item, "expires_at")
        try:
            expires_at = datetime.fromisoformat(expires.replace("Z", "+00:00"))
        except ValueError as exc:
            raise ReuseRefused("artifact expiry is not a timestamp") from exc
        if expires_at.tzinfo is None:
            raise ReuseRefused("artifact expiry is not timezone-aware")
        expired = item.get("expired")
        if type(expired) is not bool:
            raise ReuseRefused("artifact expired flag is not a boolean")
        parsed.append(
            PackageArtifact(
                name=_json_str(item, "name"),
                expired=expired,
                expires_at=expires_at,
            )
        )
    if now.tzinfo is None:
        raise ReuseRefused("clock must be timezone-aware")
    return tuple(parsed)


def fetch(repo: str, sha: str, version_text: str, dist: Path) -> tuple[str, release_version.ReleaseVersion]:
    _require_repo(repo)
    dist = Path(dist)
    if dist.exists() and any(dist.iterdir()):
        raise ReuseRefused("dist is not empty")
    dist.mkdir(parents=True, exist_ok=True)
    runs = parse_runs(_with_paths(repo, _run_list(repo, sha)))
    chosen = wait_for_main(
        runs,
        sha,
        clock=lambda: datetime.now(timezone.utc),
        poll=lambda run_id: _refresh_run(repo, run_id),
        sleep=time.sleep,
    )
    now = datetime.now(timezone.utc)
    require_artifacts(parse_artifacts(_artifact_page(repo, chosen.run_id), now), now)
    for name in sorted(PACKAGE_ARTIFACTS):
        _gh(
            [
                "run",
                "download",
                str(chosen.run_id),
                "--repo",
                repo,
                "--name",
                name,
                "--dir",
                os.fspath(dist),
            ],
            refuse=f"download of {name} failed",
        )
    version = release_version.parse_release_tag(version_text)
    verify_and_refresh_sidecars(dist, version)
    workflow_text = _workflow_text(repo, sha)
    results = tuple(
        classify_verify(*_verify_attestation(repo, sha, path))
        for path in sorted(dist.glob("*"))
        if path.is_file() and not path.name.endswith(".sha256")
    )
    produced = attestation_gate(workflow_text, results)
    return f"run {chosen.run_id} attestation={produced}", version


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    fetch_cmd = sub.add_parser("fetch")
    fetch_cmd.add_argument("--dist", type=Path, required=True)
    finish_cmd = sub.add_parser("finish")
    finish_cmd.add_argument("--version", required=True)
    finish_cmd.add_argument("--dist", type=Path, required=True)
    finish_cmd.add_argument(
        "--repo",
        type=Path,
        default=Path(__file__).resolve().parent.parent,
    )
    args = parser.parse_args(argv)
    try:
        if args.command == "fetch":
            _fetch_command(args.dist)
            return
        version = release_version.parse_release_tag(args.version)
        finish_dist(args.repo, args.dist, version)
    except (ReuseRefused, ValueError) as exc:
        if args.command == "fetch" and isinstance(exc, ReuseRefused):
            _emit("mode", "fallback")
            _emit("version", "")
            _emit("reason", _one_line(str(exc)))
            print(f"reuse refused: {exc}", file=sys.stderr)
            return
        raise SystemExit(str(exc)) from None


def _fetch_command(dist: Path) -> None:
    event = os.environ.get("REUSE_EVENT", "")
    repo = os.environ.get("GH_REPO", "")
    if event == "workflow_dispatch":
        sha = os.environ.get("REUSE_SHA_INPUT", "")
        version_text = ""
    elif event == "push":
        sha = os.environ.get("REUSE_SHA", "")
        version_text = os.environ.get("REUSE_VERSION", "")
    else:
        raise ReuseRefused("reuse runs only for a tag push or a dry run")
    if not version_text:
        version_text = _committed_tag(repo, sha)
    reason, version = fetch(repo, sha, version_text, dist)
    _emit("mode", "reuse")
    _emit("version", version.tag)
    _emit("reason", _one_line(reason))
    print(f"reuse accepted: {reason}")


def _require_repo(repo: str) -> None:
    if _REPO_RE.fullmatch(repo) is None:
        raise ReuseRefused("repository is not owner/name")


def _committed_tag(repo: str, sha: str) -> str:
    _require_repo(repo)
    if _SHA_RE.fullmatch(sha) is None:
        raise ReuseRefused("commit sha is not 40 hex characters")
    text = _gh_text(
        ["api", f"repos/{repo}/contents/Cargo.toml?ref={sha}", "-H", "Accept: application/vnd.github.raw"],
        refuse="could not read Cargo.toml at that commit",
    )
    return release_version._committed_version(text).tag


def _with_paths(repo: str, items: list) -> list:
    enriched = []
    for item in items:
        if not isinstance(item, dict):
            raise ReuseRefused("run list entry is not an object")
        run_id = _json_int(item, "databaseId")
        path = _gh_text(
            ["api", f"repos/{repo}/actions/runs/{run_id}", "--jq", ".path"],
            refuse="could not read the workflow path",
        ).strip()
        enriched.append({**item, "path": path})
    return enriched


def _run_list(repo: str, sha: str) -> list:
    if _SHA_RE.fullmatch(sha) is None:
        raise ReuseRefused("commit sha is not 40 hex characters")
    import json

    text = _gh_text(
        [
            "run",
            "list",
            "--repo",
            repo,
            "--workflow",
            "build.yml",
            "--commit",
            sha,
            "--branch",
            "main",
            "--limit",
            "20",
            "--json",
            "databaseId,headSha,headBranch,event,conclusion,status",
        ],
        refuse="could not list workflow runs",
    )
    try:
        payload = json.loads(text)
    except json.JSONDecodeError as exc:
        raise ReuseRefused("run list was not json") from exc
    if not isinstance(payload, list):
        raise ReuseRefused("run list is not a list")
    return payload


def _refresh_run(repo: str, run_id: int) -> WorkflowRun:
    import json

    text = _gh_text(
        ["api", f"repos/{repo}/actions/runs/{run_id}"],
        refuse="could not read the workflow run",
    )
    try:
        payload = json.loads(text)
    except json.JSONDecodeError as exc:
        raise ReuseRefused("workflow run was not json") from exc
    return parse_api_run(payload)


def _artifact_page(repo: str, run_id: int) -> dict:
    import json

    text = _gh_text(
        ["api", f"repos/{repo}/actions/runs/{run_id}/artifacts"],
        refuse="could not list artifacts",
    )
    try:
        payload = json.loads(text)
    except json.JSONDecodeError as exc:
        raise ReuseRefused("artifact list was not json") from exc
    return payload


def _workflow_text(repo: str, sha: str) -> str:
    return _gh_text(
        [
            "api",
            f"repos/{repo}/contents/{WORKFLOW_PATH}?ref={sha}",
            "-H",
            "Accept: application/vnd.github.raw",
        ],
        refuse="could not read the workflow at that commit",
    )


def _verify_attestation(repo: str, sha: str, archive: Path) -> tuple[int, str]:
    completed = _run(
        [
            "gh",
            "attestation",
            "verify",
            os.fspath(archive),
            "--repo",
            repo,
            "--signer-workflow",
            f"{repo}/{WORKFLOW_PATH}",
            "--source-ref",
            "refs/heads/main",
            "--source-digest",
            sha,
            "--deny-self-hosted-runners",
        ]
    )
    return completed.returncode, completed.stderr


def _gh(args: list[str], refuse: str) -> None:
    completed = _run(["gh", *args])
    if completed.returncode != 0:
        raise ReuseRefused(f"{refuse}: {_public_error(completed.stderr)}")


def _gh_text(args: list[str], refuse: str) -> str:
    completed = _run(["gh", *args])
    if completed.returncode != 0:
        raise ReuseRefused(f"{refuse}: {_public_error(completed.stderr)}")
    return completed.stdout


def _run(args: list[str]) -> subprocess.CompletedProcess:
    try:
        return subprocess.run(
            args,
            check=False,
            capture_output=True,
            text=True,
            timeout=300,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise ReuseRefused("gh could not be run") from exc


def _regular_files(dist: Path) -> tuple[Path, ...]:
    if not dist.is_dir():
        raise ReuseRefused("dist is not a directory")
    paths = []
    for path in dist.iterdir():
        if path.is_symlink() or not path.is_file():
            raise ReuseRefused(f"unexpected dist entry {path.name}")
        paths.append(path)
    return tuple(paths)


def _sha256(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def _json_int(item: dict, key: str) -> int:
    value = item.get(key)
    if type(value) is not int:
        raise ReuseRefused(f"{key} is not an integer")
    return value


def _json_str(item: dict, key: str) -> str:
    value = item.get(key)
    if not isinstance(value, str) or value == "":
        raise ReuseRefused(f"{key} is not a string")
    return value


def _json_conclusion(item: dict) -> str | None:
    if "conclusion" not in item:
        raise ReuseRefused("conclusion is not a string")
    value = item["conclusion"]
    if value is None:
        return None
    if not isinstance(value, str) or value == "":
        raise ReuseRefused("conclusion is not a string")
    return value


def _json_sha(item: dict, key: str) -> str:
    value = _json_str(item, key)
    if _SHA_RE.fullmatch(value) is None:
        raise ReuseRefused(f"{key} is not a commit sha")
    return value


def _emit(name: str, value: str) -> None:
    line = f"{name}={value}"
    path = os.environ.get("GITHUB_OUTPUT")
    if not path:
        print(line)
        return
    with open(path, "a", encoding="utf-8") as handle:
        handle.write(line + "\n")


def _one_line(text: str) -> str:
    return " ".join(text.split())


def _public_error(stderr: str) -> str:
    line = stderr.strip().splitlines()[0] if stderr.strip() else "gh failed"
    if any(marker in line for marker in ("ghp_", "ghs_", "github_pat", "Bearer ")):
        return "gh failed"
    return line[:200]


if __name__ == "__main__":
    main()

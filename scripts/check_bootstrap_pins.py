#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import os
import re
import subprocess
import sys
import tempfile
import urllib.error
import urllib.request
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

_VERSION = re.compile(r"(?m)^    version=([0-9]+\.[0-9]+\.[0-9]+)$")
_ROW = re.compile(r"(?m)^(client|server):(\S+) ([0-9a-f]{64})$")
_VERIFY = re.compile(
    r"gh attestation verify ARCHIVE --repo (?P<repo>\S+) "
    r"--cert-identity-regex '(?P<identity>[^']+)'"
)
_REPO = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
_NAME = re.compile(r"^[A-Za-z0-9._-]+$")
_ARCHIVE = {
    "client": "kiln-v{version}-{target}.tar.gz",
    "server": "kiln-server-v{version}-{target}.tar.gz",
}


class PinError(RuntimeError):
    pass


@dataclass(frozen=True)
class Pin:
    version: str
    digests: tuple[tuple[str, str], ...]


Fetch = Callable[[str, str], Path]
Verify = Callable[[Path, str, str], tuple[int, str]]


def parse_pin(text: str) -> Pin:
    versions = _VERSION.findall(text)
    if len(versions) != 1:
        raise PinError("install.sh must pin one release version")
    version = versions[0]
    rows = _ROW.findall(text)
    if not rows:
        raise PinError("install.sh does not pin release archives")
    digests: list[tuple[str, str]] = []
    seen: set[str] = set()
    for mode, target, digest in rows:
        if "/" in target or target in {".", ".."}:
            raise PinError("archive target")
        name = _ARCHIVE[mode].format(version=version, target=target)
        if _NAME.fullmatch(name) is None or name in seen:
            raise PinError(f"archive name {name}")
        seen.add(name)
        digests.append((name, digest))
    return Pin(version, tuple(digests))


def parse_verify(text: str) -> tuple[str, str]:
    found = _VERIFY.findall(text)
    if len(found) != 1:
        raise PinError("docs/releases.md must document one gh attestation verify command")
    repo, identity = found[0]
    if _REPO.fullmatch(repo) is None:
        raise PinError("documented repo")
    try:
        re.compile(identity)
    except re.error as error:
        raise PinError("documented cert-identity regex") from error
    return repo, identity


def check(pin: Pin, repo: str, identity: str, fetch: Fetch, verify: Verify) -> None:
    for name, digest in pin.digests:
        path = fetch(pin.version, name)
        actual = _sha256(path)
        if actual != digest:
            raise PinError(f"{name} sha256 is {actual}")
        code, detail = verify(path, repo, identity)
        if code != 0:
            message = f"attestation failed for {name}"
            if detail:
                message = f"{message}: {detail}"
            raise PinError(message)


def _verify(path: Path, repo: str, identity: str) -> tuple[int, str]:
    completed = subprocess.run(
        [
            "gh",
            "attestation",
            "verify",
            os.fspath(path),
            "--repo",
            repo,
            "--cert-identity-regex",
            identity,
        ],
        check=False,
        capture_output=True,
        text=True,
        timeout=300,
    )
    detail = completed.stderr.strip().splitlines()
    line = detail[-1] if detail else ""
    for key in ("GH_TOKEN", "GITHUB_TOKEN"):
        token = os.environ.get(key, "")
        if token:
            line = line.replace(token, "")
    return completed.returncode, line


def main() -> int:
    try:
        root = Path(__file__).resolve().parent.parent
        pin = parse_pin((root / "deploy/bootstrap/install.sh").read_text(encoding="utf-8"))
        repo, identity = parse_verify((root / "docs/releases.md").read_text(encoding="utf-8"))
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)

            def fetch(version: str, name: str) -> Path:
                if _NAME.fullmatch(name) is None:
                    raise PinError(f"archive name {name}")
                path = directory / name
                _download(repo, version, name, path)
                return path

            check(pin, repo, identity, fetch, _verify)
    except PinError as error:
        print(f"kiln: {error}", file=sys.stderr)
        return 1
    return 0


def _download(repo: str, version: str, name: str, dest: Path) -> None:
    url = f"https://github.com/{repo}/releases/download/v{version}/{name}"
    request = urllib.request.Request(url, headers={"User-Agent": "kiln-bootstrap-pins"})
    try:
        with urllib.request.urlopen(request, timeout=60) as response, dest.open("wb") as handle:
            while True:
                chunk = response.read(1024 * 1024)
                if not chunk:
                    break
                handle.write(chunk)
    except urllib.error.HTTPError as error:
        raise PinError(f"{name} download returned HTTP {error.code}") from error
    except urllib.error.URLError as error:
        raise PinError(f"{name} download failed") from error


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


if __name__ == "__main__":
    sys.exit(main())

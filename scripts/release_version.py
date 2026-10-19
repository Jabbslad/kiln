#!/usr/bin/env python3
"""Release tag to workspace version.

resolve, check-binary, and check-archives are the entry points.
plan_stamp rewrites the two file texts and does not touch disk.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import tarfile
import tempfile
import tomllib
import zipfile
from dataclasses import dataclass
from pathlib import Path

# deploy.sh compares with shell arithmetic and install.sh with awk.
# A tenth digit is not exact there, and a leading zero is an octal trap (08).
_COMPONENT_LIMIT = 10**9
_TAG_PATTERN = (
    r"^v(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})$"
)
_TAG_RE = re.compile(_TAG_PATTERN)
_VERSION_LINE = re.compile(r'^version = "([^"]*)"$')
_NAME_LINE = re.compile(r'^name = "([^"]*)"$')
_SOURCE_LINE = re.compile(r'^source = "([^"]*)"$')
_TABLE_LINE = re.compile(r"^\[([^\[\]]+)\]$")
_PACKAGE_TABLE = re.compile(r"^\[package\]$")
_VERSION_KEY = re.compile(r"^version(\s|=|\.|$)")
_LOCK_PACKAGE = "[[package]]"

# Clap's default command is the package name. kiln-client sets name = "kiln".
# kiln-host and kiln-api are binaries of package kiln-server.
# test_clap_names_match_installer locks this map to deploy/install.sh reported_name.
_CLAP_COMMAND = {
    "kiln": "kiln",
    "kiln-runtime": "kiln-runtime",
    "kiln-host": "kiln-server",
    "kiln-api": "kiln-server",
    "kiln-identity": "kiln-identity",
}


@dataclass(frozen=True)
class ReleaseVersion:
    """Stable product version. tag and cargo are derived from the three integers."""

    major: int
    minor: int
    patch: int

    def __post_init__(self) -> None:
        for name in ("major", "minor", "patch"):
            value = getattr(self, name)
            if isinstance(value, bool) or not isinstance(value, int):
                raise ValueError(f"{name} must be an int in 0 .. {_COMPONENT_LIMIT}")
            if not 0 <= value < _COMPONENT_LIMIT:
                raise ValueError(f"{name} must be an int in 0 .. {_COMPONENT_LIMIT}")

    @property
    def tag(self) -> str:
        """vMAJOR.MINOR.PATCH. Archives, release.json, VERSION, and the git ref."""
        return f"v{self.major}.{self.minor}.{self.patch}"

    @property
    def cargo(self) -> str:
        """MAJOR.MINOR.PATCH. The manifest, the lockfile, and clap stdout."""
        return f"{self.major}.{self.minor}.{self.patch}"


@dataclass(frozen=True)
class MemberCrate:
    """One workspace member. The stamp requires version.workspace = true."""

    name: str
    inherits_workspace_version: bool


def parse_release_tag(raw: str) -> ReleaseVersion:
    """Accept only a stable vMAJOR.MINOR.PATCH tag.

    Each component is 0 or a decimal integer of at most nine digits with no
    leading zero. Prerelease, build metadata, and the bare cargo spelling fail.
    """
    match = _TAG_RE.fullmatch(raw)
    if match is None:
        raise ValueError(f"refusing release tag {raw!r}: expected {_TAG_PATTERN}")
    return ReleaseVersion(*(int(part) for part in match.groups()))


def plan_stamp(
    manifest: str,
    lock: str,
    members: tuple[MemberCrate, ...],
    version: ReleaseVersion,
) -> tuple[str, str]:
    """Return stamped Cargo.toml and Cargo.lock text.

    The result is pure. A second call with the same version returns the same
    text. Registry packages, git packages, dependency arrays, the lockfile
    header, and non-member path packages stay as they were.
    """
    if type(version) is not ReleaseVersion:
        raise TypeError("version must be a ReleaseVersion")
    members = tuple(members)
    for member in members:
        if type(member) is not MemberCrate:
            raise TypeError("members must be MemberCrate values")
    if any(_PACKAGE_TABLE.fullmatch(_body(line)) for line in manifest.splitlines()):
        raise ValueError("root Cargo.toml has a [package] table")
    parsed = _loads(manifest, "Cargo.toml")
    declared = parsed.get("workspace", {}).get("members")
    if not isinstance(declared, list) or len(declared) != len(members):
        raise ValueError("member list does not match workspace.members")
    names = tuple(member.name for member in members)
    if len(set(names)) != len(names):
        raise ValueError("duplicate workspace member name")
    for member in members:
        if not member.inherits_workspace_version:
            raise ValueError(f"{member.name} does not inherit the workspace version")
    new_manifest = _rewrite_manifest(manifest, version.cargo)
    new_lock = _rewrite_lock(lock, names, version.cargo)
    _postcondition(lock, new_manifest, new_lock, names, version)
    return new_manifest, new_lock


def resolve(repo: Path, ref_type: str, ref_name: str) -> ReleaseVersion:
    """Stamp a tag, or return the committed version for a branch.

    This is the only writer of Cargo.toml and Cargo.lock. A tag replaces the
    lockfile and then the manifest when the bytes differ. A branch writes
    nothing. Any other ref type fails.
    """
    repo = Path(repo)
    if ref_type == "branch":
        text = _read_text(repo / "Cargo.toml")
        return _committed_version(text)
    if ref_type != "tag":
        raise ValueError(f"refusing ref_type {ref_type!r}")
    version = parse_release_tag(ref_name)
    manifest_path = repo / "Cargo.toml"
    lock_path = repo / "Cargo.lock"
    manifest_bytes = _read_bytes(manifest_path)
    lock_bytes = _read_bytes(lock_path)
    members = _load_members(repo, manifest_bytes.decode("utf-8"))
    new_manifest, new_lock = plan_stamp(
        manifest_bytes.decode("utf-8"),
        lock_bytes.decode("utf-8"),
        members,
        version,
    )
    _replace_bytes(lock_path, lock_bytes, new_lock.encode("utf-8"))
    _replace_bytes(manifest_path, manifest_bytes, new_manifest.encode("utf-8"))
    return version


def clap_line(binary: Path, version: ReleaseVersion) -> str:
    """The exact --version line, without the trailing newline."""
    if type(version) is not ReleaseVersion:
        raise TypeError("version must be a ReleaseVersion")
    stem = binary.name
    if stem.endswith(".exe"):
        stem = stem[: -len(".exe")]
    try:
        command = _CLAP_COMMAND[stem]
    except KeyError as exc:
        raise ValueError(f"unknown binary {binary.name}") from exc
    return f"{command} {version.cargo}"


def check_binary(version: ReleaseVersion, binaries: tuple[Path, ...]) -> None:
    """Require each binary's --version stdout to be the clap line plus a newline."""
    if type(version) is not ReleaseVersion:
        raise TypeError("version must be a ReleaseVersion")
    if not binaries:
        raise ValueError("no binaries to check")
    for binary in binaries:
        if binary.is_symlink() or not binary.is_file():
            raise ValueError(f"refusing non-regular binary {binary}")
        expected = (clap_line(binary, version) + "\n").encode()
        try:
            completed = subprocess.run(
                [os.fspath(binary), "--version"],
                check=False,
                capture_output=True,
                timeout=30,
            )
        except subprocess.TimeoutExpired as exc:
            raise ValueError(f"{binary} --version timed out") from exc
        except OSError as exc:
            raise ValueError(f"{binary} --version failed to start") from exc
        stdout = completed.stdout.replace(b"\r\n", b"\n")
        if completed.returncode != 0 or stdout != expected:
            raise ValueError(
                f"{binary} --version printed {completed.stdout!r}, expected {expected!r}"
            )


def check_archives(version: ReleaseVersion, dist: Path) -> None:
    """Refuse a dist directory whose archives are not this tag.

    Every basename carries the tag as one delimited component. Checksum
    siblings pair both ways. A release.json member stores version.tag.
    Archives without that member are checked by filename only.
    """
    if type(version) is not ReleaseVersion:
        raise TypeError("version must be a ReleaseVersion")
    if not dist.is_dir():
        raise ValueError(f"{dist} is not a directory")
    pattern = _dist_pattern(version)
    names: list[str] = []
    for path in dist.iterdir():
        if path.is_symlink() or not path.is_file() or pattern.fullmatch(path.name) is None:
            raise ValueError(f"unexpected dist file {path.name}")
        names.append(path.name)
    archives = [name for name in names if not name.endswith(".sha256")]
    checksums = {name for name in names if name.endswith(".sha256")}
    if not archives:
        raise ValueError(f"{dist} has no archives")
    archive_set = set(archives)
    for name in archives:
        if name + ".sha256" not in checksums:
            raise ValueError(f"missing checksum for {name}")
    for name in checksums:
        if name[: -len(".sha256")] not in archive_set:
            raise ValueError(f"checksum without archive {name}")
    for name in archives:
        payload = _release_json(dist / name)
        if payload is None:
            continue
        if payload.get("version") != version.tag:
            raise ValueError(
                f"{name} release.json version is {payload.get('version')!r}, "
                f"expected {version.tag}"
            )


def main(argv: list[str] | None = None) -> None:
    """Run resolve, check-binary, or check-archives."""
    try:
        _dispatch(argv)
    except ValueError as exc:
        raise SystemExit(str(exc)) from None


def _dispatch(argv: list[str] | None) -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    resolve_cmd = sub.add_parser("resolve")
    resolve_cmd.add_argument("--ref-type", required=True)
    resolve_cmd.add_argument("--ref-name", required=True)
    resolve_cmd.add_argument(
        "--repo",
        type=Path,
        default=Path(__file__).resolve().parent.parent,
    )

    binary_cmd = sub.add_parser("check-binary")
    binary_cmd.add_argument("--version", required=True)
    binary_cmd.add_argument("binaries", nargs="+", type=Path)

    archive_cmd = sub.add_parser("check-archives")
    archive_cmd.add_argument("--version", required=True)
    archive_cmd.add_argument("dist", type=Path)

    args = parser.parse_args(argv)
    if args.command == "resolve":
        version = resolve(args.repo, args.ref_type, args.ref_name)
        print(version.tag)
        env_path = os.environ.get("GITHUB_ENV")
        if env_path:
            with open(env_path, "a", encoding="utf-8") as handle:
                handle.write(f"VERSION={version.tag}\n")
        return
    version = parse_release_tag(args.version)
    if args.command == "check-binary":
        check_binary(version, tuple(args.binaries))
        return
    check_archives(version, args.dist)


def _committed_version(manifest: str) -> ReleaseVersion:
    parsed = _loads(manifest, "Cargo.toml")
    try:
        cargo = parsed["workspace"]["package"]["version"]
    except KeyError as exc:
        raise ValueError("Cargo.toml has no workspace package version") from exc
    if not isinstance(cargo, str):
        raise ValueError("workspace package version is not a string")
    return parse_release_tag("v" + cargo)


def _load_members(repo: Path, manifest: str) -> tuple[MemberCrate, ...]:
    parsed = _loads(manifest, "Cargo.toml")
    try:
        rels = parsed["workspace"]["members"]
    except KeyError as exc:
        raise ValueError("Cargo.toml has no workspace members") from exc
    if not isinstance(rels, list) or not all(isinstance(rel, str) for rel in rels):
        raise ValueError("workspace.members is not a list of paths")
    members: list[MemberCrate] = []
    seen: set[str] = set()
    for rel in rels:
        path = repo / rel / "Cargo.toml"
        if not path.is_file():
            raise ValueError(f"missing member manifest: {rel}")
        member = _loads(_read_text(path), str(path))
        try:
            name = member["package"]["name"]
            package_version = member["package"]["version"]
        except KeyError as exc:
            raise ValueError(f"{rel} has no package name and version") from exc
        if not isinstance(name, str) or not name or name in seen:
            raise ValueError(f"duplicate package name {name!r}")
        seen.add(name)
        members.append(MemberCrate(name, package_version == {"workspace": True}))
    return tuple(members)


def _rewrite_manifest(manifest: str, cargo: str) -> str:
    lines = manifest.splitlines(keepends=True)
    section = ""
    matches: list[int] = []
    for index, line in enumerate(lines):
        body = _body(line)
        table = _TABLE_LINE.fullmatch(body)
        if table:
            section = table.group(1)
            continue
        if section != "workspace.package":
            continue
        if _VERSION_LINE.fullmatch(body):
            matches.append(index)
            continue
        if _VERSION_KEY.match(body):
            raise ValueError("workspace.package version is not a simple assignment")
    if len(matches) != 1:
        raise ValueError(
            "Cargo.toml [workspace.package] needs exactly one version assignment"
        )
    index = matches[0]
    lines[index] = f'version = "{cargo}"{_ending(lines[index])}'
    return "".join(lines)


def _rewrite_lock(lock: str, names: tuple[str, ...], cargo: str) -> str:
    header, blocks = _split_lock(lock)
    if "\nversion = 4\n" not in f"\n{header}":
        raise ValueError("Cargo.lock header version is not 4")
    infos = [_inspect_block(block) for block in blocks]
    hits: dict[str, list[int]] = {name: [] for name in names}
    for index, info in enumerate(infos):
        name, source, version_lines = info
        is_path = source is None or source.startswith("path+")
        if is_path and name in hits:
            hits[name].append(index)
    rewritten = list(blocks)
    for name, indexes in hits.items():
        if len(indexes) != 1:
            raise ValueError(
                f"Cargo.lock has no path package for {name}. Refusing to run cargo update."
            )
        if infos[indexes[0]][2] != 1:
            raise ValueError(
                f"Cargo.lock path package {name} needs exactly one version assignment"
            )
        rewritten[indexes[0]] = _replace_version_line(blocks[indexes[0]], cargo)
    return header + "".join(rewritten)


def _inspect_block(block: str) -> tuple[str, str | None, int]:
    name = None
    source = None
    version_lines = 0
    for line in block.splitlines():
        name_match = _NAME_LINE.fullmatch(line)
        if name_match:
            if name is not None:
                raise ValueError("Cargo.lock package block has two names")
            name = name_match.group(1)
            continue
        source_match = _SOURCE_LINE.fullmatch(line)
        if source_match:
            if source is not None:
                raise ValueError(f"Cargo.lock package {name} has two sources")
            source = source_match.group(1)
            continue
        if line.startswith("source"):
            raise ValueError(f"Cargo.lock package {name} has an unreadable source")
        if _VERSION_LINE.fullmatch(line):
            version_lines += 1
        elif line.startswith("version"):
            raise ValueError(f"Cargo.lock package {name} has an unreadable version")
    if not name:
        raise ValueError("Cargo.lock package block has no name")
    return name, source, version_lines


def _replace_version_line(block: str, cargo: str) -> str:
    lines = block.splitlines(keepends=True)
    found = [
        index for index, line in enumerate(lines) if _VERSION_LINE.fullmatch(_body(line))
    ]
    if len(found) != 1:
        raise ValueError("Cargo.lock path package needs exactly one version assignment")
    index = found[0]
    lines[index] = f'version = "{cargo}"{_ending(lines[index])}'
    return "".join(lines)


def _split_lock(lock: str) -> tuple[str, list[str]]:
    lines = lock.splitlines(keepends=True)
    header: list[str] = []
    blocks: list[list[str]] = []
    current: list[str] | None = None
    for line in lines:
        if _body(line) == _LOCK_PACKAGE:
            if current is not None:
                blocks.append(current)
            current = [line]
            continue
        if current is None:
            header.append(line)
        else:
            current.append(line)
    if current is not None:
        blocks.append(current)
    if not blocks:
        raise ValueError("Cargo.lock has no packages")
    return "".join(header), ["".join(block) for block in blocks]


def _postcondition(
    old_lock: str,
    new_manifest: str,
    new_lock: str,
    names: tuple[str, ...],
    version: ReleaseVersion,
) -> None:
    if _lock_header(new_lock) != _lock_header(old_lock):
        raise ValueError("Cargo.lock header changed")
    manifest = _loads(new_manifest, "stamped Cargo.toml")
    try:
        stamped = manifest["workspace"]["package"]["version"]
    except KeyError as exc:
        raise ValueError("stamped Cargo.toml has no workspace package version") from exc
    if stamped != version.cargo:
        raise ValueError("stamped workspace version does not match the tag")
    old_doc = _loads(old_lock, "Cargo.lock")
    new_doc = _loads(new_lock, "stamped Cargo.lock")
    if old_doc.get("version") != 4 or new_doc.get("version") != 4:
        raise ValueError("Cargo.lock header version is not 4")
    old_packages = old_doc.get("package")
    new_packages = new_doc.get("package")
    if not isinstance(old_packages, list) or not isinstance(new_packages, list):
        raise ValueError("Cargo.lock has no packages")
    if len(old_packages) != len(new_packages):
        raise ValueError("Cargo.lock package count changed")
    wanted = set(names)
    seen = {name: 0 for name in names}
    for old_pkg, new_pkg in zip(old_packages, new_packages, strict=True):
        if not isinstance(old_pkg, dict) or not isinstance(new_pkg, dict):
            raise ValueError("Cargo.lock package table is unreadable")
        name = new_pkg.get("name")
        if name != old_pkg.get("name"):
            raise ValueError("Cargo.lock package name changed")
        if old_pkg.get("dependencies") != new_pkg.get("dependencies"):
            raise ValueError(f"Cargo.lock dependencies changed for {name}")
        if old_pkg.get("source") != new_pkg.get("source"):
            raise ValueError(f"Cargo.lock source changed for {name}")
        source = new_pkg.get("source")
        is_path = source is None or (isinstance(source, str) and source.startswith("path+"))
        is_foreign = isinstance(source, str) and (
            source.startswith("registry+") or source.startswith("git+")
        )
        if is_foreign and new_pkg.get("version") != old_pkg.get("version"):
            raise ValueError(f"Cargo.lock changed registry or git package {name}")
        if is_path and name in wanted:
            if new_pkg.get("version") != version.cargo:
                raise ValueError(f"Cargo.lock path package {name} is not {version.cargo}")
            seen[name] += 1
        elif new_pkg.get("version") != old_pkg.get("version"):
            raise ValueError(f"Cargo.lock changed package {name}")
    for name, count in seen.items():
        if count != 1:
            raise ValueError(
                f"Cargo.lock has no path package for {name}. Refusing to run cargo update."
            )


def _release_json(path: Path) -> dict | None:
    try:
        if path.name.endswith(".tar.gz"):
            return _tar_release_json(path)
        if path.name.endswith(".zip"):
            return _zip_release_json(path)
    except (tarfile.TarError, zipfile.BadZipFile, json.JSONDecodeError, OSError) as exc:
        raise ValueError(f"{path.name} is not a readable package") from exc
    raise ValueError(f"unexpected archive {path.name}")


def _tar_release_json(path: Path) -> dict | None:
    with tarfile.open(path, "r:gz") as archive:
        try:
            member = archive.getmember("release.json")
        except KeyError:
            return None
        if member.issym() or member.islnk() or not member.isfile():
            raise ValueError(f"{path.name} release.json is not a regular file")
        extracted = archive.extractfile(member)
        if extracted is None:
            raise ValueError(f"{path.name} release.json is not a regular file")
        payload = json.load(extracted)
    if not isinstance(payload, dict):
        raise ValueError(f"{path.name} release.json is not an object")
    return payload


def _zip_release_json(path: Path) -> dict | None:
    with zipfile.ZipFile(path) as archive:
        matches = [info for info in archive.infolist() if info.filename == "release.json"]
        if not matches:
            return None
        info = matches[0]
        if info.is_dir():
            raise ValueError(f"{path.name} release.json is not a regular file")
        payload = json.loads(archive.read(info))
    if not isinstance(payload, dict):
        raise ValueError(f"{path.name} release.json is not an object")
    return payload


def _dist_pattern(version: ReleaseVersion) -> re.Pattern[str]:
    tag = re.escape(version.tag)
    return re.compile(
        rf"^kiln(-[A-Za-z0-9]+)*-{tag}(-[A-Za-z0-9_]+)*\.(tar\.gz|zip)(\.sha256)?$"
    )


def _replace_bytes(path: Path, old: bytes, new: bytes) -> None:
    if old == new:
        return
    fd, name = tempfile.mkstemp(dir=path.parent, prefix=f".{path.name}.")
    tmp = Path(name)
    try:
        with os.fdopen(fd, "wb") as handle:
            handle.write(new)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(tmp, path)
    except Exception:
        tmp.unlink(missing_ok=True)
        raise


def _read_bytes(path: Path) -> bytes:
    try:
        return path.read_bytes()
    except OSError as exc:
        raise ValueError(f"cannot read {path}") from exc


def _read_text(path: Path) -> str:
    try:
        return _read_bytes(path).decode("utf-8")
    except UnicodeError as exc:
        raise ValueError(f"{path} is not UTF-8") from exc


def _loads(text: str, label: str) -> dict:
    try:
        parsed = tomllib.loads(text)
    except tomllib.TOMLDecodeError as exc:
        raise ValueError(f"{label} is not valid TOML") from exc
    if not isinstance(parsed, dict):
        raise ValueError(f"{label} is not a TOML table")
    return parsed


def _lock_header(lock: str) -> str:
    index = lock.find(_LOCK_PACKAGE)
    if index < 0:
        raise ValueError("Cargo.lock has no packages")
    return lock[:index]


def _body(line: str) -> str:
    if line.endswith("\r\n"):
        return line[:-2]
    if line.endswith("\n"):
        return line[:-1]
    return line


def _ending(line: str) -> str:
    if line.endswith("\r\n"):
        return "\r\n"
    if line.endswith("\n"):
        return "\n"
    return ""


if __name__ == "__main__":
    main()

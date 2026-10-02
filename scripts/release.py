#!/usr/bin/env python3
"""Package explicit public build outputs; never archive a checkout/state store."""

import argparse
import hashlib
import io
import json
import re
import tarfile
import zipfile
from pathlib import Path

TARGETS = (
    "x86_64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
)


def package(repo, bins, output, version, target, kind, image):
    if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?", version):
        raise ValueError(
            "version must be vMAJOR.MINOR.PATCH with an optional prerelease suffix"
        )
    if target not in TARGETS or kind not in ("client", "server"):
        raise ValueError("unsupported target or package kind")
    windows = target.endswith("windows-msvc")
    if kind == "client":
        binary = "kiln.exe" if windows else "kiln"
        files = {binary: bins / binary}
        name = f"kiln-{version}-{target}"
    else:
        if target != "x86_64-unknown-linux-gnu" or image is None:
            raise ValueError(
                "server requires Linux x86-64 and a prebuilt image directory"
            )
        files = {
            f"bin/{name}": bins / name
            for name in ("kiln-runtime", "kiln-host", "kiln-api", "kiln")
        }
        files.update(
            {
                "install.py": repo / "deploy/install.py",
                "bin/kiln-network": repo / "deploy/kiln-network",
                "fetch-firecracker.sh": repo / "scripts/fetch-firecracker.sh",
                "README.md": repo / "README.md",
                "docs/releases.md": repo / "docs/releases.md",
                "docs/remote-client.md": repo / "docs/remote-client.md",
                "docs/runtime.md": repo / "docs/runtime.md",
            }
        )
        files.update(
            {
                f"deploy/{name}": repo / "deploy" / name
                for name in (
                    "kiln-host.service",
                    "kiln-api.service",
                    "host.example.json",
                )
            }
        )
        files.update(
            {
                f"image/{name}": image / name
                for name in (
                    "image.json",
                    "inputs.json",
                    "vmlinux",
                    "rootfs.ext4",
                )
            }
        )
        name = f"kiln-server-{version}-{target}"
    for path in files.values():
        if path.is_symlink() or not path.is_file():
            raise FileNotFoundError(f"missing regular package input: {path}")
    output.mkdir(parents=True, exist_ok=True)
    dest = output / (name + (".zip" if windows else ".tar.gz"))
    checksum = Path(str(dest) + ".sha256")
    if dest.exists() or checksum.exists():
        raise FileExistsError(f"refusing to overwrite {dest}")
    # Exclusive creation; failed builds are not valid downloadable packages.
    with dest.open("xb") as stream:
        if windows:
            with zipfile.ZipFile(stream, "w", zipfile.ZIP_DEFLATED) as archive:
                for relative, path in files.items():
                    archive.write(path, relative)
        else:
            with tarfile.open(fileobj=stream, mode="w:gz") as archive:
                for relative, path in files.items():
                    info = archive.gettarinfo(str(path), relative)
                    info.uid = info.gid = info.mtime = 0
                    info.uname = info.gname = ""
                    info.mode = (
                        0o755
                        if relative == "kiln" or relative.startswith("bin/")
                        else 0o644
                    )
                    with path.open("rb") as source:
                        archive.addfile(info, source)
                if kind == "server":
                    data = json.dumps({"version": version, "target": target}).encode()
                    info = tarfile.TarInfo("release.json")
                    info.size, info.mode = len(data), 0o644
                    archive.addfile(info, io.BytesIO(data))
    with dest.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    with checksum.open("x") as stream:
        stream.write(f"{digest}  {dest.name}\n")
    return dest


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("kind", choices=("client", "server"))
    parser.add_argument("--version", required=True)
    parser.add_argument("--target", required=True, choices=TARGETS)
    parser.add_argument("--bin-dir", type=Path, required=True)
    parser.add_argument("--image-dir", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    print(
        package(
            Path(__file__).resolve().parent.parent,
            args.bin_dir,
            args.output,
            args.version,
            args.target,
            args.kind,
            args.image_dir,
        )
    )

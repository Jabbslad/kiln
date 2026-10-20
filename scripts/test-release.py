#!/usr/bin/env python3
"""Packaging checks use tiny stand-ins, never credentials or built VM state."""

import hashlib
import importlib.util
import json
import os
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        spec = importlib.util.spec_from_file_location(
            "release", Path(__file__).with_name("release.py")
        )
        self.assertIsNotNone(spec)
        self.assertTrue(Path(spec.origin).is_file(), "release packager is missing")
        self.release = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.release)
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self.bins = self.root / "bin"
        self.bins.mkdir()
        for name in ["kiln", "kiln.exe", "kiln-runtime", "kiln-host", "kiln-api"]:
            (self.bins / name).write_bytes(b"binary-" + name.encode())
        self.out = self.root / "dist"

    def package(self, kind="client", target="x86_64-unknown-linux-gnu", image=None):
        return self.release.package(
            self.repo, self.bins, self.out, "v0.1.0", target, kind, image
        )

    def test_client_allowlist_permissions_checksum_and_no_overwrite(self):
        (self.bins / "admin.token").write_text("secret")
        path = self.package()
        self.assertEqual(path.name, "kiln-v0.1.0-x86_64-unknown-linux-gnu.tar.gz")
        with tarfile.open(path) as archive:
            self.assertEqual(archive.getnames(), ["kiln"])
            self.assertEqual(archive.getmember("kiln").mode, 0o755)
            self.assertEqual(archive.extractfile("kiln").read(), b"binary-kiln")
        expected = hashlib.sha256(path.read_bytes()).hexdigest()
        self.assertEqual(
            Path(str(path) + ".sha256").read_text(), f"{expected}  {path.name}\n"
        )
        with self.assertRaises(FileExistsError):
            self.package()

    def test_windows_zip_contains_only_exe(self):
        with zipfile.ZipFile(self.package(target="x86_64-pc-windows-msvc")) as archive:
            self.assertEqual(archive.namelist(), ["kiln.exe"])
            self.assertEqual(archive.read("kiln.exe"), b"binary-kiln.exe")

    def test_dashboard_contains_only_static_build_and_usage(self):
        (self.repo / "web").mkdir()
        (self.repo / "web/README.md").write_text("Frontend only; no live control plane")
        (self.bins / "assets").mkdir()
        for name in ["index.html", "favicon.svg", "THIRD_PARTY_LICENSES.md", "assets/index-ab.js", "assets/index-cd.css", "assets/font-ef.woff2", "assets/hero-gh.webp"]:
            (self.bins / name).write_text(name)
        (self.bins / "admin.token").write_text("private")
        path = self.package(kind="dashboard", target="web")
        self.assertEqual(path.name, "kiln-dashboard-v0.1.0.tar.gz")
        with tarfile.open(path) as archive:
            self.assertEqual(set(archive.getnames()), {
                "index.html", "favicon.svg", "assets/index-ab.js", "assets/index-cd.css",
                "assets/font-ef.woff2", "assets/hero-gh.webp", "THIRD_PARTY_LICENSES.md", "README.md", "release.json",
            })
            self.assertEqual(archive.extractfile("assets/index-ab.js").read(), b"assets/index-ab.js")
            self.assertTrue(all(member.mode == 0o644 for member in archive.getmembers()))
            self.assertEqual(json.load(archive.extractfile("release.json")), {
                "version": "v0.1.0", "target": "web",
            })
        self.assertEqual(Path(str(path) + ".sha256").read_text().split()[0],
                         hashlib.sha256(path.read_bytes()).hexdigest())
        # Dashboard packaging runs on Linux; Windows client builders need not
        # have the privilege required to create symbolic links.
        if os.name != "nt":
            (self.bins / "assets/private.js").symlink_to(self.bins / "admin.token")
            with self.assertRaises(ValueError):
                self.package(kind="dashboard", target="web")
            (self.bins / "assets/private.js").unlink()
        (self.bins / "assets/private.pem").write_text("private")
        with self.assertRaises(ValueError):
            self.package(kind="dashboard", target="web")
        with self.assertRaises(ValueError):
            self.package(kind="client", target="web")

    def test_identity_allowlist_excludes_vm_and_private_state(self):
        (self.bins / "kiln-identity").write_bytes(b"identity-binary")
        (self.bins / "signing.pem").write_text("private fixture")
        for name in ["deploy/kiln-identity.service", "deploy/identity.example.env", "docs/identity-service.md"]:
            path = self.repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(name)
        with tarfile.open(self.package(kind="identity")) as archive:
            self.assertEqual(set(archive.getnames()), {
                "bin/kiln-identity", "deploy/kiln-identity.service",
                "deploy/identity.example.env", "docs/identity-service.md", "release.json",
            })
            self.assertEqual(archive.getmember("bin/kiln-identity").mode, 0o755)
            identity_release = json.load(archive.extractfile("release.json"))
            self.assertEqual(identity_release["version"], "v0.1.0")
            self.assertNotIn("min_upgrade_from", identity_release)
        with self.assertRaises(ValueError):
            self.package(kind="identity", target="x86_64-apple-darwin")

    def test_invalid_version_target_and_missing_binary_make_no_archive(self):
        for version, target in [
            ("../escape", "x86_64-unknown-linux-gnu"),
            ("v0.1.0-rc.1", "x86_64-unknown-linux-gnu"),
            ("v0.1.0", "arm-unknown"),
        ]:
            with self.assertRaises(ValueError):
                self.release.package(
                    self.repo, self.bins, self.out, version, target, "client", None
                )
            self.assertFalse(self.out.exists(), version)
        (self.bins / "kiln").unlink()
        with self.assertRaises(FileNotFoundError):
            self.package()
        self.assertFalse(self.out.exists())

    def test_server_contains_prebuilt_image_not_host_snapshots_or_secrets(self):
        for name in [
            "deploy/install.sh",
            "deploy/kiln-network",
            "deploy/kiln-host.service",
            "deploy/kiln-api.service",
            "deploy/kiln-door.service",
            "deploy/kiln-door-proxy.service",
            "deploy/kiln-door-proxy.socket",
            "deploy/kiln-door-proxy-public.socket",
            "deploy/paseo-caddy.override.yaml",
            "deploy/door.Caddyfile",
            "deploy/enroll-dark-forge.sh",
            "docs/runbooks/2026-10-19-door-public-cert.md",
            "deploy/host.example.json",
            "scripts/fetch-firecracker.sh",
            "README.md",
            "docs/releases.md",
            "docs/remote-client.md",
            "docs/identity-service.md",
            "docs/runtime.md",
            "docs/runbooks/2026-10-19-public-door.md",
        ]:
            path = self.repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(name)
        image = self.root / "image"
        image.mkdir()
        for name in ["image.json", "inputs.json", "rootfs.ext4", "vmlinux"]:
            (image / name).write_text(name)
        (image / "snapshot.memory").write_text("private VM memory")
        (image / "admin.token").write_text("secret")
        path = self.package(kind="server", image=image)
        with tarfile.open(path) as archive:
            names = set(archive.getnames())
            self.assertEqual(
                names,
                {
                    "bin/kiln-runtime",
                    "bin/kiln-host",
                    "bin/kiln-api",
                    "bin/kiln",
                    "bin/kiln-network",
                    "install.sh",
                    "deploy/kiln-host.service",
                    "deploy/kiln-api.service",
                    "deploy/kiln-door.service",
                    "deploy/kiln-door-proxy.service",
                    "deploy/kiln-door-proxy.socket",
                    "deploy/kiln-door-proxy-public.socket",
                    "deploy/paseo-caddy.override.yaml",
                    "deploy/door.Caddyfile",
                    "deploy/enroll-dark-forge.sh",
                    "deploy/host.example.json",
                    "fetch-firecracker.sh",
                    "README.md",
                    "docs/releases.md",
                    "docs/remote-client.md",
                    "docs/identity-service.md",
                    "docs/runtime.md",
                    "docs/runbooks/2026-10-19-public-door.md",
                    "docs/runbooks/2026-10-19-door-public-cert.md",
                    "image/image.json",
                    "image/inputs.json",
                    "image/rootfs.ext4",
                    "image/vmlinux",
                    "release.json",
                },
            )
            release = json.load(archive.extractfile("release.json"))
            self.assertEqual(release["version"], "v0.1.0")
            self.assertEqual(release["target"], "x86_64-unknown-linux-gnu")
            self.assertEqual(release["min_upgrade_from"], "0.3.0")
            self.assertEqual(archive.getmember("install.sh").mode, 0o755)
            self.assertEqual(names, bootstrap_accepts(names))


def bootstrap_accepts(archive_names):
    """Names deploy/bootstrap/install.sh accepts for this archive."""
    text = (
        Path(__file__).resolve().parent.parent / "deploy/bootstrap/install.sh"
    ).read_text()
    marker = 'cat > "$work/expected" <<FILES\n'
    start = text.index(marker) + len(marker)
    expected = set(text[start : text.index("\nFILES\n", start)].splitlines())
    if "bin/kiln-network" in archive_names:
        expected.add("bin/kiln-network")
    door_proxy = {
        "deploy/kiln-door-proxy.service",
        "deploy/kiln-door-proxy.socket",
        "deploy/kiln-door-proxy-public.socket",
        "docs/runbooks/2026-10-19-door-public-cert.md",
        "deploy/paseo-caddy.override.yaml",
        "deploy/door.Caddyfile",
    }
    if archive_names & door_proxy:
        expected.update(door_proxy)
    return expected


if __name__ == "__main__":
    unittest.main()

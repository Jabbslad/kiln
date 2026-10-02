#!/usr/bin/env python3
"""Packaging checks use tiny stand-ins, never credentials or built VM state."""

import hashlib
import importlib.util
import json
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
        for name in ["boxctl", "boxctl.exe", "box", "boxd-host", "boxd-api"]:
            (self.bins / name).write_bytes(b"binary-" + name.encode())
        self.out = self.root / "dist"

    def package(self, kind="client", target="x86_64-unknown-linux-gnu", image=None):
        return self.release.package(
            self.repo, self.bins, self.out, "v0.1.0", target, kind, image
        )

    def test_client_allowlist_permissions_checksum_and_no_overwrite(self):
        (self.bins / "admin.token").write_text("secret")
        path = self.package()
        self.assertEqual(path.name, "boxctl-v0.1.0-x86_64-unknown-linux-gnu.tar.gz")
        with tarfile.open(path) as archive:
            self.assertEqual(archive.getnames(), ["boxctl"])
            self.assertEqual(archive.getmember("boxctl").mode, 0o755)
            self.assertEqual(archive.extractfile("boxctl").read(), b"binary-boxctl")
        expected = hashlib.sha256(path.read_bytes()).hexdigest()
        self.assertEqual(
            Path(str(path) + ".sha256").read_text(), f"{expected}  {path.name}\n"
        )
        with self.assertRaises(FileExistsError):
            self.package()

    def test_windows_zip_contains_only_exe(self):
        with zipfile.ZipFile(self.package(target="x86_64-pc-windows-msvc")) as archive:
            self.assertEqual(archive.namelist(), ["boxctl.exe"])
            self.assertEqual(archive.read("boxctl.exe"), b"binary-boxctl.exe")

    def test_invalid_version_target_and_missing_binary_make_no_archive(self):
        for version, target in [
            ("../escape", "x86_64-unknown-linux-gnu"),
            ("v0.1.0", "arm-unknown"),
        ]:
            with self.assertRaises(ValueError):
                self.release.package(
                    self.repo, self.bins, self.out, version, target, "client", None
                )
        (self.bins / "boxctl").unlink()
        with self.assertRaises(FileNotFoundError):
            self.package()
        self.assertFalse(self.out.exists())

    def test_server_contains_prebuilt_image_not_host_snapshots_or_secrets(self):
        for name in [
            "deploy/install.py",
            "deploy/boxd-network",
            "deploy/boxd-host.service",
            "deploy/boxd-api.service",
            "deploy/host.example.json",
            "scripts/fetch-firecracker.sh",
            "README.md",
            "docs/releases.md",
            "docs/remote-client.md",
            "docs/runtime.md",
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
                    "bin/box",
                    "bin/boxd-host",
                    "bin/boxd-api",
                    "bin/boxctl",
                    "bin/boxd-network",
                    "install.py",
                    "deploy/boxd-host.service",
                    "deploy/boxd-api.service",
                    "deploy/host.example.json",
                    "fetch-firecracker.sh",
                    "README.md",
                    "docs/releases.md",
                    "docs/remote-client.md",
                    "docs/runtime.md",
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


if __name__ == "__main__":
    unittest.main()

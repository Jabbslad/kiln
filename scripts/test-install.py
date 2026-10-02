#!/usr/bin/env python3
"""Installer tests never install accounts/services or touch real runtime data."""

import importlib.util
import io
import json
import os
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import call, patch


class InstallTests(unittest.TestCase):
    def setUp(self):
        path = Path(__file__).resolve().parent.parent / "deploy/install.py"
        self.assertTrue(path.is_file(), "guided installer is missing")
        spec = importlib.util.spec_from_file_location("installer", path)
        self.install = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.install)
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def test_supported_ubuntu_hosts_continue_to_capability_checks(self):
        for version in ("24.04", "26.04"):
            with (
                self.subTest(version=version),
                patch.object(self.install.platform, "machine", return_value="x86_64"),
                patch.object(
                    self.install.platform,
                    "freedesktop_os_release",
                    return_value={"ID": "ubuntu", "VERSION_ID": version},
                ),
                patch.object(
                    self.install.Path, "is_dir", return_value=False
                ) as systemd,
                self.assertRaisesRegex(ValueError, "booted systemd host"),
            ):
                self.install.preflight(self.root, "127.0.0.1")
            systemd.assert_called_once()

    def test_other_distros_releases_and_architectures_still_fail_first(self):
        for distro, version, architecture in (
            ("ubuntu", "22.04", "x86_64"),
            ("ubuntu", "24.10", "x86_64"),
            ("ubuntu", "26.10", "x86_64"),
            ("debian", "26.04", "x86_64"),
            ("ubuntu", "26.04", "aarch64"),
        ):
            with (
                self.subTest(distro=distro, version=version, architecture=architecture),
                patch.object(
                    self.install.platform, "machine", return_value=architecture
                ),
                patch.object(
                    self.install.platform,
                    "freedesktop_os_release",
                    return_value={"ID": distro, "VERSION_ID": version},
                ),
                patch.object(self.install.Path, "is_dir") as systemd,
                self.assertRaisesRegex(ValueError, "supports Ubuntu"),
            ):
                self.install.preflight(self.root, "127.0.0.1")
            systemd.assert_not_called()

    def test_bind_address_is_explicitly_private_not_merely_nonglobal(self):
        for value in [
            "10.2.3.4",
            "172.16.0.1",
            "172.31.255.254",
            "192.168.3.7",
            "100.64.0.1",
            "100.127.255.254",
            "127.0.0.1",
        ]:
            self.assertEqual(self.install.private_address(value), value)
        for value in [
            "0.0.0.0",
            "8.8.8.8",
            "172.15.255.254",
            "172.32.0.1",
            "100.128.0.1",
            "169.254.1.2",
            "192.0.2.3",
            "224.0.0.1",
            "::1",
            "host;id",
        ]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                self.install.private_address(value)

    def test_numeric_identity_ranges_check_overlap_on_both_boundaries(self):
        for data in ["alice:69999:2\n", "alice:70007:1\n", "alice:60000:20000\n"]:
            with self.assertRaises(ValueError):
                self.install.check_subids(data, 70000)
        for data in ["", "alice:69999:1\n", "alice:70008:9\n"]:
            self.install.check_subids(data, 70000)

    def test_network_is_opt_in_and_uses_pinned_helper_not_client_commands(self):
        self.install.write_config(self.root, network=True)
        policy = json.loads((self.root / "isolation.json").read_text())
        self.assertEqual(
            policy["network"],
            {
                "helper": "/opt/boxd/bin/boxd-network",
                "namespace_scope": "boxd",
                "resolver": "1.1.1.1",
            },
        )

    def test_network_preflight_rejects_interface_injection_before_commands(self):
        for interface in ["", "eth0;id", "eth0\n", "../eth0", "a" * 16]:
            with (
                self.subTest(interface=interface),
                patch.object(self.install, "run") as run,
            ):
                with self.assertRaises(ValueError):
                    self.install.network_preflight(interface)
                run.assert_not_called()

    def test_existing_and_symlink_destinations_fail_before_install(self):
        target = self.root / "existing"
        target.write_text("preserve")
        with self.assertRaises(ValueError):
            self.install.require_absent([target])
        target.unlink()
        target.symlink_to(self.root / "missing")
        with self.assertRaises(ValueError):
            self.install.require_absent([target])

    def test_real_certificates_and_enrollment_do_not_export_signing_key(self):
        with patch.object(self.install.os, "chown"):
            self.install.credentials(
                self.root, os.getuid(), os.getgid(), "100.70.80.90"
            )
        cert = subprocess.check_output(
            ["openssl", "x509", "-in", str(self.root / "tls.crt"), "-noout", "-text"],
            text=True,
        )
        self.assertIn("IP Address:100.70.80.90", cert)
        self.assertIn("CA:FALSE", cert)
        subprocess.run(
            [
                "openssl",
                "verify",
                "-CAfile",
                str(self.root / "ca.crt"),
                str(self.root / "tls.crt"),
            ],
            check=True,
            capture_output=True,
        )
        token = (self.root / "admin.token").read_text().strip()
        self.assertRegex(token, r"^[0-9a-f]{64}$")
        for name in ["admin.token", "tls.key", "ca.key", "laptop.tar.gz"]:
            self.assertEqual((self.root / name).stat().st_mode & 0o777, 0o600)
        with tarfile.open(self.root / "laptop.tar.gz") as archive:
            self.assertEqual(
                set(archive.getnames()),
                {
                    "boxd-connection/admin.token",
                    "boxd-connection/ca.crt",
                    "boxd-connection/CONNECT.txt",
                },
            )
            self.assertIn(
                b"https://100.70.80.90:8443",
                archive.extractfile("boxd-connection/CONNECT.txt").read(),
            )
            self.assertNotIn(token, (self.root / "CONNECT.txt").read_text())

    def test_units_restore_cgroups_at_boot_and_do_not_enable_guest_network(self):
        self.install.write_units(self.root, "192.168.50.2")
        group = (self.root / "boxd-cgroup.service").read_text()
        self.assertIn("+cpu +memory +pids", group)
        self.assertIn("RemainAfterExit=yes", group)
        host = (self.root / "boxd-host.service.d/installer.conf").read_text()
        self.assertIn("Requires=boxd-cgroup.service", host)
        self.assertIn("After=boxd-cgroup.service", host)
        api = (self.root / "boxd-api.service.d/installer.conf").read_text()
        self.assertIn("ExecStart=\n", api)
        self.assertIn("--listen 192.168.50.2:8443", api)
        self.assertNotIn("0.0.0.0", api)

    def test_config_uses_isolation_and_server_owned_template_only(self):
        self.install.write_config(self.root, "c" * 32)
        config = json.loads((self.root / "host.json").read_text())
        self.assertFalse(config["allow_unsafe_development"])
        self.assertEqual(config["templates"], {"ubuntu-4g": "c" * 32})
        policy = json.loads((self.root / "isolation.json").read_text())
        self.assertEqual((policy["uid_base"], policy["gid_base"]), (70000, 71000))
        self.assertEqual(policy["disk_backend"], "copy")

    def test_check_only_rejection_and_cancellation_never_install(self):
        for arguments, problem, confirmation, expected in [
            ([], None, "INSTALL", 0),
            (["--apply"], ValueError("unsupported host"), "INSTALL", 1),
            (["--apply"], None, "no", 1),
        ]:
            with (
                patch.object(self.install.sys, "argv", ["install.py", *arguments]),
                patch.dict(os.environ),
                patch.object(self.install, "preflight", side_effect=problem),
                patch.object(self.install, "install") as apply,
                patch.object(self.install.os, "geteuid", return_value=0),
                patch("builtins.input", return_value=confirmation),
                patch("sys.stdout", new=io.StringIO()),
                patch("sys.stderr", new=io.StringIO()),
            ):
                self.assertEqual(self.install.main(), expected)
                apply.assert_not_called()

    def test_failed_readiness_disables_services_preserves_state_and_existing_modes(
        self,
    ):
        paths = {
            name: self.root / name
            for name in ("ETC", "STATE", "OPT", "LIBEXEC", "UNITS")
        }
        paths["UNITS"].mkdir()
        paths["LIBEXEC"].mkdir(mode=0o750)
        commands = []

        def fake_run(*args):
            command = [str(arg) for arg in args]
            commands.append(command)
            if "template" in command:
                return json.dumps({"id": "d" * 32})
            return ""

        previous_mask = os.umask(0o077)
        self.addCleanup(os.umask, previous_mask)
        with (
            patch.multiple(self.install, **paths),
            patch.dict(os.environ),
            patch.object(self.install, "run", side_effect=fake_run),
            patch.object(
                self.install.pwd,
                "getpwnam",
                return_value=SimpleNamespace(pw_uid=123, pw_gid=456),
            ),
            patch.object(self.install, "credentials"),
            patch.object(
                self.install, "healthcheck", side_effect=ValueError("not ready")
            ),
            patch.object(self.install.subprocess, "run") as cleanup,
            patch("sys.stdout", new=io.StringIO()),
            self.assertRaisesRegex(ValueError, "not ready"),
        ):
            self.install.install(self.root / "package", "127.0.0.1")
        self.assertEqual(
            cleanup.call_args_list,
            [
                call(
                    ["systemctl", "disable", "--now", name],
                    check=False,
                    capture_output=True,
                )
                for name in (
                    "boxd-api.service",
                    "boxd-host.service",
                    "boxd-cgroup.service",
                )
            ],
        )
        self.assertTrue(paths["STATE"].exists())
        config = json.loads((paths["ETC"] / "host.json").read_text())
        self.assertEqual(config["templates"], {"ubuntu-4g": "d" * 32})
        self.assertEqual(paths["LIBEXEC"].stat().st_mode & 0o777, 0o750)
        template_index = next(
            i for i, args in enumerate(commands) if "template" in args
        )
        enable_index = next(i for i, args in enumerate(commands) if "enable" in args)
        self.assertLess(template_index, enable_index)
        self.assertIn("--profile", commands[template_index])
        self.assertIn("isolated", commands[template_index])


if __name__ == "__main__":
    unittest.main()

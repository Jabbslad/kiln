#!/usr/bin/env python3
"""Installer tests never install accounts/services or touch real runtime data."""

import importlib.util
import io
import json
import os
import sqlite3
import subprocess
import tarfile
import tempfile
import unittest
from contextlib import closing, nullcontext
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
        paths = patch.multiple(
            self.install, **{key: self.root / key for key in ("ETC", "OPT", "STATE")}
        )
        paths.start()
        self.addCleanup(paths.stop)

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
                "helper": "/opt/kiln/bin/kiln-network",
                "namespace_scope": "kiln",
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

    def test_network_preflight_requires_nsenter_before_commands(self):
        with (
            patch.object(
                self.install.shutil,
                "which",
                side_effect=lambda tool: (
                    None if tool == "nsenter" else f"/usr/bin/{tool}"
                ),
            ),
            patch.object(self.install, "run") as run,
            patch.object(self.install, "require_absent"),
        ):
            with self.assertRaisesRegex(ValueError, "util-linux"):
                self.install.network_preflight("eth0")
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
                    "kiln-connection/admin.token",
                    "kiln-connection/ca.crt",
                    "kiln-connection/CONNECT.txt",
                },
            )
            self.assertIn(
                b"https://100.70.80.90:8443",
                archive.extractfile("kiln-connection/CONNECT.txt").read(),
            )
            self.assertNotIn(token, (self.root / "CONNECT.txt").read_text())

    def test_units_restore_cgroups_at_boot_and_do_not_enable_guest_network(self):
        self.install.write_units(self.root, "192.168.50.2")
        group = (self.root / "kiln-cgroup.service").read_text()
        self.assertIn("+cpu +memory +pids", group)
        self.assertIn("RemainAfterExit=yes", group)
        host = (self.root / "kiln-host.service.d/installer.conf").read_text()
        self.assertIn("Requires=kiln-cgroup.service", host)
        self.assertIn("After=kiln-cgroup.service", host)
        api = (self.root / "kiln-api.service.d/installer.conf").read_text()
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

    def test_check_only_and_rejection_never_install(self):
        for arguments, problem, expected in [
            ([], None, 0),
            (["--apply"], ValueError("unsupported host"), 1),
        ]:
            with (
                patch.object(self.install.sys, "argv", ["install.py", *arguments]),
                patch.dict(os.environ),
                patch.object(self.install, "installer_lock", side_effect=nullcontext),
                patch.object(self.install, "preflight", side_effect=problem),
                patch.object(self.install, "install") as apply,
                patch.object(self.install.os, "geteuid", return_value=0),
                patch("builtins.input", side_effect=AssertionError("must not prompt")),
                patch("sys.stdout", new=io.StringIO()),
                patch("sys.stderr", new=io.StringIO()),
            ):
                self.assertEqual(self.install.main(), expected)
                apply.assert_not_called()

    def test_explicit_apply_installs_after_checks_without_input(self):
        with (
            patch.object(
                self.install.sys,
                "argv",
                ["install.py", "--apply", "--address", "100.64.5.6"],
            ),
            patch.dict(os.environ),
            patch.object(self.install, "installer_lock", side_effect=nullcontext),
            patch.object(self.install, "preflight") as preflight,
            patch.object(self.install, "install") as apply,
            patch.object(self.install.os, "geteuid", return_value=0),
            patch("builtins.input", side_effect=AssertionError("must not prompt")),
            patch("sys.stdout", new=io.StringIO()),
        ):
            self.assertEqual(self.install.main(), 0)
            preflight.assert_called_once_with(
                Path(self.install.__file__).resolve().parent, "100.64.5.6"
            )
            apply.assert_called_once_with(
                Path(self.install.__file__).resolve().parent, "100.64.5.6", None
            )

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
                    "kiln-api.service",
                    "kiln-host.service",
                    "kiln-cgroup.service",
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


class UpgradeTests(unittest.TestCase):
    def setUp(self):
        spec = importlib.util.spec_from_file_location(
            "installer", Path(__file__).resolve().parent.parent / "deploy/install.py"
        )
        self.installer = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.installer)
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.paths = {
            key: self.root / key for key in ("ETC", "OPT", "STATE", "LIBEXEC", "UNITS")
        }
        for path in self.paths.values():
            path.mkdir(mode=0o700)
        self.bundle = self.root / "package"
        (self.bundle / "bin").mkdir(parents=True)
        (self.paths["OPT"] / "bin").mkdir()
        self.destinations = {
            name: self.paths["OPT"] / "bin" / name
            for name in ("kiln", "kiln-runtime", "kiln-network")
        }
        self.destinations.update(
            {name: self.paths["LIBEXEC"] / name for name in ("kiln-host", "kiln-api")}
        )
        for name, path in self.destinations.items():
            reported_name = "kiln-server" if name in ("kiln-host", "kiln-api") else name
            path.write_text(f"#!/bin/sh\necho '{reported_name} 0.3.2'\n")
            path.chmod(0o755)
            new = self.bundle / "bin" / name
            new.write_text(f"#!/bin/sh\necho '{reported_name} 0.3.3'\n")
            new.chmod(0o755)
        (self.bundle / "release.json").write_text(
            json.dumps({"version": "v0.3.3", "target": "x86_64-unknown-linux-gnu"})
        )
        config = {
            "runtime_dir": str(self.paths["STATE"] / "runtime"),
            "journal_dir": str(self.paths["STATE"] / "journal"),
            "socket": "/run/kiln/host.sock",
            "isolation_config": str(self.paths["ETC"] / "isolation.json"),
            "templates": {"custom": "a" * 32},
        }
        (self.paths["ETC"] / "host.json").write_text(json.dumps(config))
        (self.paths["ETC"] / "isolation.json").write_text("{}")
        for name in ("ca.crt", "tls.crt", "tls.key", "admin.token"):
            (self.paths["ETC"] / name).write_text("synthetic-test-only")
        for directory in ("runtime", "journal", "image"):
            (self.paths["STATE"] / directory).mkdir()
            (self.paths["STATE"] / directory / "preserve").write_text(directory)
        self.journal = self.paths["STATE"] / "journal/operations.sqlite"
        with closing(sqlite3.connect(self.journal)) as db, db:
            db.executescript(
                "PRAGMA user_version=1; CREATE TABLE operations(running INTEGER); INSERT INTO operations VALUES(0);"
            )
        self.catalog = [{"name": "custom", "memory_mib": 3072, "vcpus": 2}]
        self.commands = []
        self.active = {"kiln-host.service": "active", "kiln-api.service": "active"}
        self.kill_mode = "process"
        self.real_run = self.installer.run
        for patcher in (
            patch.multiple(self.installer, **self.paths),
            patch.object(self.installer, "trusted_ancestors"),
            patch.object(self.installer, "run", side_effect=self.fake_run),
            patch("sys.stdout", new=io.StringIO()),
        ):
            patcher.start()
            self.addCleanup(patcher.stop)
        self.health = patch.object(
            self.installer, "healthcheck", return_value=self.catalog
        ).start()
        self.addCleanup(patch.stopall)
        self.original = {
            name: path.read_bytes() for name, path in self.destinations.items()
        }
        self.data = {
            str(p.relative_to(self.root)): p.read_bytes()
            for base in ("ETC", "STATE")
            for p in self.paths[base].rglob("*")
            if p.is_file()
        }

    def fake_run(self, *args):
        args = [str(a) for a in args]
        self.commands.append(args)
        if args[0] != "systemctl":
            if "--check" in args:
                return "checked"
            return self.real_run(*args)
        if args[1] in ("stop", "start"):
            for service in args[2:]:
                self.active[service] = "inactive" if args[1] == "stop" else "active"
            return ""
        self.assertEqual(args[1], "show")
        service = args[-1]
        if "--property=ActiveState" in args:
            return self.active[service]
        if "--property=KillMode" in args:
            return self.kill_mode
        if "--property=ExecStart" in args:
            name = service.removesuffix(".service")
            path = self.destinations[name]
            if name == "kiln-host":
                argv = f"{path} --config {self.paths['ETC'] / 'host.json'}"
            elif name == "kiln-network":
                argv = f"{path} provision enp7s0"
            else:
                argv = f"{path} --listen 100.64.5.6:8443 --host-socket /run/kiln/host.sock --token-file {self.paths['ETC'] / 'admin.token'} --tls-cert {self.paths['ETC'] / 'tls.crt'} --tls-key {self.paths['ETC'] / 'tls.key'}"
            return f"{{ path={path} ; argv[]={argv} ; ignore_errors=no ; }}"
        self.fail(f"unexpected systemctl command: {args}")

    def assert_data_preserved(self):
        for relative, data in self.data.items():
            self.assertEqual((self.root / relative).read_bytes(), data)

    def assert_original_binaries(self):
        for name, path in self.destinations.items():
            self.assertEqual(path.read_bytes(), self.original[name])

    def test_update_preserves_configuration_and_state_and_backs_up_binaries(self):
        self.installer.upgrade(self.bundle, apply=True)
        for name, path in self.destinations.items():
            self.assertEqual(
                path.read_bytes(), (self.bundle / "bin" / name).read_bytes()
            )
        self.assert_data_preserved()
        actions = [
            args
            for args in self.commands
            if args[:2] in (["systemctl", "stop"], ["systemctl", "start"])
        ]
        self.assertEqual(
            actions,
            [
                ["systemctl", "stop", "kiln-api.service"],
                ["systemctl", "stop", "kiln-host.service"],
                ["systemctl", "start", "kiln-host.service"],
                ["systemctl", "start", "kiln-api.service"],
            ],
        )
        self.health.assert_has_calls(
            [call("100.64.5.6"), call("100.64.5.6", self.catalog)]
        )
        backups = list((self.paths["OPT"] / "upgrades").glob("*/kiln-api"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].read_bytes(), self.original["kiln-api"])
        self.assertFalse((self.paths["OPT"] / "pending-upgrade.json").exists())
        self.commands.clear()
        self.installer.upgrade(self.bundle, apply=True)
        self.assertFalse(any(args[1] == "stop" for args in self.commands))
        self.assertEqual(
            list((self.paths["OPT"] / "upgrades").glob("*/kiln-api")), backups
        )

    def test_check_only_does_not_mutate_or_stop_services(self):
        self.installer.upgrade(self.bundle)
        self.assert_original_binaries()
        self.assert_data_preserved()
        self.assertFalse((self.paths["OPT"] / "upgrades").exists())
        self.assertFalse(any(args[1] == "stop" for args in self.commands))

    def test_busy_operations_restore_gateway_without_stopping_host(self):
        with closing(sqlite3.connect(self.journal)) as db, db:
            db.execute("INSERT INTO operations VALUES(1)")
        with (
            patch.object(self.installer.time, "sleep"),
            self.assertRaisesRegex(ValueError, "restored"),
        ):
            self.installer.upgrade(self.bundle, apply=True)
        self.assertNotIn(["systemctl", "stop", "kiln-host.service"], self.commands)
        self.assert_original_binaries()
        self.assertEqual(set(self.active.values()), {"active"})

    def test_operations_finish_before_host_is_stopped(self):
        with closing(sqlite3.connect(self.journal)) as db, db:
            db.execute("INSERT INTO operations VALUES(1)")

        def finish(_):
            self.assertNotIn(["systemctl", "stop", "kiln-host.service"], self.commands)
            with closing(sqlite3.connect(self.journal)) as db, db:
                db.execute("UPDATE operations SET running=0")

        with patch.object(self.installer.time, "sleep", side_effect=finish) as sleep:
            self.installer.upgrade(self.bundle, apply=True)
        sleep.assert_called_once_with(1)

    def test_start_failure_rolls_back_and_interrupt_retains_recovery_marker(self):
        def fail_start(*args):
            if args == ("systemctl", "start", "kiln-host.service") and not failed[0]:
                failed[0] = True
                raise subprocess.CalledProcessError(1, args)
            return self.fake_run(*args)

        failed = [False]
        with (
            patch.object(self.installer, "run", side_effect=fail_start),
            self.assertRaisesRegex(ValueError, "restored"),
        ):
            self.installer.upgrade(self.bundle, apply=True)
        self.assert_original_binaries()

        def interrupt(*args):
            if args == ("systemctl", "stop", "kiln-host.service"):
                raise KeyboardInterrupt
            return self.fake_run(*args)

        with (
            patch.object(self.installer, "run", side_effect=interrupt),
            self.assertRaises(KeyboardInterrupt),
        ):
            self.installer.upgrade(self.bundle, apply=True)
        pending = self.paths["OPT"] / "pending-upgrade.json"
        self.assertTrue(pending.exists())
        recovery = json.loads(pending.read_text())
        for name, data in self.original.items():
            self.assertEqual((Path(recovery["backup"]) / name).read_bytes(), data)
        with self.assertRaisesRegex(ValueError, "pending"):
            self.installer.upgrade(self.bundle, apply=True)

    def test_main_dispatches_existing_install_without_fresh_preflight(self):
        with (
            patch.object(self.installer, "installer_lock", side_effect=nullcontext),
            patch.object(self.installer.sys, "argv", ["install.py", "--apply"]),
            patch.object(self.installer.os, "geteuid", return_value=0),
            patch.dict(os.environ),
            patch.object(self.installer, "preflight") as fresh,
            patch.object(self.installer, "upgrade") as upgrade,
        ):
            self.assertEqual(self.installer.main(), 0)
        fresh.assert_not_called()
        upgrade.assert_called_once_with(
            Path(self.installer.__file__).parent, None, None, False, True
        )

    def test_readiness_failure_restores_old_binaries(self):
        self.health.side_effect = [
            self.catalog,
            ValueError("new service unhealthy"),
            self.catalog,
        ]
        with self.assertRaisesRegex(ValueError, "restored"):
            self.installer.upgrade(self.bundle, apply=True)
        self.assert_original_binaries()
        self.assert_data_preserved()
        self.assertEqual(set(self.active.values()), {"active"})
        self.assertFalse((self.paths["OPT"] / "pending-upgrade.json").exists())

    def test_partial_binary_replacement_is_rolled_back(self):
        original_copy = self.installer.atomic_copy
        failed = False

        def copy(source, destination):
            nonlocal failed
            if source == self.bundle / "bin/kiln-api" and not failed:
                failed = True
                raise OSError("disk error")
            return original_copy(source, destination)

        with (
            patch.object(self.installer, "atomic_copy", side_effect=copy),
            self.assertRaisesRegex(ValueError, "restored"),
        ):
            self.installer.upgrade(self.bundle, apply=True)
        self.assertTrue(failed)
        self.assert_original_binaries()
        self.assert_data_preserved()

    def test_rollback_failure_retains_marker_and_prevents_retry(self):
        self.health.side_effect = [
            self.catalog,
            ValueError("new failed"),
            ValueError("old failed"),
        ]
        with self.assertRaisesRegex(ValueError, "recovery"):
            self.installer.upgrade(self.bundle, apply=True)
        self.assertTrue((self.paths["OPT"] / "pending-upgrade.json").is_file())
        self.assert_original_binaries()
        self.commands.clear()
        with self.assertRaisesRegex(ValueError, "pending"):
            self.installer.upgrade(self.bundle, apply=True)
        self.assertEqual(self.commands, [])

    def test_incompatible_versions_and_overrides_do_not_stop_services(self):
        for value in ("0.2.2", "0.3.99"):
            for name, path in self.destinations.items():
                path.write_bytes(self.original[name].replace(b"0.3.2", value.encode()))
            with self.assertRaisesRegex(ValueError, "incompatible"):
                self.installer.upgrade(self.bundle, apply=True)
        for name, path in self.destinations.items():
            path.write_bytes(self.original[name])
        for kwargs in (
            {"address": "192.168.1.6"},
            {"network": True},
            {"network_uplink": "eth0"},
        ):
            with self.assertRaises(ValueError):
                self.installer.upgrade(self.bundle, apply=True, **kwargs)
        self.kill_mode = "control-group"
        with self.assertRaisesRegex(ValueError, "KillMode"):
            self.installer.upgrade(self.bundle, apply=True)
        self.assertFalse(any(args[1] == "stop" for args in self.commands))
        self.assert_data_preserved()

    def test_missing_or_symlink_binary_refused_before_services_stop(self):
        path = self.destinations["kiln-api"]
        path.unlink()
        with self.assertRaises(ValueError):
            self.installer.upgrade(self.bundle, apply=True)
        path.symlink_to(self.bundle / "bin/kiln-api")
        with self.assertRaises(ValueError):
            self.installer.upgrade(self.bundle, apply=True)
        self.assertFalse(any(args[1] == "stop" for args in self.commands))

    def test_mixed_versions_unhealthy_service_and_missing_state_are_refused(self):
        binary = self.destinations["kiln-api"]
        binary.write_bytes(self.original["kiln-api"].replace(b"0.3.2", b"0.3.0"))
        with self.assertRaisesRegex(ValueError, "mixed"):
            self.installer.upgrade(self.bundle, apply=True)
        binary.write_bytes(self.original["kiln-api"])
        self.active["kiln-host.service"] = "failed"
        with self.assertRaisesRegex(ValueError, "healthy"):
            self.installer.upgrade(self.bundle, apply=True)
        self.active["kiln-host.service"] = "active"
        (self.paths["STATE"] / "runtime").rename(self.root / "saved-runtime")
        with self.assertRaisesRegex(ValueError, "missing installed runtime_dir"):
            self.installer.upgrade(self.bundle, apply=True)
        self.assertFalse(any(args[1] == "stop" for args in self.commands))

    def test_v030_networked_update_preserves_explicit_matching_options(self):
        for name, path in self.destinations.items():
            path.write_bytes(self.original[name].replace(b"0.3.2", b"0.3.0"))
        policy = self.paths["ETC"] / "isolation.json"
        policy.write_text('{"network":{"namespace_scope":"kiln"}}')
        with self.assertRaisesRegex(ValueError, "uplink"):
            self.installer.upgrade(self.bundle, network_uplink="wrong0", apply=True)
        self.installer.upgrade(
            self.bundle,
            address="100.64.5.6",
            network=True,
            network_uplink="enp7s0",
            apply=True,
        )
        self.assertEqual(policy.read_text(), '{"network":{"namespace_scope":"kiln"}}')
        self.assertNotIn(["systemctl", "stop", "kiln-network.service"], self.commands)

    def test_installer_lock_refuses_concurrent_installers(self):
        with patch.object(self.installer, "LOCK", self.root / "install.lock"):
            with self.installer.installer_lock():
                with self.assertRaisesRegex(ValueError, "another installer"):
                    with self.installer.installer_lock():
                        self.fail("lock was not exclusive")
            with self.installer.installer_lock():
                pass


if __name__ == "__main__":
    unittest.main()

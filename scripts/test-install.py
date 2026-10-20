#!/usr/bin/env python3
"""Drive deploy/install.sh against a fixture root. Never provision this host."""

import hashlib
import json
import os
import shutil
import sqlite3
import stat
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
INSTALL = ROOT / "deploy/install.sh"
HOST_UNIT = ROOT / "deploy/kiln-host.service"
API_UNIT = ROOT / "deploy/kiln-api.service"
FRESH_CATALOG = '[{"name":"ubuntu-4g","memory_mib":4096,"vcpus":1}]'
UPGRADE_CATALOG = '[{"name":"custom","memory_mib":3072,"vcpus":2}]'
TEMPLATE_ID = "d" * 32
NAMES = ("kiln", "kiln-runtime", "kiln-network", "kiln-host", "kiln-api")


def write_executable(path, text):
    path.write_text(text)
    path.chmod(0o755)


class InstallerTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.package = self.root / "package"
        self.stubs = self.root / "stubs"
        self.state = self.root / "stub-state"
        self.log = self.root / "commands.log"
        self.curl_seq = self.root / "curl.seq"
        self.curl_body = self.root / "curl.body"
        self.cp_marker = self.root / "cp-failed"
        self.mv_marker = self.root / "mv-failed"
        self.sleep_marker = self.root / "sleep-cleared"
        for path in (self.package, self.stubs, self.state):
            path.mkdir()
        script = self.package / "install.sh"
        shutil.copy(INSTALL, script)
        script.chmod(0o755)
        self.script = script
        self._write_stubs()

    def _write_stubs(self):
        log = self.log
        state = self.state
        write_executable(
            self.stubs / "systemctl",
            f"""#!/bin/bash
printf '%s\\n' "systemctl $*" >> {log!s}
prop= service=
for arg in "$@"; do
  case "$arg" in
    --property=*) prop=${{arg#--property=}} ;;
    --value|--no-pager) ;;
    show|stop|start|enable|disable|daemon-reload) ;;
    *) service=$arg ;;
  esac
done
case "$1" in
  show)
    case "$prop" in
      LoadState)
        if [ -f {state}/load-state ]; then cat {state}/load-state; else printf '%s\\n' not-found; fi
        ;;
      ActiveState) cat {state}/active-"$service" ;;
      KillMode) cat {state}/killmode ;;
      ExecStart) cat {state}/exec-"$service" ;;
      *) printf '%s\\n' "" ;;
    esac
    ;;
  stop|start)
    for svc in "${{@:2}}"; do
      if [ "$1" = stop ]; then printf '%s\\n' inactive; else printf '%s\\n' active; fi > {state}/active-"$svc"
    done
    if [ "$1" = start ] && [ "$2" = kiln-host.service ] && [ -f {state}/fail-start-host ]; then
      rm -f {state}/fail-start-host
      exit 1
    fi
    if [ "$1" = stop ] && [ "$2" = kiln-host.service ] && [ -f {state}/term-on-stop-host ]; then
      rm -f {state}/term-on-stop-host
      kill -TERM "$PPID"
      sleep 5
      exit 143
    fi
    ;;
  enable)
    if [ -f {state}/fail-enable ]; then exit 1; fi
    ;;
  disable|daemon-reload) ;;
  *) exit 1 ;;
esac
""",
        )
        write_executable(
            self.stubs / "systemd-analyze",
            f"#!/bin/bash\nprintf '%s\\n' \"systemd-analyze $*\" >> {log!s}\nexit 0\n",
        )
        passwd = self.root / "etc/passwd"
        write_executable(
            self.stubs / "useradd",
            f"""#!/bin/bash
printf '%s\\n' "useradd $*" >> {log!s}
name=${{!#}}
uid=$(id -u)
gid=$(id -g)
prev=
for arg in "$@"; do
  case "$prev" in
    --uid) uid=$arg ;;
    --gid) gid=$arg ;;
  esac
  prev=$arg
done
printf '%s\\n' "$name:x:$uid:$gid::/nonexistent:/usr/sbin/nologin" >> {passwd!s}
exit 0
""",
        )
        write_executable(
            self.stubs / "groupadd",
            f"#!/bin/bash\nprintf '%s\\n' \"groupadd $*\" >> {log!s}\nexit 0\n",
        )
        write_executable(
            self.stubs / "curl",
            f"""#!/bin/bash
printf '%s\\n' "curl $*" >> {log!s}
if [ ! -s {self.curl_seq} ]; then exit 1; fi
line=$(head -n 1 {self.curl_seq})
tail -n +2 {self.curl_seq} > {self.curl_seq}.next
mv {self.curl_seq}.next {self.curl_seq}
case "$line" in
  ok) cat {self.curl_body}; exit 0 ;;
  bad) printf '%s' '{{}}'; exit 0 ;;
  *) exit 1 ;;
esac
""",
        )
        write_executable(
            self.stubs / "sleep",
            f"""#!/bin/bash
printf '%s\\n' "sleep $*" >> {log!s}
if [ -n "${{KILN_SLEEP_CLEAR_JOURNAL:-}}" ] && [ ! -f {self.sleep_marker} ]; then
  touch {self.sleep_marker}
  sqlite3 "$KILN_SLEEP_CLEAR_JOURNAL" 'UPDATE operations SET running=0;'
fi
exit 0
""",
        )
        write_executable(
            self.stubs / "cp",
            f"""#!/bin/bash
printf '%s\\n' "cp $*" >> {log!s}
if [ -n "${{KILN_CP_FAIL_SOURCE:-}}" ] && [ ! -f {self.cp_marker} ]; then
  for arg in "$@"; do
    if [ "$arg" = "$KILN_CP_FAIL_SOURCE" ]; then
      touch {self.cp_marker}
      exit 1
    fi
  done
fi
exec /bin/cp "$@"
""",
        )
        write_executable(
            self.stubs / "mv",
            f"""#!/bin/bash
printf '%s\\n' "mv $*" >> {log!s}
if [ -n "${{KILN_MV_FAIL:-}}" ] && [ ! -f {self.mv_marker} ]; then
  live=0
  backup=0
  for arg in "$@"; do
    case "$arg" in
      */.kiln-update-*) live=1 ;;
      */upgrades/*) backup=1 ;;
    esac
  done
  if [ "$live" = 1 ] && [ "$backup" = 0 ]; then
    touch {self.mv_marker}
    exit 1
  fi
fi
exec /bin/mv "$@"
""",
        )
        for name in ("ip", "nft", "sysctl"):
            write_executable(
                self.stubs / name,
                f"#!/bin/bash\nprintf '%s\\n' \"{name} $*\" >> {log!s}\nexit 0\n",
            )
        (self.state / "killmode").write_text("process\n")

    def env(self, **extra):
        base = os.environ.copy()
        base["KILN_INSTALL_FIXTURE"] = str(self.root)
        base["PATH"] = str(self.stubs) + os.pathsep + base.get("PATH", "")
        base.update(extra)
        return base

    def invoke(self, *args, **extra):
        return subprocess.run(
            ["bash", str(self.script), *args],
            env=self.env(**extra),
            text=True,
            capture_output=True,
            stdin=subprocess.DEVNULL,
            check=False,
        )

    def lines(self, prefix=None):
        if not self.log.exists():
            return []
        rows = self.log.read_text().splitlines()
        if prefix is None:
            return rows
        return [row for row in rows if row.startswith(prefix)]

    def plan_curl(self, *steps, body):
        lines = []
        for step in steps:
            if step == "ok":
                lines.append("ok")
            elif step == "bad":
                lines.append("bad")
            elif step == "fail":
                lines.extend(["fail"] * 30)
            else:
                raise AssertionError(step)
        self.curl_seq.write_text(("\n".join(lines) + "\n") if lines else "")
        self.curl_body.write_text(body)

    def host_skeleton(self, distro="ubuntu", version="24.04"):
        for relative in (
            "etc",
            "opt",
            "usr/local",
            "var/lib",
            "run/systemd/system",
            "sys/fs/cgroup",
            "dev",
            "etc/systemd/system",
            "proc",
        ):
            (self.root / relative).mkdir(parents=True, exist_ok=True)
        (self.root / "etc/os-release").write_text(f"ID={distro}\nVERSION_ID={version}\n")
        (self.root / "proc/meminfo").write_text("MemAvailable:       8388608 kB\n")
        (self.root / "sys/fs/cgroup/cgroup.controllers").write_text("cpu memory pids\n")
        (self.root / "dev/kvm").touch()
        (self.root / "etc/passwd").write_text("")
        (self.root / "etc/group").write_text("")

    def package_inputs(self):
        kernel = b"kernel-bytes"
        rootfs = b"rootfs-bytes"
        image = self.package / "image"
        image.mkdir(exist_ok=True)
        (image / "vmlinux").write_bytes(kernel)
        (image / "rootfs.ext4").write_bytes(rootfs)
        (image / "inputs.json").write_text("{}\n")
        manifest = {
            "boot_mode": "systemd_warm",
            "architecture": "x86_64",
            "kernel_path": "vmlinux",
            "rootfs_path": "rootfs.ext4",
            "kernel_sha256": hashlib.sha256(kernel).hexdigest(),
            "rootfs_sha256": hashlib.sha256(rootfs).hexdigest(),
        }
        (image / "image.json").write_text(json.dumps(manifest) + "\n")
        deploy = self.package / "deploy"
        deploy.mkdir(exist_ok=True)
        shutil.copy(HOST_UNIT, deploy / "kiln-host.service")
        shutil.copy(API_UNIT, deploy / "kiln-api.service")
        bindir = self.package / "bin"
        bindir.mkdir(exist_ok=True)
        runtime_log = self.log
        write_executable(
            bindir / "kiln-runtime",
            f"""#!/bin/bash
printf '%s\\n' "runtime PATH=$PATH" >> {runtime_log!s}
printf '%s\\n' "runtime $*" >> {runtime_log!s}
case " $* " in
  *" template build "*)
    printf '%s\\n' '{{"id":"{TEMPLATE_ID}"}}'
    ;;
esac
exit 0
""",
        )
        for name in ("kiln", "kiln-host", "kiln-api", "kiln-network"):
            write_executable(
                bindir / name,
                f"#!/bin/bash\nprintf '%s\\n' \"bin/{name} $*\" >> {runtime_log!s}\nexit 0\n",
            )
        write_executable(
            self.package / "fetch-firecracker.sh",
            "#!/bin/bash\nprintf 'firecracker\\n' > \"$1/firecracker\"\n"
            "printf 'jailer\\n' > \"$1/jailer\"\nchmod 755 \"$1/firecracker\" \"$1/jailer\"\n",
        )

    def fresh_ready(self):
        self.host_skeleton()
        self.package_inputs()
        libexec = self.root / "usr/local/libexec"
        libexec.mkdir()
        libexec.chmod(0o750)
        self.plan_curl("ok", body=FRESH_CATALOG)


class FreshTests(InstallerTest):
    def test_installer_names_binaries_and_keeps_the_production_lock_path(self):
        text = INSTALL.read_text()
        self.assertNotIn("kiln-*", text)
        self.assertIn("/run/lock/kiln-install.lock", text)
        self.assertIn("/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin", text)

    def test_private_addresses_fail_closed_and_local_bind_is_separate(self):
        self.host_skeleton()
        for value in (
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
        ):
            with self.subTest(value=value):
                result = self.invoke("--address", value)
                self.assertNotEqual(result.returncode, 0, result.stderr)
                self.assertIn("private", result.stderr)
                self.assertFalse((self.root / "etc/kiln").exists())
        for value in (
            "10.2.3.4",
            "172.16.0.1",
            "172.31.255.254",
            "192.168.3.7",
            "100.64.0.1",
            "100.127.255.254",
        ):
            with self.subTest(value=value):
                result = self.invoke("--address", value)
                self.assertNotEqual(result.returncode, 0, result.stderr)
                self.assertIn("assigned", result.stderr)
                self.assertNotIn("private/VPN", result.stderr)

    def test_supported_hosts_reach_systemd_and_other_hosts_stop_first(self):
        for distro, version in (("ubuntu", "24.04"), ("ubuntu", "26.04"), ("debian", "13")):
            with self.subTest(distro=distro, version=version):
                self.host_skeleton(distro, version)
                shutil.rmtree(self.root / "run/systemd/system")
                result = self.invoke("--address", "127.0.0.1")
                self.assertIn("booted systemd", result.stderr)
                self.assertNotIn("supports Ubuntu", result.stderr)
        write_executable(self.stubs / "uname", "#!/bin/sh\nprintf '%s\\n' \"uname $*\" >> \"$KILN_LOG\"\nif [ \"$1\" = -m ]; then printf '%s\\n' aarch64; exit 0; fi\nexit 0\n")
        for distro, version in (
            ("ubuntu", "22.04"),
            ("ubuntu", "24.10"),
            ("debian", "12"),
            ("ubuntu", "26.04"),
        ):
            self.log.write_text("")
            self.host_skeleton(distro, version)
            shutil.rmtree(self.root / "run/systemd/system")
            result = self.invoke("--address", "127.0.0.1", KILN_LOG=str(self.log))
            self.assertIn("supports Ubuntu", result.stderr)
            self.assertNotIn("booted systemd", result.stderr)
            if (distro, version) == ("ubuntu", "26.04"):
                self.assertTrue(self.lines("uname"))
            else:
                self.assertFalse(self.lines("uname"))

    def test_low_memory_stops_before_the_port_bind(self):
        self.host_skeleton()
        (self.root / "proc/meminfo").write_text("MemAvailable:       1024 kB\n")
        result = self.invoke("--address", "100.64.5.6")
        self.assertIn("6 GiB", result.stderr)
        self.assertNotIn("assigned", result.stderr)

    def test_identity_range_and_destination_conflicts_stop_before_install(self):
        self.host_skeleton()
        (self.root / "etc/subuid").write_text("alice:69999:2\n")
        result = self.invoke("--address", "127.0.0.1")
        self.assertIn("reserved range", result.stderr)
        (self.root / "etc/subuid").write_text("alice:70008:9\n")
        (self.root / "etc/passwd").write_text("other:x:70000:1:::\n")
        result = self.invoke("--address", "127.0.0.1")
        self.assertIn("UID range", result.stderr)
        (self.root / "etc/passwd").write_text("")
        status = self.root / "proc/4242/status"
        status.parent.mkdir()
        status.write_text("Uid:\t70000\t0\t0\t0\n")
        result = self.invoke("--address", "127.0.0.1")
        self.assertIn("jail identity", result.stderr)
        shutil.rmtree(self.root / "proc/4242")
        libexec = self.root / "usr/local/libexec"
        libexec.mkdir()
        host = libexec / "kiln-host"
        host.write_text("keep")
        result = self.invoke("--address", "127.0.0.1")
        self.assertIn(f"refusing existing destination {host}", result.stderr)
        host.unlink()
        api = libexec / "kiln-api"
        api.symlink_to(self.root / "missing")
        result = self.invoke("--address", "127.0.0.1")
        self.assertIn(f"refusing existing destination {api}", result.stderr)
        api.unlink()
        etc = self.root / "etc/kiln"
        etc.mkdir()
        result = self.invoke("--address", "127.0.0.1")
        self.assertIn("incomplete installation", result.stderr)
        self.assertFalse(self.lines("systemctl stop"))
        etc.rmdir()
        (self.state / "load-state").write_text("loaded\n")
        result = self.invoke("--address", "127.0.0.1")
        self.assertIn("existing kiln-cgroup.service", result.stderr)

    def test_network_preflight_rejects_injection_and_missing_nsenter_before_ip(self):
        self.fresh_ready()
        result = self.invoke("--address", "127.0.0.1", "--network-uplink", "eth0;id")
        self.assertIn("interface", result.stderr)
        self.assertFalse(self.lines("ip "))
        self.log.write_text("")
        result = self.invoke(
            "--address",
            "127.0.0.1",
            "--network-uplink",
            "eth0",
            KILN_INSTALL_MISSING="nsenter",
        )
        self.assertIn("util-linux", result.stderr)
        self.assertFalse(self.lines("ip "))
        net = self.root / "sys/class/net/kiln0"
        net.parent.mkdir(parents=True)
        net.mkdir()
        result = self.invoke("--address", "127.0.0.1", "--network-uplink", "eth0")
        self.assertIn("refusing existing destination", result.stderr)
        self.assertIn("kiln0", result.stderr)

    def test_check_only_does_not_install(self):
        self.fresh_ready()
        result = self.invoke("--address", "127.0.0.1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Check only", result.stdout)
        self.assertIn("/usr/local/libexec/kiln-host", result.stdout)
        self.assertNotIn("kiln-*", result.stdout)
        self.assertFalse((self.root / "etc/kiln").exists())
        self.assertFalse((self.root / "opt/kiln").exists())
        self.assertEqual((self.root / "usr/local/libexec").stat().st_mode & 0o777, 0o750)

    def test_apply_installs_certificates_units_and_network_without_prompt(self):
        self.fresh_ready()
        result = self.invoke(
            "--address",
            "127.0.0.1",
            "--apply",
            "--network-uplink",
            "eth0",
        )
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assertIn("Ready: https://127.0.0.1:8443", result.stdout)
        self.assertIn("enroll --url https://127.0.0.1:8443", result.stdout)
        etc = self.root / "etc/kiln"
        cert = subprocess.check_output(
            ["openssl", "x509", "-in", str(etc / "tls.crt"), "-noout", "-text"],
            text=True,
        )
        self.assertIn("IP Address:127.0.0.1", cert)
        self.assertIn("CA:FALSE", cert)
        subprocess.run(
            ["openssl", "verify", "-CAfile", str(etc / "ca.crt"), str(etc / "tls.crt")],
            check=True,
            capture_output=True,
        )
        token = (etc / "admin.token").read_text().strip()
        self.assertRegex(token, r"^[0-9a-f]{64}$")
        for name in ("admin.token", "tls.key", "ca.key", "laptop.tar.gz"):
            self.assertEqual((etc / name).stat().st_mode & 0o777, 0o600)
        self.assertNotIn(token, (etc / "CONNECT.txt").read_text())
        with tarfile.open(etc / "laptop.tar.gz") as archive:
            self.assertEqual(
                set(archive.getnames()),
                {
                    "kiln-connection/admin.token",
                    "kiln-connection/ca.crt",
                    "kiln-connection/CONNECT.txt",
                },
            )
            self.assertIn(b"https://127.0.0.1:8443", archive.extractfile("kiln-connection/CONNECT.txt").read())
        curl_log = "\n".join(self.lines("curl "))
        self.assertNotIn(token, curl_log)
        self.assertNotIn("Bearer", curl_log)
        self.assertIn("-H @-", curl_log)
        self.assertNotIn(token, self.log.read_text())
        opt = self.root / "opt/kiln"
        units = self.root / "etc/systemd/system"
        self.assertEqual((units / "kiln-host.service").read_bytes(), HOST_UNIT.read_bytes())
        self.assertEqual((units / "kiln-api.service").read_bytes(), API_UNIT.read_bytes())
        group = (units / "kiln-cgroup.service").read_text()
        self.assertIn("+cpu +memory +pids", group)
        self.assertIn("RemainAfterExit=yes", group)
        host_drop = (units / "kiln-host.service.d/installer.conf").read_text()
        self.assertIn("Requires=kiln-cgroup.service", host_drop)
        self.assertIn("After=kiln-cgroup.service", host_drop)
        self.assertEqual((units / "kiln-host.service.d").stat().st_mode & 0o777, 0o700)
        api = (units / "kiln-api.service.d/installer.conf").read_text()
        self.assertIn("ExecStart=\n", api)
        self.assertIn("--listen 127.0.0.1:8443", api)
        self.assertNotIn("0.0.0.0", api)
        network = (units / "kiln-network.service").read_text()
        self.assertIn("ExecStart=/opt/kiln/bin/kiln-network provision eth0\n", network)
        self.assertIn("Requires=kiln-network.service", (units / "kiln-host.service.d/network.conf").read_text())
        config = json.loads((etc / "host.json").read_text())
        self.assertFalse(config["allow_unsafe_development"])
        self.assertEqual(config["templates"], {"ubuntu-4g": TEMPLATE_ID})
        self.assertEqual(config["runtime_dir"], "/var/lib/kiln/runtime")
        policy = json.loads((etc / "isolation.json").read_text())
        self.assertEqual((policy["uid_base"], policy["gid_base"]), (70000, 71000))
        self.assertEqual(policy["disk_backend"], "copy")
        self.assertEqual(
            policy["network"],
            {
                "helper": "/opt/kiln/bin/kiln-network",
                "namespace_scope": "kiln",
                "resolver": "1.1.1.1",
            },
        )
        self.assertEqual((self.root / "usr/local/libexec").stat().st_mode & 0o777, 0o750)
        self.assertEqual(etc.stat().st_mode & 0o777, 0o755)
        self.assertEqual((self.root / "var/lib/kiln").stat().st_mode & 0o777, 0o700)
        self.assertEqual(opt.stat().st_mode & 0o777, 0o755)
        self.assertTrue((opt / "bin/firecracker").is_file())
        runtime = next(line for line in self.lines("runtime PATH="))
        self.assertTrue(runtime.split("=", 1)[1].startswith(str(opt / "bin") + ":"))
        rows = self.lines()
        template_at = next(i for i, line in enumerate(rows) if "template" in line and "build" in line)
        enable_at = next(i for i, line in enumerate(rows) if line.startswith("systemctl enable"))
        self.assertLess(template_at, enable_at)
        self.assertIn("--profile", rows[template_at])
        self.assertIn("isolated", rows[template_at])
        self.assertTrue(self.lines("ip link show dev eth0"))

    def test_failed_readiness_disables_services_and_keeps_state(self):
        self.fresh_ready()
        self.plan_curl("fail", body=FRESH_CATALOG)
        result = self.invoke("--address", "127.0.0.1", "--apply")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("readiness", result.stderr)
        disabled = [line for line in self.lines("systemctl disable")]
        self.assertEqual(
            disabled,
            [
                "systemctl disable --now kiln-api.service",
                "systemctl disable --now kiln-host.service",
                "systemctl disable --now kiln-cgroup.service",
            ],
        )
        self.assertTrue((self.root / "var/lib/kiln").is_dir())
        config = json.loads((self.root / "etc/kiln/host.json").read_text())
        self.assertEqual(config["templates"], {"ubuntu-4g": TEMPLATE_ID})
        self.assertEqual((self.root / "usr/local/libexec").stat().st_mode & 0o777, 0o750)


class UpgradeTests(InstallerTest):
    def setUp(self):
        super().setUp()
        self.host_skeleton()
        self.etc = self.root / "etc/kiln"
        self.opt = self.root / "opt/kiln"
        self.var = self.root / "var/lib/kiln"
        self.libexec = self.root / "usr/local/libexec"
        for path in (self.etc, self.opt / "bin", self.var, self.libexec):
            path.mkdir(parents=True)
        self.destinations = {
            name: (self.opt / "bin" / name if name in ("kiln", "kiln-runtime", "kiln-network") else self.libexec / name)
            for name in NAMES
        }
        self.identity = self.libexec / "kiln-identity"
        self.identity_rollback = self.libexec / "kiln-identity.rollback-v033"
        self.identity.write_bytes(b"identity-bytes-v1")
        self.identity_rollback.write_bytes(b"handmade-rollback-v033")
        self.firecracker = self.opt / "bin/firecracker"
        self.jailer = self.opt / "bin/jailer"
        self.firecracker.write_bytes(b"firecracker-1.17.0")
        self.jailer.write_bytes(b"jailer-1.17.0")
        snap = self.var / "runtime/snapshots/a1323edbf1db1ff8bef3d5ccd9c6ebb6"
        snap.mkdir(parents=True)
        self.snapshot = snap / "memory.snap"
        self.snapshot.write_bytes(b"snapshot-marker-not-6gb")
        (snap / "disk.ext4").write_bytes(b"disk-marker")
        (self.var / "runtime").mkdir(exist_ok=True)
        (self.var / "journal").mkdir()
        (self.var / "image").mkdir()
        for directory in ("runtime", "journal", "image"):
            (self.var / directory / "preserve").write_text(directory)
        self.journal = self.var / "journal/operations.sqlite"
        with sqlite3.connect(self.journal) as db:
            db.execute("PRAGMA journal_mode=DELETE")
            db.executescript(
                "PRAGMA user_version=1; CREATE TABLE operations(running INTEGER); INSERT INTO operations VALUES(0);"
            )
        config = {
            "runtime_dir": str(self.var / "runtime"),
            "journal_dir": str(self.var / "journal"),
            "socket": "/run/kiln/host.sock",
            "isolation_config": str(self.etc / "isolation.json"),
            "templates": {"custom": "a" * 32},
        }
        (self.etc / "host.json").write_text(json.dumps(config))
        (self.etc / "isolation.json").write_text("{}\n")
        for name in ("ca.crt", "tls.crt", "tls.key", "admin.token"):
            (self.etc / name).write_text("synthetic-test-only")
        for name, mode in (("identity.json", 0o640), ("directory.token", 0o600)):
            path = self.etc / name
            path.write_text("synthetic-enrollment-only")
            path.chmod(mode)
        self.side = {
            path: (path.stat().st_ino, path.read_bytes())
            for path in (
                self.identity,
                self.identity_rollback,
                self.snapshot,
                self.firecracker,
                self.jailer,
            )
        }
        self.write_release("0.4.4", min_upgrade_from="0.3.0")
        self.write_versions("0.3.2", "0.4.4")
        self.write_units()
        self.plan_curl("ok", "ok", body=UPGRADE_CATALOG)
        self.data = {
            path.relative_to(self.root): path.read_bytes()
            for base in (self.etc, self.var)
            for path in base.rglob("*")
            if path.is_file()
        }

    def reported(self, name):
        return "kiln-server" if name in ("kiln-host", "kiln-api") else name

    def write_versions(self, installed, target, installed_extra="", target_extra=""):
        bindir = self.package / "bin"
        bindir.mkdir(exist_ok=True)
        for name in NAMES:
            reported = self.reported(name)
            if name == "kiln-network":
                installed_text = "#!/bin/sh\nexit 0\n"
                target_text = "#!/bin/sh\nexit 0\n" + target_extra
            else:
                installed_text = f"#!/bin/sh\necho '{reported} {installed}'\n" + installed_extra
                target_text = f"#!/bin/sh\necho '{reported} {target}'\n" + target_extra
            write_executable(self.destinations[name], installed_text)
            write_executable(bindir / name, target_text)
        self.original = {name: path.read_bytes() for name, path in self.destinations.items()}

    def write_release(self, version, target="x86_64-unknown-linux-gnu", **extra):
        payload = {"version": version if str(version).startswith("v") else f"v{version}", "target": target}
        payload.update(extra)
        (self.package / "release.json").write_text(json.dumps(payload))

    def write_units(self, address="100.64.5.6", uplink="enp7s0"):
        host = self.destinations["kiln-host"]
        api = self.destinations["kiln-api"]
        network = self.destinations["kiln-network"]
        (self.state / "exec-kiln-host.service").write_text(
            f"{{ path={host} ; argv[]={host} --config {self.etc / 'host.json'} ; ignore_errors=no ; }}\n"
        )
        (self.state / "exec-kiln-api.service").write_text(
            "{ path="
            + str(api)
            + " ; argv[]="
            + str(api)
            + f" --listen {address}:8443 --host-socket /run/kiln/host.sock --token-file {self.etc / 'admin.token'}"
            + f" --tls-cert {self.etc / 'tls.crt'} --tls-key {self.etc / 'tls.key'} ; ignore_errors=no ; }}\n"
        )
        (self.state / "exec-kiln-network.service").write_text(
            f"{{ path={network} ; argv[]={network} provision {uplink} ; ignore_errors=no ; }}\n"
        )
        for service in ("kiln-host.service", "kiln-api.service"):
            (self.state / f"active-{service}").write_text("active\n")

    def assert_data_preserved(self):
        for relative, data in self.data.items():
            self.assertEqual((self.root / relative).read_bytes(), data, relative)
        for name, mode in (("identity.json", 0o640), ("directory.token", 0o600)):
            self.assertEqual((self.etc / name).stat().st_mode & 0o777, mode)

    def assert_original_binaries(self):
        for name, path in self.destinations.items():
            self.assertEqual(path.read_bytes(), self.original[name], name)

    def assert_untouched_neighbors(self):
        for path, (inode, data) in self.side.items():
            self.assertEqual(path.stat().st_ino, inode, path)
            self.assertEqual(path.read_bytes(), data, path)
        self.assertFalse((self.opt / "libexec").exists())
        self.assertFalse((self.opt / "release.json").exists())
        self.assertFalse((self.etc / "release.json").exists())
        upgrades = self.opt / "upgrades"
        if upgrades.exists():
            for path in upgrades.rglob("*"):
                if path.is_file():
                    self.assertIn(path.name, NAMES, path)
                    self.assertNotIn("snap", path.name)

    def test_update_preserves_configuration_identity_and_snapshot(self):
        result = self.invoke("--apply")
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assertIn("kiln login", result.stdout)
        for name, path in self.destinations.items():
            self.assertEqual(path.read_bytes(), (self.package / "bin" / name).read_bytes(), name)
        self.assert_data_preserved()
        self.assert_untouched_neighbors()
        actions = [line for line in self.lines("systemctl ") if line.startswith(("systemctl stop", "systemctl start"))]
        self.assertEqual(
            actions,
            [
                "systemctl stop kiln-api.service",
                "systemctl stop kiln-host.service",
                "systemctl start kiln-host.service",
                "systemctl start kiln-api.service",
            ],
        )
        self.assertNotIn("systemctl stop kiln-network.service", self.lines())
        backups = list((self.opt / "upgrades").glob("*/kiln-api"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(sorted(path.name for path in backups[0].parent.iterdir()), sorted(NAMES))
        self.assertEqual(backups[0].read_bytes(), self.original["kiln-api"])
        self.assertFalse((self.opt / "pending-upgrade.json").exists())
        self.log.write_text("")
        self.plan_curl("ok", body=UPGRADE_CATALOG)
        again = self.invoke("--apply")
        self.assertEqual(again.returncode, 0, again.stderr + again.stdout)
        self.assertIn("already current", again.stdout)
        self.assertFalse(self.lines("systemctl stop"))
        self.assertEqual(list((self.opt / "upgrades").glob("*/kiln-api")), backups)
        self.assert_untouched_neighbors()

    def test_upgrade_disables_the_public_door_socket(self):
        unit = self.root / "etc/systemd/system/kiln-door-proxy.service"
        unit.write_text(
            "[Service]\nSockets=kiln-door-proxy.socket kiln-door-proxy-public.socket\n"
        )
        (self.state / "load-state").write_text("loaded\n")
        result = self.invoke("--apply")
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assertEqual(
            unit.read_text(),
            "[Service]\nSockets=kiln-door-proxy.socket\n",
        )
        self.assertIn(
            "systemctl disable --now kiln-door-proxy-public.socket",
            self.lines(),
        )
        self.assertFalse(
            any("enable" in line and "kiln-door-proxy-public" in line for line in self.lines())
        )
        self.assertIn(
            "Binaries are already updated. Closing public tcp/8443.",
            result.stdout,
        )
        self.plan_curl("ok", body=UPGRADE_CATALOG)
        commented = (
            "# kiln-door-proxy-public.socket stays named only in this comment\n"
            "[Service]\nSockets=kiln-door-proxy.socket\n"
        )
        unit.write_text(commented)
        self.log.write_text("")
        again = self.invoke("--apply")
        self.assertEqual(again.returncode, 0, again.stderr + again.stdout)
        self.assertEqual(unit.read_text(), commented)
        self.assertFalse(self.lines("systemctl daemon-reload"))
        self.assertFalse(self.lines("systemctl restart"))

    def test_dark_forge_layout_upgrades_and_rolls_back_without_release_json(self):
        self.write_release("0.4.5")
        self.write_versions("0.4.4", "0.4.5")
        self.data = {
            path.relative_to(self.root): path.read_bytes()
            for base in (self.etc, self.var)
            for path in base.rglob("*")
            if path.is_file()
        }
        result = self.invoke("--apply")
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assertIn("0.4.4 -> 0.4.5", result.stdout)
        self.assertFalse((self.opt / "upgrades").exists() and (self.opt / "pending-upgrade.json").exists())
        self.assert_data_preserved()
        self.assert_untouched_neighbors()
        backup = next((self.opt / "upgrades").glob("0.4.4-*"))
        self.assertEqual(sorted(path.name for path in backup.iterdir()), sorted(NAMES))

        self.write_versions("0.4.4", "0.4.5")
        self.plan_curl("ok", "fail", "ok", body=UPGRADE_CATALOG)
        shutil.rmtree(self.opt / "upgrades")
        failed = self.invoke("--apply")
        self.assertNotEqual(failed.returncode, 0, failed.stdout)
        self.assertIn("restored", failed.stderr)
        self.assert_original_binaries()
        self.assert_untouched_neighbors()
        self.assertFalse((self.opt / "pending-upgrade.json").exists())
        self.assertTrue(any(path.name == "kiln-api" for path in (self.opt / "upgrades").rglob("kiln-api")))

    def test_newer_patch_and_minor_releases_replace_binaries(self):
        for installed, target, field in (
            ("0.4.4", "0.4.5", False),
            ("0.4.4", "0.4.10", False),
            ("0.9.0", "0.10.0", True),
            ("0.3.99", "0.4.4", True),
        ):
            with self.subTest(installed=installed, target=target):
                extra = {"min_upgrade_from": "0.3.0"} if field else {}
                self.write_release(target, **extra)
                self.write_versions(installed, target)
                self.plan_curl("ok", "ok", body=UPGRADE_CATALOG)
                result = self.invoke("--apply")
                self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
                self.assertEqual(self.destinations["kiln"].read_bytes(), (self.package / "bin/kiln").read_bytes())
                self.assert_untouched_neighbors()
                self.write_versions(target, target)
                self.plan_curl("ok", body=UPGRADE_CATALOG)
                again = self.invoke("--apply")
                self.assertIn("already current", again.stdout)

    def test_downgrade_floor_and_missing_guard_stop_before_any_change(self):
        cases = (
            ("0.4.5", "0.4.4", {}, "refusing downgrade from 0.4.5 to 0.4.4"),
            ("0.4.10", "0.4.4", {}, "refusing downgrade from 0.4.10 to 0.4.4"),
            ("0.10.0", "0.9.0", {}, "refusing downgrade from 0.10.0 to 0.9.0"),
            ("0.4.4", "0.4.6", {"min_upgrade_from": "0.4.5"}, "package requires min_upgrade_from 0.4.5"),
            ("0.2.2", "0.4.4", {}, "older than 0.3.0"),
            ("0.3.0", "0.4.4", {}, "without min_upgrade_from"),
            ("0.9.0", "0.10.0", {}, "without min_upgrade_from"),
            ("0.4.0", "0.4.5-rc.1", {}, "unsupported update package"),
            ("0.4.4", "v0.4", {}, "unsupported update package"),
        )
        for installed, target, extra, message in cases:
            with self.subTest(installed=installed, target=target):
                self.log.write_text("")
                self.write_release(target, **extra)
                self.write_versions(installed, target)
                result = self.invoke("--apply")
                self.assertNotEqual(result.returncode, 0, result.stdout)
                self.assertIn(message, result.stderr)
                self.assertFalse(self.lines("systemctl stop"))
                self.assertFalse(self.lines("curl "))
                self.assertFalse((self.opt / "upgrades").exists())
                self.assertFalse((self.opt / "pending-upgrade.json").exists())
                self.assert_original_binaries()
                self.assert_untouched_neighbors()
        self.write_release("0.4.4", target="aarch64")
        self.write_versions("0.4.4", "0.4.4")
        result = self.invoke("--apply")
        self.assertIn("unsupported update package", result.stderr)
        self.assertFalse(self.lines("systemctl stop"))

    def test_same_version_replaces_only_different_bytes(self):
        self.write_release("0.4.4", min_upgrade_from="0.3.0")
        self.write_versions("0.4.4", "0.4.4")
        self.plan_curl("ok", body=UPGRADE_CATALOG)
        same = self.invoke("--apply")
        self.assertEqual(same.returncode, 0, same.stderr)
        self.assertIn("Kiln 0.4.4 is already current; nothing changed.", same.stdout)
        self.assertFalse((self.opt / "upgrades").exists())
        self.assertFalse(self.lines("systemctl stop"))
        self.write_versions("0.4.4", "0.4.4", target_extra="# rebuilt\n")
        self.plan_curl("ok", "ok", body=UPGRADE_CATALOG)
        changed = self.invoke("--apply")
        self.assertEqual(changed.returncode, 0, changed.stderr + changed.stdout)
        self.assertIn("# rebuilt", self.destinations["kiln"].read_text())
        self.assert_untouched_neighbors()

    def test_check_only_does_not_mutate_or_stop_services(self):
        result = self.invoke()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Check only", result.stdout)
        self.assert_original_binaries()
        self.assert_data_preserved()
        self.assert_untouched_neighbors()
        self.assertFalse((self.opt / "upgrades").exists())
        self.assertFalse(self.lines("systemctl stop"))

    def test_connection_guidance_follows_enrollment_files(self):
        self.write_versions("0.4.4", "0.4.4")
        self.plan_curl("ok", body=UPGRADE_CATALOG)
        enrolled = self.invoke()
        self.assertIn("kiln login", enrolled.stdout)
        self.assertNotIn("kiln-api enroll", enrolled.stdout)
        self.plan_curl("ok", body=UPGRADE_CATALOG)
        (self.etc / "enrollment.pending.json").write_text("{}\n")
        pending = self.invoke()
        self.assertIn("enroll --resume", pending.stdout)
        self.assertNotIn("--url", pending.stdout)
        self.plan_curl("ok", body=UPGRADE_CATALOG)
        (self.etc / "enrollment.pending.json").unlink()
        (self.etc / "identity.json").unlink()
        fresh = self.invoke()
        self.assertIn("enroll --url https://100.64.5.6:8443", fresh.stdout)

    def test_busy_operations_restore_gateway_without_stopping_host(self):
        with sqlite3.connect(self.journal) as db:
            db.execute("INSERT INTO operations VALUES(1)")
        self.plan_curl("ok", "ok", body=UPGRADE_CATALOG)
        result = self.invoke("--apply")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("restored", result.stderr)
        self.assertNotIn("systemctl stop kiln-host.service", self.lines())
        self.assertGreaterEqual(len(self.lines("sleep ")), 60)
        self.assert_original_binaries()
        self.assert_untouched_neighbors()
        self.assertEqual((self.state / "active-kiln-host.service").read_text().strip(), "active")
        self.assertEqual((self.state / "active-kiln-api.service").read_text().strip(), "active")

    def test_operations_finish_before_host_is_stopped(self):
        with sqlite3.connect(self.journal) as db:
            db.execute("INSERT INTO operations VALUES(1)")
        result = self.invoke("--apply", KILN_SLEEP_CLEAR_JOURNAL=str(self.journal))
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        sleep_at = next(i for i, line in enumerate(self.lines()) if line.startswith("sleep "))
        stop_host = next(i for i, line in enumerate(self.lines()) if line == "systemctl stop kiln-host.service")
        self.assertLess(sleep_at, stop_host)
        self.assertEqual(len(self.lines("sleep ")), 1)

    def test_start_failure_rolls_back_and_signal_retains_the_marker(self):
        (self.state / "fail-start-host").write_text("1\n")
        self.plan_curl("ok", "ok", body=UPGRADE_CATALOG)
        failed = self.invoke("--apply")
        self.assertNotEqual(failed.returncode, 0)
        self.assertIn("restored", failed.stderr)
        self.assert_original_binaries()
        self.assert_untouched_neighbors()
        self.assertFalse((self.opt / "pending-upgrade.json").exists())

        (self.state / "term-on-stop-host").write_text("1\n")
        self.plan_curl("ok", body=UPGRADE_CATALOG)
        interrupted = self.invoke("--apply")
        self.assertNotEqual(interrupted.returncode, 0)
        pending = self.opt / "pending-upgrade.json"
        self.assertTrue(pending.is_file())
        recovery = json.loads(pending.read_text())
        for name, data in self.original.items():
            self.assertEqual((Path(recovery["backup"]) / name).read_bytes(), data)
        self.assert_untouched_neighbors()
        self.log.write_text("")
        retry = self.invoke("--apply")
        self.assertIn("pending", retry.stderr)
        self.assertEqual(self.lines("systemctl "), [])

    def test_readiness_and_partial_copy_roll_back(self):
        self.plan_curl("ok", "fail", "ok", body=UPGRADE_CATALOG)
        failed = self.invoke("--apply")
        self.assertIn("restored", failed.stderr)
        self.assert_original_binaries()
        self.assert_data_preserved()
        self.assert_untouched_neighbors()
        self.assertFalse((self.opt / "pending-upgrade.json").exists())
        self.assertEqual((self.state / "active-kiln-host.service").read_text().strip(), "active")

        shutil.rmtree(self.opt / "upgrades")
        self.plan_curl("ok", "ok", body=UPGRADE_CATALOG)
        copied = self.invoke("--apply", KILN_CP_FAIL_SOURCE=str(self.package / "bin/kiln-api"))
        self.assertIn("restored", copied.stderr)
        self.assertTrue(self.cp_marker.is_file())
        self.assert_original_binaries()
        self.assert_data_preserved()
        self.assert_untouched_neighbors()

        self.plan_curl("ok", "ok", body=UPGRADE_CATALOG)
        replaced = self.invoke("--apply", KILN_MV_FAIL="1")
        self.assertNotEqual(replaced.returncode, 0, replaced.stdout)
        self.assertIn("restored", replaced.stderr)
        self.assertTrue(self.mv_marker.is_file())
        self.assert_original_binaries()
        self.assert_data_preserved()
        self.assert_untouched_neighbors()
        self.assertFalse((self.opt / "pending-upgrade.json").exists())

    def test_rollback_failure_retains_marker_and_blocks_retry(self):
        self.plan_curl("ok", "fail", "fail", body=UPGRADE_CATALOG)
        failed = self.invoke("--apply")
        self.assertIn("recovery", failed.stderr)
        self.assertTrue((self.opt / "pending-upgrade.json").is_file())
        self.assert_original_binaries()
        self.assert_untouched_neighbors()
        self.log.write_text("")
        retry = self.invoke("--apply")
        self.assertIn("pending", retry.stderr)
        self.assertEqual(self.lines(), [])

    def test_bad_address_network_and_killmode_do_not_stop_services(self):
        for args, message in (
            (("--address", "192.168.1.6"), "preserves the existing address"),
            (("--network",), "cannot enable networking"),
            (("--network-uplink", "eth0"), "cannot enable networking"),
        ):
            with self.subTest(args=args):
                self.log.write_text("")
                result = self.invoke("--apply", *args)
                self.assertIn(message, result.stderr)
                self.assertFalse(self.lines("systemctl stop"))
        (self.state / "killmode").write_text("control-group\n")
        result = self.invoke("--apply")
        self.assertIn("KillMode", result.stderr)
        self.assertFalse(self.lines("systemctl stop"))
        self.assert_data_preserved()
        self.assert_untouched_neighbors()

    def test_missing_symlink_mixed_and_unhealthy_states_stop_nothing(self):
        path = self.destinations["kiln-api"]
        path.unlink()
        missing = self.invoke("--apply")
        self.assertIn("missing regular file", missing.stderr)
        path.symlink_to(self.package / "bin/kiln-api")
        linked = self.invoke("--apply")
        self.assertIn("missing regular file", linked.stderr)
        path.unlink()
        self.write_versions("0.3.2", "0.4.4")
        binary = self.destinations["kiln-api"]
        binary.write_bytes(self.original["kiln-api"].replace(b"0.3.2", b"0.3.0"))
        binary.chmod(0o755)
        mixed = self.invoke("--apply")
        self.assertIn("mixed", mixed.stderr)
        self.write_versions("0.3.2", "0.4.4")
        (self.state / "active-kiln-host.service").write_text("failed\n")
        unhealthy = self.invoke("--apply")
        self.assertIn("healthy", unhealthy.stderr)
        (self.state / "active-kiln-host.service").write_text("active\n")
        (self.var / "runtime").rename(self.root / "saved-runtime")
        absent = self.invoke("--apply")
        self.assertIn("missing installed runtime_dir", absent.stderr)
        self.assertFalse(self.lines("systemctl stop"))
        saved = self.root / "saved-runtime/snapshots/a1323edbf1db1ff8bef3d5ccd9c6ebb6/memory.snap"
        self.assertEqual(saved.read_bytes(), b"snapshot-marker-not-6gb")
        self.assertEqual(self.identity.read_bytes(), b"identity-bytes-v1")
        self.assertEqual(self.identity_rollback.read_bytes(), b"handmade-rollback-v033")

    def test_networked_update_preserves_matching_options(self):
        self.write_versions("0.3.0", "0.4.4")
        policy = self.etc / "isolation.json"
        policy.write_text('{"network":{"namespace_scope":"kiln"}}\n')
        refused = self.invoke("--apply", "--network-uplink", "wrong0")
        self.assertIn("uplink", refused.stderr)
        self.assertFalse(self.lines("systemctl stop"))
        self.plan_curl("ok", "ok", body=UPGRADE_CATALOG)
        updated = self.invoke(
            "--apply",
            "--address",
            "100.64.5.6",
            "--network",
            "--network-uplink",
            "enp7s0",
        )
        self.assertEqual(updated.returncode, 0, updated.stderr + updated.stdout)
        self.assertEqual(policy.read_text(), '{"network":{"namespace_scope":"kiln"}}\n')
        self.assertNotIn("systemctl stop kiln-network.service", self.lines())
        self.assert_untouched_neighbors()

    def test_unrecognized_installed_version_is_not_blamed_on_the_package(self):
        text = self.destinations["kiln"].read_text().replace("0.3.2", "0.3")
        self.destinations["kiln"].write_text(text)
        self.destinations["kiln"].chmod(0o755)
        result = self.invoke("--apply")
        self.assertIn("unrecognized installed binary: kiln", result.stderr)
        self.assertNotIn("package", result.stderr)
        self.assertFalse(self.lines("systemctl stop"))

    def test_installer_lock_is_exclusive_and_is_not_replaced(self):
        import fcntl

        lock = self.root / "run/lock/kiln-install.lock"
        lock.parent.mkdir(parents=True)
        fd = os.open(lock, os.O_CREAT | os.O_RDWR, 0o600)
        inode = os.fstat(fd).st_ino
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        held = self.invoke("--address", "8.8.8.8")
        self.assertIn("another installer is running", held.stderr)
        self.assertEqual(lock.stat().st_ino, inode)
        fcntl.flock(fd, fcntl.LOCK_UN)
        os.close(fd)
        released = self.invoke("--address", "8.8.8.8")
        self.assertIn("private", released.stderr)
        self.assertNotIn("another installer", released.stderr)
        self.assertEqual(lock.stat().st_ino, inode)

        os.chmod(lock, 0o666)
        unsafe = self.invoke("--address", "127.0.0.1")
        self.assertIn("unsafe installer lock", unsafe.stderr)
        self.assertTrue(lock.is_file())
        self.assertEqual(lock.stat().st_ino, inode)
        lock.unlink()
        lock.symlink_to(self.root / "elsewhere")
        (self.root / "elsewhere").write_text("nope")
        linked = self.invoke("--address", "127.0.0.1")
        self.assertIn("unsafe installer lock", linked.stderr)
        self.assertTrue(lock.is_symlink())


if __name__ == "__main__":
    unittest.main()

#!/usr/bin/env python3
"""Run the real shell bootstrap without a TTY; never provision this host."""

import hashlib
import io
import json
import os
import re
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def pinned_version():
    text = (ROOT / "deploy/bootstrap/install.sh").read_text()
    match = re.search(r"(?m)^    version=([0-9]+\.[0-9]+\.[0-9]+)$", text)
    if match is None:
        raise AssertionError("install.sh does not pin a release version")
    return match.group(1)


PINNED = pinned_version()
SERVER_FILES = [
    "bin/kiln-runtime",
    "bin/kiln",
    "bin/kiln-host",
    "bin/kiln-api",
    "install.sh",
    "fetch-firecracker.sh",
    "README.md",
    "docs/releases.md",
    "docs/remote-client.md",
    "docs/runtime.md",
    "docs/identity-service.md",
    "deploy/kiln-host.service",
    "deploy/kiln-api.service",
    "deploy/host.example.json",
    "image/image.json",
    "image/inputs.json",
    "image/vmlinux",
    "image/rootfs.ext4",
    "release.json",
]


class BootstrapTests(unittest.TestCase):
    def setUp(self):
        self.assertTrue(
            (ROOT / "deploy/bootstrap/install.sh").is_file(), "bootstrap is missing"
        )
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for name in ("home", "bin", "tmp", "systemd"):
            (self.root / name).mkdir()
        (self.root / "os-release").write_text('ID=ubuntu\nVERSION_ID="24.04"\n')
        (self.root / "kvm").touch()
        (self.root / "controllers").write_text("cpu memory pids\n")
        (self.root / "meminfo").write_text("MemAvailable:   8388608 kB\n")
        self.archive = self.root / "archive.tar.gz"
        self.env = {
            **os.environ,
            "HOME": str(self.root / "home"),
            "TMPDIR": str(self.root / "tmp"),
            "PATH": str(self.root / "bin") + ":" + os.environ["PATH"],
            "FIXTURE": str(self.root),
            "OS": "Linux",
            "ARCH": "x86_64",
            "GLIBC": "glibc 2.39",
            "HTTP": "200",
            "ROUTE": "1.1.1.1 via 192.168.50.1 dev eth0 src 192.168.50.7 uid 1000\n    cache",
            "EXPECTED_ADDRESS": "192.168.50.7",
            "EXPECTED_UPLINK": "",
            "PINNED": PINNED,
        }
        self.env.pop("KILN_NETWORK_UPLINK", None)
        self.executable(
            "uname",
            '#!/bin/sh\ncase "$1" in -s) echo "$OS";; -m) echo "$ARCH";; esac\n',
        )
        self.executable("getconf", '#!/bin/sh\necho "$GLIBC"\n')
        self.executable("id", "#!/bin/sh\necho 1000\n")
        self.executable(
            "ip",
            '#!/bin/sh\n[ "$*" = "-4 route get 1.1.1.1" ] || exit 2\nprintf "%s\\n" "$ROUTE"\n',
        )
        self.executable(
            "df",
            "#!/bin/sh\n"
            '[ "$1" = -B1 ] || exit 2\n'
            "printf '%s\\n' 'Filesystem 1B-blocks Used Available Use% Mounted' "
            "'fixture 30000000000 1 30000000000 1% /'\n",
        )
        self.executable(
            "curl",
            f"#!{sys.executable}\n"
            + """
import json, os, pathlib, shutil, sys
root = pathlib.Path(os.environ['FIXTURE'])
args = sys.argv[1:]
assert args[0] == '-q', 'curl user configuration must be disabled'
assert '--location-trusted' not in args
assert '--location' in args
assert '--proto' in args and args[args.index('--proto') + 1] == '=https'
assert args[args.index('--proto-redir') + 1] == '=https'
assert '--netrc' not in args and '--config' not in args and '--user' not in args
assert not any('Authorization' in arg for arg in args)
assert args[-1].startswith('https://github.com/Jabbslad/kiln/releases/download/v' + os.environ['PINNED'] + '/kiln-')
record = {'args': args}
with (root / 'requests').open('a') as f:
    f.write(json.dumps(record) + '\\n')
output = pathlib.Path(args[args.index('--output') + 1])
code = os.environ['HTTP']
if code == '200':
    shutil.copyfile(root / 'archive.tar.gz', output)
else:
    output.write_text('not an archive')
if '--write-out' in args:
    print(code, end='')
""",
        )
        self.executable(
            "python3",
            f"#!{sys.executable}\n"
            + """
import os, sys
if any("bind" in arg and "socket" in arg for arg in sys.argv[1:]):
    raise SystemExit(int(os.environ.get("BIND_FAIL", "0")))
os.execv(sys.executable, [sys.executable, *sys.argv[1:]])
""",
        )
        self.executable(
            "sudo",
            f"#!{sys.executable}\n"
            + """
import json, os, pathlib, subprocess, sys
root = pathlib.Path(os.environ['FIXTURE'])
assert sys.argv[1] == '-n', 'sudo must never prompt'
args = sys.argv[2:]
with (root / 'privileged').open('a') as f:
    f.write(json.dumps(args) + '\\n')
assert sys.stdin.read() == '', 'privileged commands must not consume script stdin'
if args[0] == 'true':
    sys.exit(int(os.environ.get('SUDO_FAILURE', '0')))
if args[0] == 'bash':
    sys.exit(subprocess.call(args))
if args[0] == 'python3':
    sys.exit(subprocess.call([sys.executable, *args[1:]]))
if args[0] == 'sh' and len(args) >= 3 and 'kvm' in args[2]:
    sys.exit(0 if (root / 'kvm').exists() else 1)
if args[0] == 'env' and 'apt-get' in args:
    sub = args[args.index('apt-get') + 1]
    assert sub not in ('upgrade', 'dist-upgrade', 'full-upgrade'), args
    if os.environ.get('APT_FAIL') == sub:
        sys.exit(100)
    sys.exit(0)
""",
        )

    def executable(self, name, text):
        path = self.root / "bin" / name
        path.write_text(text)
        path.chmod(0o755)

    def package(self, members=None, server=False):
        if members is None:
            members = {
                "kiln": f"#!/bin/sh\necho 'kiln {PINNED}'\n".encode()
            }
        if server:
            members = dict.fromkeys([*SERVER_FILES, "bin/kiln-network"], b"fixture\n")
            members["install.sh"] = (
                b"#!/bin/bash\nset -euo pipefail\n"
                b'expected="--address ${EXPECTED_ADDRESS} --apply"\n'
                b'if [ -n "${EXPECTED_UPLINK}" ]; then expected="${expected} --network-uplink ${EXPECTED_UPLINK}"; fi\n'
                b'[ "$*" = "$expected" ]\n'
                b"printf '%s\\n' 'provisioner completed'\n"
            )
        with tarfile.open(self.archive, "w:gz") as archive:
            for name, data in members.items():
                info = tarfile.TarInfo(name)
                if isinstance(data, tuple):
                    info.type = tarfile.SYMTYPE
                    info.linkname = data[0]
                    archive.addfile(info)
                else:
                    info.size = len(data)
                    info.mode = 0o755
                    archive.addfile(info, io.BytesIO(data))
        return hashlib.sha256(self.archive.read_bytes()).hexdigest()

    def script(self, digest):
        source = (ROOT / "deploy/bootstrap/install.sh").read_text()
        start, tail = source.split("# BEGIN RELEASE ASSETS\n", 1)
        _, end = tail.split("# END RELEASE ASSETS", 1)
        rows = "\n".join(
            f"{key} {digest}"
            for key in (
                [
                    "client:x86_64-unknown-linux-gnu",
                    "client:x86_64-apple-darwin",
                    "client:aarch64-apple-darwin",
                    "server:x86_64-unknown-linux-gnu",
                ]
            )
        )
        source = (
            start + "# BEGIN RELEASE ASSETS\n" + rows + "\n# END RELEASE ASSETS" + end
        )
        for old, new in [
            ("/etc/os-release", "os-release"),
            ("/dev/kvm", "kvm"),
            ("/proc/meminfo", "meminfo"),
            ("/run/systemd/system", "systemd"),
            ("/sys/fs/cgroup/cgroup.controllers", "controllers"),
            ("/etc/kiln", "etc-kiln"),
            ("/opt/kiln", "opt-kiln"),
            ("/var/lib/kiln", "state-kiln"),
        ]:
            source = source.replace(old, str(self.root / new))
        path = self.root / "install.sh"
        path.write_text(source)
        return path

    def run_bootstrap(self, digest, args=()):
        script = self.script(digest)
        result = subprocess.run(
            [
                "sh",
                "-c",
                'file=$1; shift; cat "$file" | sh -s -- "$@"',
                "test",
                str(script),
                *args,
            ],
            env=self.env,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            start_new_session=True,
            timeout=15,
        )
        output = result.stdout + result.stderr
        self.assertNotIn("GitHub token:", output)
        self.assertEqual(
            list((self.root / "tmp").iterdir()),
            [],
            "temporary files remain",
        )
        return result.returncode, output

    def requests(self):
        path = self.root / "requests"
        return (
            [json.loads(line) for line in path.read_text().splitlines()]
            if path.exists()
            else []
        )

    def test_pinned_version_matches_published_release(self):
        self.assertEqual(pinned_version(), "0.4.7")

    def test_client_platforms_install_correct_asset(self):
        for system, arch, expected in [
            ("Linux", "x86_64", "x86_64-unknown-linux-gnu"),
            ("Darwin", "x86_64", "x86_64-apple-darwin"),
            ("Darwin", "arm64", "aarch64-apple-darwin"),
        ]:
            with self.subTest(system=system, arch=arch):
                self.env.update(OS=system, ARCH=arch)
                code, out = self.run_bootstrap(self.package())
                self.assertEqual(code, 0, out)
                self.assertIn("kiln login", out)
                self.assertNotIn("Next: connect using the server enrollment bundle", out)
                binary = self.root / "home/.local/bin/kiln"
                self.assertEqual(
                    subprocess.check_output([binary, "--version"], text=True),
                    f"kiln {PINNED}\n",
                )
                request = self.requests()[-1]
                self.assertEqual(
                    request["args"][-1],
                    f"https://github.com/Jabbslad/kiln/releases/download/v{PINNED}/kiln-v{PINNED}-{expected}.tar.gz",
                )
                binary.unlink()

    def test_bad_digest_never_installs(self):
        self.package()
        code, out = self.run_bootstrap("0" * 64)
        self.assertNotEqual(code, 0, out)
        self.assertIn("checksum", out.lower())
        self.assertFalse((self.root / "home/.local/bin/kiln").exists())

    def test_http_error_never_installs(self):
        self.env["HTTP"] = "404"
        code, out = self.run_bootstrap(self.package())
        self.assertNotEqual(code, 0, out)
        self.assertIn("404", out)
        self.assertFalse((self.root / "home/.local/bin/kiln").exists())

    def test_unsafe_archives_are_rejected_even_with_correct_digest(self):
        for members in [
            {"../escaped": b"bad"},
            {"kiln": ("/bin/sh",)},
            {"kiln": b"ok", "extra": b"bad"},
        ]:
            with self.subTest(members=members):
                code, out = self.run_bootstrap(self.package(members))
                self.assertNotEqual(code, 0, out)
                self.assertIn("archive", out.lower())
                self.assertFalse((self.root / "home/.local/bin/kiln").exists())

    def test_existing_client_is_preserved_without_downloading(self):
        binary = self.root / "home/.local/bin/kiln"
        binary.parent.mkdir(parents=True)
        binary.write_text("preserve")
        code, out = self.run_bootstrap(self.package())
        self.assertNotEqual(code, 0, out)
        self.assertEqual(binary.read_text(), "preserve")
        self.assertEqual(self.requests(), [])
        binary.unlink()
        binary.symlink_to("missing-target")
        code, out = self.run_bootstrap(self.package())
        self.assertNotEqual(code, 0, out)
        self.assertEqual(os.readlink(binary), "missing-target")
        self.assertEqual(self.requests(), [])

    def test_platform_and_glibc_rejections_do_not_prompt_or_download(self):
        for values in [
            {"OS": "FreeBSD"},
            {"ARCH": "aarch64"},
            {"GLIBC": "glibc 2.38"},
            {"GLIBC": "musl 1.2"},
        ]:
            with self.subTest(values=values):
                old = self.env.copy()
                self.env.update(values)
                code, out = self.run_bootstrap(self.package())
                self.assertNotEqual(code, 0, out)
                self.assertNotIn("GitHub token: ", out)
                self.assertEqual(self.requests(), [])
                self.env = old

    def old_client(self):
        binary = self.root / "home/.local/bin/kiln"
        binary.parent.mkdir(parents=True, exist_ok=True)
        binary.write_text("#!/bin/sh\necho 'kiln 0.1.1'\n")
        binary.chmod(0o755)
        return binary

    def test_rerun_replaces_client_and_keeps_previous_and_profiles(self):
        binary = self.old_client()
        original = binary.read_bytes()
        profile = self.root / "home/.config/kiln/profiles.json"
        profile.parent.mkdir(parents=True)
        profile.write_text("preserve profile and credential references")
        code, out = self.run_bootstrap(self.package())
        self.assertEqual(code, 0, out)
        self.assertEqual(
            subprocess.check_output([binary, "--version"], text=True),
            f"kiln {PINNED}\n",
        )
        self.assertEqual(binary.with_name("kiln.previous").read_bytes(), original)
        self.assertEqual(
            profile.read_text(), "preserve profile and credential references"
        )
        self.assertEqual(
            sorted(p.name for p in binary.parent.iterdir()), ["kiln", "kiln.previous"]
        )
        installed = binary.stat().st_ino
        code, out = self.run_bootstrap(self.package())
        self.assertEqual(code, 0, out)
        self.assertIn("already current", out)
        self.assertEqual(binary.stat().st_ino, installed)
        self.assertEqual(binary.with_name("kiln.previous").read_bytes(), original)

    def test_failed_upgrade_preserves_current_and_previous_clients(self):
        binary = self.old_client()
        original = binary.read_bytes()
        previous = binary.with_name("kiln.previous")
        previous.write_bytes(b"older backup")
        for failure in ("checksum", "version", "HTTP"):
            with self.subTest(failure=failure):
                self.env["HTTP"] = "403" if failure == "HTTP" else "200"
                digest = (
                    self.package({"kiln": b"#!/bin/sh\necho wrong-version\n"})
                    if failure == "version"
                    else self.package()
                )
                code, out = self.run_bootstrap(
                    "0" * 64 if failure == "checksum" else digest,
                )
                self.assertNotEqual(code, 0, out)
                self.assertIn(failure.lower(), out.lower())
                self.assertEqual(binary.read_bytes(), original)
                self.assertEqual(previous.read_bytes(), b"older backup")
                self.assertFalse((binary.parent / ".kiln-install.lock").exists())

    def test_upgrade_rejects_symlinks_and_busy_lock_before_downloading(self):
        binary = self.old_client()
        for unsafe in (binary, binary.with_name("kiln.previous")):
            if unsafe.exists():
                unsafe.unlink()
            unsafe.symlink_to("missing-target")
            code, out = self.run_bootstrap(self.package())
            self.assertNotEqual(code, 0, out)
            self.assertIn("regular file", out)
            self.assertTrue(unsafe.is_symlink())
            self.assertEqual(self.requests(), [])
            unsafe.unlink()
            self.old_client()
        lock = binary.parent / ".kiln-install.lock"
        lock.mkdir()
        code, out = self.run_bootstrap(self.package())
        self.assertNotEqual(code, 0, out)
        self.assertIn("another installer", out)
        self.assertTrue(lock.exists())
        self.assertEqual(self.requests(), [])

    def test_upgrade_failed_publication_preserves_working_client(self):
        binary = self.old_client()
        original = binary.read_bytes()
        real_mv = subprocess.check_output(
            ["sh", "-c", "command -v mv"], text=True
        ).strip()
        self.executable(
            "mv",
            f'#!/bin/sh\nfor arg do last=$arg; done\ncase "$last" in */kiln) exit 1;; esac\nexec "{real_mv}" "$@"\n',
        )
        code, out = self.run_bootstrap(self.package())
        self.assertNotEqual(code, 0, out)
        self.assertIn("replace", out.lower())
        self.assertEqual(binary.read_bytes(), original)
        self.assertEqual(binary.with_name("kiln.previous").read_bytes(), original)
        self.assertEqual(
            sorted(p.name for p in binary.parent.iterdir()), ["kiln", "kiln.previous"]
        )

    def test_legacy_upgrade_flag_is_an_optional_client_alias(self):
        code, out = self.run_bootstrap(self.package(), args=("client", "--upgrade"))
        self.assertEqual(code, 0, out)

    def test_newer_client_is_not_downgraded(self):
        binary = self.old_client()
        major, minor, patch = (int(part) for part in PINNED.split("."))
        newer = {
            f"{major}.{minor}.{patch + 1}",
            "0.4.10",
            "0.5.0",
            "1.0.0",
        }
        pin = tuple(int(part) for part in PINNED.split("."))
        for version in sorted(newer):
            if tuple(int(part) for part in version.split(".")) <= pin:
                continue
            binary.write_text(f"#!/bin/sh\necho 'kiln {version}'\n")
            code, out = self.run_bootstrap(self.package())
            self.assertNotEqual(code, 0, out)
            self.assertIn("newer", out)
            self.assertIn(version, binary.read_text())
            self.assertFalse(binary.with_name("kiln.previous").exists())

    def test_multiline_client_version_is_refused_before_download(self):
        binary = self.old_client()
        binary.write_text("#!/bin/sh\nprintf 'kiln 0.3.0\\nnot-a-version\\n'\n")
        original = binary.read_bytes()
        code, out = self.run_bootstrap(self.package())
        self.assertNotEqual(code, 0, out)
        self.assertEqual(binary.read_bytes(), original)
        self.assertEqual(self.requests(), [])

    def test_existing_server_skips_apt_and_route_detection(self):
        (self.root / "etc-kiln").mkdir()
        self.env["ROUTE"] = ""
        members = dict.fromkeys([*SERVER_FILES, "bin/kiln-network"], b"fixture\n")
        members["install.sh"] = (
            b"#!/bin/bash\nset -euo pipefail\n[ \"$*\" = --apply ]\nprintf '%s\\n' updated\n"
        )
        code, out = self.run_bootstrap(self.package(members), args=("server",))
        self.assertEqual(code, 0, out)
        self.assertIn("updated", out)
        commands = [
            json.loads(line)
            for line in (self.root / "privileged").read_text().splitlines()
        ]
        self.assertEqual(commands[0], ["true"])
        self.assertEqual(commands[1][0], "bash")
        self.assertEqual(len(commands), 2)

    def test_server_unsupported_host_never_requests_privileges(self):
        for distro, version in [
            ("ubuntu", "22.04"),
            ("ubuntu", "24.10"),
            ("ubuntu", "26.10"),
            ("debian", "12"),
            ("debian", "26.04"),
        ]:
            with self.subTest(distro=distro, version=version):
                (self.root / "os-release").write_text(
                    f'ID={distro}\nVERSION_ID="{version}"\n'
                )
                code, out = self.run_bootstrap(
                    self.package(server=True), args=("server",)
                )
                self.assertNotEqual(code, 0, out)
                self.assertIn("24.04", out)
                self.assertEqual(self.requests(), [])
                self.assertFalse((self.root / "privileged").exists())

    def test_ubuntu_2604_reaches_server_provisioning(self):
        (self.root / "os-release").write_text('ID=ubuntu\nVERSION_ID="26.04"\n')
        self.test_server_detects_address_and_installs_without_tty()

    def test_debian_13_reaches_server_provisioning(self):
        (self.root / "os-release").write_text('ID=debian\nVERSION_ID="13"\n')
        self.test_server_detects_address_and_installs_without_tty()

    def test_server_detects_address_and_installs_without_tty(self):
        code, out = self.run_bootstrap(
            self.package(server=True),
            args=("server",),
        )
        self.assertEqual(code, 0, out)
        self.assertIn("provisioner completed", out)
        commands = [
            json.loads(line)
            for line in (self.root / "privileged").read_text().splitlines()
        ]
        self.assertEqual(commands[0], ["true"])
        self.assertIn("kvm", commands[1][2])
        self.assertEqual(commands[2][-2:], ["apt-get", "update"])
        self.assertNotIn("python3", commands[3])
        self.assertNotIn("sqlite3", commands[3])
        self.assertEqual(commands[4][0], "bash")
        self.assertTrue(
            self.requests()[0]["args"][-1].endswith(
                f"/kiln-server-v{PINNED}-x86_64-unknown-linux-gnu.tar.gz"
            )
        )

    def test_kvm_access_is_checked_by_privileged_provisioner(self):
        (self.root / "kvm").chmod(0)
        self.test_server_detects_address_and_installs_without_tty()

    def test_network_auto_detection_and_explicit_overrides(self):
        for args, address, uplink in [
            (("--network",), "192.168.50.7", "eth0"),
            (
                ("--address", "100.64.5.6", "--network-uplink", "enp2s0"),
                "100.64.5.6",
                "enp2s0",
            ),
        ]:
            with self.subTest(args=args):
                self.env.update(EXPECTED_ADDRESS=address, EXPECTED_UPLINK=uplink)
                code, out = self.run_bootstrap(
                    self.package(server=True), args=("server", *args)
                )
                self.assertEqual(code, 0, out)
                commands = [
                    json.loads(line)
                    for line in (self.root / "privileged").read_text().splitlines()
                ]
                for package in ("nftables", "iproute2", "util-linux"):
                    self.assertIn(package, commands[-2])

    def test_invalid_addresses_do_not_run_sudo(self):
        for address in [
            "8.8.8.8",
            "100.63.0.1",
            "172.32.0.1",
            "10.0.0.999",
            "10.0.0.01",
        ]:
            with self.subTest(address=address):
                code, out = self.run_bootstrap(
                    self.package(server=True),
                    args=("server", "--address", address),
                )
                self.assertNotEqual(code, 0, out)
                self.assertIn("private/VPN", out)
                self.assertFalse((self.root / "privileged").exists())

    def test_bad_route_fails_without_mutations(self):
        for route in (
            "",
            "1.1.1.1 dev eth0 src 8.8.8.8",
            "1.1.1.1 dev eth0 src 10.1.2.3 src 10.4.5.6",
        ):
            with self.subTest(route=route):
                self.env["ROUTE"] = route
                code, out = self.run_bootstrap(
                    self.package(server=True), args=("server",)
                )
                self.assertNotEqual(code, 0, out)
                self.assertIn("--address", out)
                self.assertFalse((self.root / "privileged").exists())

    def test_explicit_address_works_without_default_route(self):
        self.env.update(ROUTE="", EXPECTED_ADDRESS="172.31.8.9")
        code, out = self.run_bootstrap(
            self.package(server=True), args=("server", "--address", "172.31.8.9")
        )
        self.assertEqual(code, 0, out)

    def test_sudo_failure_does_not_download_or_install(self):
        self.env["SUDO_FAILURE"] = "1"
        code, out = self.run_bootstrap(self.package(server=True), args=("server",))
        self.assertNotEqual(code, 0, out)
        self.assertIn("sudo", out)
        self.assertEqual(self.requests(), [])
        self.assertEqual(
            (self.root / "privileged").read_text().splitlines(), ['["true"]']
        )

    def test_bad_options_never_download_or_provision(self):
        for args in (
            ("server", "--address"),
            ("server", "--network-uplink", "bad;name"),
            ("server", "--network-uplink", "eth0\nbad;name"),
            ("server", "--address", "10.1.2.3\ninvalid"),
            ("client", "--network"),
            ("server", "--unknown"),
        ):
            with self.subTest(args=args):
                code, out = self.run_bootstrap(self.package(server=True), args=args)
                self.assertNotEqual(code, 0, out)
                self.assertEqual(self.requests(), [])
                self.assertFalse((self.root / "privileged").exists())

    def test_network_environment_override_and_missing_helper(self):
        self.env.update(KILN_NETWORK_UPLINK="enp4s0", EXPECTED_UPLINK="enp4s0")
        code, out = self.run_bootstrap(self.package(server=True), args=("server",))
        self.assertEqual(code, 0, out)
        (self.root / "privileged").unlink()
        code, out = self.run_bootstrap(
            self.package(dict.fromkeys(SERVER_FILES, b"fixture\n")), args=("server",)
        )
        self.assertNotEqual(code, 0, out)
        self.assertIn("no guest networking", out)
        self.assertEqual(
            (self.root / "privileged").read_text().splitlines(), ['["true"]']
        )

    def test_missing_or_ambiguous_network_device_requires_override(self):
        for route in (
            "1.1.1.1 src 192.168.50.7",
            "1.1.1.1 dev eth0 dev eth1 src 192.168.50.7",
        ):
            with self.subTest(route=route):
                self.env["ROUTE"] = route
                code, out = self.run_bootstrap(
                    self.package(server=True), args=("server", "--network")
                )
                self.assertNotEqual(code, 0, out)
                self.assertIn("--network-uplink", out)
                self.assertFalse((self.root / "privileged").exists())

    def privileged(self):
        path = self.root / "privileged"
        if not path.exists():
            return ""
        return path.read_text()

    def test_resource_limits_stop_install_before_apt(self):
        (self.root / "meminfo").write_text("MemAvailable:   1024 kB\n")
        code, out = self.run_bootstrap(self.package(server=True), args=("server",))
        self.assertNotEqual(code, 0, out)
        self.assertIn("6 GiB", out)
        self.assertNotIn("apt-get", self.privileged())

    def test_low_disk_stops_install_before_apt(self):
        self.executable(
            "df",
            "#!/bin/sh\nprintf '%s\\n' 'Filesystem 1B-blocks Used Available Use% Mounted' 'tmp 1 1 1 100% /'\n",
        )
        code, out = self.run_bootstrap(self.package(server=True), args=("server",))
        self.assertNotEqual(code, 0, out)
        self.assertIn("24 GiB", out)
        self.assertNotIn("apt-get", self.privileged())

    def test_bootstrap_leaves_port_binding_to_the_installer(self):
        text = (ROOT / "deploy/bootstrap/install.sh").read_text()
        self.assertNotIn("socket", text)
        self.env["BIND_FAIL"] = "1"
        code, out = self.run_bootstrap(self.package(server=True), args=("server",))
        self.assertEqual(code, 0, out)
        self.assertIn("provisioner completed", out)

    def test_python_archive_is_rejected_before_apt(self):
        members = dict.fromkeys([*SERVER_FILES, "bin/kiln-network"], b"fixture\n")
        del members["install.sh"]
        members["install.py"] = b"print('must-not-run')\n"
        code, out = self.run_bootstrap(self.package(members), args=("server",))
        self.assertNotEqual(code, 0, out)
        self.assertIn("Unexpected archive contents.", out)
        self.assertNotIn("must-not-run", out)
        self.assertNotIn("apt-get", self.privileged())

    def test_shell_archive_uses_bash_and_sqlite_not_python(self):
        members = dict.fromkeys([*SERVER_FILES, "bin/kiln-network"], b"fixture\n")
        members["install.sh"] = b"""#!/bin/bash
set -euo pipefail
got="$*"
expected="--address ${EXPECTED_ADDRESS} --apply"
if [ -n "${EXPECTED_UPLINK}" ]; then
  expected="${expected} --network-uplink ${EXPECTED_UPLINK}"
fi
[ "$got" = "$expected" ]
printf '%s\\n' 'provisioner completed'
"""
        code, out = self.run_bootstrap(self.package(members), args=("server",))
        self.assertEqual(code, 0, out)
        self.assertIn("provisioner completed", out)
        commands = [json.loads(line) for line in (self.root / "privileged").read_text().splitlines()]
        self.assertNotIn("sqlite3", commands[3])
        self.assertNotIn("python3", commands[3])
        self.assertEqual(commands[4][0], "bash")

    def test_existing_shell_archive_skips_apt(self):
        (self.root / "etc-kiln").mkdir()
        self.env["ROUTE"] = ""
        members = dict.fromkeys([*SERVER_FILES, "bin/kiln-network"], b"fixture\n")
        members["install.sh"] = b"#!/bin/bash\nset -euo pipefail\n[ \"$*\" = --apply ]\nprintf '%s\\n' updated\n"
        code, out = self.run_bootstrap(self.package(members), args=("server",))
        self.assertEqual(code, 0, out)
        self.assertIn("updated", out)
        commands = [json.loads(line) for line in (self.root / "privileged").read_text().splitlines()]
        self.assertEqual(commands[0], ["true"])
        self.assertEqual(commands[1][0], "bash")
        self.assertEqual(len(commands), 2)
        self.assertNotIn("sqlite3", self.privileged())

    def path_without(self, name):
        hidden = self.root / f"hide-{name}"
        hidden.mkdir()
        for directory in os.environ["PATH"].split(os.pathsep):
            base = Path(directory)
            if not base.is_dir():
                continue
            for entry in base.iterdir():
                dest = hidden / entry.name
                if entry.name == name or dest.exists():
                    continue
                try:
                    dest.symlink_to(entry)
                except OSError:
                    continue
        return str(self.root / "bin") + os.pathsep + str(hidden)

    def apt_subcommands(self, commands):
        found = []
        for command in commands:
            if "apt-get" not in command:
                continue
            found.append(command[command.index("apt-get") + 1])
        return found

    def test_sqlite3_install_is_exact_and_skipped_when_present(self):
        update = [
            "env",
            "DEBIAN_FRONTEND=noninteractive",
            "NEEDRESTART_MODE=l",
            "apt-get",
            "update",
        ]
        install = [
            "env",
            "DEBIAN_FRONTEND=noninteractive",
            "NEEDRESTART_MODE=l",
            "apt-get",
            "install",
            "-y",
            "--no-install-recommends",
            "--no-upgrade",
            "sqlite3",
        ]
        text = (ROOT / "deploy/bootstrap/install.sh").read_text()
        self.assertIn(
            "DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l apt-get update",
            text,
        )
        self.assertIn(
            "DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l apt-get install -y --no-install-recommends --no-upgrade sqlite3",
            text,
        )
        self.assertNotIn("apt-get upgrade", text)
        self.assertNotIn("dist-upgrade", text)
        members = dict.fromkeys([*SERVER_FILES, "bin/kiln-network"], b"fixture\n")
        members["install.sh"] = b"#!/bin/bash\nset -euo pipefail\nprintf '%s\\n' 'provisioner completed'\n"
        code, out = self.run_bootstrap(self.package(members), args=("server",))
        self.assertEqual(code, 0, out)
        present = [json.loads(line) for line in (self.root / "privileged").read_text().splitlines()]
        self.assertNotIn(install, present)
        self.assertNotIn(update, present)
        self.assertTrue(any(command[-2:] == ["apt-get", "update"] for command in present))
        self.assertNotIn("upgrade", self.apt_subcommands(present))
        self.assertNotIn("dist-upgrade", self.apt_subcommands(present))

        (self.root / "privileged").unlink()
        self.env["PATH"] = self.path_without("sqlite3")
        code, out = self.run_bootstrap(self.package(members), args=("server",))
        self.assertEqual(code, 0, out)
        missing = [json.loads(line) for line in (self.root / "privileged").read_text().splitlines()]
        self.assertEqual(missing.count(install), 1)
        self.assertLess(missing.index(update), missing.index(install))
        general = next(command for command in missing if command[-1] == "passwd")
        self.assertNotIn("sqlite3", general)
        self.assertNotIn("python3", general)
        self.assertNotIn("upgrade", self.apt_subcommands(missing))
        self.assertNotIn("dist-upgrade", self.apt_subcommands(missing))

        (self.root / "etc-kiln").mkdir()
        (self.root / "privileged").unlink()
        self.env["ROUTE"] = ""
        members["install.sh"] = b"#!/bin/bash\nset -euo pipefail\n[ \"$*\" = --apply ]\nprintf '%s\\n' updated\n"
        code, out = self.run_bootstrap(self.package(members), args=("server",))
        self.assertEqual(code, 0, out)
        existing = [json.loads(line) for line in (self.root / "privileged").read_text().splitlines()]
        self.assertEqual(len(existing), 4, existing)
        self.assertEqual(existing[0], ["true"])
        self.assertEqual(existing[1], update)
        self.assertEqual(existing[2], install)
        self.assertEqual(existing[3][0], "bash")
        self.assertEqual(existing[3][-1], "--apply")
        self.assertNotIn("upgrade", self.apt_subcommands(existing))
        self.assertNotIn("dist-upgrade", self.apt_subcommands(existing))

    def test_sqlite3_apt_failure_does_not_run_the_installer(self):
        (self.root / "etc-kiln").mkdir()
        self.env.update(ROUTE="", APT_FAIL="update", PATH=self.path_without("sqlite3"))
        members = dict.fromkeys([*SERVER_FILES, "bin/kiln-network"], b"fixture\n")
        members["install.sh"] = b"#!/bin/bash\nprintf '%s\\n' updated\n"
        code, out = self.run_bootstrap(self.package(members), args=("server",))
        self.assertNotEqual(code, 0, out)
        self.assertNotIn("updated", out)
        commands = [json.loads(line) for line in (self.root / "privileged").read_text().splitlines()]
        self.assertTrue(any(command[-2:] == ["apt-get", "update"] for command in commands))
        self.assertFalse(any(command[0] == "bash" for command in commands))
        self.assertFalse(any(command[-1] == "sqlite3" for command in commands))

    def test_kvm_access_is_probed_before_apt(self):
        code, out = self.run_bootstrap(self.package(server=True), args=("server",))
        self.assertEqual(code, 0, out)
        commands = [
            json.loads(line) for line in (self.root / "privileged").read_text().splitlines()
        ]
        kvm = next(i for i, command in enumerate(commands) if "kvm" in json.dumps(command))
        update = next(
            i for i, command in enumerate(commands) if command[-2:] == ["apt-get", "update"]
        )
        self.assertLess(kvm, update)

    def test_wrong_binary_version_is_not_installed(self):
        code, out = self.run_bootstrap(
            self.package({"kiln": b"#!/bin/sh\necho 'kiln 9.9.9'\n"})
        )
        self.assertNotEqual(code, 0, out)
        self.assertIn("version", out)
        self.assertFalse((self.root / "home/.local/bin/kiln").exists())


if __name__ == "__main__":
    unittest.main()

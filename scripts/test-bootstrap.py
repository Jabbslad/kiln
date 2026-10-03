#!/usr/bin/env python3
"""Run the real shell bootstrap without a TTY; never provision this host."""

import hashlib
import io
import json
import os
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SERVER_FILES = [
    "bin/kiln-runtime",
    "bin/kiln",
    "bin/kiln-host",
    "bin/kiln-api",
    "install.py",
    "fetch-firecracker.sh",
    "README.md",
    "docs/releases.md",
    "docs/remote-client.md",
    "docs/runtime.md",
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
assert args[-1].startswith('https://github.com/Jabbslad/kiln/releases/download/v0.3.2/kiln-')
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
if args[0] == 'python3':
    sys.exit(subprocess.call([sys.executable, *args[1:]]))
assert args[:4] == ['env', 'DEBIAN_FRONTEND=noninteractive', 'NEEDRESTART_MODE=a', 'apt-get']
""",
        )

    def executable(self, name, text):
        path = self.root / "bin" / name
        path.write_text(text)
        path.chmod(0o755)

    def package(self, members=None, server=False):
        if members is None:
            members = {"kiln": b"#!/bin/sh\necho 'kiln 0.3.2'\n"}
        if server:
            members = dict.fromkeys([*SERVER_FILES, "bin/kiln-network"], b"fixture\n")
            members["install.py"] = (
                b"import os, sys\nexpected = ['--address', os.environ['EXPECTED_ADDRESS'], '--apply']\n"
                b"if os.environ['EXPECTED_UPLINK']: expected += ['--network-uplink', os.environ['EXPECTED_UPLINK']]\n"
                b"assert sys.argv[1:] == expected, sys.argv\nassert sys.stdin.read() == ''\nprint('provisioner completed')\n"
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
            ("/run/systemd/system", "systemd"),
            ("/sys/fs/cgroup/cgroup.controllers", "controllers"),
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
                binary = self.root / "home/.local/bin/kiln"
                self.assertEqual(
                    subprocess.check_output([binary, "--version"], text=True),
                    "kiln 0.3.2\n",
                )
                request = self.requests()[-1]
                self.assertEqual(
                    request["args"][-1],
                    f"https://github.com/Jabbslad/kiln/releases/download/v0.3.2/kiln-v0.3.2-{expected}.tar.gz",
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

    def test_explicit_upgrade_replaces_client_and_keeps_previous_and_profiles(self):
        binary = self.old_client()
        original = binary.read_bytes()
        profile = self.root / "home/.config/kiln/profiles.json"
        profile.parent.mkdir(parents=True)
        profile.write_text("preserve profile and credential references")
        code, out = self.run_bootstrap(self.package(), args=("client", "--upgrade"))
        self.assertEqual(code, 0, out)
        self.assertEqual(
            subprocess.check_output([binary, "--version"], text=True), "kiln 0.3.2\n"
        )
        self.assertEqual(binary.with_name("kiln.previous").read_bytes(), original)
        self.assertEqual(
            profile.read_text(), "preserve profile and credential references"
        )
        self.assertEqual(
            sorted(p.name for p in binary.parent.iterdir()), ["kiln", "kiln.previous"]
        )

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
                    args=("client", "--upgrade"),
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
            code, out = self.run_bootstrap(self.package(), args=("client", "--upgrade"))
            self.assertNotEqual(code, 0, out)
            self.assertIn("regular file", out)
            self.assertTrue(unsafe.is_symlink())
            self.assertEqual(self.requests(), [])
            unsafe.unlink()
            self.old_client()
        lock = binary.parent / ".kiln-install.lock"
        lock.mkdir()
        code, out = self.run_bootstrap(self.package(), args=("client", "--upgrade"))
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
        code, out = self.run_bootstrap(self.package(), args=("client", "--upgrade"))
        self.assertNotEqual(code, 0, out)
        self.assertIn("replace", out.lower())
        self.assertEqual(binary.read_bytes(), original)
        self.assertEqual(binary.with_name("kiln.previous").read_bytes(), original)
        self.assertEqual(
            sorted(p.name for p in binary.parent.iterdir()), ["kiln", "kiln.previous"]
        )

    def test_upgrade_flag_is_client_only_and_requires_existing_file(self):
        for args in [("server", "--upgrade"), ("client", "--upgrade")]:
            with self.subTest(args=args):
                code, out = self.run_bootstrap(self.package(), args=args)
                self.assertNotEqual(code, 0, out)
                self.assertEqual(self.requests(), [])
                self.assertFalse((self.root / "privileged").exists())

    def test_server_unsupported_host_never_requests_privileges(self):
        for distro, version in [
            ("ubuntu", "22.04"),
            ("ubuntu", "24.10"),
            ("ubuntu", "26.10"),
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
        self.assertEqual(commands[1][-2:], ["apt-get", "update"])
        self.assertIn("python3", commands[2])
        self.assertEqual(commands[3][0], "python3")
        self.assertTrue(
            self.requests()[0]["args"][-1].endswith(
                "/kiln-server-v0.3.2-x86_64-unknown-linux-gnu.tar.gz"
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

    def test_wrong_binary_version_is_not_installed(self):
        code, out = self.run_bootstrap(
            self.package({"kiln": b"#!/bin/sh\necho 'kiln 9.9.9'\n"})
        )
        self.assertNotEqual(code, 0, out)
        self.assertIn("version", out)
        self.assertFalse((self.root / "home/.local/bin/kiln").exists())


if __name__ == "__main__":
    unittest.main()

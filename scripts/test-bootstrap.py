#!/usr/bin/env python3
"""Run the real shell bootstrap through a pipe/TTY; never provision this host."""

import hashlib
import io
import json
import os
import pty
import select
import signal
import subprocess
import sys
import tarfile
import tempfile
import termios
import time
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TOKEN = "github_pat_test_not_a_real_credential"
SERVER_FILES = [
    "bin/box",
    "bin/boxctl",
    "bin/boxd-host",
    "bin/boxd-api",
    "install.py",
    "fetch-firecracker.sh",
    "README.md",
    "docs/releases.md",
    "docs/remote-client.md",
    "docs/runtime.md",
    "deploy/boxd-host.service",
    "deploy/boxd-api.service",
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
            "REDIRECT": "https://release-assets.githubusercontent.com/test",
        }
        self.executable(
            "uname",
            '#!/bin/sh\ncase "$1" in -s) echo "$OS";; -m) echo "$ARCH";; esac\n',
        )
        self.executable("getconf", '#!/bin/sh\necho "$GLIBC"\n')
        self.executable("id", "#!/bin/sh\necho 1000\n")
        self.executable(
            "curl",
            f"#!{sys.executable}\n"
            + """
import json, os, pathlib, shutil, sys
root = pathlib.Path(os.environ['FIXTURE'])
args = sys.argv[1:]
assert args[0] == '-q', 'curl user configuration must be disabled'
assert '--location-trusted' not in args
assert '--location' not in args and '-L' not in args
assert '--proto' in args and args[args.index('--proto') + 1] == '=https'
record = {'args': args}
if '--config' in args:
    config = pathlib.Path(args[args.index('--config') + 1])
    record['private_config'] = config.stat().st_mode & 0o777 == 0o600
    record['has_auth'] = 'Authorization: Bearer github_pat_test_not_a_real_credential' in config.read_text()
    if record['has_auth']:
        assert args[-1].startswith('https://api.github.com/repos/Jabbslad/boxd/releases/assets/')
    else:
        assert config.read_text().startswith('url = "https://release-assets.githubusercontent.com/')
        assert 'Authorization' not in config.read_text()
else:
    record['has_auth'] = False
with (root / 'requests').open('a') as f:
    f.write(json.dumps(record) + '\\n')
output = pathlib.Path(args[args.index('--output') + 1])
code = os.environ['HTTP'] if record['has_auth'] else '200'
if '--dump-header' in args:
    pathlib.Path(args[args.index('--dump-header') + 1]).write_text(
        'HTTP/2 ' + code + '\\r\\nLocation: ' + os.environ['REDIRECT'] + '\\r\\n\\r\\n')
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
with (root / 'privileged').open('a') as f:
    f.write(json.dumps(sys.argv[1:]) + '\\n')
if sys.argv[1] == 'python3':
    assert os.isatty(0), 'provisioner must read from TTY, not script pipe'
    assert not list((root / 'tmp').glob('*/auth.conf')), 'token must be deleted before sudo'
    sys.exit(subprocess.call([sys.executable, *sys.argv[2:]]))
assert sys.argv[1] == 'apt-get', 'unexpected privileged action'
""",
        )

    def executable(self, name, text):
        path = self.root / "bin" / name
        path.write_text(text)
        path.chmod(0o755)

    def package(self, members=None, server=False):
        if members is None:
            members = {"boxctl": b"#!/bin/sh\necho 'box-client 0.2.1'\n"}
        if server:
            members = dict.fromkeys(SERVER_FILES, b"fixture\n")
            members["install.py"] = (
                b"import sys\nassert sys.argv[1:] == ['--address', '192.168.50.7', '--apply']\nassert input('Type INSTALL: ') == 'INSTALL'\nprint('provisioner completed')\n"
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
            f"{key} {index} {digest}"
            for index, key in enumerate(
                [
                    "client:x86_64-unknown-linux-gnu",
                    "client:x86_64-apple-darwin",
                    "client:aarch64-apple-darwin",
                    "server:x86_64-unknown-linux-gnu",
                ],
                101,
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

    def run_bootstrap(self, digest, args=(), answers=None, interrupt=False):
        script = self.script(digest)
        if answers is None:
            answers = [(b"GitHub token: ", TOKEN)]
        pid, master = pty.fork()
        if pid == 0:
            os.execve(
                "/bin/sh",
                [
                    "sh",
                    "-c",
                    'file=$1; shift; cat "$file" | sh -s -- "$@"',
                    "test",
                    str(script),
                    *args,
                ],
                self.env,
            )
        output = b""
        deadline = time.monotonic() + 12
        answer_index = 0
        interrupted = False
        status = None
        waited = 0
        try:
            while time.monotonic() < deadline:
                if select.select([master], [], [], 0.05)[0]:
                    try:
                        chunk = os.read(master, 65536)
                    except OSError:
                        break
                    if not chunk:
                        break
                    output += chunk
                if interrupt and b"GitHub token: " in output and not interrupted:
                    os.write(master, b"\x03")
                    interrupted = True
                elif answer_index < len(answers) and answers[answer_index][0] in output:
                    os.write(master, (answers[answer_index][1] + "\n").encode())
                    answer_index += 1
                waited, status = os.waitpid(pid, os.WNOHANG)
                if waited:
                    break
            else:
                os.killpg(pid, signal.SIGKILL)
                self.fail("bootstrap timed out: " + output.decode(errors="replace"))
            if not waited:
                _, status = os.waitpid(pid, 0)
            echo = bool(termios.tcgetattr(master)[3] & termios.ECHO)
        finally:
            os.close(master)
        self.assertNotIn(TOKEN, output.decode(errors="replace"))
        self.assertTrue(echo, "terminal echo was not restored")
        self.assertEqual(
            list((self.root / "tmp").iterdir()),
            [],
            "temporary credentials/files remain",
        )
        return os.waitstatus_to_exitcode(status), output.decode(errors="replace")

    def requests(self):
        path = self.root / "requests"
        return (
            [json.loads(line) for line in path.read_text().splitlines()]
            if path.exists()
            else []
        )

    def test_client_platforms_install_correct_asset(self):
        for system, arch, expected in [
            ("Linux", "x86_64", 101),
            ("Darwin", "x86_64", 102),
            ("Darwin", "arm64", 103),
        ]:
            with self.subTest(system=system, arch=arch):
                self.env.update(OS=system, ARCH=arch)
                code, out = self.run_bootstrap(self.package())
                self.assertEqual(code, 0, out)
                binary = self.root / "home/.local/bin/boxctl"
                self.assertEqual(
                    subprocess.check_output([binary, "--version"], text=True),
                    "box-client 0.2.1\n",
                )
                request = self.requests()[-1]
                self.assertTrue(request["private_config"])
                self.assertTrue(request["has_auth"])
                self.assertTrue(request["args"][-1].endswith(f"/{expected}"))
                self.assertNotIn(TOKEN, json.dumps(request))
                binary.unlink()

    def test_redirect_does_not_receive_authentication(self):
        self.env["HTTP"] = "302"
        code, out = self.run_bootstrap(self.package())
        self.assertEqual(code, 0, out)
        self.assertEqual([r["has_auth"] for r in self.requests()], [True, False])

    def test_unexpected_redirect_stops(self):
        self.env.update(HTTP="302", REDIRECT="https://attacker.example/package")
        code, out = self.run_bootstrap(self.package())
        self.assertNotEqual(code, 0, out)
        self.assertEqual(len(self.requests()), 1)

    def test_bad_digest_never_installs(self):
        self.package()
        code, out = self.run_bootstrap("0" * 64)
        self.assertNotEqual(code, 0, out)
        self.assertIn("checksum", out.lower())
        self.assertFalse((self.root / "home/.local/bin/boxctl").exists())

    def test_http_error_never_installs(self):
        self.env["HTTP"] = "404"
        code, out = self.run_bootstrap(self.package())
        self.assertNotEqual(code, 0, out)
        self.assertIn("404", out)
        self.assertFalse((self.root / "home/.local/bin/boxctl").exists())

    def test_unsafe_archives_are_rejected_even_with_correct_digest(self):
        for members in [
            {"../escaped": b"bad"},
            {"boxctl": ("/bin/sh",)},
            {"boxctl": b"ok", "extra": b"bad"},
        ]:
            with self.subTest(members=members):
                code, out = self.run_bootstrap(self.package(members))
                self.assertNotEqual(code, 0, out)
                self.assertIn("archive", out.lower())
                self.assertFalse((self.root / "home/.local/bin/boxctl").exists())

    def test_existing_client_is_preserved_without_downloading(self):
        binary = self.root / "home/.local/bin/boxctl"
        binary.parent.mkdir(parents=True)
        binary.write_text("preserve")
        code, out = self.run_bootstrap(self.package(), answers=[])
        self.assertNotEqual(code, 0, out)
        self.assertEqual(binary.read_text(), "preserve")
        self.assertEqual(self.requests(), [])
        binary.unlink()
        binary.symlink_to("missing-target")
        code, out = self.run_bootstrap(self.package(), answers=[])
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
                code, out = self.run_bootstrap(self.package(), answers=[])
                self.assertNotEqual(code, 0, out)
                self.assertNotIn("GitHub token: ", out)
                self.assertEqual(self.requests(), [])
                self.env = old

    def old_client(self):
        binary = self.root / "home/.local/bin/boxctl"
        binary.parent.mkdir(parents=True, exist_ok=True)
        binary.write_text("#!/bin/sh\necho 'box-client 0.1.1'\n")
        binary.chmod(0o755)
        return binary

    def test_explicit_upgrade_replaces_client_and_keeps_previous_and_profiles(self):
        binary = self.old_client()
        original = binary.read_bytes()
        profile = self.root / "home/.config/boxd/profiles.json"
        profile.parent.mkdir(parents=True)
        profile.write_text("preserve profile and credential references")
        code, out = self.run_bootstrap(self.package(), args=("client", "--upgrade"))
        self.assertEqual(code, 0, out)
        self.assertEqual(subprocess.check_output([binary, "--version"], text=True), "box-client 0.2.1\n")
        self.assertEqual(binary.with_name("boxctl.previous").read_bytes(), original)
        self.assertEqual(profile.read_text(), "preserve profile and credential references")
        self.assertEqual(sorted(p.name for p in binary.parent.iterdir()), ["boxctl", "boxctl.previous"])

    def test_failed_upgrade_preserves_current_and_previous_clients(self):
        binary = self.old_client()
        original = binary.read_bytes()
        previous = binary.with_name("boxctl.previous")
        previous.write_bytes(b"older backup")
        for failure in ("checksum", "version", "HTTP"):
            with self.subTest(failure=failure):
                self.env["HTTP"] = "403" if failure == "HTTP" else "200"
                digest = self.package({"boxctl": b"#!/bin/sh\necho wrong-version\n"}) if failure == "version" else self.package()
                code, out = self.run_bootstrap("0" * 64 if failure == "checksum" else digest, args=("client", "--upgrade"))
                self.assertNotEqual(code, 0, out)
                self.assertIn(failure.lower(), out.lower())
                self.assertEqual(binary.read_bytes(), original)
                self.assertEqual(previous.read_bytes(), b"older backup")
                self.assertFalse((binary.parent / ".boxctl-install.lock").exists())

    def test_upgrade_rejects_symlinks_and_busy_lock_before_downloading(self):
        binary = self.old_client()
        for unsafe in (binary, binary.with_name("boxctl.previous")):
            if unsafe.exists():
                unsafe.unlink()
            unsafe.symlink_to("missing-target")
            code, out = self.run_bootstrap(self.package(), args=("client", "--upgrade"), answers=[])
            self.assertNotEqual(code, 0, out)
            self.assertIn("regular file", out)
            self.assertTrue(unsafe.is_symlink())
            self.assertEqual(self.requests(), [])
            unsafe.unlink()
            self.old_client()
        lock = binary.parent / ".boxctl-install.lock"
        lock.mkdir()
        code, out = self.run_bootstrap(self.package(), args=("client", "--upgrade"), answers=[])
        self.assertNotEqual(code, 0, out)
        self.assertIn("another installer", out)
        self.assertTrue(lock.exists())
        self.assertEqual(self.requests(), [])

    def test_upgrade_failed_publication_preserves_working_client(self):
        binary = self.old_client()
        original = binary.read_bytes()
        real_mv = subprocess.check_output(["sh", "-c", "command -v mv"], text=True).strip()
        self.executable("mv", f'#!/bin/sh\nfor arg do last=$arg; done\ncase "$last" in */boxctl) exit 1;; esac\nexec "{real_mv}" "$@"\n')
        code, out = self.run_bootstrap(self.package(), args=("client", "--upgrade"))
        self.assertNotEqual(code, 0, out)
        self.assertIn("replace", out.lower())
        self.assertEqual(binary.read_bytes(), original)
        self.assertEqual(binary.with_name("boxctl.previous").read_bytes(), original)
        self.assertEqual(sorted(p.name for p in binary.parent.iterdir()), ["boxctl", "boxctl.previous"])

    def test_cancelled_upgrade_preserves_client_and_releases_lock(self):
        binary = self.old_client()
        original = binary.read_bytes()
        code, out = self.run_bootstrap(self.package(), args=("client", "--upgrade"), interrupt=True)
        self.assertNotEqual(code, 0, out)
        self.assertEqual(binary.read_bytes(), original)
        self.assertEqual(sorted(p.name for p in binary.parent.iterdir()), ["boxctl"])
        self.assertEqual(self.requests(), [])

    def test_upgrade_flag_is_client_only_and_requires_existing_file(self):
        for args in [("server", "--upgrade"), ("client", "--upgrade")]:
            with self.subTest(args=args):
                code, out = self.run_bootstrap(self.package(), args=args, answers=[])
                self.assertNotEqual(code, 0, out)
                self.assertEqual(self.requests(), [])
                self.assertFalse((self.root / "privileged").exists())

    def test_cancelled_token_prompt_restores_echo(self):
        code, out = self.run_bootstrap(self.package(), interrupt=True)
        self.assertNotEqual(code, 0, out)
        self.assertEqual(self.requests(), [])

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
                    self.package(server=True), args=("server",), answers=[]
                )
                self.assertNotEqual(code, 0, out)
                self.assertIn("24.04", out)
                self.assertEqual(self.requests(), [])
                self.assertFalse((self.root / "privileged").exists())

    def test_ubuntu_2604_reaches_server_provisioning(self):
        (self.root / "os-release").write_text('ID=ubuntu\nVERSION_ID="26.04"\n')
        self.test_server_dependency_and_provisioner_confirmation_use_tty()

    def test_server_dependency_and_provisioner_confirmation_use_tty(self):
        code, out = self.run_bootstrap(
            self.package(server=True),
            args=("server",),
            answers=[
                (b"GitHub token: ", TOKEN),
                (b"Server private IPv4 address: ", "192.168.50.7"),
                (b"Type SETUP", "SETUP"),
                (b"Type INSTALL", "INSTALL"),
            ],
        )
        self.assertEqual(code, 0, out)
        self.assertIn("provisioner completed", out)
        commands = [
            json.loads(line)
            for line in (self.root / "privileged").read_text().splitlines()
        ]
        self.assertEqual(commands[0], ["apt-get", "update"])
        self.assertIn("python3", commands[1])
        self.assertEqual(commands[2][0], "python3")

    def test_kvm_access_is_checked_by_privileged_provisioner(self):
        (self.root / "kvm").chmod(0)
        self.test_server_dependency_and_provisioner_confirmation_use_tty()

    def test_network_opt_in_requires_capable_package_and_passes_explicit_uplink(self):
        self.env["BOXD_NETWORK_UPLINK"] = "eth0"
        members = dict.fromkeys([*SERVER_FILES, "bin/boxd-network"], b"fixture\n")
        members["install.py"] = (
            b"import sys\nassert sys.argv[1:] == ['--address', '192.168.50.7', '--apply', '--network-uplink', 'eth0']\nassert input('Type INSTALL: ') == 'INSTALL'\n"
        )
        code, out = self.run_bootstrap(self.package(members), args=("server",), answers=[
            (b"GitHub token: ", TOKEN),
            (b"Server private IPv4 address: ", "192.168.50.7"),
            (b"Type SETUP", "SETUP"), (b"Type INSTALL", "INSTALL"),
        ])
        self.assertEqual(code, 0, out)
        commands = [json.loads(line) for line in (self.root / "privileged").read_text().splitlines()]
        self.assertIn("nftables", commands[1])
        self.assertIn("iproute2", commands[1])

    def test_server_cancellation_and_invalid_addresses_do_not_run_sudo(self):
        for address in [
            "192.168.50.7",
            "8.8.8.8",
            "100.63.0.1",
            "172.32.0.1",
            "10.0.0.999",
            "10.0.0.01",
        ]:
            with self.subTest(address=address):
                code, out = self.run_bootstrap(
                    self.package(server=True),
                    args=("server",),
                    answers=[
                        (b"GitHub token: ", TOKEN),
                        (b"Server private IPv4 address: ", address),
                        (b"Type SETUP", "no"),
                    ],
                )
                self.assertNotEqual(code, 0, out)
                self.assertIn(
                    "Cancelled" if address == "192.168.50.7" else "Use a private/VPN",
                    out,
                )
                self.assertFalse((self.root / "privileged").exists())

    def test_wrong_binary_version_is_not_installed(self):
        code, out = self.run_bootstrap(
            self.package({"boxctl": b"#!/bin/sh\necho 'box-client 9.9.9'\n"})
        )
        self.assertNotEqual(code, 0, out)
        self.assertIn("version", out)
        self.assertFalse((self.root / "home/.local/bin/boxctl").exists())

    def test_token_config_injection_is_rejected(self):
        code, out = self.run_bootstrap(
            self.package(), answers=[(b"GitHub token: ", 'bad"token')]
        )
        self.assertNotEqual(code, 0, out)
        self.assertEqual(self.requests(), [])


if __name__ == "__main__":
    unittest.main()

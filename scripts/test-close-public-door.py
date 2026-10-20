#!/usr/bin/env python3
"""Upgrade closes the public door listener and leaves the bridge serving."""

import os
import re
import signal
import socket
import subprocess
import tempfile
import textwrap
import time
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
INSTALL = ROOT / "deploy/install.sh"
SERVICE = ROOT / "deploy/kiln-door-proxy.service"
PUBLIC_UNIT = ROOT / "deploy/kiln-door-proxy-public.socket"
PUBLIC = "127.0.0.1:8443"
BRIDGE = "127.0.0.1:8444"
PROXYD = "/lib/systemd/systemd-socket-proxyd"


def extract_function(source, name):
    match = re.search(rf"^{name}\(\) \{{.*?^\}}", source, re.M | re.S)
    if match is None:
        raise AssertionError(f"{name} is missing from deploy/install.sh")
    return match.group(0)


def listeners():
    completed = subprocess.run(["ss", "-ltn"], check=True, capture_output=True, text=True)
    rows = [
        line
        for line in completed.stdout.splitlines()
        if line.startswith("State") or ":8443" in line or ":8444" in line
    ]
    return "\n".join(rows)


def probe(port):
    sock = socket.socket()
    sock.settimeout(1)
    try:
        sock.connect(("127.0.0.1", port))
        sock.sendall(b"hi\n")
        data = sock.recv(32)
        return f"port {port} answered {data!r}"
    except OSError as exc:
        return f"port {port} failed {exc}"
    finally:
        sock.close()


class ClosePublicDoor(unittest.TestCase):
    def test_upgrade_closes_8443_and_keeps_8444(self):
        """127.0.0.1 stands in for the public address and the bridge address."""
        work = Path(tempfile.mkdtemp(prefix="kiln-close-8443-"))
        self.addCleanup(self.cleanup, work)
        units = work / "units"
        state = work / "state"
        units.mkdir()
        state.mkdir()
        backend_port = self.backend(work)
        (units / "kiln-door-proxy.service").write_text(
            "Sockets=kiln-door-proxy.socket kiln-door-proxy-public.socket\n"
        )
        (units / "kiln-door-proxy-public.socket").write_text("ListenStream=127.0.0.1:8443\n")
        (state / "public-active").write_text("active\n")
        (state / "proxy-active").write_text("active\n")
        log = work / "systemctl.log"
        self.write_systemctl(work, units, state, log, backend_port)
        self.spawn(work, state, (PUBLIC, BRIDGE), backend_port)
        time.sleep(0.3)
        before = listeners() + "\n" + probe(8443) + "\n" + probe(8444)
        print(before, flush=True)
        env = os.environ.copy()
        env["PATH"] = str(work / "bin") + os.pathsep + env.get("PATH", "")
        env["UNITS"] = str(units)
        function = extract_function(INSTALL.read_text(), "close_public_door")
        completed = subprocess.run(
            ["bash", "-c", "set -euo pipefail\n" + function + "\nclose_public_door\n"],
            env=env,
            text=True,
            capture_output=True,
            timeout=15,
        )
        after = listeners() + "\n" + probe(8443) + "\n" + probe(8444)
        print(completed.stdout, completed.stderr, after, sep="\n", flush=True)
        self.assertEqual(completed.returncode, 0, completed.stderr)
        commands = log.read_text()
        self.assertIn("systemctl disable --now kiln-door-proxy-public.socket", commands)
        self.assertNotIn("systemctl enable", commands)
        self.assertNotIn(":8443", listeners().split("State", 1)[-1])
        self.assertIn(":8444", listeners())
        self.assertIn("port 8443 failed", after)
        self.assertIn("port 8444 answered b'door-ok\\n'", after)
        service = SERVICE.read_text()
        self.assertIn("Sockets=kiln-door-proxy.socket\n", service)
        self.assertNotIn("kiln-door-proxy-public.socket", service)
        self.assertFalse(PUBLIC_UNIT.exists())

    def backend(self, work):
        path = work / "backend.py"
        path.write_text(
            textwrap.dedent(
                """\
                import socket
                server = socket.socket()
                server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
                server.bind(("127.0.0.1", 0))
                server.listen(16)
                print(server.getsockname()[1], flush=True)
                while True:
                    conn, _addr = server.accept()
                    conn.sendall(b"door-ok\\n")
                    conn.close()
                """
            )
        )
        proc = subprocess.Popen(
            ["python3", str(path)],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
        )
        (work / "backend.pid").write_text(str(proc.pid))
        line = proc.stdout.readline()
        return int(line)

    def spawn(self, work, state, addresses, backend_port):
        command = ["systemd-socket-activate"]
        for address in addresses:
            command.extend(["-l", address])
        command.extend([PROXYD, f"127.0.0.1:{backend_port}"])
        proc = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        (state / "proxy.pid").write_text(str(proc.pid))
        (work / "proxy.pid").write_text(str(proc.pid))

    def write_systemctl(self, work, units, state, log, backend_port):
        bindir = work / "bin"
        bindir.mkdir()
        script = bindir / "systemctl"
        script.write_text(
            textwrap.dedent(
                f"""\
                #!/bin/bash
                printf '%s\\n' "systemctl $*" >> {log}
                cmd=$1
                shift
                unit=
                for arg in "$@"; do
                  case "$arg" in
                    --*) ;;
                    *) unit=$arg ;;
                  esac
                done
                case "$cmd" in
                  show)
                    if [ -f {units}/kiln-door-proxy-public.socket ]; then
                      printf '%s\\n' loaded
                    else
                      printf '%s\\n' not-found
                    fi
                    ;;
                  is-active)
                    quiet=0
                    marker={state}/proxy-active
                    for arg in "$@"; do
                      case "$arg" in
                        --quiet) quiet=1 ;;
                        --*) ;;
                        *) unit=$arg ;;
                      esac
                    done
                    if [ "$unit" = kiln-door-proxy-public.socket ]; then
                      marker={state}/public-active
                    fi
                    if [ -f "$marker" ]; then
                      [ "$quiet" = 1 ] || printf '%s\\n' active
                      exit 0
                    fi
                    [ "$quiet" = 1 ] || printf '%s\\n' inactive
                    exit 3
                    ;;
                  disable)
                    rm -f {state}/public-active
                    ;;
                  daemon-reload) ;;
                  restart)
                    if [ -f {state}/proxy.pid ]; then
                      old=$(cat {state}/proxy.pid)
                      kill "$old" 2>/dev/null || true
                      for _ in 1 2 3 4 5 6 7 8 9 10; do
                        kill -0 "$old" 2>/dev/null || break
                        sleep 0.05
                      done
                    fi
                    args=(systemd-socket-activate)
                    if grep -q kiln-door-proxy-public.socket {units}/kiln-door-proxy.service; then
                      args+=(-l {PUBLIC})
                    fi
                    args+=(-l {BRIDGE} {PROXYD} 127.0.0.1:{backend_port})
                    setsid "${{args[@]}}" >/dev/null 2>&1 < /dev/null &
                    echo $! > {state}/proxy.pid
                    echo $! > {work}/proxy.pid
                    sleep 0.3
                    ;;
                  *) exit 1 ;;
                esac
                """
            )
        )
        script.chmod(0o755)

    def cleanup(self, work):
        for name in ("proxy.pid", "backend.pid"):
            path = work / name
            if not path.exists():
                continue
            pid = int(path.read_text() or "0")
            if pid:
                os.kill(pid, signal.SIGTERM)
        for path in work.rglob("proxy.pid"):
            pid = int(path.read_text() or "0")
            if pid:
                try:
                    os.kill(pid, signal.SIGTERM)
                except OSError:
                    pass


if __name__ == "__main__":
    unittest.main()

#!/usr/bin/env python3
"""The door Caddy container keeps 172.18.0.3 and a read-only CA across recreate."""

import os
import shutil
import socket
import ssl
import subprocess
import tempfile
import textwrap
import time
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
COMPOSE = ROOT / "deploy/caddy-compose.yaml"
DOOR = ROOT / "deploy/door.Caddyfile"
SOCKET = ROOT / "deploy/kiln-door-proxy.socket"
RUNBOOK = ROOT / "docs/runbooks/2026-10-19-door-public-cert.md"
PIN = "172.18.0.3"
GATEWAY = "172.18.0.1"
CA_TARGET = "/etc/kiln/ca.crt"


def docker_base():
    for base in (["docker"], ["sudo", "docker"]):
        if subprocess.call(
            base + ["info"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
        ) == 0:
            return base
    return None


def bind_blocks(text):
    blocks = []
    current = None
    for line in text.splitlines():
        if line.startswith("      - type:"):
            if current:
                blocks.append(current)
            current = [line]
        elif current is not None and (
            line.startswith("      - ") or (line and not line.startswith(" "))
        ):
            blocks.append(current)
            current = None
        elif current is not None:
            current.append(line)
    if current:
        blocks.append(current)
    return ["\n".join(block) for block in blocks]


def bind_target(text, target):
    for block in bind_blocks(text):
        if f"target: {target}\n" in block + "\n":
            return block
    return None


class DoorCaddyContract(unittest.TestCase):
    def test_pinned_address_matches_the_door_socket(self):
        socket_text = SOCKET.read_text()
        compose = COMPOSE.read_text()
        self.assertIn(f"IPAddressAllow={PIN}/32\n", socket_text)
        self.assertIn(f"ListenStream={GATEWAY}:8444\n", socket_text)
        self.assertIn(f"ipv4_address: {PIN}\n", compose)
        self.assertIn("subnet: 172.18.0.0/16\n", compose)
        self.assertIn(f"gateway: {GATEWAY}\n", compose)
        allow = socket_text.split("IPAddressAllow=", 1)[1].split("/", 1)[0]
        pinned = compose.split("ipv4_address:", 1)[1].split()[0]
        self.assertEqual(allow, pinned)

    def test_ca_bind_is_read_only_at_create_and_is_not_the_key_directory(self):
        compose = COMPOSE.read_text()
        mount = bind_target(compose, CA_TARGET)
        self.assertIsNotNone(mount)
        self.assertIn("source: ${KILN_CA_CERT:-/etc/kiln/ca.crt}\n", mount)
        self.assertIn("read_only: true\n", mount)
        self.assertIn("create_host_path: false", mount)
        source = mount.split("source:", 1)[1].split("\n", 1)[0].strip()
        self.assertTrue(source.endswith("ca.crt}"))
        self.assertNotEqual(source, "/etc/kiln")
        for secret in ("ca.key", "door.key", "directory.token", "tls.key"):
            self.assertNotIn(secret, compose)
        self.assertNotIn("kiln-identity", compose)
        self.assertNotIn("8092", compose)
        caddyfile = bind_target(compose, "/etc/caddy/Caddyfile")
        self.assertIsNotNone(caddyfile)
        self.assertIn("read_only: true\n", caddyfile)
        self.assertIn("create_host_path: false", caddyfile)
        self.assertIn("source: ${KILN_CADDYFILE:-/opt/paseo/Caddyfile}\n", caddyfile)
        self.assertIn("caddy-data:/data\n", compose)
        self.assertIn("caddy-config:/config\n", compose)
        self.assertIn("image: ${KILN_CADDY_IMAGE:-caddy:2.10.2}\n", compose)

    def test_door_site_trusts_the_mounted_ca_with_the_current_directive(self):
        door = DOOR.read_text()
        runbook = RUNBOOK.read_text()
        self.assertIn("tls_trust_pool file {\n", door)
        self.assertIn(f"pem_file {CA_TARGET}\n", door)
        self.assertIn(f"https://{GATEWAY}:8444", door)
        self.assertIn("tls_server_name cli.dark-forge.dev\n", door)
        self.assertFalse(any(line.startswith("dark-forge.dev") for line in door.splitlines()))
        self.assertNotIn("tls_trusted_ca_certs", door)
        self.assertNotIn("tls_trusted_ca_certs /etc/kiln/ca.crt", runbook)
        self.assertIn("tls_trust_pool file {", runbook)
        self.assertIn(
            "docker compose -p kiln-door-caddy -f deploy/caddy-compose.yaml up -d --force-recreate",
            runbook,
        )


class DoorCaddyRecreate(unittest.TestCase):
    def test_recreate_keeps_the_address_the_ca_and_the_401(self):
        if os.environ.get("KILN_CADDY_DOCKER") != "1":
            self.skipTest("set KILN_CADDY_DOCKER=1 to recreate the container")
        base = docker_base()
        if base is None:
            self.fail("docker is not available")
        self.recreate(base)

    def recreate(self, base):
        work = Path(tempfile.mkdtemp(prefix="kiln-caddy-proof-"))
        self.addCleanup(shutil.rmtree, work, True)
        project = "kiln-caddy-proof"
        compose = ["compose", "-p", project, "-f", str(COMPOSE)]
        self.addCleanup(self.down, base, compose)

        self.lost_copy(base, work)

        ca_key = work / "ca.key"
        ca = work / "ca.crt"
        leaf_key = work / "standin.key"
        leaf = work / "standin.crt"
        public_key = work / "public.key"
        public = work / "public.crt"
        subprocess.check_call(
            [
                "openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2",
                "-subj", "/CN=kiln private CA",
                "-keyout", str(ca_key), "-out", str(ca),
            ],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        ca_key.chmod(0o600)
        ca.chmod(0o644)
        subprocess.check_call(
            [
                "openssl", "req", "-new", "-newkey", "rsa:2048", "-nodes",
                "-subj", "/CN=cli.dark-forge.dev",
                "-keyout", str(leaf_key), "-out", str(work / "standin.csr"),
            ],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        ext = work / "standin.ext"
        ext.write_text(
            "basicConstraints=critical,CA:FALSE\n"
            "keyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n"
            "subjectAltName=DNS:cli.dark-forge.dev\n"
        )
        subprocess.check_call(
            [
                "openssl", "x509", "-req", "-in", str(work / "standin.csr"),
                "-CA", str(ca), "-CAkey", str(ca_key),
                "-set_serial", "1", "-days", "2", "-sha256",
                "-extfile", str(ext), "-out", str(leaf),
            ],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        leaf_key.chmod(0o600)
        subprocess.check_call(
            [
                "openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2",
                "-subj", "/CN=cli.dark-forge.dev",
                "-addext", "subjectAltName=DNS:cli.dark-forge.dev",
                "-keyout", str(public_key), "-out", str(public),
            ],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        public.chmod(0o644)
        public_key.chmod(0o644)
        door = DOOR.read_text()
        head, rest = door.split("{", 1)
        caddyfile = work / "Caddyfile"
        caddyfile.write_text(
            "{\n\tadmin off\n\tauto_https off\n}\nhttps://"
            + head
            + "{\n\ttls /etc/caddy/public.crt /etc/caddy/public.key\n"
            + rest
        )
        env = os.environ.copy()
        env.update(
            {
                "KILN_CADDYFILE": str(caddyfile),
                "KILN_CA_CERT": str(ca),
                "KILN_CADDY_IMAGE": "caddy:2.10.2",
            }
        )
        # sudo drops the caller environment, so pass the three paths explicitly.
        if base[0] == "sudo":
            base = [
                "sudo", "env",
                f"KILN_CADDYFILE={caddyfile}",
                f"KILN_CA_CERT={ca}",
                "KILN_CADDY_IMAGE=caddy:2.10.2",
                *base[1:],
            ]
        # The public cert mounts are test-only. The production file has no tls line.
        override = work / "override.yaml"
        override.write_text(
            textwrap.dedent(
                f"""\
                services:
                  caddy:
                    volumes:
                      - type: bind
                        source: {public}
                        target: /etc/caddy/public.crt
                        read_only: true
                        bind:
                          create_host_path: false
                      - type: bind
                        source: {public_key}
                        target: /etc/caddy/public.key
                        read_only: true
                        bind:
                          create_host_path: false
                """
            )
        )
        up = base + compose + ["-f", str(override), "up", "-d", "--force-recreate"]
        subprocess.check_call(up, env=env, stdout=subprocess.DEVNULL)
        first = self.container_id(base, compose, env)
        self.assert_container(base, first, ca)
        standin = StandIn(leaf, leaf_key)
        standin.start()
        self.addCleanup(standin.stop)
        first_body = self.curl()
        print(f"through-caddy {first_body!r}", flush=True)
        self.assertEqual(first_body, "door-stand-in 401 from 172.18.0.3\n")
        denied = self.host_status()
        print(f"other-source {denied[0]} {denied[1]!r}", flush=True)
        self.assertEqual(denied[0], 403)
        self.assertIn("from ", denied[1])
        self.assertNotIn(f"from {PIN}", denied[1])
        subprocess.check_call(up, env=env, stdout=subprocess.DEVNULL)
        second = self.container_id(base, compose, env)
        self.assertNotEqual(first, second)
        print(f"recreated {first[:12]} then {second[:12]}", flush=True)
        self.assert_container(base, second, ca)
        second_body = self.curl()
        print(f"through-caddy-after {second_body!r}", flush=True)
        self.assertEqual(second_body, "door-stand-in 401 from 172.18.0.3\n")
        adapted = subprocess.check_output(
            base + ["run", "--rm", "-v", f"{DOOR}:/etc/caddy/Caddyfile:ro",
                    "caddy:2.10.2", "caddy", "adapt",
                    "--config", "/etc/caddy/Caddyfile", "--adapter", "caddyfile"],
            stderr=subprocess.PIPE, text=True,
        )
        self.assertIn('"provider":"file"', adapted)
        self.assertIn(CA_TARGET, adapted)
        self.assertNotIn("deprecated", adapted)

    def lost_copy(self, base, work):
        """A file copied into a running container is gone after recreate."""
        source = work / "copied.crt"
        source.write_text("copied-ca\n")
        name = "kiln-caddy-copy-loss"
        subprocess.call(
            base + ["rm", "-f", name],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        subprocess.check_call(
            base + ["run", "-d", "--name", name, "caddy:2.10.2", "sleep", "30"],
            stdout=subprocess.DEVNULL,
        )
        self.addCleanup(
            subprocess.call, base + ["rm", "-f", name],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        copied = subprocess.run(
            base + ["cp", str(source), f"{name}:{CA_TARGET}"],
            text=True, capture_output=True,
        )
        self.assertNotEqual(copied.returncode, 0, copied.stderr)
        subprocess.check_call(base + ["exec", name, "mkdir", "-p", "/etc/kiln"])
        subprocess.check_call(base + ["cp", str(source), f"{name}:{CA_TARGET}"])
        inside = subprocess.check_output(base + ["exec", name, "cat", CA_TARGET], text=True)
        self.assertEqual(inside, "copied-ca\n")
        subprocess.check_call(base + ["rm", "-f", name], stdout=subprocess.DEVNULL)
        subprocess.check_call(
            base + ["run", "-d", "--name", name, "caddy:2.10.2", "sleep", "30"],
            stdout=subprocess.DEVNULL,
        )
        missing = subprocess.run(
            base + ["exec", name, "cat", CA_TARGET], text=True, capture_output=True,
        )
        self.assertNotEqual(missing.returncode, 0)
        subprocess.check_call(base + ["rm", "-f", name], stdout=subprocess.DEVNULL)

    def container_id(self, base, compose, env):
        raw = subprocess.check_output(base + compose + ["ps", "-q"], env=env, text=True).strip()
        self.assertNotEqual(raw, "")
        return raw.splitlines()[0]

    def assert_container(self, base, container, ca):
        address = subprocess.check_output(
            base + ["inspect", "-f", "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}", container],
            text=True,
        ).strip()
        self.assertEqual(address, PIN)
        print(f"address {address}", flush=True)
        mounts = subprocess.check_output(
            base + ["exec", container, "cat", "/proc/mounts"], text=True,
        )
        matched = [line for line in mounts.splitlines() if f" {CA_TARGET} " in line]
        self.assertEqual(len(matched), 1, mounts)
        self.assertIn(" ro,", matched[0])
        print(matched[0], flush=True)
        body = subprocess.check_output(base + ["exec", container, "cat", CA_TARGET])
        self.assertEqual(body, ca.read_bytes())
        listing = subprocess.check_output(
            base + ["exec", container, "ls", "-1", "/etc/kiln"], text=True,
        )
        self.assertEqual(listing.split(), ["ca.crt"])
        wrote = subprocess.run(
            base + ["exec", container, "touch", CA_TARGET], text=True, capture_output=True,
        )
        self.assertNotEqual(wrote.returncode, 0)
        self.assertIn("Read-only file system", wrote.stderr)

    def curl(self):
        last = ""
        for _ in range(20):
            completed = subprocess.run(
                [
                    "curl", "-sk", "--resolve", "cli.dark-forge.dev:443:127.0.0.1",
                    "--max-time", "5", "https://cli.dark-forge.dev/v1/templates",
                ],
                text=True, capture_output=True,
            )
            if completed.returncode == 0 and completed.stdout.startswith("door-stand-in "):
                return completed.stdout
            last = completed.stderr or completed.stdout
            time.sleep(0.25)
        self.fail(last)

    def host_status(self):
        context = ssl._create_unverified_context()
        with socket.create_connection((GATEWAY, 8444), timeout=5) as raw:
            with context.wrap_socket(raw, server_hostname="cli.dark-forge.dev") as conn:
                conn.sendall(b"GET /v1/templates HTTP/1.1\r\nHost: cli.dark-forge.dev\r\nConnection: close\r\n\r\n")
                data = b""
                while True:
                    chunk = conn.recv(4096)
                    if not chunk:
                        break
                    data += chunk
        head, _, body = data.partition(b"\r\n\r\n")
        status = int(head.split()[1])
        return status, body.decode()

    def down(self, base, compose):
        subprocess.call(
            base + compose + ["down", "--remove-orphans"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )


class StandIn:
    def __init__(self, cert, key):
        self.cert = cert
        self.key = key
        self.httpd = None

    def start(self):
        cert = self.cert
        key = self.key

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                peer = self.connection.getpeername()[0]
                code = 401 if peer == PIN else 403
                body = f"door-stand-in {code} from {peer}\n".encode()
                self.send_response(code)
                self.send_header("Content-Length", str(len(body)))
                self.send_header("Content-Type", "text/plain")
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, fmt, *args):
                return

        self.httpd = HTTPServer((GATEWAY, 8444), Handler)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cert, key)
        self.httpd.socket = context.wrap_socket(self.httpd.socket, server_side=True)
        import threading
        self.thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)
        self.thread.start()
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            try:
                with socket.create_connection((GATEWAY, 8444), timeout=0.2):
                    return
            except OSError:
                time.sleep(0.05)
        raise RuntimeError("stand-in did not listen")

    def stop(self):
        if self.httpd is not None:
            self.httpd.shutdown()
            self.httpd.server_close()


if __name__ == "__main__":
    unittest.main()

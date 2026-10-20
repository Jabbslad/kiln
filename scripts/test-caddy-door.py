#!/usr/bin/env python3
"""Paseo's caddy service keeps 172.18.0.3 and a read-only CA across recreate."""

import ipaddress
import json
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
OVERRIDE = ROOT / "deploy/paseo-caddy.override.yaml"
DOOR = ROOT / "deploy/door.Caddyfile"
SOCKET = ROOT / "deploy/kiln-door-proxy.socket"
RUNBOOK = ROOT / "docs/runbooks/2026-10-19-door-public-cert.md"
PIN = "172.18.0.3"
GATEWAY = "172.18.0.1"
CA_TARGET = "/etc/kiln/ca.crt"
IMAGE = "caddy:2.10.2"
REFUSAL = "no configured subnet contains IP address 172.18.0.3"


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


def ipam_config(base, network):
    raw = subprocess.check_output(
        base + ["network", "inspect", network, "--format", "{{json .IPAM.Config}}"],
        text=True,
    )
    print(f"ipam {network} {raw.strip()}", flush=True)
    return json.loads(raw) or []


def subnet_contains(configs, pin):
    address = ipaddress.ip_address(pin)
    for item in configs:
        subnet = (item or {}).get("Subnet") or ""
        if subnet and address in ipaddress.ip_network(subnet, strict=False):
            return True
    return False


def pin_holders(base, network, pin):
    raw = subprocess.check_output(base + ["network", "inspect", network], text=True)
    holders = []
    for cid, info in (json.loads(raw)[0].get("Containers") or {}).items():
        ip = (info.get("IPv4Address") or "").split("/")[0]
        if ip == pin:
            holders.append(cid)
    return holders


def paseo_caddy(base, cid):
    raw = subprocess.check_output(
        base + ["inspect", "-f", "{{json .Config.Labels}}", cid], text=True
    )
    labels = json.loads(raw) or {}
    return (
        labels.get("com.docker.compose.project") == "paseo"
        and labels.get("com.docker.compose.service") == "caddy"
    )


def preflight(base, network):
    """Abort unless IPAM Config contains the pin and only paseo caddy holds it."""
    if not subnet_contains(ipam_config(base, network), PIN):
        return False, f"no subnet contains {PIN}"
    for cid in pin_holders(base, network, PIN):
        if not paseo_caddy(base, cid):
            return False, f"another container holds {PIN}"
    return True, "ok"


class DoorCaddyContract(unittest.TestCase):
    def test_pinned_address_matches_the_door_socket(self):
        socket_text = SOCKET.read_text()
        override = OVERRIDE.read_text()
        self.assertIn(f"IPAddressAllow={PIN}/32\n", socket_text)
        self.assertIn(f"ListenStream={GATEWAY}:8444\n", socket_text)
        self.assertIn(f"ipv4_address: {PIN}\n", override)
        allow = socket_text.split("IPAddressAllow=", 1)[1].split("/", 1)[0]
        pinned = override.split("ipv4_address:", 1)[1].split()[0]
        self.assertEqual(allow, pinned)
        self.assertNotIn("subnet:", override)
        self.assertNotIn("kiln-edge", override)
        self.assertNotIn("image:", override)
        self.assertNotIn("ports:", override)

    def test_ca_bind_is_read_only_at_create_and_is_not_the_key_directory(self):
        override = OVERRIDE.read_text()
        mount = bind_target(override, CA_TARGET)
        self.assertIsNotNone(mount)
        self.assertEqual(len(bind_blocks(override)), 1)
        self.assertIn("source: ${KILN_CA_CERT:-/etc/kiln/ca.crt}\n", mount)
        self.assertIn("read_only: true\n", mount)
        self.assertIn("create_host_path: false", mount)
        source = mount.split("source:", 1)[1].split("\n", 1)[0].strip()
        self.assertTrue(source.endswith("ca.crt}"))
        self.assertNotEqual(source, "/etc/kiln")
        for secret in ("ca.key", "door.key", "directory.token", "tls.key"):
            self.assertNotIn(secret, override)
        self.assertNotIn("kiln-identity", override)
        self.assertNotIn("8092", override)
        self.assertNotIn("caddy-data", override)
        self.assertNotIn("/etc/caddy/Caddyfile", override)

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
        self.assertIn("docker network inspect paseo_default", runbook)
        self.assertIn("A check that the address is free would always abort.", runbook)
        self.assertIn(REFUSAL, runbook)
        self.assertIn("That needs a separate plan.", runbook)
        self.assertIn("COMPOSE_FILE", runbook)
        self.assertIn("docker compose stop caddy\n", runbook)
        self.assertIn("docker compose rm -f caddy\n", runbook)
        self.assertIn("docker compose up -d --no-deps caddy\n", runbook)
        self.assertNotIn("kiln-door-caddy", runbook)
        self.assertNotIn("caddy-compose.yaml", runbook)


class DoorCaddyRecreate(unittest.TestCase):
    def test_preflight_aborts_when_no_subnet_contains_the_pin(self):
        if os.environ.get("KILN_CADDY_DOCKER") != "1":
            self.skipTest("set KILN_CADDY_DOCKER=1 to recreate the container")
        base = docker_base()
        if base is None:
            self.fail("docker is not available")
        network = "paseo_nosub"
        name = "kiln-nosub-caddy"
        work = Path(tempfile.mkdtemp(prefix="kiln-caddy-nosub-"))
        self.addCleanup(shutil.rmtree, work, True)
        self.addCleanup(self.remove, base, name, network, work)
        subprocess.call(base + ["rm", "-f", name], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        subprocess.call(base + ["network", "rm", network], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        subprocess.check_call(
            base + ["network", "create", "--subnet", "172.19.0.0/16", "--gateway", "172.19.0.1", network],
            stdout=subprocess.DEVNULL,
        )
        compose = work / "compose.yaml"
        compose.write_text(textwrap.dedent(f"""\
            services:
              caddy:
                image: {IMAGE}
                command: ["sleep", "60"]
                container_name: {name}
                restart: unless-stopped
            networks:
              default:
                name: {network}
                external: true
            """))
        subprocess.check_call(
            base + ["compose", "-p", "paseo-nosub", "-f", str(compose), "up", "-d"],
            stdout=subprocess.DEVNULL,
        )
        before = self.state(base, name)
        ok, reason = preflight(base, network)
        print(f"preflight {ok} {reason}", flush=True)
        self.assertFalse(ok)
        self.assertEqual(reason, f"no subnet contains {PIN}")
        after = self.state(base, name)
        self.assertEqual(before, after)
        self.assertEqual(after[1], "running")
        print(f"untouched {before[0][:12]} {after[1]}", flush=True)
        ca = work / "ca.crt"
        ca.write_text("placeholder\n")
        cmd = ["compose", "-p", "paseo-nosub", "-f", str(compose), "-f", str(OVERRIDE),
               "up", "-d", "--force-recreate"]
        env = os.environ.copy()
        env["KILN_CA_CERT"] = str(ca)
        if base[0] == "sudo":
            cmd = ["sudo", "env", f"KILN_CA_CERT={ca}", "docker", *cmd]
        else:
            cmd = [*base, *cmd]
        refused = subprocess.run(cmd, text=True, capture_output=True, env=env)
        print(refused.stderr, flush=True)
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn(REFUSAL, refused.stderr)
        still = self.state(base, name)
        self.assertEqual(still[0], before[0])
        self.assertEqual(still[1], "running")
        print(f"compose-refused still {still[0][:12]} {still[1]}", flush=True)

    def test_recreate_keeps_the_address_the_ca_and_the_401(self):
        if os.environ.get("KILN_CADDY_DOCKER") != "1":
            self.skipTest("set KILN_CADDY_DOCKER=1 to recreate the container")
        base = docker_base()
        if base is None:
            self.fail("docker is not available")
        self.recreate(base)

    def recreate(self, base):
        work = Path(tempfile.mkdtemp(prefix="kiln-caddy-proof-"))
        project = work / "paseo"
        project.mkdir()
        self.addCleanup(shutil.rmtree, work, True)
        self.addCleanup(self.down_paseo, base, project)
        self.lost_copy(base, work)
        subprocess.call(base + ["rm", "-f", "caddy"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.prepare_volumes(base)
        ca, public, public_key, leaf, leaf_key = self.certs(work)
        caddyfile = project / "Caddyfile"
        caddyfile.write_text(textwrap.dedent("""\
            {
            	admin off
            	auto_https off
            }
            http://app.nowcode.com {
            	respond "nowcode-stand-in"
            }
            """))
        (project / "compose.yaml").write_text(textwrap.dedent(f"""\
            services:
              caddy:
                image: {IMAGE}
                container_name: caddy
                restart: unless-stopped
                ports:
                  - "80:80"
                  - "443:443"
                volumes:
                  - type: bind
                    source: ./Caddyfile
                    target: /etc/caddy/Caddyfile
                    read_only: true
                    bind:
                      create_host_path: false
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
                  - paseo_caddy_data:/data
                  - paseo_caddy_config:/config
            volumes:
              paseo_caddy_data:
                name: paseo_caddy_data
                external: true
              paseo_caddy_config:
                name: paseo_caddy_config
                external: true
            """))
        (project / ".env").write_text("COMPOSE_PROJECT_NAME=paseo\n")
        subprocess.call(base + ["rm", "-f", "caddy"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.compose(base, project, ["up", "-d"])
        first = self.container_id(base, project)
        address = self.address(base, first)
        print(f"address-before {address}", flush=True)
        ok, reason = preflight(base, "paseo_default")
        self.assertTrue(ok, reason)
        stranger = subprocess.run(
            base + ["run", "-d", "--name", "kiln-caddy-stranger", "--network", "paseo_default",
                    "--ip", PIN, IMAGE, "sleep", "60"],
            text=True, capture_output=True,
        )
        if stranger.returncode == 0:
            self.addCleanup(
                subprocess.call, base + ["rm", "-f", "kiln-caddy-stranger"],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            )
            blocked, why = preflight(base, "paseo_default")
            print(f"stranger-preflight {blocked} {why}", flush=True)
            self.assertFalse(blocked)
            self.assertEqual(why, f"another container holds {PIN}")
            self.assertEqual(self.state(base, "caddy")[0], first)
            print(f"stranger-left-caddy {first[:12]} running", flush=True)
            subprocess.check_call(base + ["rm", "-f", "kiln-caddy-stranger"], stdout=subprocess.DEVNULL)
        else:
            print(f"stranger-not-started {stranger.stderr.strip()}", flush=True)
        ok, reason = preflight(base, "paseo_default")
        print(f"preflight {ok} {reason}", flush=True)
        self.assertTrue(ok, reason)
        head, rest = DOOR.read_text().split("{", 1)
        caddyfile.write_text(
            "{\n\tadmin off\n\tauto_https off\n}\nhttps://"
            + head
            + "{\n\ttls /etc/caddy/public.crt /etc/caddy/public.key\n"
            + rest
            + "\nhttp://app.nowcode.com {\n\trespond \"nowcode-stand-in\"\n}\n"
        )
        (project / ".env").write_text(
            "COMPOSE_PROJECT_NAME=paseo\n"
            f"COMPOSE_FILE=compose.yaml:{OVERRIDE}\n"
            f"KILN_CA_CERT={ca}\n"
        )
        self.compose(base, project, ["up", "-d", "--force-recreate", "--no-deps", "caddy"])
        pinned = self.container_id(base, project)
        self.assert_container(base, pinned, ca)
        standin = StandIn(leaf, leaf_key)
        standin.start()
        self.addCleanup(standin.stop)
        body = self.curl()
        print(f"through-caddy {body!r}", flush=True)
        self.assertEqual(body, "door-stand-in 401 from 172.18.0.3\n")
        extra = self.curl_extra()
        print(f"extra-site {extra!r}", flush=True)
        self.assertEqual(extra, "nowcode-stand-in")
        denied = self.host_status()
        print(f"other-source {denied[0]} {denied[1]!r}", flush=True)
        self.assertEqual(denied[0], 403)
        self.assertNotIn(f"from {PIN}", denied[1])
        self.assertEqual(self.markers(base, pinned), ("data-marker\n", "config-marker\n"))
        print("markers data-marker config-marker", flush=True)
        rendered = subprocess.check_output(base + ["compose", "config"], cwd=project, text=True)
        self.assertIn(f"ipv4_address: {PIN}", rendered)
        self.assertIn(CA_TARGET, rendered)
        self.compose(base, project, ["up", "-d"])
        plain = self.container_id(base, project)
        self.assert_container(base, plain, ca)
        print(f"plain-up {self.address(base, plain)}", flush=True)
        self.compose(base, project, ["up", "-d", "--force-recreate", "--no-deps", "caddy"])
        again = self.container_id(base, project)
        self.assertNotEqual(pinned, again)
        print(f"recreated {pinned[:12]} then {again[:12]}", flush=True)
        self.assert_container(base, again, ca)
        again_body = self.curl()
        print(f"through-caddy-after {again_body!r}", flush=True)
        self.assertEqual(again_body, "door-stand-in 401 from 172.18.0.3\n")
        self.assertEqual(self.curl_extra(), "nowcode-stand-in")
        self.assertEqual(self.markers(base, again), ("data-marker\n", "config-marker\n"))
        caddyfile.write_text(textwrap.dedent("""\
            {
            	admin off
            	auto_https off
            }
            http://app.nowcode.com {
            	respond "nowcode-stand-in"
            }
            """))
        (project / ".env").write_text("COMPOSE_PROJECT_NAME=paseo\n")
        self.compose(base, project, ["stop", "caddy"])
        self.compose(base, project, ["rm", "-f", "caddy"])
        self.compose(base, project, ["up", "-d", "--no-deps", "caddy"])
        rolled = self.container_id(base, project)
        status = self.state(base, "caddy")
        self.assertEqual(status[1], "running")
        self.assertNotEqual(rolled, again)
        mounts = json.loads(subprocess.check_output(
            base + ["inspect", "-f", "{{json .Mounts}}", rolled], text=True
        ))
        self.assertNotIn(CA_TARGET, [item.get("Destination") for item in mounts])
        rolled_extra = self.curl_extra()
        self.assertEqual(rolled_extra, "nowcode-stand-in")
        self.assertEqual(self.markers(base, rolled), ("data-marker\n", "config-marker\n"))
        print(f"rollback {rolled[:12]} running ca-mount-absent extra {rolled_extra!r}", flush=True)
        adapted = subprocess.check_output(
            base + ["run", "--rm", "-v", f"{DOOR}:/etc/caddy/Caddyfile:ro",
                    IMAGE, "caddy", "adapt", "--config", "/etc/caddy/Caddyfile",
                    "--adapter", "caddyfile"],
            stderr=subprocess.PIPE, text=True,
        )
        self.assertIn('"provider":"file"', adapted)
        self.assertIn(CA_TARGET, adapted)
        self.assertNotIn("deprecated", adapted)

    def certs(self, work):
        ca_key = work / "ca.key"
        ca = work / "ca.crt"
        leaf_key = work / "standin.key"
        leaf = work / "standin.crt"
        public_key = work / "public.key"
        public = work / "public.crt"
        subprocess.check_call(
            ["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2",
             "-subj", "/CN=kiln private CA", "-keyout", str(ca_key), "-out", str(ca)],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        ca_key.chmod(0o600)
        ca.chmod(0o644)
        subprocess.check_call(
            ["openssl", "req", "-new", "-newkey", "rsa:2048", "-nodes",
             "-subj", "/CN=cli.dark-forge.dev", "-keyout", str(leaf_key),
             "-out", str(work / "standin.csr")],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        ext = work / "standin.ext"
        ext.write_text(
            "basicConstraints=critical,CA:FALSE\n"
            "keyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n"
            "subjectAltName=DNS:cli.dark-forge.dev\n"
        )
        subprocess.check_call(
            ["openssl", "x509", "-req", "-in", str(work / "standin.csr"),
             "-CA", str(ca), "-CAkey", str(ca_key), "-set_serial", "1", "-days", "2",
             "-sha256", "-extfile", str(ext), "-out", str(leaf)],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        leaf_key.chmod(0o600)
        subprocess.check_call(
            ["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2",
             "-subj", "/CN=cli.dark-forge.dev", "-addext", "subjectAltName=DNS:cli.dark-forge.dev",
             "-keyout", str(public_key), "-out", str(public)],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        public.chmod(0o644)
        public_key.chmod(0o644)
        return ca, public, public_key, leaf, leaf_key

    def prepare_volumes(self, base):
        for volume in ("paseo_caddy_data", "paseo_caddy_config"):
            subprocess.call(base + ["volume", "rm", volume], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            subprocess.check_call(base + ["volume", "create", volume], stdout=subprocess.DEVNULL)
        subprocess.check_call(
            base + ["run", "--rm", "--entrypoint", "sh",
                    "-v", "paseo_caddy_data:/data", "-v", "paseo_caddy_config:/config",
                    IMAGE, "-c", "echo data-marker >/data/marker && echo config-marker >/config/marker"],
            stdout=subprocess.DEVNULL,
        )

    def compose(self, base, project, args):
        subprocess.check_call(base + ["compose", *args], cwd=project, stdout=subprocess.DEVNULL)

    def lost_copy(self, base, work):
        source = work / "copied.crt"
        source.write_text("copied-ca\n")
        name = "kiln-caddy-copy-loss"
        subprocess.call(base + ["rm", "-f", name], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        subprocess.check_call(
            base + ["run", "-d", "--name", name, IMAGE, "sleep", "30"],
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
            base + ["run", "-d", "--name", name, IMAGE, "sleep", "30"],
            stdout=subprocess.DEVNULL,
        )
        missing = subprocess.run(
            base + ["exec", name, "cat", CA_TARGET], text=True, capture_output=True,
        )
        self.assertNotEqual(missing.returncode, 0)
        subprocess.check_call(base + ["rm", "-f", name], stdout=subprocess.DEVNULL)

    def container_id(self, base, project):
        raw = subprocess.check_output(base + ["compose", "ps", "-q"], cwd=project, text=True).strip()
        self.assertNotEqual(raw, "")
        return raw.splitlines()[0]

    def address(self, base, container):
        return subprocess.check_output(
            base + ["inspect", "-f", "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}", container],
            text=True,
        ).strip()

    def state(self, base, name):
        raw = subprocess.check_output(
            base + ["inspect", "-f", "{{.Id}} {{.State.Status}}", name], text=True
        ).split()
        return raw[0], raw[1]

    def assert_container(self, base, container, ca):
        address = self.address(base, container)
        self.assertEqual(address, PIN)
        print(f"address {address}", flush=True)
        mounts = subprocess.check_output(base + ["exec", container, "cat", "/proc/mounts"], text=True)
        matched = [line for line in mounts.splitlines() if f" {CA_TARGET} " in line]
        self.assertEqual(len(matched), 1, mounts)
        self.assertIn(" ro,", matched[0])
        print(matched[0], flush=True)
        body = subprocess.check_output(base + ["exec", container, "cat", CA_TARGET])
        self.assertEqual(body, ca.read_bytes())
        listing = subprocess.check_output(base + ["exec", container, "ls", "-1", "/etc/kiln"], text=True)
        self.assertEqual(listing.split(), ["ca.crt"])
        wrote = subprocess.run(
            base + ["exec", container, "touch", CA_TARGET], text=True, capture_output=True,
        )
        self.assertNotEqual(wrote.returncode, 0)
        self.assertIn("Read-only file system", wrote.stderr)

    def markers(self, base, container):
        data = subprocess.check_output(base + ["exec", container, "cat", "/data/marker"], text=True)
        config = subprocess.check_output(base + ["exec", container, "cat", "/config/marker"], text=True)
        return data, config

    def curl(self):
        last = ""
        for _ in range(20):
            completed = subprocess.run(
                ["curl", "-sk", "--resolve", "cli.dark-forge.dev:443:127.0.0.1",
                 "--max-time", "5", "https://cli.dark-forge.dev/v1/templates"],
                text=True, capture_output=True,
            )
            if completed.returncode == 0 and completed.stdout.startswith("door-stand-in "):
                return completed.stdout
            last = completed.stderr or completed.stdout
            time.sleep(0.25)
        self.fail(last)

    def curl_extra(self):
        last = ""
        for _ in range(20):
            completed = subprocess.run(
                ["curl", "-s", "--resolve", "app.nowcode.com:80:127.0.0.1",
                 "--max-time", "5", "http://app.nowcode.com/"],
                text=True, capture_output=True,
            )
            if completed.returncode == 0 and completed.stdout:
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
        return int(head.split()[1]), body.decode()

    def down_paseo(self, base, project):
        subprocess.call(
            base + ["compose", "down", "--remove-orphans"],
            cwd=project, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        subprocess.call(base + ["rm", "-f", "caddy", "kiln-caddy-stranger"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for volume in ("paseo_caddy_data", "paseo_caddy_config"):
            subprocess.call(base + ["volume", "rm", volume], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    def remove(self, base, name, network, work):
        compose = work / "compose.yaml"
        if compose.exists():
            subprocess.call(
                base + ["compose", "-p", "paseo-nosub", "-f", str(compose), "down", "--remove-orphans"],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            )
        subprocess.call(base + ["rm", "-f", name], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        subprocess.call(base + ["network", "rm", network], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


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


APP_SITE = """\
app.dark-forge.dev {
	header {
		X-Content-Type-Options "nosniff"
		X-Robots-Tag "noindex, nofollow"
	}
	request_body {
		max_size 80KB
	}
	reverse_proxy 172.18.0.1:8093 {
		header_up X-Forwarded-For {http.request.remote.host}
		transport http {
			dial_timeout 3s
			response_header_timeout 75s
		}
	}
}
"""


class AppSiteRunbook(unittest.TestCase):
    def test_site_is_cleartext_and_the_runbook_can_restore_identity(self):
        site = (ROOT / "deploy/app.Caddyfile").read_text()
        runbook = (ROOT / "docs/runbooks/2026-10-20-app-dark-forge-caddy.md").read_text()
        unit = (ROOT / "deploy/kiln-identity.service").read_text()
        binary = "/usr/local/libexec/kiln-identity"
        database = "/var/lib/kiln-identity/identity.db"
        self.assertEqual(site, APP_SITE)
        self.assertEqual(site.count("reverse_proxy "), 1)
        self.assertNotIn("tls_trust_pool", site)
        self.assertNotIn("8443", site)
        self.assertNotIn("dns-01", site)
        self.assertIn(f"ExecStart={binary} ", unit)
        self.assertIn(f"--database {database} ", unit)
        self.assertIn("--signing-key-file ", unit)
        self.assertNotIn("signing.pem", runbook)
        self.assertNotIn("dns-01", runbook)
        self.assertNotIn(".pem", runbook)
        self.assertNotIn(".key", runbook)
        self.assertIn(binary, runbook)
        self.assertIn(database, runbook)
        self.assertIn("identity.db-wal", runbook)
        self.assertIn("identity.db-shm", runbook)
        yes = "Restart `kiln-identity` only after Jamie says yes."
        self.assertIn(yes, runbook)
        self.assertLess(runbook.index(yes), runbook.index("systemctl start kiln-identity"))
        self.assertLess(
            runbook.index(f"sha256sum {database}"),
            runbook.index("systemctl start kiln-identity"),
        )
        self.assertIn("systemctl stop kiln-identity", runbook)
        self.assertIn("sha256sum -c /var/backups/kiln-identity-app/checksums", runbook)
        self.assertIn(
            "install -m 755 /var/backups/kiln-identity-app/kiln-identity "
            + binary,
            runbook,
        )
        self.assertLess(
            runbook.index("cp -a /opt/paseo/Caddyfile /opt/paseo/Caddyfile.bak-app-dark-forge"),
            runbook.index("cat deploy/app.Caddyfile >> /opt/paseo/Caddyfile"),
        )
        self.assertIn("caddy validate", runbook)
        self.assertIn("caddy reload", runbook)
        self.assertIn("https://dark-forge.dev/login", runbook)
        self.assertIn("Light the way in.", runbook)
        self.assertIn("https://cli.dark-forge.dev/v1/templates", runbook)
        self.assertIn("HTTP 401", runbook)
        self.assertIn("tcp/8443 stays closed.", runbook)
        self.assertIn("Do not add an accept rule.", runbook)
        self.assertNotIn("dport 8443 accept", runbook)
        self.assertIn("127.0.0.1:8093", runbook)
        self.assertIn("172.18.0.1:8093", runbook)
        self.assertIn("The browser check is for Kiln Ops after the release.", runbook)
        self.assertIn("This pull request does not run it.", runbook)
        self.assertNotIn("This procedure does not start `kiln-app.service`.", runbook)
        self.assertNotIn("Stop if that output shows `127.0.0.1:8093`", runbook)
        self.assertIn("cannot install dashboard archive", runbook)
        self.assertIn("cannot restore the dashboard", runbook)
        self.assertIn("The live directory is already back.", runbook)
        self.assertIn("Leave the backup where it is.", runbook)
        self.assertNotIn("the live directory is gone", runbook)
        self.assertIn(
            "cp -a /usr/local/share/kiln/dashboard /var/backups/kiln-dashboard-app/dashboard",
            runbook,
        )
        self.assertLess(
            runbook.index("cp -a /usr/local/share/kiln/dashboard"),
            runbook.index("bash install.sh --apply --dashboard-archive /var/tmp/kiln-dashboard.tar.gz"),
        )
        self.assertLess(
            runbook.index("bash install.sh --apply --dashboard-archive /var/tmp/kiln-dashboard.tar.gz"),
            runbook.index("systemctl restart kiln-app"),
        )
        self.assertLess(
            runbook.index("systemctl restart kiln-app"),
            runbook.index("systemctl enable --now kiln-app-proxy.socket"),
        )
        self.assertLess(
            runbook.index("systemctl is-active kiln-app"),
            runbook.index("systemctl enable --now kiln-app-proxy.socket"),
        )
        self.assertIn("http://127.0.0.1:8093/v1/x", runbook)
        self.assertLess(
            runbook.index("systemctl enable --now kiln-app-proxy.socket"),
            runbook.index("getent ahostsv4 app.dark-forge.dev"),
        )
        self.assertIn("}app.dark-forge.dev {", runbook)
        self.assertNotIn("grep -q '}app.dark-forge.dev {'", runbook)
        self.assertNotIn("if ! grep -qx 'app.dark-forge.dev {' /opt/paseo/Caddyfile; then", runbook)
        self.assertIn('line == "}app.dark-forge.dev {"', runbook)
        self.assertIn('line == "app.dark-forge.dev {"', runbook)
        self.assertIn("If a whole line is `}app.dark-forge.dev {`", runbook)
        self.assertNotIn("already contains `}app.dark-forge.dev {`", runbook)
        dns_block = self._fenced_sh("getent ahostsv4 app.dark-forge.dev")
        self.assertLess(dns_block.index("caddy validate"), dns_block.index("caddy reload"))
        self.assertIn("printf '\\n' >> /opt/paseo/Caddyfile", runbook)
        self.assertIn("awk '{ print $1 }' | sort -u", runbook)
        self.assertIn('[ -z "$apex" ]', runbook)
        self.assertIn("Restored Caddyfile failed validation. Did not reload.", runbook)
        self.assertLess(
            runbook.index("caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile; then"),
            runbook.rindex("caddy reload --config /etc/caddy/Caddyfile --adapter caddyfile"),
        )
        self.assertLess(
            runbook.index("getent ahostsv4 app.dark-forge.dev"),
            runbook.index("docker exec caddy caddy reload"),
        )
        self.assertIn("78.46.98.73", runbook)
        self.assertIn("https://app.dark-forge.dev/", runbook)
        self.assertIn("https://app.dark-forge.dev/v1/x", runbook)
        self.assertIn('"code":"not_found"', runbook)
        self.assertIn("text/html", runbook)
        self.assertLess(
            runbook.index("https://app.dark-forge.dev/"),
            runbook.index('if [ "$rollback" -eq 1 ]; then'),
        )
        self.assertLess(
            runbook.index('if [ "$rollback" -eq 1 ]; then'),
            runbook.rindex("cp -a /opt/paseo/Caddyfile.bak-app-dark-forge /opt/paseo/Caddyfile"),
        )

    def test_append_repairs_a_missing_newline_and_does_not_repeat(self):
        site = ROOT / "deploy/app.Caddyfile"
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            live = root / "Caddyfile"
            live.write_bytes(b'example.test {\n\trespond "ok"\n}')
            broken = root / "broken.Caddyfile"
            broken.write_bytes(live.read_bytes() + site.read_bytes())
            self.assertIn(b"}app.dark-forge.dev {", broken.read_bytes())
            rejected = subprocess.run(
                ["caddy", "adapt", "--config", str(broken), "--adapter", "caddyfile"],
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(rejected.returncode, 0, rejected.stderr)
            self.assertIn("unrecognized directive", rejected.stderr)
            self._append_site(live, site)
            self._append_site(live, site)
            text = live.read_text()
            self.assertEqual(text.count("app.dark-forge.dev {"), 1)
            self.assertNotIn("}app.dark-forge.dev", text)
            self.assertTrue(text.endswith("\n"))
            adapted = subprocess.run(
                ["caddy", "adapt", "--config", str(live), "--adapter", "caddyfile"],
                capture_output=True,
                text=True,
            )
            self.assertEqual(adapted.returncode, 0, adapted.stderr)

    def test_append_repairs_a_glued_site(self):
        site = ROOT / "deploy/app.Caddyfile"
        with tempfile.TemporaryDirectory() as tmp:
            live = Path(tmp) / "Caddyfile"
            live.write_bytes(b'example.test {\n\trespond "ok"\n}' + site.read_bytes())
            self.assertIn(b"}app.dark-forge.dev {", live.read_bytes())
            rejected = subprocess.run(
                ["caddy", "validate", "--config", str(live), "--adapter", "caddyfile"],
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(rejected.returncode, 0, rejected.stderr)
            self._append_site(live, site)
            repaired = live.read_bytes()
            self._append_site(live, site)
            self.assertEqual(live.read_bytes(), repaired)
            text = live.read_text()
            self.assertEqual(text.count("app.dark-forge.dev {"), 1)
            self.assertNotIn("}app.dark-forge.dev", text)
            self.assertNotIn(b"\r", repaired)
            checked = subprocess.run(
                ["caddy", "validate", "--config", str(live), "--adapter", "caddyfile"],
                capture_output=True,
                text=True,
            )
            self.assertEqual(checked.returncode, 0, checked.stderr)

    def test_append_keeps_crlf_comments_and_indentation(self):
        site = (ROOT / "deploy/app.Caddyfile").read_bytes()
        header, _, body = site.partition(b"\n")
        self.assertEqual(header, b"app.dark-forge.dev {")
        body_crlf = body.replace(b"\n", b"\r\n")
        prefix = b'example.test {\r\n\trespond "ok"\r\n'
        glued = prefix + b"}" + header + b"\r\n" + body_crlf
        already = prefix + b"}\r\n" + header + b"\r\n" + body_crlf
        comment = b'example.test {\n\trespond "ok"\n}\n# keep }app.dark-forge.dev { in this note\n'
        indented = b'example.test {\n\trespond "ok"\n  }app.dark-forge.dev {\n}\n'
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            glued_path = root / "glued"
            glued_path.write_bytes(glued)
            self._append_site(glued_path, ROOT / "deploy/app.Caddyfile")
            repaired = glued_path.read_bytes()
            self._append_site(glued_path, ROOT / "deploy/app.Caddyfile")
            self.assertEqual(glued_path.read_bytes(), repaired)
            self.assertNotIn(b"\n", repaired.replace(b"\r\n", b""))
            self.assertTrue(repaired.startswith(prefix))
            self.assertTrue(repaired.endswith(body_crlf))
            self.assertEqual(repaired.count(b"app.dark-forge.dev {"), 1)
            self.assertNotIn(b"}app.dark-forge.dev", repaired)
            checked = subprocess.run(
                ["caddy", "validate", "--config", str(glued_path), "--adapter", "caddyfile"],
                capture_output=True,
                text=True,
            )
            self.assertEqual(checked.returncode, 0, checked.stderr)
            self.assertNotIn("ambiguous", checked.stderr)
            split_path = root / "split"
            split_path.write_bytes(already)
            self._append_site(split_path, ROOT / "deploy/app.Caddyfile")
            self.assertEqual(split_path.read_bytes(), already)
            comment_path = root / "comment"
            comment_path.write_bytes(comment)
            self._append_site(comment_path, ROOT / "deploy/app.Caddyfile")
            commented = comment_path.read_bytes()
            self.assertTrue(commented.startswith(comment))
            self.assertIn(b"# keep }app.dark-forge.dev { in this note\n", commented)
            self.assertEqual(commented.count(b"\napp.dark-forge.dev {"), 1)
            indent_path = root / "indent"
            indent_path.write_bytes(indented)
            self._append_site(indent_path, ROOT / "deploy/app.Caddyfile")
            indented_out = indent_path.read_bytes()
            self.assertTrue(indented_out.startswith(indented))
            self.assertIn(b"\n  }app.dark-forge.dev {\n", indented_out)
            plain = root / "plain"
            plain.write_bytes(b'example.test {\r\n\trespond "ok"\r\n}\r\n')
            self._append_site(plain, ROOT / "deploy/app.Caddyfile")
            appended = plain.read_bytes()
            self._append_site(plain, ROOT / "deploy/app.Caddyfile")
            self.assertEqual(plain.read_bytes(), appended)
            self.assertTrue(appended.startswith(b'example.test {\r\n\trespond "ok"\r\n}\r\n'))
            self.assertNotIn(b"\n", appended.replace(b"\r\n", b""))
            self.assertEqual(appended.count(b"app.dark-forge.dev {"), 1)
            plain_checked = subprocess.run(
                ["caddy", "validate", "--config", str(plain), "--adapter", "caddyfile"],
                capture_output=True,
                text=True,
            )
            self.assertEqual(plain_checked.returncode, 0, plain_checked.stderr)

    def test_ci_installs_caddy_from_one_checked_archive(self):
        workflow = (ROOT / ".github/workflows/build.yml").read_text()
        script_path = ROOT / "scripts/install-caddy.sh"
        self.assertTrue(script_path.is_file())
        script = script_path.read_text()
        self.assertEqual(workflow.count("bash scripts/install-caddy.sh"), 2)
        self.assertNotIn("caddy_2.10.2_linux_amd64.tar.gz", workflow)
        self.assertEqual(script.count("caddy_2.10.2_linux_amd64.tar.gz"), 1)
        self.assertIn("sha512sum -c -", script)
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bindir = root / "bin"
            bindir.mkdir()
            log = root / "sudo.log"
            (bindir / "curl").write_text(
                "#!/bin/sh\n"
                "prev=\n"
                "out=\n"
                "for arg in \"$@\"; do\n"
                "  if [ \"$prev\" = -o ]; then out=$arg; fi\n"
                "  prev=$arg\n"
                "done\n"
                "printf '%s' 'not a tarball' > \"$out\"\n"
            )
            (bindir / "sudo").write_text(
                "#!/bin/sh\n"
                f"printf '%s\\n' \"$*\" >> {log}\n"
            )
            os.chmod(bindir / "curl", 0o755)
            os.chmod(bindir / "sudo", 0o755)
            env = os.environ.copy()
            env["PATH"] = str(bindir) + os.pathsep + env.get("PATH", "")
            completed = subprocess.run(
                ["bash", str(script_path)],
                capture_output=True,
                text=True,
                env=env,
            )
            self.assertNotEqual(completed.returncode, 0, completed.stdout)
            self.assertIn("FAILED", completed.stdout)
            self.assertIn("computed checksum did NOT match", completed.stderr)
            self.assertFalse(log.exists())

    def test_dns_compares_every_address(self):
        same_app = "\n".join([
            "10.0.0.2 STREAM app.dark-forge.dev",
            "10.0.0.2 DGRAM",
            "10.0.0.2 RAW",
            "10.0.0.1 STREAM app.dark-forge.dev",
            "10.0.0.1 DGRAM",
            "10.0.0.1 RAW",
            "",
        ])
        same_apex = "\n".join([
            "10.0.0.1 STREAM dark-forge.dev",
            "10.0.0.1 DGRAM",
            "10.0.0.1 RAW",
            "10.0.0.2 STREAM dark-forge.dev",
            "10.0.0.2 DGRAM",
            "10.0.0.2 RAW",
            "",
        ])
        extra_app = "\n".join([
            "10.0.0.1 STREAM app.dark-forge.dev",
            "10.0.0.1 DGRAM",
            "10.0.0.1 RAW",
            "10.0.0.2 STREAM app.dark-forge.dev",
            "10.0.0.2 DGRAM",
            "10.0.0.2 RAW",
            "",
        ])
        one_apex = "\n".join([
            "10.0.0.1 STREAM dark-forge.dev",
            "10.0.0.1 DGRAM",
            "10.0.0.1 RAW",
            "",
        ])
        cases = (
            ("same set in a different order", same_app, same_apex, False),
            ("first row matches and a later address does not", extra_app, one_apex, True),
            ("app.dark-forge.dev has no address", "", one_apex, True),
            ("dark-forge.dev has no address", same_app, "", True),
        )
        for name, app_rows, apex_rows, restore in cases:
            with self.subTest(name=name):
                restored, reloaded = self._dns_check(app_rows, apex_rows)
                self.assertEqual(restored, restore, name)
                self.assertEqual(reloaded, not restore, name)

    def test_dns_reload_follows_validate_exit(self):
        rows = "\n".join([
            "78.46.98.73 STREAM name",
            "78.46.98.73 DGRAM",
            "78.46.98.73 RAW",
            "",
        ])
        app = rows.replace("name", "app.dark-forge.dev")
        apex = rows.replace("name", "dark-forge.dev")
        restored, reloaded = self._dns_check(app, apex, validate_exit=1)
        self.assertTrue(restored)
        self.assertFalse(reloaded)
        kept, reloaded_ok = self._dns_check(app, apex, validate_exit=0)
        self.assertFalse(kept)
        self.assertTrue(reloaded_ok)

    def test_rollback_reload_follows_validate(self):
        real = shutil.which("caddy")
        self.assertIsNotNone(real, "caddy is not installed")
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            admin_port, http_port = self._two_ports()
            admin = f"127.0.0.1:{admin_port}"
            live = root / "Caddyfile"
            backup = root / "Caddyfile.bak"
            live.write_text(
                "{\n"
                f"\tadmin {admin}\n"
                "\tauto_https off\n"
                "}\n"
                f"http://127.0.0.1:{http_port} {{\n"
                "\tbind 127.0.0.1\n"
                '\trespond "live"\n'
                "}\n"
            )
            home = root / "home"
            data = root / "xdg"
            config = root / "xdg-config"
            for path in (home, data, config):
                path.mkdir()
            env = os.environ.copy()
            env.update({
                "HOME": str(home),
                "XDG_DATA_HOME": str(data),
                "XDG_CONFIG_HOME": str(config),
            })
            server_log = root / "caddy-server.log"
            with server_log.open("w") as handle:
                subprocess.check_call(
                    [real, "start", "--config", str(live), "--adapter", "caddyfile"],
                    env=env,
                    stdout=handle,
                    stderr=handle,
                )
            try:
                self._wait_http(f"http://127.0.0.1:{http_port}/", "live")
                backup.write_text(
                    f"http://127.0.0.1:{http_port} {{\n"
                    "\tnot_a_directive\n"
                    "}\n"
                )
                failed_out, failed_log = self._rollback(root, live, backup, admin, real, env)
                self.assertIn("Restored Caddyfile failed validation. Did not reload.", failed_out)
                self.assertNotIn("reload", failed_log)
                self.assertEqual(self._curl(f"http://127.0.0.1:{http_port}/"), "live")
                backup.write_text(
                    "{\n"
                    f"\tadmin {admin}\n"
                    "\tauto_https off\n"
                    "}\n"
                    f"http://127.0.0.1:{http_port} {{\n"
                    "\tbind 127.0.0.1\n"
                    '\trespond "restored"\n'
                    "}\n"
                )
                reloaded_out, reloaded_log = self._rollback(root, live, backup, admin, real, env)
                self.assertNotIn("Did not reload.", reloaded_out)
                self.assertIn("reload", reloaded_log)
                self.assertEqual(self._curl(f"http://127.0.0.1:{http_port}/"), "restored")
            finally:
                subprocess.call(
                    [real, "stop", "--address", admin],
                    env=env,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                )

    def _fenced_sh(self, marker):
        runbook = (ROOT / "docs/runbooks/2026-10-20-app-dark-forge-caddy.md").read_text()
        cursor = 0
        while True:
            start = runbook.index("```sh\n", cursor)
            end = runbook.index("\n```", start)
            block = runbook[start + len("```sh\n"):end]
            if marker in block:
                return block
            cursor = end + 4

    def _host_paths(self, block):
        block = block.replace("/opt/paseo/Caddyfile.bak-app-dark-forge", '"$backup"')
        return block.replace("/opt/paseo/Caddyfile", '"$caddyfile"')

    def _append_site(self, live, site):
        block = self._host_paths(self._fenced_sh("printf '\\n' >> /opt/paseo/Caddyfile"))
        block = block.replace("deploy/app.Caddyfile", '"$site"')
        completed = subprocess.run(
            ["bash", "-c", "caddyfile=$1\nsite=$2\n" + block, "append-site", str(live), str(site)],
            capture_output=True,
            text=True,
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)

    def _dns_check(self, app_rows, apex_rows, validate_exit=0):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            answers = root / "answers"
            answers.mkdir()
            (answers / "app.dark-forge.dev").write_text(app_rows)
            (answers / "dark-forge.dev").write_text(apex_rows)
            bindir = root / "bin"
            bindir.mkdir()
            log = root / "docker.log"
            (bindir / "getent").write_text("#!/bin/sh\ncat \"$GETENT_DIR/$2\"\n")
            (bindir / "docker").write_text(
                "#!/bin/sh\n"
                "printf '%s\\n' \"$*\" >> \"$DOCKER_LOG\"\n"
                "printf '%s\\n' 'Valid configuration'\n"
                "if [ \"$4\" = validate ]; then\n"
                f"  exit {int(validate_exit)}\n"
                "fi\n"
                "exit 0\n"
            )
            os.chmod(bindir / "getent", 0o755)
            os.chmod(bindir / "docker", 0o755)
            live = root / "Caddyfile"
            backup = root / "Caddyfile.bak"
            live.write_text("edited\n")
            backup.write_text("backup\n")
            block = self._host_paths(self._fenced_sh("getent ahostsv4 app.dark-forge.dev"))
            env = os.environ.copy()
            env["PATH"] = str(bindir) + os.pathsep + env.get("PATH", "")
            env["GETENT_DIR"] = str(answers)
            env["DOCKER_LOG"] = str(log)
            completed = subprocess.run(
                ["bash", "-c", "caddyfile=$1\nbackup=$2\n" + block, "dns", str(live), str(backup)],
                capture_output=True,
                text=True,
                env=env,
            )
            self.assertEqual(completed.returncode, 0, completed.stderr)
            restored = live.read_text() == "backup\n"
            reloaded = "reload" in log.read_text() if log.exists() else False
            return restored, reloaded

    def _rollback(self, root, live, backup, admin, real, env):
        bindir = root / "bin"
        bindir.mkdir(exist_ok=True)
        log = root / "docker.log"
        log.write_text("")
        shim = bindir / "docker"
        shim.write_text(
            "#!/bin/sh\n"
            "printf '%s\\n' \"$*\" >> \"$DOCKER_LOG\"\n"
            "action=$4\n"
            "if [ \"$action\" = validate ]; then\n"
            f"  exec {real} validate --config \"$CADDYFILE\" --adapter caddyfile\n"
            "fi\n"
            "if [ \"$action\" = reload ]; then\n"
            f"  exec {real} reload --config \"$CADDYFILE\" --adapter caddyfile --address \"$CADDY_ADMIN\"\n"
            "fi\n"
            "printf 'unexpected docker %s\\n' \"$*\" >&2\n"
            "exit 99\n"
        )
        os.chmod(shim, 0o755)
        block = self._host_paths(self._fenced_sh('if [ "$rollback" -eq 1 ]; then'))
        block = block[block.index('if [ "$rollback" -eq 1 ]; then'):]
        run_env = env.copy()
        run_env["PATH"] = str(bindir) + os.pathsep + run_env.get("PATH", "")
        run_env["DOCKER_LOG"] = str(log)
        run_env["CADDYFILE"] = str(live)
        run_env["CADDY_ADMIN"] = admin
        completed = subprocess.run(
            ["bash", "-c", "rollback=1\ncaddyfile=$1\nbackup=$2\n" + block, "rollback", str(live), str(backup)],
            capture_output=True,
            text=True,
            env=run_env,
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        return completed.stdout, log.read_text()

    def _two_ports(self):
        admin = socket.socket()
        http = socket.socket()
        try:
            admin.bind(("127.0.0.1", 0))
            http.bind(("127.0.0.1", 0))
            return admin.getsockname()[1], http.getsockname()[1]
        finally:
            admin.close()
            http.close()

    def _wait_http(self, url, body):
        deadline = time.monotonic() + 5
        last = ""
        while time.monotonic() < deadline:
            completed = self._curl_result(url)
            if completed.returncode == 0 and completed.stdout == body:
                return
            last = completed.stderr or completed.stdout
            time.sleep(0.05)
        self.fail(last)

    def _curl(self, url):
        completed = self._curl_result(url)
        self.assertEqual(completed.returncode, 0, completed.stderr)
        return completed.stdout

    def _curl_result(self, url):
        return subprocess.run(
            ["curl", "-fsS", "--max-time", "2", url],
            capture_output=True,
            text=True,
        )


if __name__ == "__main__":
    unittest.main()

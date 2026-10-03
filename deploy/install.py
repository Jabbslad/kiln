#!/usr/bin/env python3
"""Install or update a verified Kiln server release, preserving existing state."""

import argparse
import fcntl
import grp
import hashlib
import ipaddress
import json
import os
import platform
import pwd
import re
import secrets
import shutil
import socket
import sqlite3
import ssl
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.error
import urllib.request
from contextlib import closing, contextmanager
from pathlib import Path

ETC = Path("/etc/kiln")
STATE = Path("/var/lib/kiln")
OPT = Path("/opt/kiln")
LIBEXEC = Path("/usr/local/libexec")
UNITS = Path("/etc/systemd/system")
CGROUP = Path("/sys/fs/cgroup/kiln")
LOCK = Path("/run/lock/kiln-install.lock")
SERVICES = ("kiln-cgroup.service", "kiln-host.service", "kiln-api.service")


def run(*args):
    return subprocess.run(
        [str(arg) for arg in args], check=True, capture_output=True, text=True
    ).stdout


def private_address(value):
    address = ipaddress.IPv4Address(value)
    # is_private also accepts documentation/unspecified ranges, but not CGNAT.
    ranges = (
        "10.0.0.0/8",
        "172.16.0.0/12",
        "192.168.0.0/16",
        "100.64.0.0/10",
        "127.0.0.1/32",
    )
    if not any(address in ipaddress.IPv4Network(network) for network in ranges):
        raise ValueError(
            "use a private/VPN IPv4 address, or 127.0.0.1 for local-only access"
        )
    return str(address)


def require_absent(paths):
    for path in paths:
        if path.exists() or path.is_symlink():
            raise ValueError(
                f"refusing existing destination {path}; this is not an upgrade/recovery tool"
            )


def check_subids(text, base):
    for line in text.splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        _, first, count = line.rsplit(":", 2)
        if int(first) < base + 8 and int(first) + int(count) > base:
            raise ValueError(
                f"reserved range {base}..{base + 7} overlaps subordinate IDs"
            )


def trusted_ancestors(path):
    for parent in [path, *path.parents]:
        if parent.is_symlink():
            raise ValueError(f"symlink destination ancestor: {parent}")
        if parent.exists():
            stat = parent.stat()
            if not parent.is_dir() or stat.st_uid != 0 or stat.st_mode & 0o022:
                raise ValueError(
                    f"destination ancestor must be a root-owned, non-writable directory: {parent}"
                )


def preflight(bundle, address):
    address = private_address(address)
    release = platform.freedesktop_os_release()
    if (
        platform.machine() != "x86_64"
        or release.get("ID") != "ubuntu"
        or release.get("VERSION_ID") not in ("24.04", "26.04")
    ):
        raise ValueError("installer supports Ubuntu 24.04 and 26.04 x86-64 only")
    if not Path("/run/systemd/system").is_dir():
        raise ValueError("a booted systemd host is required (not a container)")
    for tool in (
        "openssl",
        "systemctl",
        "systemd-analyze",
        "groupadd",
        "useradd",
        "curl",
        "bash",
        "tar",
        "sha256sum",
        "install",
        "realpath",
    ):
        if not shutil.which(tool):
            raise ValueError(
                f"missing prerequisite: {tool}; install host packages before retrying"
            )
    # Opening KVM verifies access, without launching a VM.
    with open("/dev/kvm", "r+b", buffering=0):
        pass
    controllers = Path("/sys/fs/cgroup/cgroup.controllers").read_text().split()
    if not {"cpu", "memory", "pids"}.issubset(controllers):
        raise ValueError("cgroup v2 with cpu, memory and pids controllers is required")
    destinations = [
        ETC,
        STATE,
        OPT,
        CGROUP,
        Path("/run/kiln"),
        LIBEXEC / "kiln-host",
        LIBEXEC / "kiln-api",
        *(UNITS / service for service in SERVICES),
        UNITS / "kiln-host.service.d",
        UNITS / "kiln-api.service.d",
    ]
    require_absent(destinations)
    for path in destinations:
        trusted_ancestors(path.parent)
    for parent in [LIBEXEC, *LIBEXEC.parents]:
        if parent.exists() and not parent.stat().st_mode & 0o001:
            raise ValueError(
                f"the unprivileged gateway needs directory traversal through {parent}"
            )
    for service in SERVICES:
        if (
            run("systemctl", "show", "--property=LoadState", "--value", service).strip()
            != "not-found"
        ):
            raise ValueError(f"an existing {service} unit is installed")
    users, groups = pwd.getpwall(), grp.getgrall()
    names = {"kiln-api", *(f"kiln-vm{i}" for i in range(8))}
    if any(user.pw_name in names or 70000 <= user.pw_uid <= 70007 for user in users):
        raise ValueError(
            "kiln account name or UID range 70000..70007 is already allocated"
        )
    if any(
        group.gr_name in names or 71000 <= group.gr_gid <= 71007 for group in groups
    ):
        raise ValueError(
            "kiln group name or GID range 71000..71007 is already allocated"
        )
    for path, base in [(Path("/etc/subuid"), 70000), (Path("/etc/subgid"), 71000)]:
        if path.exists():
            check_subids(path.read_text(), base)
    # Old jail identities can outlive an account or manager process.
    for status in Path("/proc").glob("[0-9]*/status"):
        try:
            for line in status.read_text().splitlines():
                if line.startswith(("Uid:", "Gid:")):
                    base = 70000 if line.startswith("Uid:") else 71000
                    if any(base <= int(value) < base + 8 for value in line.split()[1:]):
                        raise ValueError(
                            "a running process uses the intended jail identity range"
                        )
        except FileNotFoundError:
            pass
    if shutil.disk_usage(STATE.parent).free < 24 * 1024**3:
        raise ValueError(
            "at least 24 GiB free on /var/lib is required for initial copy-backend storage"
        )
    mem = dict(
        line.split(":", 1) for line in Path("/proc/meminfo").read_text().splitlines()
    )
    if int(mem["MemAvailable"].split()[0]) < 6 * 1024**2:
        raise ValueError(
            "at least 6 GiB available host RAM is required for the initial 4 GiB template"
        )
    with socket.socket() as probe:
        probe.bind((address, 8443))  # Must be assigned locally, with an unused port.
    for relative in (
        "bin/kiln-runtime",
        "bin/kiln",
        "bin/kiln-host",
        "bin/kiln-api",
        "bin/kiln-network",
        "fetch-firecracker.sh",
        "deploy/kiln-host.service",
        "deploy/kiln-api.service",
        "image/image.json",
        "image/inputs.json",
    ):
        path = bundle / relative
        if path.is_symlink() or not path.is_file():
            raise ValueError(f"missing regular release input: {relative}")
    manifest = json.loads((bundle / "image/image.json").read_text())
    if (
        manifest.get("boot_mode") != "systemd_warm"
        or manifest.get("architecture") != "x86_64"
    ):
        raise ValueError("release must contain the per-box-identity warm x86-64 image")
    for kind, name in (("kernel", "vmlinux"), ("rootfs", "rootfs.ext4")):
        path = bundle / "image" / name
        if (
            manifest.get(f"{kind}_path") != name
            or path.is_symlink()
            or not path.is_file()
        ):
            raise ValueError(f"invalid release image path: {name}")
        with path.open("rb") as stream:
            if hashlib.file_digest(stream, "sha256").hexdigest() != manifest.get(
                f"{kind}_sha256"
            ):
                raise ValueError(f"release image checksum mismatch: {name}")


def write(path, text, mode=0o600):
    # All generated outputs are new. A partial install must be reviewed, not overwritten.
    with path.open("x") as stream:
        stream.write(text)
    path.chmod(mode)


def network_preflight(uplink):
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.:-]{0,14}", uplink):
        raise ValueError("network uplink must be a Linux interface name")
    for tool in ("ip", "nft", "sysctl", "nsenter"):
        if not shutil.which(tool):
            raise ValueError(
                "networking requires iproute2, nftables and util-linux packages"
            )
    run("ip", "link", "show", "dev", uplink)
    require_absent([Path("/sys/class/net/kiln0"), UNITS / "kiln-network.service"])


def write_config(directory, template_id=None, network=False):
    write(
        directory / "isolation.json",
        json.dumps(
            {
                "firecracker": "/opt/kiln/bin/firecracker",
                "jailer": "/opt/kiln/bin/jailer",
                "cgroup_parent": "kiln",
                "uid_base": 70000,
                "gid_base": 71000,
                "disk_backend": "copy",
                **(
                    {
                        "network": {
                            "helper": "/opt/kiln/bin/kiln-network",
                            "namespace_scope": "kiln",
                            "resolver": "1.1.1.1",
                        }
                    }
                    if network
                    else {}
                ),
            },
            indent=2,
        )
        + "\n",
    )
    write(
        directory / "host.json",
        json.dumps(
            {
                "runtime_dir": "/var/lib/kiln/runtime",
                "journal_dir": "/var/lib/kiln/journal",
                "socket": "/run/kiln/host.sock",
                "isolation_config": "/etc/kiln/isolation.json",
                "allow_unsafe_development": False,
                "templates": {"ubuntu-4g": template_id} if template_id else {},
            },
            indent=2,
        )
        + "\n",
    )


def credentials(directory, uid, gid, address):
    private_address(address)
    # A private CA signs a server leaf. The signing key never leaves this host.
    run(
        "openssl",
        "req",
        "-x509",
        "-newkey",
        "rsa:3072",
        "-nodes",
        "-days",
        "3650",
        "-subj",
        "/CN=kiln private CA",
        "-addext",
        "basicConstraints=critical,CA:TRUE,pathlen:0",
        "-addext",
        "keyUsage=critical,keyCertSign,cRLSign",
        "-keyout",
        directory / "ca.key",
        "-out",
        directory / "ca.crt",
    )
    run(
        "openssl",
        "req",
        "-new",
        "-newkey",
        "rsa:2048",
        "-nodes",
        "-subj",
        f"/CN={address}",
        "-keyout",
        directory / "tls.key",
        "-out",
        directory / "tls.csr",
    )
    extensions = directory / "tls.ext"
    write(
        extensions,
        f"basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=IP:{address}\n",
    )
    run(
        "openssl",
        "x509",
        "-req",
        "-in",
        directory / "tls.csr",
        "-CA",
        directory / "ca.crt",
        "-CAkey",
        directory / "ca.key",
        "-set_serial",
        str(secrets.randbits(128) or 1),
        "-days",
        "365",
        "-sha256",
        "-extfile",
        extensions,
        "-out",
        directory / "tls.crt",
    )
    write(directory / "admin.token", secrets.token_hex(32) + "\n")
    for name in ("ca.key", "tls.key", "admin.token"):
        (directory / name).chmod(0o600)
    for name in ("tls.key", "tls.crt", "admin.token"):
        os.chown(directory / name, uid, gid)
    for name in ("ca.crt", "tls.crt"):
        (directory / name).chmod(0o644)
    (directory / "tls.csr").unlink()
    extensions.unlink()
    write(
        directory / "CONNECT.txt",
        (
            "This bundle grants administrator access. Store it privately; never upload it to GitHub.\n"
            "Run from this extracted directory on your laptop with kiln on PATH:\n\n"
            f"kiln profile add default --url https://{address}:8443 --token-file admin.token --ca-file ca.crt\n"
            "kiln templates\nkiln create --template ubuntu-4g --name first-box\n\n"
            "Keep this directory: the profile stores file references, not copies.\n"
            "Unix: chmod 700 .; chmod 600 admin.token\n"
            "Windows: restrict this directory and token's ACL to your own account.\n"
            "The server leaf certificate expires in one year; arrange renewal before expiry.\n"
        ),
    )
    bundle = directory / "laptop.tar.gz"
    with (
        bundle.open("xb") as stream,
        tarfile.open(fileobj=stream, mode="w:gz") as archive,
    ):
        for name in ("admin.token", "ca.crt", "CONNECT.txt"):
            info = archive.gettarinfo(str(directory / name), f"kiln-connection/{name}")
            info.uid = info.gid = 0
            info.uname = info.gname = ""
            info.mode = 0o600
            with (directory / name).open("rb") as source:
                archive.addfile(info, source)
    bundle.chmod(0o600)


def write_units(directory, address):
    private_address(address)
    write(
        directory / "kiln-cgroup.service",
        """[Unit]
Description=Prepare kiln cgroup v2 controllers
After=local-fs.target
Before=kiln-host.service

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/bin/sh -ec 'printf "+cpu +memory +pids\\n" > /sys/fs/cgroup/cgroup.subtree_control; mkdir -p /sys/fs/cgroup/kiln; printf "+cpu +memory +pids\\n" > /sys/fs/cgroup/kiln/cgroup.subtree_control'

[Install]
WantedBy=multi-user.target
""",
        0o644,
    )
    for service in ("kiln-host", "kiln-api"):
        (directory / f"{service}.service.d").mkdir(mode=0o755)
    write(
        directory / "kiln-host.service.d/installer.conf",
        "[Unit]\nRequires=kiln-cgroup.service\nAfter=kiln-cgroup.service\n",
        0o644,
    )
    write(
        directory / "kiln-api.service.d/installer.conf",
        (
            "[Service]\nExecStart=\n"
            f"ExecStart=/usr/local/libexec/kiln-api --listen {address}:8443 "
            "--host-socket /run/kiln/host.sock --token-file /etc/kiln/admin.token "
            "--tls-cert /etc/kiln/tls.crt --tls-key /etc/kiln/tls.key\n"
        ),
        0o644,
    )


def healthcheck(address, expected=None):
    context = ssl.create_default_context(cafile=str(ETC / "ca.crt"))
    opener = urllib.request.build_opener(
        urllib.request.ProxyHandler({}), urllib.request.HTTPSHandler(context=context)
    )
    request = urllib.request.Request(
        f"https://{address}:8443/v1/templates",
        headers={
            "Authorization": "Bearer " + (ETC / "admin.token").read_text().strip(),
        },
    )
    for _ in range(30):
        try:
            with opener.open(request, timeout=2) as response:
                catalog = json.load(response)
            if not isinstance(catalog, list) or (
                expected is not None and catalog != expected
            ):
                raise ValueError(
                    "installed service returned an unexpected template catalog"
                )
            return catalog
        except (urllib.error.URLError, TimeoutError):
            time.sleep(1)
    raise ValueError(
        "HTTPS readiness failed; inspect journalctl -u kiln-host -u kiln-api"
    )


def install(bundle, address, network_uplink=None):
    # preflight has already rejected all existing destinations. No automatic rollback
    # of accounts/disks/VMs: retain evidence if template preparation fails partway.
    os.umask(0o077)
    ETC.mkdir(mode=0o755)
    ETC.chmod(0o755)
    STATE.mkdir(mode=0o700)
    OPT.mkdir(mode=0o755)
    OPT.chmod(0o755)
    (OPT / "bin").mkdir(mode=0o755)
    (OPT / "bin").chmod(0o755)
    if not LIBEXEC.exists():
        LIBEXEC.mkdir(mode=0o755)
        LIBEXEC.chmod(0o755)
    for name in ("kiln-runtime", "kiln", "kiln-network"):
        run(
            "install",
            "-o",
            "root",
            "-g",
            "root",
            "-m",
            "0755",
            bundle / "bin" / name,
            OPT / "bin" / name,
        )
    for name in ("kiln-host", "kiln-api"):
        run(
            "install",
            "-o",
            "root",
            "-g",
            "root",
            "-m",
            "0755",
            bundle / "bin" / name,
            LIBEXEC / name,
        )
    print("Downloading checksum-pinned Firecracker 1.17.0…", flush=True)
    run("bash", bundle / "fetch-firecracker.sh", OPT / "bin")
    run("groupadd", "--system", "kiln-api")
    run(
        "useradd",
        "--system",
        "--gid",
        "kiln-api",
        "--no-create-home",
        "--home-dir",
        "/nonexistent",
        "--shell",
        "/usr/sbin/nologin",
        "kiln-api",
    )
    for slot in range(8):
        name = f"kiln-vm{slot}"
        run("groupadd", "--gid", str(71000 + slot), name)
        run(
            "useradd",
            "--uid",
            str(70000 + slot),
            "--gid",
            name,
            "--no-create-home",
            "--no-log-init",
            "--home-dir",
            "/nonexistent",
            "--shell",
            "/usr/sbin/nologin",
            "--password",
            "!",
            name,
        )
    account = pwd.getpwnam("kiln-api")
    credentials(ETC, account.pw_uid, account.pw_gid, address)
    image = STATE / "image"
    image.mkdir(mode=0o700)
    for name in ("vmlinux", "rootfs.ext4", "image.json", "inputs.json"):
        run(
            "install",
            "-o",
            "root",
            "-g",
            "root",
            "-m",
            "0444",
            bundle / "image" / name,
            image / name,
        )
    write_config(ETC, network=network_uplink is not None)
    services = SERVICES
    if network_uplink is not None:
        # Uplink was validated before INSTALL. This is an explicit fresh-host
        # routing/firewall change, never a side effect of ordinary runtime use.
        write(
            UNITS / "kiln-network.service",
            (
                "[Unit]\nDescription=kiln internet egress policy\n"
                "Wants=network-online.target\nAfter=network-online.target\n"
                "Before=kiln-host.service\n\n[Service]\nType=oneshot\nRemainAfterExit=yes\n"
                f"ExecStart=/opt/kiln/bin/kiln-network provision {network_uplink}\n"
                "\n[Install]\nWantedBy=multi-user.target\n"
            ),
            0o644,
        )
        services = ("kiln-network.service", *SERVICES)
    for service in ("kiln-host.service", "kiln-api.service"):
        run(
            "install",
            "-o",
            "root",
            "-g",
            "root",
            "-m",
            "0644",
            bundle / "deploy" / service,
            UNITS / service,
        )
    write_units(UNITS, address)
    if network_uplink is not None:
        write(
            UNITS / "kiln-host.service.d/network.conf",
            "[Unit]\nRequires=kiln-network.service\nAfter=kiln-network.service\n",
            0o644,
        )
    run("systemd-analyze", "verify", *(UNITS / service for service in services))
    run("systemctl", "daemon-reload")
    if network_uplink is not None:
        run("systemctl", "start", "kiln-network.service")
    run("systemctl", "start", "kiln-cgroup.service")
    os.environ["PATH"] = "/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin"
    local = [
        OPT / "bin/kiln-runtime",
        "--state-dir",
        STATE / "runtime",
        "--isolation-config",
        ETC / "isolation.json",
    ]
    run(*local, "doctor")
    print("Preparing the host-specific 4 GiB warm template…", flush=True)
    template = json.loads(
        run(
            *local,
            "template",
            "build",
            "--image",
            image / "image.json",
            "--memory-mib",
            "4096",
            "--vcpus",
            "1",
            "--profile",
            "isolated",
        )
    )
    if not re.fullmatch(r"[a-f0-9]{32}", template.get("id", "")):
        raise ValueError("template build returned an invalid ID")
    config = json.loads((ETC / "host.json").read_text())
    config["templates"] = {"ubuntu-4g": template["id"]}
    (ETC / "host.json").write_text(json.dumps(config, indent=2) + "\n")
    run(LIBEXEC / "kiln-host", "--config", ETC / "host.json", "--check")
    # A failed health check must not leave services enabled at the next reboot.
    try:
        run("systemctl", "enable", "--now", *services)
        healthcheck(address, [{"name": "ubuntu-4g", "memory_mib": 4096, "vcpus": 1}])
    except Exception:
        for service in reversed(services):
            subprocess.run(
                ["systemctl", "disable", "--now", service],
                check=False,
                capture_output=True,
            )
        raise
    print(
        f"Ready: https://{address}:8443\n"
        "Keep /etc/kiln/laptop.tar.gz private for direct administrator recovery.\n"
        "TLS renewal is due within one year."
    )
    connection_instructions(address)


def connection_instructions(address):
    if (ETC / "enrollment.pending.json").exists():
        print(f"Resume server enrollment: sudo {LIBEXEC}/kiln-api enroll --resume")
    elif (ETC / "identity.json").exists():
        print("Server enrollment preserved. On your laptop, run: kiln login")
    else:
        print(
            "One-time browser enrollment (does not restart guest VMs):\n"
            f"  sudo {LIBEXEC}/kiln-api enroll --url https://{address}:8443\n"
            "Approve using GitHub or Google, then run kiln login on your laptop using the same provider.\n"
            "Installation never enrolls a server or opens a browser automatically."
        )


@contextmanager
def installer_lock():
    # Keep the inode: unlinking a flock file allows two simultaneous owners.
    fd = os.open(LOCK, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        info = os.fstat(fd)
        if (
            not stat.S_ISREG(info.st_mode)
            or info.st_uid != os.geteuid()
            or info.st_mode & 0o022
        ):
            raise ValueError("unsafe installer lock")
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise ValueError("another installer is running") from None
        yield
    finally:
        os.close(fd)


def sync_directory(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def atomic_copy(source, destination):
    fd, temporary = tempfile.mkstemp(prefix=".kiln-update-", dir=destination.parent)
    try:
        with os.fdopen(fd, "wb") as output, source.open("rb") as stream:
            shutil.copyfileobj(stream, output)
            output.flush()
            os.fchmod(output.fileno(), 0o755)
            os.fsync(output.fileno())
        os.replace(temporary, destination)
        sync_directory(destination.parent)
    finally:
        Path(temporary).unlink(missing_ok=True)


def regular_file(path, executable=False):
    if path.is_symlink() or not path.is_file():
        raise ValueError(
            f"missing regular file: {path}; partial installs require recovery"
        )
    if executable:
        info = path.stat()
        if (
            info.st_uid != os.geteuid()
            or info.st_mode & 0o022
            or not info.st_mode & 0o111
        ):
            raise ValueError(f"unsafe executable: {path}")


def service_property(service, name):
    return run("systemctl", "show", f"--property={name}", "--value", service).strip()


def service_arguments(service, executable):
    # Only installer-shaped units are supported. Never interpret shell commands
    # or guess how a custom wrapper starts the service.
    value = service_property(service, "ExecStart")
    match = re.fullmatch(r"\{ path=([^;]+) ; argv\[\]=([^;]+) ; [^{}]*\}", value)
    if not match or match[1] != str(executable):
        raise ValueError(f"unsupported ExecStart for {service}")
    return match[2].split()


def wait_for_operations(journal):
    # Stop the gateway first, then let accepted lifecycle/exec operations finish.
    # Local operator commands must not run concurrently with an update.
    regular_file(journal)
    with closing(sqlite3.connect(journal.as_uri() + "?mode=ro", uri=True)) as db:
        if db.execute("PRAGMA user_version").fetchone()[0] != 1:
            raise ValueError("unsupported operation journal version")
        for _ in range(60):
            if (
                db.execute(
                    "SELECT count(*) FROM operations WHERE running=1"
                ).fetchone()[0]
                == 0
            ):
                return
            time.sleep(1)
    raise ValueError("operations are still running; retry once they have completed")


def upgrade(bundle, address=None, network_uplink=None, network=False, apply=False):
    pending = OPT / "pending-upgrade.json"
    if pending.exists() or pending.is_symlink():
        raise ValueError(f"pending update requires recovery: inspect {pending}")
    for directory in (ETC, STATE, OPT, OPT / "bin", LIBEXEC):
        if not directory.is_dir():
            raise ValueError(f"incomplete installation: {directory}; recovery required")
        trusted_ancestors(directory)
    destinations = {
        name: OPT / "bin" / name for name in ("kiln", "kiln-runtime", "kiln-network")
    }
    destinations.update({name: LIBEXEC / name for name in ("kiln-host", "kiln-api")})
    release = json.loads((bundle / "release.json").read_text())
    target = release.get("version", "").removeprefix("v")
    # Binary-only compatibility was reviewed: runtime, journal, guest protocol,
    # Firecracker and unit contracts are unchanged across these releases.
    if target != "0.4.0" or release.get("target") != "x86_64-unknown-linux-gnu":
        raise ValueError("unsupported update package")
    versions = set()
    for name, destination in destinations.items():
        regular_file(destination, executable=True)
        regular_file(bundle / "bin" / name)
        if name == "kiln-network":  # Fixed-purpose shell helper has no --version.
            continue
        reported_name = "kiln-server" if name in ("kiln-host", "kiln-api") else name
        found = run(destination, "--version").strip()
        match = re.fullmatch(re.escape(reported_name) + r" (\d+\.\d+\.\d+)", found)
        if not match:
            raise ValueError(f"unrecognized installed binary: {name}")
        versions.add(match[1])
        if (
            run(bundle / "bin" / name, "--version").strip()
            != f"{reported_name} {target}"
        ):
            raise ValueError(f"package binary version mismatch: {name}")
    if len(versions) != 1 or not versions <= {"0.3.0", "0.3.2", "0.3.3", target}:
        raise ValueError(
            "incompatible or mixed installed versions; no downgrade or state migration is supported"
        )
    for service in ("kiln-host.service", "kiln-api.service"):
        if service_property(service, "ActiveState") != "active":
            raise ValueError(f"{service} must be healthy and active before updating")
    if service_property("kiln-host.service", "KillMode") != "process":
        raise ValueError("kiln-host KillMode must be process to preserve guest VMMs")
    if service_arguments("kiln-host.service", LIBEXEC / "kiln-host") != [
        str(LIBEXEC / "kiln-host"),
        "--config",
        str(ETC / "host.json"),
    ]:
        raise ValueError("unsupported host service configuration")
    regular_file(ETC / "host.json")
    config = json.loads((ETC / "host.json").read_text())
    for key in ("runtime_dir", "journal_dir"):
        if config.get(key) != str(STATE / key.removesuffix("_dir")):
            raise ValueError("updates require standard runtime and journal directories")
        if not Path(config[key]).is_dir():
            raise ValueError(f"missing installed {key}")
    if (
        config.get("isolation_config") != str(ETC / "isolation.json")
        or config.get("socket") != "/run/kiln/host.sock"
    ):
        raise ValueError("unsupported host paths")
    api = service_arguments("kiln-api.service", LIBEXEC / "kiln-api")
    if len(api) != 11 or api[1] != "--listen" or not api[2].endswith(":8443"):
        raise ValueError("unsupported API service configuration")
    installed_address = private_address(api[2].removesuffix(":8443"))
    if api != [
        str(LIBEXEC / "kiln-api"),
        "--listen",
        f"{installed_address}:8443",
        "--host-socket",
        config["socket"],
        "--token-file",
        str(ETC / "admin.token"),
        "--tls-cert",
        str(ETC / "tls.crt"),
        "--tls-key",
        str(ETC / "tls.key"),
    ]:
        raise ValueError("unsupported API service configuration")
    if address is not None and private_address(address) != installed_address:
        raise ValueError("update preserves the existing address and TLS identity")
    regular_file(ETC / "isolation.json")
    policy = json.loads((ETC / "isolation.json").read_text())
    if network or network_uplink is not None:
        if not policy.get("network"):
            raise ValueError("update cannot enable networking in an existing store")
        if network_uplink is not None and service_arguments(
            "kiln-network.service", OPT / "bin/kiln-network"
        ) != [str(OPT / "bin/kiln-network"), "provision", network_uplink]:
            raise ValueError("update cannot change the installed network uplink")
    catalog = healthcheck(installed_address)
    if all(
        path.read_bytes() == (bundle / "bin" / name).read_bytes()
        for name, path in destinations.items()
    ):
        print(f"Kiln {target} is already current; nothing changed.")
        connection_instructions(installed_address)
        return
    print(
        f"Kiln {next(iter(versions))} → {target}: preserve configuration and VM state; briefly disconnect API/SSH sessions.",
        flush=True,
    )
    if not apply:
        print("Check only: rerun with sudo and --apply to update.")
        return
    backups = OPT / "upgrades"
    trusted_ancestors(backups)
    backups.mkdir(mode=0o700, exist_ok=True)
    backup = Path(tempfile.mkdtemp(prefix=f"{next(iter(versions))}-", dir=backups))
    for name, destination in destinations.items():
        atomic_copy(destination, backup / name)
    sync_directory(backups)
    # Write recovery information durably before stopping or replacing anything.
    with pending.open("x") as stream:
        os.fchmod(stream.fileno(), 0o600)
        json.dump(
            {
                "from": next(iter(versions)),
                "to": target,
                "backup": str(backup),
                "destinations": {
                    name: str(path) for name, path in destinations.items()
                },
            },
            stream,
        )
        stream.flush()
        os.fsync(stream.fileno())
    sync_directory(OPT)
    host_stopped = False
    try:
        run("systemctl", "stop", "kiln-api.service")
        wait_for_operations(STATE / "journal/operations.sqlite")
        host_stopped = True
        run("systemctl", "stop", "kiln-host.service")
        for name, destination in destinations.items():
            atomic_copy(bundle / "bin" / name, destination)
        run("systemctl", "start", "kiln-host.service")
        run("systemctl", "start", "kiln-api.service")
        healthcheck(installed_address, catalog)
    except Exception as error:
        try:
            if host_stopped:
                run("systemctl", "stop", "kiln-api.service")
                run("systemctl", "stop", "kiln-host.service")
                for name, destination in destinations.items():
                    atomic_copy(backup / name, destination)
                run("systemctl", "start", "kiln-host.service")
            run("systemctl", "start", "kiln-api.service")
            healthcheck(installed_address, catalog)
        except Exception:
            raise ValueError(
                f"update and rollback failed; manual recovery required using {pending} and {backup}"
            ) from None
        pending.unlink()
        sync_directory(OPT)
        raise ValueError(
            f"update failed ({type(error).__name__}); previous binaries/services restored. Backup: {backup}"
        ) from None
    pending.unlink()
    sync_directory(OPT)
    print(
        f"Updated to Kiln {target}. Ready: https://{installed_address}:8443\nPrevious binaries: {backup}"
    )
    connection_instructions(installed_address)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--address",
        help="assigned private/VPN IPv4; default local-only on fresh install, preserved on update",
    )
    parser.add_argument(
        "--apply",
        action="store_true",
        help="authorize unattended installation after preflight (no confirmation prompt)",
    )
    parser.add_argument(
        "--network-uplink",
        help="opt in to isolated internet egress via this host interface",
    )
    parser.add_argument(
        "--network",
        action="store_true",
        help="require existing guest networking when updating",
    )
    args = parser.parse_args()
    os.environ["PATH"] = "/usr/sbin:/usr/bin:/sbin:/bin"
    try:
        if args.apply and os.geteuid() != 0:
            raise ValueError("--apply requires sudo/root")
        with installer_lock():
            return provision(args)
    except (
        OSError,
        ValueError,
        sqlite3.Error,
        subprocess.CalledProcessError,
        EOFError,
    ) as error:
        # Do not echo command output: future tools could include credentials in it.
        print(
            f"Setup stopped: {error}\nRetain state and inspect logs; do not delete VM state or blindly retry an interrupted update.",
            file=sys.stderr,
        )
        return 1


def provision(args):
    bundle = Path(__file__).resolve().parent
    if any(path.exists() or path.is_symlink() for path in (ETC, OPT, STATE)):
        upgrade(bundle, args.address, args.network_uplink, args.network, args.apply)
        return 0
    if args.network:
        raise ValueError("fresh manual installs need --network-uplink INTERFACE")
    address = private_address(args.address or "127.0.0.1")
    preflight(bundle, address)
    if args.network_uplink is not None:
        network_preflight(args.network_uplink)
    print(
        f"Checks passed. Proposed installation:\n"
        f"  Ubuntu / KVM, endpoint https://{address}:8443\n"
        "  /opt/kiln, /usr/local/libexec/kiln-*, /etc/kiln, /var/lib/kiln\n"
        "  kiln-api and eight locked VM accounts; UID 70000..70007 / GID 71000..71007\n"
        "  Enable cpu/memory/pids cgroup controllers and three systemd services\n"
        "  Generate private CA, one-year server certificate and administrator token\n"
        "  Build a 4 GiB / 1-vCPU warm Ubuntu template using the copy disk backend\n"
        "  No overlay-pool changes\n"
        "Review any other runtime's numeric ID reservations before proceeding."
    )
    print(
        f"  Enable IPv4 forwarding, filtered guest NAT via {args.network_uplink} and a network service."
        if args.network_uplink is not None
        else "  No firewall, routing or guest-network changes."
    )
    if not args.apply:
        print(
            "Check only: no installation performed. Rerun with sudo and --apply to install."
        )
        return 0
    if os.geteuid() != 0:
        raise ValueError("--apply requires sudo/root")
    install(bundle, address, args.network_uplink)
    return 0


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""Destructive tests ONLY inside disposable, trusted, unjailed KVM guests.

Usage: python3 scripts/test-warm-guest.py IMAGE_JSON FIRECRACKER OUTPUT_DIR
Requires KVM access, cp and debugfs. No sudo, networking, or production state.
"""
import concurrent.futures
import json
import os
from pathlib import Path
import socket
import struct
import subprocess
import sys
import tempfile
import time


def request(path, value):
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(10)
        connection.connect(str(path))
        connection.sendall(b"CONNECT 1024\n")
        stream = connection.makefile("rb")
        handshake = stream.readline()
        if not handshake.startswith(b"OK "):
            raise ConnectionError(f"vsock listener unavailable: {handshake!r}")
        payload = json.dumps(value).encode()
        connection.sendall(struct.pack(">I", len(payload)) + payload)
        header = stream.read(4)
        if len(header) != 4:
            raise ConnectionError("bootstrap listener handed off")
        size, = struct.unpack(">I", header)
        assert size <= 1024 * 1024
        return json.loads(stream.read(size))


def wait_hello(path, initialized):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        try:
            reply = request(path, dict(type="hello", version=1))
            if reply == dict(type="hello", version=1, initialized=initialized):
                return
        except (OSError, ConnectionError, AssertionError):
            pass
        time.sleep(.02)
    raise AssertionError(f"guest never reached initialized={initialized}")


def debugfs(disk, command):
    result = subprocess.run(["debugfs", "-w", "-R", command, str(disk)],
                            check=True, capture_output=True, text=True)
    # debugfs sometimes exits 0 on errors. Validate fixtures at guest boot too.
    if any(error in result.stderr for error in ["File not found", "Could not", "not found by"]):
        raise AssertionError(result.stderr)


def inject(disk, scratch, guest_path, contents, executable=False):
    source = scratch / "injected"
    source.write_text(contents)
    debugfs(disk, f"write {source} {guest_path}")
    if executable:
        debugfs(disk, f"set_inode_field {guest_path} mode 0100755")


def check_case(image_path, firecracker, output, case):
    image = json.loads(image_path.read_text())
    assert image["boot_mode"] in ("systemd_warm", "systemd_warm_shared")
    shared = image["boot_mode"] == "systemd_warm_shared"
    with tempfile.TemporaryDirectory(prefix="boxd-warm-") as directory:
        scratch = Path(directory)
        disk = scratch / "disk.ext4"
        subprocess.run(["cp", "--reflink=auto", "--sparse=always",
                        str(image_path.parent / image["rootfs_path"]), str(disk)], check=True)
        if case == "success":
            real = scratch / "systemctl"
            debugfs(disk, f"dump /usr/bin/systemctl {real}")
            debugfs(disk, f"write {real} /usr/bin/systemctl.real")
            debugfs(disk, "set_inode_field /usr/bin/systemctl.real mode 0100755")
            debugfs(disk, "rm /usr/bin/systemctl")
            inject(disk, scratch, "/usr/bin/systemctl", "#!/bin/sh\n"
                   'case "$1" in daemon-reexec|daemon-reload) echo "$1" >> /run/test-manager-refresh ;; esac\n'
                   'exec /usr/bin/systemctl.real "$@"\n', True)
        if case in ("fdstore", "credentials"):
            debugfs(disk, "mkdir /etc/systemd/system/systemd-sysctl.service.d")
            if case == "fdstore":
                inject(disk, scratch, "/fdstore.py", """import array, os, socket
fd = os.memfd_create('synthetic-test-secret')
os.write(fd, b'test-only-not-a-real-secret')
address = os.environ['NOTIFY_SOCKET'].replace('@', '\\0', 1)
s = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
s.connect(address)
s.sendmsg([b'FDSTORE=1\\nFDNAME=synthetic'], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array('i', [fd]))])
""")
                settings = "ExecStart=\nExecStart=/usr/bin/python3 /fdstore.py\nNotifyAccess=all\nFileDescriptorStoreMax=1\nFileDescriptorStorePreserve=yes\n"
            else:
                settings = "SetCredential=boxd-probe:synthetic-test-only\n"
            inject(disk, scratch, "/etc/systemd/system/systemd-sysctl.service.d/probe.conf",
                   "[Service]\n" + settings)
        if case in ("reset-failure", "marker-failure"):
            real = scratch / "busctl"
            debugfs(disk, f"dump /usr/bin/busctl {real}")
            debugfs(disk, f"write {real} /usr/bin/busctl.real")
            debugfs(disk, "set_inode_field /usr/bin/busctl.real mode 0100755")
            debugfs(disk, "rm /usr/bin/busctl")
            if case == "reset-failure":
                hook = 'if [ "$1" = call ]; then echo injected-reset-failure >&2; exit 42; fi\n'
            else:
                hook = 'if [ "$1" = call ] && [ "$2" = org.freedesktop.DBus ]; then mkdir /run/boxd-initialized; fi\n'
            inject(disk, scratch, "/usr/bin/busctl", "#!/bin/sh\n" + hook + 'exec /usr/bin/busctl.real "$@"\n', True)
        vsock = scratch / "vsock"
        config = scratch / "config.json"
        config.write_text(json.dumps({
            "boot-source": {"kernel_image_path": str(image_path.parent / image["kernel_path"]),
                            "boot_args": "console=ttyS0 reboot=k panic=1 pci=off root=/dev/vda rw init=/sbin/box-init boxd.warm=1 initcall_blacklist=load_umh quiet" + (" boxd.retain_pid1=1" if shared else "")},
            "machine-config": {"vcpu_count": 1, "mem_size_mib": 256},
            "drives": [{"drive_id": "rootfs", "path_on_host": str(disk), "is_root_device": True, "is_read_only": False}],
            "vsock": {"guest_cid": 3, "uds_path": str(vsock)},
        }))
        log = output / f"{case}.log"
        with log.open("w") as console:
            process = subprocess.Popen([str(firecracker), "--no-api", "--config-file", str(config)],
                                       stdout=console, stderr=subprocess.STDOUT)
            try:
                if case in ("fdstore", "credentials"):
                    expected = "systemd-sysctl.service active/exited, 1 stored FDs" if case == "fdstore" else "template service contains credentials"
                    deadline = time.monotonic() + 20
                    while expected not in log.read_text() and time.monotonic() < deadline:
                        try:
                            request(vsock, dict(type="hello", version=1))
                        except (OSError, ConnectionError, AssertionError):
                            pass
                        else:
                            raise AssertionError("unsafe template exposed listener")
                        time.sleep(.05)
                    assert expected in log.read_text(), log.read_text()[-4000:]
                    return dict(case=case, rejected=expected)
                wait_hello(vsock, False)
                # Activation sources must remain gated, including delayed capture.
                time.sleep(3)
                wait_hello(vsock, False)
                identity = ("11111111111111111111111111111111" if shared and case != "identity-mismatch"
                            else "37aabbee1234567890abcdef98765432")
                execute = dict(type="exec", argv=["/bin/cat", "/etc/machine-id"], cwd=None, env={}, timeout_ms=1000)
                assert request(vsock, execute)["code"] == "not_initialized"
                initialize = dict(type="initialize", hostname="box-wire-test", machine_id=identity, entropy=list(os.urandom(64)))
                with concurrent.futures.ThreadPoolExecutor(max_workers=2) as workers:
                    reset = workers.submit(request, vsock, initialize)
                    # Race raw requests with the reset/handoff, bypassing host readiness.
                    replies = []
                    for _ in range(12):
                        try:
                            reply = request(vsock, execute)
                            if reply["type"] == "exec":
                                assert reply["exit_code"] == 0
                                assert bytes(reply["stdout"]).decode() == identity + "\n"
                                assert case == "success", "workload escaped failed handoff"
                            else:
                                assert reply["code"] == "not_initialized"
                            replies.append(reply["type"])
                        except (OSError, ConnectionError):
                            replies.append("handoff-disconnect")
                    result = reset.result()
                if case == "success":
                    assert result["type"] == "initialized", result
                    wait_hello(vsock, True)
                    assert bytes(request(vsock, execute)["stdout"]).decode() == identity + "\n"
                    login = request(vsock, dict(execute, argv=["/bin/sh", "-c",
                        "test ! -e /run/nologin && systemctl is-active systemd-user-sessions.service"]))
                    assert login["exit_code"] == 0, "initialized guest still blocks user logins: " + repr(login)
                    assert bytes(login["stdout"]) == b"active\n", login
                    refresh = request(vsock, dict(execute, argv=["/bin/cat", "/run/test-manager-refresh"]))
                    assert refresh["exit_code"] == 0, refresh
                    assert bytes(refresh["stdout"]).decode() == ("daemon-reload\n" if shared else "daemon-reexec\n")
                else:
                    assert result["code"] == "initialization_failed", result
                    wait_hello(vsock, False)
                    assert request(vsock, execute)["code"] == "not_initialized"
                timings = [json.loads(line.split("boxd_warm_reset ", 1)[1]) for line in log.read_text().splitlines()
                           if "boxd_warm_reset " in line]
                assert len(timings) == 1, timings
                timing = timings[0]
                assert timing["retain_pid1"] == shared
                assert timing["success"] == (case in ("success", "marker-failure"))
                assert sum(timing["phases_ms"].values()) <= timing["total_ms"]
                if timing["success"]:
                    assert set(timing["phases_ms"]) == {"audit", "provision", "unmask", "manager_refresh", "start_services", "verify_identity"}
                if case == "identity-mismatch":
                    assert timing["phases_ms"] == {}, "identity rejection must precede guest mutation"
                return dict(case=case, initialize=result, concurrent_replies=replies, reset=timing)
            finally:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


if __name__ == "__main__":
    image, firecracker, output = [Path(arg).resolve() for arg in sys.argv[1:]]
    output.mkdir(parents=True, exist_ok=True)
    results = []
    cases = ["success", "fdstore", "credentials", "reset-failure", "marker-failure"]
    if json.loads(image.read_text())["boot_mode"] == "systemd_warm_shared":
        cases.append("identity-mismatch")
    for case in cases:
        results.append(check_case(image, firecracker, output, case))
        print(json.dumps(results[-1]), flush=True)
    (output / "results.json").write_text(json.dumps(results, indent=2) + "\n")

# dark-forge.dev Kiln state snapshot, 2026-10-19

## Purpose

This is the read-only starting state of the Kiln install on `dark-forge.dev`, captured for the shell installer port's tests. The host was measured over SSH as root on 2026-10-19 at 15:44:17 UTC, after the same-day install recorded in [2026-10-19-dark-forge-first-install.md](2026-10-19-dark-forge-first-install.md).

No package was installed or upgraded. No unit was started, stopped, restarted, or reloaded. No file on the host was created, edited, moved, or deleted. `kiln-identity.service`, `kiln-identity-proxy.service`, `kiln-identity-proxy.socket`, and `/root/.ssh/authorized_keys` were only read.

The deploy key fingerprint checked before connecting was `SHA256:AGD6iqWdOAB1R7LeRTlbIMYyvx+FvkIDw8MTWEVtrd4`. The pinned ed25519 host key fingerprint was `SHA256:/yIiijBqkbGh9lFju3N9ZhbJGIqv+NjC0R1CyWh9g9s`. The private key is not in this file.

## Paths from `deploy/install.py`

Taken from `deploy/install.py` at main `daf9d04`. The file was read before the host was inspected; it is unchanged in the commits that landed between that read and this branch.

| Role | Path |
| --- | --- |
| Pending-upgrade marker | `/opt/kiln/pending-upgrade.json`, written only while an update is in progress and unlinked after success or after a rollback that restores the previous binaries |
| Rollback directory | `/opt/kiln/upgrades/<installed-version>-<mkdtemp>/` with copies of `kiln`, `kiln-runtime`, `kiln-network`, `kiln-host`, and `kiln-api` |
| Version source | The installer reads `release.json` from the release bundle. It does not install that file. Installed versions come from each binary's `--version` |
| `/opt/kiln/bin` | `kiln`, `kiln-runtime`, `kiln-network`, `firecracker`, `jailer` |
| libexec | `/usr/local/libexec/kiln-host` and `/usr/local/libexec/kiln-api`. `install.py` does not use `/opt/kiln/libexec` |
| Config | `/etc/kiln/isolation.json`, `/etc/kiln/host.json`, plus credential files `ca.key`, `ca.crt`, `tls.key`, `tls.crt`, `admin.token`, `CONNECT.txt`, `laptop.tar.gz` |
| Enrollment markers | `/etc/kiln/enrollment.pending.json`, `/etc/kiln/identity.json` |
| State | `/var/lib/kiln/image/` (`vmlinux`, `rootfs.ext4`, `image.json`, `inputs.json`), `/var/lib/kiln/runtime`, `/var/lib/kiln/journal/operations.sqlite` |
| Units | `/etc/systemd/system/kiln-cgroup.service`, `kiln-host.service`, `kiln-api.service`, drop-ins `kiln-host.service.d/installer.conf` and `kiln-api.service.d/installer.conf`. `kiln-network.service` and `kiln-host.service.d/network.conf` exist only when a network uplink was requested |
| Runtime sockets | `/run/kiln/host.sock`, `/run/lock/kiln-install.lock`, `/sys/fs/cgroup/kiln` |

Credential file contents were not read.

## Summary

`/opt/kiln/upgrades/` does not exist. `ls -laR /opt/kiln/upgrades/` returned `No such file or directory` and exit 2.

`/opt/kiln/pending-upgrade.json` does not exist. The existence test printed `absent`.

`/opt/kiln/libexec` does not exist. `/opt/kiln/release.json` and `/etc/kiln/release.json` do not exist. `/etc/kiln/enrollment.pending.json` and `/etc/kiln/identity.json` do not exist. `kiln-network.service` and `kiln-host.service.d/network.conf` do not exist.

`/opt/kiln/bin` contains `firecracker`, `jailer`, `kiln`, `kiln-network`, and `kiln-runtime`, all mtime `Oct 19 17:11`. JSON files under the installer trees, at maxdepth 2, are `/etc/kiln/host.json`, `/etc/kiln/isolation.json`, `/var/lib/kiln/image/image.json`, `/var/lib/kiln/image/inputs.json`, and `/var/lib/kiln/runtime/isolation.json`. `/var/lib/kiln/runtime/boxes` is empty. `/var/lib/kiln/runtime/snapshots` contains one directory, `a1323edbf1db1ff8bef3d5ccd9c6ebb6`, with `disk.ext4`, `memory.snap`, `snapshot.json`, and `state.snap`.

`/usr/local/libexec` also contains `kiln-identity` (mtime `Oct 19 12:42`) and `kiln-identity.rollback-v033` (mtime `Oct 3 20:04`). Those names are not produced by `install.py`'s `/opt/kiln/upgrades` rollback. Neither binary was executed.

### Binary versions

`--version` was tried, then `version`. `sha256sum` follows.

| Binary | `--version` | `version` | SHA-256 |
| --- | --- | --- | --- |
| `/opt/kiln/bin/firecracker` | `Firecracker v1.17.0` (exit 0; the process also logged an exit line) | unexpected argument `version`, exit 153 | `99ad0f5cd0514a88aad0e9ae8cfdb3cc3b4ab9d190e1194602406c786b5de7a5` |
| `/opt/kiln/bin/jailer` | `Jailer v1.17.0` (exit 0) | unexpected argument `version`, exit 1 | `65ef226e96f0ceda55ba643f445801ef2cc0ea667ef67cad8ac4f406c9c8434f` |
| `/opt/kiln/bin/kiln` | `kiln 0.4.4` (exit 0) | unrecognized subcommand, exit 2 | `834fe9b6346deb517ac5a4f9fe6fca337808982e9e407446e99d0f12507a4104` |
| `/opt/kiln/bin/kiln-network` | usage error, exit 1 | usage error, exit 1 | `6aa537753ef5ee4c1b307b3a13d9639b52ebb7c56b02d86f811119c975146d1a` |
| `/opt/kiln/bin/kiln-runtime` | `kiln-runtime 0.4.4` (exit 0) | unrecognized subcommand, exit 2 | `37655760174ccf74a9566f0dc5867c631cf7d4a67d9c3e4ea50094701f240b8e` |
| `/usr/local/libexec/kiln-host` | `kiln-server 0.4.4` (exit 0) | unexpected argument, exit 2 | `03c70e1bd462b6daf64aa2c9ac2859985010f1dfe774baa6c4f8e99f7ce17c1c` |
| `/usr/local/libexec/kiln-api` | `kiln-server 0.4.4` (exit 0) | unrecognized subcommand, exit 2 | `b5513873f6d688a6d8e5f1672c5a3ba7f35704cf2c6bb94a0d2ea30efd715a9b` |

`command -v firecracker` and `command -v jailer` both failed on the host default PATH (`/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin`). `/usr/local/bin/firecracker --version` and `/usr/local/bin/jailer --version` both failed with `No such file or directory`, exit 127. The copies that exist are `/opt/kiln/bin/firecracker` and `/opt/kiln/bin/jailer`.

### Unit file hashes

`systemctl list-unit-files 'kiln*'` listed six units. `DropInPaths` was empty for three of them.

| Unit | Fragment | SHA-256 | Drop-in | SHA-256 |
| --- | --- | --- | --- | --- |
| `kiln-api.service` | `/etc/systemd/system/kiln-api.service` | `10106a8635594d43c25ae84fdd25d97dddcf21964cda93c2710ced53033895b0` | `.../kiln-api.service.d/installer.conf` | `680a0796f8e67befc9d394c334e4a0fd5a9afe48022aec07c14467d96046e193` |
| `kiln-cgroup.service` | `/etc/systemd/system/kiln-cgroup.service` | `5ef31b1265b4cedd5b909b46d27c8d3a933bfec2d5ed6d5c9aaee400db93586d` | none | |
| `kiln-host.service` | `/etc/systemd/system/kiln-host.service` | `6c5847b513c91a4caa5e15cd76127f4c7a5a59682fff63fc310343b27c6d0f5a` | `.../kiln-host.service.d/installer.conf` | `f2178301e8f14562612786d68046bc8e18aee8ef38029eb5dcc273320b541e95` |
| `kiln-identity-proxy.service` | `/etc/systemd/system/kiln-identity-proxy.service` | `f9e0c98316c5f83d12de0abb3b885d6060f8b05a6618ba4f1360e0fbcbd37d6e` | none | |
| `kiln-identity.service` | `/etc/systemd/system/kiln-identity.service` | `31667738c2b275d7b326abcdfa0b6097c947b62005506693d90ab5df80122554` | `.../kiln-identity.service.d/10-provider-readiness.conf` | `08aca9fe20443428eed6d902c6d9c47999f8361ea7cb7bb9b22fbd84e739f9e2` |
| `kiln-identity-proxy.socket` | `/etc/systemd/system/kiln-identity-proxy.socket` | `d04bd26752276f842f93c7f4f6929cdccfacf81abbdc37b35f44519eca6b5141` | none | |

### `kiln-runtime doctor`

`kiln-runtime` is not on the default PATH. The command `kiln-runtime doctor` printed `command not found` and exited 127, so it produced no `development_ready` field.

`/opt/kiln/bin/kiln-runtime doctor` on that same PATH exited 1 with `"development_ready": false`. `firecracker_version` and `jailer_version` were `unavailable`, and the only problem was that Firecracker 1.17.0 is required. Doctor looks those tools up on `PATH`. They are installed in `/opt/kiln/bin`, which is not on the default PATH.

`install.py` sets `PATH=/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin` before it runs doctor. That invocation exited 0:

```json
{
  "architecture": "x86_64",
  "development_ready": true,
  "firecracker_version": "1.17.0",
  "isolated_ready": false,
  "jailer_version": "1.17.0",
  "problems": [],
  "schema_version": 1
}
```

`isolated_ready` stayed false because this invocation did not pass `--isolation-config`.

## Before and after

Each SSH session ran the same three checks first and last. The three blocks were byte-identical within every session, and identical across the three sessions.

`systemctl show` printed `MainPID` and `NRestarts` for the two services and only `ActiveEnterTimestamp` for the socket. Those lines were unchanged:

- `kiln-identity.service`: `MainPID=3522047`, `NRestarts=0`, `ActiveEnterTimestamp=Mon 2026-10-19 12:42:17 CEST`
- `kiln-identity-proxy.service`: `MainPID=3672592`, `NRestarts=0`, `ActiveEnterTimestamp=Mon 2026-10-19 17:42:16 CEST`
- `kiln-identity-proxy.socket`: `ActiveEnterTimestamp=Sat 2026-10-03 20:14:48 CEST`

`sha256sum /root/.ssh/authorized_keys` stayed `4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37`.

`dpkg -l | sha256sum` stayed `b63a5c449c27b54383f15d202762cc6c08d03d438ff77ec70448934fa4fd4713`.

## Commands and raw output

Version probes were wrapped in `timeout -k 2 15` so a hung process would be stopped. Every probe exited with the program's own status. None exited 124.

### Session 1

The remote shell exported `PATH=/usr/sbin:/usr/bin:/sbin:/bin:/usr/local/sbin:/usr/local/bin` before these commands. That is the same directory set as the host default PATH. Session 2 repeats `kiln-runtime doctor` with the host's unmodified PATH and order.

```text
=== CONTEXT ===
$ date -u
Mon Oct 19 03:44:17 PM UTC 2026
exit:0

$ hostname
dark-forge.dev
exit:0

$ pwd
/root
exit:0

=== BEFORE ===
$ systemctl show -p MainPID,NRestarts,ActiveEnterTimestamp kiln-identity.service kiln-identity-proxy.service kiln-identity-proxy.socket
MainPID=3522047
NRestarts=0
ActiveEnterTimestamp=Mon 2026-10-19 12:42:17 CEST

MainPID=3672592
NRestarts=0
ActiveEnterTimestamp=Mon 2026-10-19 17:42:16 CEST

ActiveEnterTimestamp=Sat 2026-10-03 20:14:48 CEST
exit:0

$ sha256sum /root/.ssh/authorized_keys
4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37  /root/.ssh/authorized_keys
exit:0

$ dpkg -l | sha256sum
b63a5c449c27b54383f15d202762cc6c08d03d438ff77ec70448934fa4fd4713  -
exit:0

=== UPGRADES DIR ===
$ ls -laR /opt/kiln/upgrades/
ls: cannot access '/opt/kiln/upgrades/': No such file or directory
exit:2

=== OPT LAYOUT ===
$ ls -la /opt/kiln/ /opt/kiln/bin /opt/kiln/libexec
ls: cannot access '/opt/kiln/libexec': No such file or directory
/opt/kiln/:
total 12
drwxr-xr-x 3 root root 4096 Oct 19 17:11 .
drwxr-xr-x 5 root root 4096 Oct 19 17:11 ..
drwxr-xr-x 2 root root 4096 Oct 19 17:11 bin

/opt/kiln/bin:
total 22072
drwxr-xr-x 2 root root    4096 Oct 19 17:11 .
drwxr-xr-x 3 root root    4096 Oct 19 17:11 ..
-rwxr-xr-x 1 root root 3676680 Oct 19 17:11 firecracker
-rwxr-xr-x 1 root root 2243000 Oct 19 17:11 jailer
-rwxr-xr-x 1 root root 8166808 Oct 19 17:11 kiln
-rwxr-xr-x 1 root root    6077 Oct 19 17:11 kiln-network
-rwxr-xr-x 1 root root 8492816 Oct 19 17:11 kiln-runtime
exit:2

$ test -e /opt/kiln/pending-upgrade.json && cat /opt/kiln/pending-upgrade.json || echo absent
absent
exit:0

$ find /opt/kiln /etc/kiln /var/lib/kiln -maxdepth 2 -name '*.json' -newer /etc/hostname 2>/dev/null
/etc/kiln/host.json
/etc/kiln/isolation.json
/var/lib/kiln/image/image.json
/var/lib/kiln/image/inputs.json
/var/lib/kiln/runtime/isolation.json
exit:0

$ find /opt/kiln /etc/kiln /var/lib/kiln -maxdepth 2 -name '*.json' 2>/dev/null
/etc/kiln/host.json
/etc/kiln/isolation.json
/var/lib/kiln/image/image.json
/var/lib/kiln/image/inputs.json
/var/lib/kiln/runtime/isolation.json
exit:0

$ ls -la /etc/kiln /var/lib/kiln
/etc/kiln:
total 44
drwxr-xr-x  2 root     root     4096 Oct 19 17:12 .
drwxr-xr-x 75 root     root     4096 Oct 19 17:11 ..
-rw-------  1 kiln-api kiln-api   65 Oct 19 17:11 admin.token
-rw-r--r--  1 root     root     1501 Oct 19 17:11 ca.crt
-rw-------  1 root     root     2484 Oct 19 17:11 ca.key
-rw-------  1 root     root      579 Oct 19 17:11 CONNECT.txt
-rw-------  1 root     root      283 Oct 19 17:12 host.json
-rw-------  1 root     root      180 Oct 19 17:12 isolation.json
-rw-------  1 root     root     1807 Oct 19 17:11 laptop.tar.gz
-rw-r--r--  1 kiln-api kiln-api 1363 Oct 19 17:11 tls.crt
-rw-------  1 kiln-api kiln-api 1704 Oct 19 17:11 tls.key

/var/lib/kiln:
total 20
drwx------  5 root kiln-api 4096 Oct 19 17:12 .
drwxr-xr-x 26 root root     4096 Oct 19 17:11 ..
drwx------  2 root kiln-api 4096 Oct 19 17:12 image
drwx------  2 root kiln-api 4096 Oct 19 17:12 journal
drwx------  4 root kiln-api 4096 Oct 19 17:12 runtime
exit:0

=== INSTALL.PY PATHS ===
$ ls -ld /opt/kiln /opt/kiln/bin /opt/kiln/libexec /opt/kiln/upgrades /opt/kiln/pending-upgrade.json /opt/kiln/release.json /opt/kiln/bin/kiln /opt/kiln/bin/kiln-runtime /opt/kiln/bin/kiln-network /opt/kiln/bin/firecracker /opt/kiln/bin/jailer /usr/local/libexec /usr/local/libexec/kiln-host /usr/local/libexec/kiln-api /usr/local/libexec/kiln-identity /etc/kiln /etc/kiln/isolation.json /etc/kiln/host.json /etc/kiln/enrollment.pending.json /etc/kiln/identity.json /etc/kiln/ca.key /etc/kiln/ca.crt /etc/kiln/tls.key /etc/kiln/tls.crt /etc/kiln/admin.token /etc/kiln/CONNECT.txt /etc/kiln/laptop.tar.gz /etc/kiln/release.json /var/lib/kiln /var/lib/kiln/image /var/lib/kiln/image/image.json /var/lib/kiln/image/inputs.json /var/lib/kiln/image/vmlinux /var/lib/kiln/image/rootfs.ext4 /var/lib/kiln/runtime /var/lib/kiln/journal /var/lib/kiln/journal/operations.sqlite /etc/systemd/system/kiln-host.service /etc/systemd/system/kiln-api.service /etc/systemd/system/kiln-cgroup.service /etc/systemd/system/kiln-network.service /etc/systemd/system/kiln-host.service.d /etc/systemd/system/kiln-api.service.d /etc/systemd/system/kiln-host.service.d/installer.conf /etc/systemd/system/kiln-api.service.d/installer.conf /etc/systemd/system/kiln-host.service.d/network.conf /run/lock/kiln-install.lock /run/kiln /run/kiln/host.sock /sys/fs/cgroup/kiln
ls: cannot access '/opt/kiln/libexec': No such file or directory
ls: cannot access '/opt/kiln/upgrades': No such file or directory
ls: cannot access '/opt/kiln/pending-upgrade.json': No such file or directory
ls: cannot access '/opt/kiln/release.json': No such file or directory
ls: cannot access '/etc/kiln/enrollment.pending.json': No such file or directory
ls: cannot access '/etc/kiln/identity.json': No such file or directory
ls: cannot access '/etc/kiln/release.json': No such file or directory
ls: cannot access '/etc/systemd/system/kiln-network.service': No such file or directory
ls: cannot access '/etc/systemd/system/kiln-host.service.d/network.conf': No such file or directory
drwxr-xr-x 2 root     root           4096 Oct 19 17:12 /etc/kiln
-rw------- 1 kiln-api kiln-api         65 Oct 19 17:11 /etc/kiln/admin.token
-rw-r--r-- 1 root     root           1501 Oct 19 17:11 /etc/kiln/ca.crt
-rw------- 1 root     root           2484 Oct 19 17:11 /etc/kiln/ca.key
-rw------- 1 root     root            579 Oct 19 17:11 /etc/kiln/CONNECT.txt
-rw------- 1 root     root            283 Oct 19 17:12 /etc/kiln/host.json
-rw------- 1 root     root            180 Oct 19 17:12 /etc/kiln/isolation.json
-rw------- 1 root     root           1807 Oct 19 17:11 /etc/kiln/laptop.tar.gz
-rw-r--r-- 1 kiln-api kiln-api       1363 Oct 19 17:11 /etc/kiln/tls.crt
-rw------- 1 kiln-api kiln-api       1704 Oct 19 17:11 /etc/kiln/tls.key
-rw-r--r-- 1 root     root            750 Oct 19 17:12 /etc/systemd/system/kiln-api.service
drwx------ 2 root     root           4096 Oct 19 17:12 /etc/systemd/system/kiln-api.service.d
-rw-r--r-- 1 root     root            209 Oct 19 17:12 /etc/systemd/system/kiln-api.service.d/installer.conf
-rw-r--r-- 1 root     root            383 Oct 19 17:12 /etc/systemd/system/kiln-cgroup.service
-rw-r--r-- 1 root     root            643 Oct 19 17:12 /etc/systemd/system/kiln-host.service
drwx------ 2 root     root           4096 Oct 19 17:12 /etc/systemd/system/kiln-host.service.d
-rw-r--r-- 1 root     root             62 Oct 19 17:12 /etc/systemd/system/kiln-host.service.d/installer.conf
drwxr-xr-x 3 root     root           4096 Oct 19 17:11 /opt/kiln
drwxr-xr-x 2 root     root           4096 Oct 19 17:11 /opt/kiln/bin
-rwxr-xr-x 1 root     root        3676680 Oct 19 17:11 /opt/kiln/bin/firecracker
-rwxr-xr-x 1 root     root        2243000 Oct 19 17:11 /opt/kiln/bin/jailer
-rwxr-xr-x 1 root     root        8166808 Oct 19 17:11 /opt/kiln/bin/kiln
-rwxr-xr-x 1 root     root           6077 Oct 19 17:11 /opt/kiln/bin/kiln-network
-rwxr-xr-x 1 root     root        8492816 Oct 19 17:11 /opt/kiln/bin/kiln-runtime
drwxr-x--- 2 root     kiln-api         60 Oct 19 17:12 /run/kiln
srw-rw---- 1 root     kiln-api          0 Oct 19 17:12 /run/kiln/host.sock
-rw------- 1 root     root              0 Oct 19 17:11 /run/lock/kiln-install.lock
drwxr-xr-x 2 root     root              0 Oct 19 17:12 /sys/fs/cgroup/kiln
drwxr-xr-x 2 root     root           4096 Oct 19 17:11 /usr/local/libexec
-rwxr-xr-x 1 root     root       10438672 Oct 19 17:11 /usr/local/libexec/kiln-api
-rwxr-xr-x 1 root     root       11390504 Oct 19 17:11 /usr/local/libexec/kiln-host
-rwxr-xr-x 1 root     root       11919456 Oct 19 12:42 /usr/local/libexec/kiln-identity
drwx------ 5 root     kiln-api       4096 Oct 19 17:12 /var/lib/kiln
drwx------ 2 root     kiln-api       4096 Oct 19 17:12 /var/lib/kiln/image
-r--r--r-- 1 root     kiln-api        350 Oct 19 17:12 /var/lib/kiln/image/image.json
-r--r--r-- 1 root     kiln-api        590 Oct 19 17:12 /var/lib/kiln/image/inputs.json
-r--r--r-- 1 root     kiln-api 2147483648 Oct 19 17:12 /var/lib/kiln/image/rootfs.ext4
-r--r--r-- 1 root     kiln-api   44279576 Oct 19 17:11 /var/lib/kiln/image/vmlinux
drwx------ 2 root     kiln-api       4096 Oct 19 17:12 /var/lib/kiln/journal
-rw------- 1 root     kiln-api      16384 Oct 19 17:12 /var/lib/kiln/journal/operations.sqlite
drwx------ 4 root     kiln-api       4096 Oct 19 17:12 /var/lib/kiln/runtime
exit:2

$ ls -la /usr/local/libexec /var/lib/kiln/image /var/lib/kiln/runtime /var/lib/kiln/journal /etc/systemd/system/kiln-host.service.d /etc/systemd/system/kiln-api.service.d
/etc/systemd/system/kiln-api.service.d:
total 12
drwx------  2 root root 4096 Oct 19 17:12 .
drwxr-xr-x 24 root root 4096 Oct 19 17:12 ..
-rw-r--r--  1 root root  209 Oct 19 17:12 installer.conf

/etc/systemd/system/kiln-host.service.d:
total 12
drwx------  2 root root 4096 Oct 19 17:12 .
drwxr-xr-x 24 root root 4096 Oct 19 17:12 ..
-rw-r--r--  1 root root   62 Oct 19 17:12 installer.conf

/usr/local/libexec:
total 44612
drwxr-xr-x  2 root root     4096 Oct 19 17:11 .
drwxr-xr-x 11 root root     4096 Jun 28 06:10 ..
-rwxr-xr-x  1 root root 10438672 Oct 19 17:11 kiln-api
-rwxr-xr-x  1 root root 11390504 Oct 19 17:11 kiln-host
-rwxr-xr-x  1 root root 11919456 Oct 19 12:42 kiln-identity
-rwxr-xr-x  1 root root 11919208 Oct  3 20:04 kiln-identity.rollback-v033

/var/lib/kiln/image:
total 2140416
drwx------ 2 root kiln-api       4096 Oct 19 17:12 .
drwx------ 5 root kiln-api       4096 Oct 19 17:12 ..
-r--r--r-- 1 root kiln-api        350 Oct 19 17:12 image.json
-r--r--r-- 1 root kiln-api        590 Oct 19 17:12 inputs.json
-r--r--r-- 1 root kiln-api 2147483648 Oct 19 17:12 rootfs.ext4
-r--r--r-- 1 root kiln-api   44279576 Oct 19 17:11 vmlinux

/var/lib/kiln/journal:
total 116
drwx------ 2 root kiln-api  4096 Oct 19 17:12 .
drwx------ 5 root kiln-api  4096 Oct 19 17:12 ..
-rw------- 1 root kiln-api     0 Oct 19 17:12 .lock
-rw------- 1 root kiln-api 16384 Oct 19 17:12 operations.sqlite
-rw------- 1 root kiln-api 32768 Oct 19 17:12 operations.sqlite-shm
-rw------- 1 root kiln-api 53592 Oct 19 17:13 operations.sqlite-wal

/var/lib/kiln/runtime:
total 20
drwx------ 4 root kiln-api 4096 Oct 19 17:12 .
drwx------ 5 root kiln-api 4096 Oct 19 17:12 ..
drwx------ 2 root kiln-api 4096 Oct 19 17:13 boxes
-rw------- 1 root kiln-api  132 Oct 19 17:12 isolation.json
-rw------- 1 root kiln-api    0 Oct 19 17:12 .lock
-rw------- 1 root kiln-api    0 Oct 19 17:12 .service.lock
drwx------ 3 root kiln-api 4096 Oct 19 17:12 snapshots
exit:0

=== BINARY VERSIONS ===
$ timeout -k 2 15 /opt/kiln/bin/firecracker --version
Firecracker v1.17.0

2026-10-19T17:44:17.850209820 [anonymous-instance:main] Firecracker exiting successfully. exit_code=0
exit:0

$ timeout -k 2 15 /opt/kiln/bin/firecracker version
2026-10-19T17:44:17.851415086 [anonymous-instance:main] Arguments parsing error: Found argument 'version' which wasn't expected, or isn't valid in this context. \n\nFor more information try --help.
Error: ParseArguments(UnexpectedArgument("version"))
2026-10-19T17:44:17.851453036 [anonymous-instance:main] Firecracker exiting with error. exit_code=153
exit:153

$ sha256sum /opt/kiln/bin/firecracker
99ad0f5cd0514a88aad0e9ae8cfdb3cc3b4ab9d190e1194602406c786b5de7a5  /opt/kiln/bin/firecracker
exit:0

$ timeout -k 2 15 /opt/kiln/bin/jailer --version
Jailer v1.17.0

exit:0

$ timeout -k 2 15 /opt/kiln/bin/jailer version
Failed to parse arguments: Found argument 'version' which wasn't expected, or isn't valid in this context.
Error: ArgumentParsing(UnexpectedArgument("version"))
exit:1

$ sha256sum /opt/kiln/bin/jailer
65ef226e96f0ceda55ba643f445801ef2cc0ea667ef67cad8ac4f406c9c8434f  /opt/kiln/bin/jailer
exit:0

$ timeout -k 2 15 /opt/kiln/bin/kiln --version
kiln 0.4.4
exit:0

$ timeout -k 2 15 /opt/kiln/bin/kiln version
error: unrecognized subcommand 'version'

  tip: a similar subcommand exists: 'operation'

Usage: kiln [OPTIONS] <COMMAND>

For more information, try '--help'.
exit:2

$ sha256sum /opt/kiln/bin/kiln
834fe9b6346deb517ac5a4f9fe6fca337808982e9e407446e99d0f12507a4104  /opt/kiln/bin/kiln
exit:0

$ timeout -k 2 15 /opt/kiln/bin/kiln-network --version
kiln-network: expected provision, prepare, activate, cleanup, or run
exit:1

$ timeout -k 2 15 /opt/kiln/bin/kiln-network version
kiln-network: expected provision, prepare, activate, cleanup, or run
exit:1

$ sha256sum /opt/kiln/bin/kiln-network
6aa537753ef5ee4c1b307b3a13d9639b52ebb7c56b02d86f811119c975146d1a  /opt/kiln/bin/kiln-network
exit:0

$ timeout -k 2 15 /opt/kiln/bin/kiln-runtime --version
kiln-runtime 0.4.4
exit:0

$ timeout -k 2 15 /opt/kiln/bin/kiln-runtime version
error: unrecognized subcommand 'version'

Usage: kiln-runtime [OPTIONS] <COMMAND>

For more information, try '--help'.
exit:2

$ sha256sum /opt/kiln/bin/kiln-runtime
37655760174ccf74a9566f0dc5867c631cf7d4a67d9c3e4ea50094701f240b8e  /opt/kiln/bin/kiln-runtime
exit:0

$ (list regular files in /opt/kiln/libexec)
directory absent

$ timeout -k 2 15 /usr/local/libexec/kiln-host --version
kiln-server 0.4.4
exit:0

$ timeout -k 2 15 /usr/local/libexec/kiln-host version
error: unexpected argument 'version' found

Usage: kiln-host [OPTIONS] --config <CONFIG>

For more information, try '--help'.
exit:2

$ sha256sum /usr/local/libexec/kiln-host
03c70e1bd462b6daf64aa2c9ac2859985010f1dfe774baa6c4f8e99f7ce17c1c  /usr/local/libexec/kiln-host
exit:0

$ timeout -k 2 15 /usr/local/libexec/kiln-api --version
kiln-server 0.4.4
exit:0

$ timeout -k 2 15 /usr/local/libexec/kiln-api version
error: unrecognized subcommand 'version'

Usage: kiln-api [OPTIONS] --host-socket <HOST_SOCKET> --token-file <TOKEN_FILE> --tls-cert <TLS_CERT> --tls-key <TLS_KEY>
       kiln-api [OPTIONS] <COMMAND>

For more information, try '--help'.
exit:2

$ sha256sum /usr/local/libexec/kiln-api
b5513873f6d688a6d8e5f1672c5a3ba7f35704cf2c6bb94a0d2ea30efd715a9b  /usr/local/libexec/kiln-api
exit:0

=== FIRECRACKER AND JAILER ===
$ command -v firecracker
exit:1

$ command -v jailer
exit:1

$ timeout -k 2 15 /usr/local/bin/firecracker --version
timeout: failed to run command ‘/usr/local/bin/firecracker’: No such file or directory
exit:127

$ timeout -k 2 15 /usr/local/bin/jailer --version
timeout: failed to run command ‘/usr/local/bin/jailer’: No such file or directory
exit:127

=== UNITS ===
$ systemctl list-unit-files 'kiln*'
UNIT FILE                   STATE   PRESET
kiln-api.service            enabled enabled
kiln-cgroup.service         enabled enabled
kiln-host.service           enabled enabled
kiln-identity-proxy.service static  -
kiln-identity.service       enabled enabled
kiln-identity-proxy.socket  enabled enabled

6 unit files listed.
exit:0

$ systemctl show -p FragmentPath,DropInPaths kiln-api.service
FragmentPath=/etc/systemd/system/kiln-api.service
DropInPaths=/etc/systemd/system/kiln-api.service.d/installer.conf
exit:0

$ sha256sum /etc/systemd/system/kiln-api.service
10106a8635594d43c25ae84fdd25d97dddcf21964cda93c2710ced53033895b0  /etc/systemd/system/kiln-api.service
exit:0

$ sha256sum /etc/systemd/system/kiln-api.service.d/installer.conf
680a0796f8e67befc9d394c334e4a0fd5a9afe48022aec07c14467d96046e193  /etc/systemd/system/kiln-api.service.d/installer.conf
exit:0

$ systemctl show -p FragmentPath,DropInPaths kiln-cgroup.service
FragmentPath=/etc/systemd/system/kiln-cgroup.service
DropInPaths=
exit:0

$ sha256sum /etc/systemd/system/kiln-cgroup.service
5ef31b1265b4cedd5b909b46d27c8d3a933bfec2d5ed6d5c9aaee400db93586d  /etc/systemd/system/kiln-cgroup.service
exit:0

$ systemctl show -p FragmentPath,DropInPaths kiln-host.service
FragmentPath=/etc/systemd/system/kiln-host.service
DropInPaths=/etc/systemd/system/kiln-host.service.d/installer.conf
exit:0

$ sha256sum /etc/systemd/system/kiln-host.service
6c5847b513c91a4caa5e15cd76127f4c7a5a59682fff63fc310343b27c6d0f5a  /etc/systemd/system/kiln-host.service
exit:0

$ sha256sum /etc/systemd/system/kiln-host.service.d/installer.conf
f2178301e8f14562612786d68046bc8e18aee8ef38029eb5dcc273320b541e95  /etc/systemd/system/kiln-host.service.d/installer.conf
exit:0

$ systemctl show -p FragmentPath,DropInPaths kiln-identity-proxy.service
FragmentPath=/etc/systemd/system/kiln-identity-proxy.service
DropInPaths=
exit:0

$ sha256sum /etc/systemd/system/kiln-identity-proxy.service
f9e0c98316c5f83d12de0abb3b885d6060f8b05a6618ba4f1360e0fbcbd37d6e  /etc/systemd/system/kiln-identity-proxy.service
exit:0

$ systemctl show -p FragmentPath,DropInPaths kiln-identity.service
FragmentPath=/etc/systemd/system/kiln-identity.service
DropInPaths=/etc/systemd/system/kiln-identity.service.d/10-provider-readiness.conf
exit:0

$ sha256sum /etc/systemd/system/kiln-identity.service
31667738c2b275d7b326abcdfa0b6097c947b62005506693d90ab5df80122554  /etc/systemd/system/kiln-identity.service
exit:0

$ sha256sum /etc/systemd/system/kiln-identity.service.d/10-provider-readiness.conf
08aca9fe20443428eed6d902c6d9c47999f8361ea7cb7bb9b22fbd84e739f9e2  /etc/systemd/system/kiln-identity.service.d/10-provider-readiness.conf
exit:0

$ systemctl show -p FragmentPath,DropInPaths kiln-identity-proxy.socket
FragmentPath=/etc/systemd/system/kiln-identity-proxy.socket
DropInPaths=
exit:0

$ sha256sum /etc/systemd/system/kiln-identity-proxy.socket
d04bd26752276f842f93c7f4f6929cdccfacf81abbdc37b35f44519eca6b5141  /etc/systemd/system/kiln-identity-proxy.socket
exit:0

=== DOCTOR ===
$ command -v kiln-runtime
exit:1

$ echo PATH=$PATH
PATH=/usr/sbin:/usr/bin:/sbin:/bin:/usr/local/sbin:/usr/local/bin
exit:0

kiln-runtime not on PATH

$ timeout -k 2 60 /opt/kiln/bin/kiln-runtime doctor
{
  "architecture": "x86_64",
  "development_ready": false,
  "firecracker_version": "unavailable",
  "isolated_ready": false,
  "jailer_version": "unavailable",
  "problems": [
    "Firecracker 1.17.0 is required (1.16.0 breaks vsock after pause/resume)"
  ],
  "schema_version": 1
}
exit:1

=== AFTER ===
$ systemctl show -p MainPID,NRestarts,ActiveEnterTimestamp kiln-identity.service kiln-identity-proxy.service kiln-identity-proxy.socket
MainPID=3522047
NRestarts=0
ActiveEnterTimestamp=Mon 2026-10-19 12:42:17 CEST

MainPID=3672592
NRestarts=0
ActiveEnterTimestamp=Mon 2026-10-19 17:42:16 CEST

ActiveEnterTimestamp=Sat 2026-10-03 20:14:48 CEST
exit:0

$ sha256sum /root/.ssh/authorized_keys
4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37  /root/.ssh/authorized_keys
exit:0

$ dpkg -l | sha256sum
b63a5c449c27b54383f15d202762cc6c08d03d438ff77ec70448934fa4fd4713  -
exit:0

=== END ===
```

### Session 2

Unmodified SSH `PATH`. This session repeats doctor and lists the runtime box and snapshot directories.

```text
=== BEFORE ===
$ systemctl show -p MainPID,NRestarts,ActiveEnterTimestamp kiln-identity.service kiln-identity-proxy.service kiln-identity-proxy.socket
MainPID=3522047
NRestarts=0
ActiveEnterTimestamp=Mon 2026-10-19 12:42:17 CEST

MainPID=3672592
NRestarts=0
ActiveEnterTimestamp=Mon 2026-10-19 17:42:16 CEST

ActiveEnterTimestamp=Sat 2026-10-03 20:14:48 CEST
exit:0

$ sha256sum /root/.ssh/authorized_keys
4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37  /root/.ssh/authorized_keys
exit:0

$ dpkg -l | sha256sum
b63a5c449c27b54383f15d202762cc6c08d03d438ff77ec70448934fa4fd4713  -
exit:0

=== DEFAULT PATH ===
$ printf "%s\n" "$PATH"
/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
exit:0

$ command -v kiln-runtime
exit:1

$ command -v firecracker
exit:1

$ command -v jailer
exit:1

$ kiln-runtime doctor
bash: line 1: kiln-runtime: command not found
exit:127

=== INSTALLED BINARY, DEFAULT PATH ===
$ timeout -k 2 60 /opt/kiln/bin/kiln-runtime doctor
{
  "architecture": "x86_64",
  "development_ready": false,
  "firecracker_version": "unavailable",
  "isolated_ready": false,
  "jailer_version": "unavailable",
  "problems": [
    "Firecracker 1.17.0 is required (1.16.0 breaks vsock after pause/resume)"
  ],
  "schema_version": 1
}
exit:1

=== INSTALL.PY PATH ===
$ PATH=/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin /opt/kiln/bin/kiln-runtime doctor
{
  "architecture": "x86_64",
  "development_ready": true,
  "firecracker_version": "1.17.0",
  "isolated_ready": false,
  "jailer_version": "1.17.0",
  "problems": [],
  "schema_version": 1
}
exit:0

=== RUNTIME CHILDREN ===
$ ls -la /var/lib/kiln/runtime/boxes /var/lib/kiln/runtime/snapshots
/var/lib/kiln/runtime/boxes:
total 8
drwx------ 2 root kiln-api 4096 Oct 19 17:13 .
drwx------ 4 root kiln-api 4096 Oct 19 17:12 ..

/var/lib/kiln/runtime/snapshots:
total 12
drwx------ 3 root kiln-api 4096 Oct 19 17:12 .
drwx------ 4 root kiln-api 4096 Oct 19 17:12 ..
drwx------ 2 root kiln-api 4096 Oct 19 17:12 a1323edbf1db1ff8bef3d5ccd9c6ebb6
exit:0

=== AFTER ===
$ systemctl show -p MainPID,NRestarts,ActiveEnterTimestamp kiln-identity.service kiln-identity-proxy.service kiln-identity-proxy.socket
MainPID=3522047
NRestarts=0
ActiveEnterTimestamp=Mon 2026-10-19 12:42:17 CEST

MainPID=3672592
NRestarts=0
ActiveEnterTimestamp=Mon 2026-10-19 17:42:16 CEST

ActiveEnterTimestamp=Sat 2026-10-03 20:14:48 CEST
exit:0

$ sha256sum /root/.ssh/authorized_keys
4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37  /root/.ssh/authorized_keys
exit:0

$ dpkg -l | sha256sum
b63a5c449c27b54383f15d202762cc6c08d03d438ff77ec70448934fa4fd4713  -
exit:0
```

### Session 3

Listing of the one snapshot directory, with the same before and after checks.

```text
=== BEFORE ===
$ systemctl show -p MainPID,NRestarts,ActiveEnterTimestamp kiln-identity.service kiln-identity-proxy.service kiln-identity-proxy.socket
MainPID=3522047
NRestarts=0
ActiveEnterTimestamp=Mon 2026-10-19 12:42:17 CEST

MainPID=3672592
NRestarts=0
ActiveEnterTimestamp=Mon 2026-10-19 17:42:16 CEST

ActiveEnterTimestamp=Sat 2026-10-03 20:14:48 CEST
exit:0

$ sha256sum /root/.ssh/authorized_keys
4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37  /root/.ssh/authorized_keys
exit:0

$ dpkg -l | sha256sum
b63a5c449c27b54383f15d202762cc6c08d03d438ff77ec70448934fa4fd4713  -
exit:0

=== SNAPSHOT DIR ===
$ ls -la /var/lib/kiln/runtime/snapshots/a1323edbf1db1ff8bef3d5ccd9c6ebb6
total 638256
drwx------ 2 root kiln-api       4096 Oct 19 17:12 .
drwx------ 3 root kiln-api       4096 Oct 19 17:12 ..
-r-------- 1 root kiln-api 2147483648 Oct 19 17:12 disk.ext4
-r-------- 1 root kiln-api 4294967296 Oct 19 17:12 memory.snap
-rw------- 1 root kiln-api       1869 Oct 19 17:12 snapshot.json
-r-------- 1 root kiln-api      14102 Oct 19 17:12 state.snap
exit:0

=== AFTER ===
$ systemctl show -p MainPID,NRestarts,ActiveEnterTimestamp kiln-identity.service kiln-identity-proxy.service kiln-identity-proxy.socket
MainPID=3522047
NRestarts=0
ActiveEnterTimestamp=Mon 2026-10-19 12:42:17 CEST

MainPID=3672592
NRestarts=0
ActiveEnterTimestamp=Mon 2026-10-19 17:42:16 CEST

ActiveEnterTimestamp=Sat 2026-10-03 20:14:48 CEST
exit:0

$ sha256sum /root/.ssh/authorized_keys
4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37  /root/.ssh/authorized_keys
exit:0

$ dpkg -l | sha256sum
b63a5c449c27b54383f15d202762cc6c08d03d438ff77ec70448934fa4fd4713  -
exit:0
```

# dark-forge.dev first install

Record of the first Kiln server install on dark-forge.dev. The host is bare-metal Debian 13.6 (trixie), x86_64, 12 CPUs, 62 GB RAM, with `/dev/kvm`. Source commit is main `dfd14e176f185fc5a1d852d369db0253500e1157`. Binaries and the guest image are the published v0.4.4 server archive. The installer that ran is `deploy/install.py` from that commit.

Result. `kiln-runtime doctor` printed `"development_ready": true`. A real `ubuntu-4g` box ran `uname -a` and printed `kiln-smoke-ok`. The box was deleted. `kiln list` then reported no boxes. Firecracker and jailer are v1.17.0. Identity units, their config, and `/root/.ssh/authorized_keys` were unchanged. `dpkg -l` was unchanged. No apt command ran.

SSH used `ssh -i <keyfile> -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile=<pinned known_hosts> -o BatchMode=yes root@dark-forge.dev`. The deploy key fingerprint is `SHA256:AGD6iqWdOAB1R7LeRTlbIMYyvx+FvkIDw8MTWEVtrd4`. The host key is ED25519 `SHA256:/yIiijBqkbGh9lFju3N9ZhbJGIqv+NjC0R1CyWh9g9s`. The private key is not in this file.

## CI gate

Workflow run https://github.com/Jabbslad/kiln/actions/runs/39274237685 on `dfd14e176f185fc5a1d852d369db0253500e1157` finished with conclusion `success`.

| Check | Conclusion |
| --- | --- |
| server-check | success |
| server-package | success |
| client (ubuntu-24.04, x86_64-unknown-linux-gnu) | success |
| client (macos-15, aarch64-apple-darwin) | success |
| dashboard | success |
| Socket Security: Project Report | success |
| draft-release | skipped |

`draft-release` runs only on a `v*` tag push. This commit is a branch push, so the skip is the workflow condition, not a failed check. Install started after every executed check had conclusion `success`.

## Install choices

`deploy/install.py` on this commit accepts Debian 13. The `install.py` inside the v0.4.4 archive does not. The archive copy was replaced with main's file before any install. `fetch-firecracker.sh` in the archive is byte-identical to main `scripts/fetch-firecracker.sh`, so that file was left as published.

The installer has no `apt-get` step. Host tools it requires were already installed. No package was upgraded. The API listens on `127.0.0.1:8443`. No `--network-uplink` was passed, so the installer made no firewall, routing, or guest-NAT change. Enrollment was not run.

Unmerged `deploy/github/install-remote.sh` from PR #7 was not used.

## 1. Read-only preflight

Command on the server:

```sh
set -u
echo '=== date ==='
date -u
echo '=== os-release ==='
cat /etc/os-release
echo '=== uname ==='
uname -a
echo '=== nproc ==='
nproc
echo '=== meminfo available ==='
awk '/MemTotal|MemAvailable|MemFree/ {print}' /proc/meminfo
echo '=== disk ==='
df -hT
echo '=== df bytes /var ==='
df -B1 / /var /var/lib /opt 2>/dev/null || df -B1 /
echo '=== kvm ==='
ls -l /dev/kvm
stat -c 'mode=%a owner=%U group=%G' /dev/kvm
id
getent group kvm || true
echo '=== cgroup ==='
cat /sys/fs/cgroup/cgroup.controllers
echo '=== systemd booted ==='
if [ -d /run/systemd/system ]; then echo yes; else echo no; fi
echo '=== tools ==='
for t in openssl systemctl systemd-analyze groupadd useradd curl bash tar sha256sum install realpath python3 ip nft sysctl nsenter firecracker jailer; do
  if command -v "$t" >/dev/null 2>&1; then
    printf '%s %s\n' "$t" "$(command -v "$t")"
  else
    printf '%s MISSING\n' "$t"
  fi
done
echo '=== firecracker versions if present ==='
if command -v firecracker >/dev/null 2>&1; then firecracker --version || true; fi
if command -v jailer >/dev/null 2>&1; then jailer --version || true; fi
echo '=== kiln units ==='
systemctl list-unit-files --no-pager --type=service --type=socket 'kiln*' || true
echo '=== kiln unit states ==='
systemctl list-units --all --no-pager 'kiln*' || true
echo '=== identity show ==='
for u in kiln-identity.service kiln-identity-proxy.service kiln-identity-proxy.socket; do
  echo "--- $u ---"
  systemctl show -p ActiveEnterTimestamp,MainPID,NRestarts,LoadState,ActiveState,SubState,FragmentPath,DropInPaths,UnitFileState -- "$u"
done
echo '=== identity file hashes ==='
{
  for u in kiln-identity.service kiln-identity-proxy.service kiln-identity-proxy.socket; do
    systemctl show -p FragmentPath --value -- "$u"
    systemctl show -p DropInPaths --value -- "$u" | tr ' ' '\n'
  done
  if [ -d /etc/systemd/system ]; then find /etc/systemd/system /usr/lib/systemd/system /lib/systemd/system -name 'kiln-identity*' -o -name 'kiln-identity-proxy*' 2>/dev/null; fi
} | awk 'NF && !seen[$0]++' | while read -r p; do
  if [ -f "$p" ]; then sha256sum "$p"; else printf 'ABSENT %s\n' "$p"; fi
done
echo '=== login.token ==='
if [ -f /etc/kiln/login.token ]; then sha256sum /etc/kiln/login.token; else echo 'ABSENT /etc/kiln/login.token'; fi
echo '=== identity config dir hashes ==='
if [ -d /etc/kiln-identity ]; then find /etc/kiln-identity -type f -print0 | sort -z | xargs -0 sha256sum; else echo 'ABSENT /etc/kiln-identity'; fi
if [ -d /etc/kiln ]; then echo '--- /etc/kiln file list hashes ---'; find /etc/kiln -type f -print0 | sort -z | xargs -0 sha256sum; else echo 'ABSENT /etc/kiln'; fi
echo '=== authorized_keys ==='
if [ -f /root/.ssh/authorized_keys ]; then sha256sum /root/.ssh/authorized_keys; else echo 'ABSENT /root/.ssh/authorized_keys'; fi
find /root /home /etc -name authorized_keys -type f 2>/dev/null | sort | while read -r p; do sha256sum "$p"; done
echo '=== existing kiln paths ==='
for p in /opt/kiln /var/lib/kiln /etc/kiln /usr/local/libexec/kiln-host /usr/local/libexec/kiln-api /usr/local/libexec/kiln-identity /run/kiln; do
  if [ -e "$p" ] || [ -L "$p" ]; then printf 'EXISTS %s\n' "$p"; else printf 'ABSENT %s\n' "$p"; fi
done
echo '=== dpkg kiln/firecracker ==='
dpkg -l | awk 'BEGIN{IGNORECASE=1} /kiln|firecracker|jailer/ {print}'
echo '=== PREFLIGHT_DONE ==='
```

Output:

```text
=== date ===
Mon Oct 19 03:04:46 PM UTC 2026
=== os-release ===
PRETTY_NAME="Debian GNU/Linux 13 (trixie)"
NAME="Debian GNU/Linux"
VERSION_ID="13"
VERSION="13 (trixie)"
VERSION_CODENAME=trixie
DEBIAN_VERSION_FULL=13.6
ID=debian
HOME_URL="https://www.debian.org/"
SUPPORT_URL="https://www.debian.org/support"
BUG_REPORT_URL="https://bugs.debian.org/"
=== uname ===
Linux dark-forge.dev 6.12.100+deb13-amd64 #1 SMP PREEMPT_DYNAMIC Debian 6.12.100-1 (2026-07-30) x86_64 GNU/Linux
=== nproc ===
12
=== meminfo available ===
MemTotal:       65772112 kB
MemFree:         7240244 kB
MemAvailable:   63196536 kB
=== disk ===
Filesystem     Type      Size  Used Avail Use% Mounted on
udev           devtmpfs   32G     0   32G   0% /dev
tmpfs          tmpfs     6.3G  1.1M  6.3G   1% /run
/dev/md2       ext4      436G   50G  364G  13% /
tmpfs          tmpfs      32G     0   32G   0% /dev/shm
tmpfs          tmpfs     5.0M     0  5.0M   0% /run/lock
tmpfs          tmpfs     1.0M     0  1.0M   0% /run/credentials/systemd-journald.service
tmpfs          tmpfs      32G     0   32G   0% /tmp
/dev/md1       ext3      989M  149M  790M  16% /boot
tmpfs          tmpfs     1.0M     0  1.0M   0% /run/credentials/getty@tty1.service
overlay        overlay   436G   50G  364G  13% /var/lib/docker/rootfs/overlayfs/970927c154c9b6a04b185de08b5d3a5cdaba553554c608db78a2c977a6e631f2
overlay        overlay   436G   50G  364G  13% /var/lib/docker/rootfs/overlayfs/826e7f164f64c9858a33616f958e94627ebce18d9b672bed62d6852a652d126d
overlay        overlay   436G   50G  364G  13% /var/lib/docker/rootfs/overlayfs/ef0635c603c33f07e1aacba6735a4d7db2eb6cd6853451d6c7c869e63b227bc7
overlay        overlay   436G   50G  364G  13% /var/lib/docker/rootfs/overlayfs/a398b31a015fdc30ae14322c46c3957ee96c24268df0709386d98d550c4774ab
tmpfs          tmpfs     6.3G  8.0K  6.3G   1% /run/user/0
=== df bytes /var ===
Filesystem        1B-blocks        Used    Available Use% Mounted on
/dev/md2       467909804032 53335318528 390730743808  13% /
/dev/md2       467909804032 53335318528 390730743808  13% /
/dev/md2       467909804032 53335318528 390730743808  13% /
/dev/md2       467909804032 53335318528 390730743808  13% /
=== kvm ===
crw-rw---- 1 root kvm 10, 232 Aug  6 01:08 /dev/kvm
mode=660 owner=root group=kvm
uid=0(root) gid=0(root) groups=0(root)
kvm:x:993:
=== cgroup ===
cpuset cpu io memory hugetlb pids rdma misc
=== systemd booted ===
yes
=== tools ===
openssl /usr/bin/openssl
systemctl /usr/bin/systemctl
systemd-analyze /usr/bin/systemd-analyze
groupadd /usr/sbin/groupadd
useradd /usr/sbin/useradd
curl /usr/bin/curl
bash /usr/bin/bash
tar /usr/bin/tar
sha256sum /usr/bin/sha256sum
install /usr/bin/install
realpath /usr/bin/realpath
python3 /usr/bin/python3
ip /usr/sbin/ip
nft /usr/sbin/nft
sysctl /usr/sbin/sysctl
nsenter /usr/bin/nsenter
firecracker MISSING
jailer MISSING
=== firecracker versions if present ===
=== kiln units ===
UNIT FILE                   STATE   PRESET
kiln-identity-proxy.service static  -
kiln-identity.service       enabled enabled
kiln-identity-proxy.socket  enabled enabled

3 unit files listed.
=== kiln unit states ===
  UNIT                        LOAD   ACTIVE   SUB       DESCRIPTION
  kiln-identity-proxy.service loaded inactive dead      Private Caddy bridge to loopback Kiln identity
  kiln-identity.service       loaded active   running   Kiln identity and server discovery
  kiln-identity-proxy.socket  loaded active   listening Private Caddy bridge socket for Kiln identity

Legend: LOAD   → Reflects whether the unit definition was properly loaded.
        ACTIVE → The high-level unit activation state, i.e. generalization of SUB.
        SUB    → The low-level unit activation state, values depend on unit type.

3 loaded units listed.
To show all installed unit files use 'systemctl list-unit-files'.
=== identity show ===
--- kiln-identity.service ---
MainPID=3522047
NRestarts=0
LoadState=loaded
ActiveState=active
SubState=running
FragmentPath=/etc/systemd/system/kiln-identity.service
DropInPaths=/etc/systemd/system/kiln-identity.service.d/10-provider-readiness.conf
UnitFileState=enabled
ActiveEnterTimestamp=Mon 2026-10-19 12:42:17 CEST
--- kiln-identity-proxy.service ---
MainPID=0
NRestarts=0
LoadState=loaded
ActiveState=inactive
SubState=dead
FragmentPath=/etc/systemd/system/kiln-identity-proxy.service
DropInPaths=
UnitFileState=static
ActiveEnterTimestamp=Mon 2026-10-19 16:51:59 CEST
--- kiln-identity-proxy.socket ---
LoadState=loaded
ActiveState=active
SubState=listening
FragmentPath=/etc/systemd/system/kiln-identity-proxy.socket
DropInPaths=
UnitFileState=enabled
ActiveEnterTimestamp=Sat 2026-10-03 20:14:48 CEST
=== identity file hashes ===
31667738c2b275d7b326abcdfa0b6097c947b62005506693d90ab5df80122554  /etc/systemd/system/kiln-identity.service
08aca9fe20443428eed6d902c6d9c47999f8361ea7cb7bb9b22fbd84e739f9e2  /etc/systemd/system/kiln-identity.service.d/10-provider-readiness.conf
f9e0c98316c5f83d12de0abb3b885d6060f8b05a6618ba4f1360e0fbcbd37d6e  /etc/systemd/system/kiln-identity-proxy.service
d04bd26752276f842f93c7f4f6929cdccfacf81abbdc37b35f44519eca6b5141  /etc/systemd/system/kiln-identity-proxy.socket
ABSENT /etc/systemd/system/kiln-identity.service.d
31667738c2b275d7b326abcdfa0b6097c947b62005506693d90ab5df80122554  /etc/systemd/system/multi-user.target.wants/kiln-identity.service
d04bd26752276f842f93c7f4f6929cdccfacf81abbdc37b35f44519eca6b5141  /etc/systemd/system/sockets.target.wants/kiln-identity-proxy.socket
=== login.token ===
ABSENT /etc/kiln/login.token
=== identity config dir hashes ===
7508a7c9782a09e3181a4bd6a70938ba033eb826bcb8d6eafd53a3610061e495  /etc/kiln-identity/github.secret
9f7da71cffb6cdf12653975b136703b05dc836742879a194ba089733482a3757  /etc/kiln-identity/google.secret
6e0f03257ca0ed6039f1bd3ba855049a0619beba8789163696836027f8c56abb  /etc/kiln-identity/service.env
b1b7a26a116cb539bd4a19c4f4a8f9554f6896f6e7303567a4c50ebeefd87eb8  /etc/kiln-identity/signing.pem
ABSENT /etc/kiln
=== authorized_keys ===
4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37  /root/.ssh/authorized_keys
4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37  /root/.ssh/authorized_keys
=== existing kiln paths ===
ABSENT /opt/kiln
ABSENT /var/lib/kiln
ABSENT /etc/kiln
ABSENT /usr/local/libexec/kiln-host
ABSENT /usr/local/libexec/kiln-api
EXISTS /usr/local/libexec/kiln-identity
ABSENT /run/kiln
=== dpkg kiln/firecracker ===
=== PREFLIGHT_DONE ===
```

The `ABSENT /etc/systemd/system/kiln-identity.service.d` line is the directory itself. The drop-in file above it was hashed. `/etc/kiln/login.token` was absent, so there is no digest for it.

Then:

```sh
dpkg -l
```

The listing is 369 lines. sha256 `b63a5c449c27b54383f15d202762cc6c08d03d438ff77ec70448934fa4fd4713`.

## 2. Exact identity properties before install

```sh
for u in kiln-identity.service kiln-identity-proxy.service kiln-identity-proxy.socket; do
  echo "--- $u ---"
  systemctl show -p ActiveEnterTimestamp,MainPID,NRestarts -- "$u"
done
```

```text
--- kiln-identity.service ---
MainPID=3522047
NRestarts=0
ActiveEnterTimestamp=Mon 2026-10-19 12:42:17 CEST
--- kiln-identity-proxy.service ---
MainPID=0
NRestarts=0
ActiveEnterTimestamp=Mon 2026-10-19 16:51:59 CEST
--- kiln-identity-proxy.socket ---
ActiveEnterTimestamp=Sat 2026-10-03 20:14:48 CEST
```

`kiln-identity-proxy.socket` did not print `MainPID` or `NRestarts`.

## 3. Snapshot immediately before the first write

```sh
set -u
echo '=== date ==='
date -u
echo '=== identity show ==='
for u in kiln-identity.service kiln-identity-proxy.service kiln-identity-proxy.socket; do
  echo "--- $u ---"
  systemctl show -p ActiveEnterTimestamp,MainPID,NRestarts -- "$u"
done
echo '=== identity and unit hashes ==='
sha256sum \
  /etc/systemd/system/kiln-identity.service \
  /etc/systemd/system/kiln-identity.service.d/10-provider-readiness.conf \
  /etc/systemd/system/kiln-identity-proxy.service \
  /etc/systemd/system/kiln-identity-proxy.socket \
  /etc/kiln-identity/github.secret \
  /etc/kiln-identity/google.secret \
  /etc/kiln-identity/service.env \
  /etc/kiln-identity/signing.pem
if [ -f /etc/kiln/login.token ]; then sha256sum /etc/kiln/login.token; else echo 'ABSENT /etc/kiln/login.token'; fi
echo '=== authorized_keys ==='
sha256sum /root/.ssh/authorized_keys
find /root /home /etc -name authorized_keys -type f -exec sha256sum {} +
echo '=== python ==='
python3 --version
echo '=== listeners 8443 ==='
ss -ltn | awk '$4 ~ /:8443$/' || true
```

```text
=== date ===
Mon Oct 19 03:10:59 PM UTC 2026
=== identity show ===
--- kiln-identity.service ---
MainPID=3522047
NRestarts=0
ActiveEnterTimestamp=Mon 2026-10-19 12:42:17 CEST
--- kiln-identity-proxy.service ---
MainPID=0
NRestarts=0
ActiveEnterTimestamp=Mon 2026-10-19 16:51:59 CEST
--- kiln-identity-proxy.socket ---
ActiveEnterTimestamp=Sat 2026-10-03 20:14:48 CEST
=== identity and unit hashes ===
31667738c2b275d7b326abcdfa0b6097c947b62005506693d90ab5df80122554  /etc/systemd/system/kiln-identity.service
08aca9fe20443428eed6d902c6d9c47999f8361ea7cb7bb9b22fbd84e739f9e2  /etc/systemd/system/kiln-identity.service.d/10-provider-readiness.conf
f9e0c98316c5f83d12de0abb3b885d6060f8b05a6618ba4f1360e0fbcbd37d6e  /etc/systemd/system/kiln-identity-proxy.service
d04bd26752276f842f93c7f4f6929cdccfacf81abbdc37b35f44519eca6b5141  /etc/systemd/system/kiln-identity-proxy.socket
7508a7c9782a09e3181a4bd6a70938ba033eb826bcb8d6eafd53a3610061e495  /etc/kiln-identity/github.secret
9f7da71cffb6cdf12653975b136703b05dc836742879a194ba089733482a3757  /etc/kiln-identity/google.secret
6e0f03257ca0ed6039f1bd3ba855049a0619beba8789163696836027f8c56abb  /etc/kiln-identity/service.env
b1b7a26a116cb539bd4a19c4f4a8f9554f6896f6e7303567a4c50ebeefd87eb8  /etc/kiln-identity/signing.pem
ABSENT /etc/kiln/login.token
=== authorized_keys ===
4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37  /root/.ssh/authorized_keys
4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37  /root/.ssh/authorized_keys
=== python ===
Python 3.13.5
=== listeners 8443 ===
```

`dpkg -l` again. Same 369 lines. Same sha256 `b63a5c449c27b54383f15d202762cc6c08d03d438ff77ec70448934fa4fd4713`. Nothing was listening on port 8443.

## 4. Download v0.4.4 and verify the published checksum

```sh
set -euo pipefail
umask 077
echo '=== date ==='
date -u
install_root=/var/tmp/kiln-v044-install
mkdir -p "$install_root"
cd "$install_root"
base=https://github.com/Jabbslad/kiln/releases/download/v0.4.4
archive=kiln-server-v0.4.4-x86_64-unknown-linux-gnu.tar.gz
echo '+ curl --fail --location --retry 3 --output kiln-server-v0.4.4-x86_64-unknown-linux-gnu.tar.gz.sha256 https://github.com/Jabbslad/kiln/releases/download/v0.4.4/kiln-server-v0.4.4-x86_64-unknown-linux-gnu.tar.gz.sha256'
curl --fail --location --retry 3 --output "$archive.sha256" "$base/$archive.sha256"
echo '+ curl --fail --location --retry 3 --output kiln-server-v0.4.4-x86_64-unknown-linux-gnu.tar.gz https://github.com/Jabbslad/kiln/releases/download/v0.4.4/kiln-server-v0.4.4-x86_64-unknown-linux-gnu.tar.gz'
curl --fail --location --retry 3 --output "$archive" "$base/$archive"
echo '+ cat kiln-server-v0.4.4-x86_64-unknown-linux-gnu.tar.gz.sha256'
cat "$archive.sha256"
echo '+ sha256sum -c kiln-server-v0.4.4-x86_64-unknown-linux-gnu.tar.gz.sha256'
sha256sum -c "$archive.sha256"
echo '+ sha256sum kiln-server-v0.4.4-x86_64-unknown-linux-gnu.tar.gz'
sha256sum "$archive"
echo '+ mkdir -p package && tar -xzf archive -C package'
rm -rf package
mkdir package
tar -xzf "$archive" -C package
echo '+ find package -maxdepth 2 -printf type'
find package -maxdepth 2 -printf '%y %p\n' | sort
echo '+ sha256 release installer files'
sha256sum package/install.py package/fetch-firecracker.sh
echo DOWNLOAD_OK
```

Stdout (curl progress on stderr omitted):

```text
=== date ===
Mon Oct 19 03:11:10 PM UTC 2026
+ curl --fail --location --retry 3 --output kiln-server-v0.4.4-x86_64-unknown-linux-gnu.tar.gz.sha256 https://github.com/Jabbslad/kiln/releases/download/v0.4.4/kiln-server-v0.4.4-x86_64-unknown-linux-gnu.tar.gz.sha256
+ curl --fail --location --retry 3 --output kiln-server-v0.4.4-x86_64-unknown-linux-gnu.tar.gz https://github.com/Jabbslad/kiln/releases/download/v0.4.4/kiln-server-v0.4.4-x86_64-unknown-linux-gnu.tar.gz
+ cat kiln-server-v0.4.4-x86_64-unknown-linux-gnu.tar.gz.sha256
bbc358efeb977c9a023f6427c4482f8a974eb3b9b0cb714ce07b276eaa15aa1d  kiln-server-v0.4.4-x86_64-unknown-linux-gnu.tar.gz
+ sha256sum -c kiln-server-v0.4.4-x86_64-unknown-linux-gnu.tar.gz.sha256
kiln-server-v0.4.4-x86_64-unknown-linux-gnu.tar.gz: OK
+ sha256sum kiln-server-v0.4.4-x86_64-unknown-linux-gnu.tar.gz
bbc358efeb977c9a023f6427c4482f8a974eb3b9b0cb714ce07b276eaa15aa1d  kiln-server-v0.4.4-x86_64-unknown-linux-gnu.tar.gz
+ mkdir -p package && tar -xzf archive -C package
+ find package -maxdepth 2 -printf type
d package
d package/bin
d package/deploy
d package/docs
d package/image
f package/bin/kiln
f package/bin/kiln-api
f package/bin/kiln-host
f package/bin/kiln-network
f package/bin/kiln-runtime
f package/deploy/host.example.json
f package/deploy/kiln-api.service
f package/deploy/kiln-host.service
f package/docs/identity-service.md
f package/docs/releases.md
f package/docs/remote-client.md
f package/docs/runtime.md
f package/fetch-firecracker.sh
f package/image/image.json
f package/image/inputs.json
f package/image/rootfs.ext4
f package/image/vmlinux
f package/install.py
f package/README.md
f package/release.json
+ sha256 release installer files
781f4fe4489569c6d610bae950803af32550d2914f04c29a6d58d725054a7dc5  package/install.py
5b806a40333fc128c71072ec91a4172019e5db272b6a36a12db7e335c574d948  package/fetch-firecracker.sh
DOWNLOAD_OK
```

The archive digest matches the GitHub release asset digest `sha256:bbc358efeb977c9a023f6427c4482f8a974eb3b9b0cb714ce07b276eaa15aa1d` and the published `.sha256` file. Only this server archive and its `.sha256` file were downloaded from the Kiln release. The identity archive was not downloaded.

Main `deploy/install.py` sha256 is `bf9cb1635423f397933e84cd0bf45aa7c632ff299a8edb9c5377e539d5cb58c7`. Main `scripts/fetch-firecracker.sh` sha256 is `5b806a40333fc128c71072ec91a4172019e5db272b6a36a12db7e335c574d948`.

## 5. Use main's install.py, then check only

`scp` copied main `deploy/install.py` to `/var/tmp/kiln-v044-install/package/install.py.from-main`. That copy is not a server shell command.

```sh
set -euo pipefail
cd /var/tmp/kiln-v044-install/package
echo '+ sha256sum install.py install.py.from-main fetch-firecracker.sh'
sha256sum install.py install.py.from-main fetch-firecracker.sh
echo '+ install.py debian support'
python3 - <<'PY'
from pathlib import Path
rel = Path("install.py").read_text()
main = Path("install.py.from-main").read_text()
print("release_has_debian_13", '("debian", "13")' in rel)
print("main_has_debian_13", '("debian", "13")' in main)
print("bytes_equal", rel == main)
PY
echo '+ cp install.py.from-main install.py'
cp install.py.from-main install.py
chmod 644 install.py
echo '+ sha256sum install.py'
sha256sum install.py
echo '+ python3 install.py --address 127.0.0.1'
python3 install.py --address 127.0.0.1
echo CHECK_ONLY_EXIT:$?
```

```text
+ sha256sum install.py install.py.from-main fetch-firecracker.sh
781f4fe4489569c6d610bae950803af32550d2914f04c29a6d58d725054a7dc5  install.py
bf9cb1635423f397933e84cd0bf45aa7c632ff299a8edb9c5377e539d5cb58c7  install.py.from-main
5b806a40333fc128c71072ec91a4172019e5db272b6a36a12db7e335c574d948  fetch-firecracker.sh
+ install.py debian support
release_has_debian_13 False
main_has_debian_13 True
bytes_equal False
+ cp install.py.from-main install.py
+ sha256sum install.py
bf9cb1635423f397933e84cd0bf45aa7c632ff299a8edb9c5377e539d5cb58c7  install.py
+ python3 install.py --address 127.0.0.1
Checks passed. Proposed installation:
  Ubuntu / KVM, endpoint https://127.0.0.1:8443
  /opt/kiln, /usr/local/libexec/kiln-*, /etc/kiln, /var/lib/kiln
  kiln-api and eight locked VM accounts; UID 70000..70007 / GID 71000..71007
  Enable cpu/memory/pids cgroup controllers and three systemd services
  Generate private CA, one-year server certificate and administrator token
  Build a 4 GiB / 1-vCPU warm Ubuntu template using the copy disk backend
  No overlay-pool changes
Review any other runtime's numeric ID reservations before proceeding.
  No firewall, routing or guest-network changes.
Check only: no installation performed. Rerun with sudo and --apply to install.
CHECK_ONLY_EXIT:0
```

The "Ubuntu / KVM" line is the installer's fixed text. The host is Debian 13, and this check passed on Debian 13.

## 6. Apply the install

```sh
set -u
cd /var/tmp/kiln-v044-install/package
echo '=== date ==='
date -u
echo '+ python3 install.py --address 127.0.0.1 --apply'
python3 install.py --address 127.0.0.1 --apply
echo APPLY_EXIT:$?
date -u
```

```text
=== date ===
Mon Oct 19 03:11:56 PM UTC 2026
+ python3 install.py --address 127.0.0.1 --apply
Checks passed. Proposed installation:
  Ubuntu / KVM, endpoint https://127.0.0.1:8443
  /opt/kiln, /usr/local/libexec/kiln-*, /etc/kiln, /var/lib/kiln
  kiln-api and eight locked VM accounts; UID 70000..70007 / GID 71000..71007
  Enable cpu/memory/pids cgroup controllers and three systemd services
  Generate private CA, one-year server certificate and administrator token
  Build a 4 GiB / 1-vCPU warm Ubuntu template using the copy disk backend
  No overlay-pool changes
Review any other runtime's numeric ID reservations before proceeding.
  No firewall, routing or guest-network changes.
Downloading checksum-pinned Firecracker 1.17.0…
Preparing the host-specific 4 GiB warm template…
Ready: https://127.0.0.1:8443
Keep /etc/kiln/laptop.tar.gz private for direct administrator recovery.
TLS renewal is due within one year.
One-time browser enrollment (does not restart guest VMs):
  sudo /usr/local/libexec/kiln-api enroll --url https://127.0.0.1:8443
Approve using GitHub or Google, then run kiln login on your laptop using the same provider.
Installation never enrolls a server or opens a browser automatically.
APPLY_EXIT:0
Mon Oct 19 03:12:19 PM UTC 2026
```

`install.py` calls `fetch-firecracker.sh`, which downloads `https://github.com/firecracker-microvm/firecracker/releases/download/v1.17.0/firecracker-v1.17.0-x86_64.tgz` and refuses to continue unless the tarball matches `06094a1108ae9e82aa4c23a775aa92758f53f1175d422270d9d6162cb9ade558`. Apply exited 0, so that check passed. A second download of the same tarball was checked again in the next section.

## 7. Versions, doctor, service status, journal, Firecracker checksum

```sh
set -u
echo '=== date ==='
date -u
echo '+ /opt/kiln/bin/firecracker --version'
/opt/kiln/bin/firecracker --version
echo firecracker_exit:$?
echo '+ /opt/kiln/bin/jailer --version'
/opt/kiln/bin/jailer --version
echo jailer_exit:$?
echo '+ PATH=/opt/kiln/bin:$PATH command -v firecracker jailer kiln-runtime'
PATH="/opt/kiln/bin:$PATH" bash -lc 'command -v firecracker; command -v jailer; command -v kiln-runtime'
echo '+ PATH=/opt/kiln/bin:$PATH kiln-runtime doctor'
PATH="/opt/kiln/bin:$PATH" kiln-runtime doctor
echo doctor_exit:$?
echo '+ systemctl status kiln-cgroup.service kiln-host.service kiln-api.service --no-pager --full'
systemctl status kiln-cgroup.service kiln-host.service kiln-api.service --no-pager --full
echo status_exit:$?
echo '+ journalctl -u kiln-cgroup.service -u kiln-host.service -u kiln-api.service --no-pager'
journalctl -u kiln-cgroup.service -u kiln-host.service -u kiln-api.service --no-pager
echo journal_exit:$?
echo '+ firecracker tarball checksum'
tmp=$(mktemp -d /var/tmp/fc-verify.XXXXXX)
curl --fail --location --retry 3 --output "$tmp/release.tgz" https://github.com/firecracker-microvm/firecracker/releases/download/v1.17.0/firecracker-v1.17.0-x86_64.tgz
printf '%s  %s\n' 06094a1108ae9e82aa4c23a775aa92758f53f1175d422270d9d6162cb9ade558 "$tmp/release.tgz" | sha256sum -c -
sha256sum "$tmp/release.tgz"
rm -rf "$tmp"
echo DOCTOR_SECTION_DONE
```

```text
=== date ===
Mon Oct 19 03:12:41 PM UTC 2026
+ /opt/kiln/bin/firecracker --version
Firecracker v1.17.0

2026-10-19T17:12:41.897439026 [anonymous-instance:main] Firecracker exiting successfully. exit_code=0
firecracker_exit:0
+ /opt/kiln/bin/jailer --version
Jailer v1.17.0

jailer_exit:0
+ PATH=/opt/kiln/bin:$PATH command -v firecracker jailer kiln-runtime
+ PATH=/opt/kiln/bin:$PATH kiln-runtime doctor
{
  "architecture": "x86_64",
  "development_ready": true,
  "firecracker_version": "1.17.0",
  "isolated_ready": false,
  "jailer_version": "1.17.0",
  "problems": [],
  "schema_version": 1
}
doctor_exit:0
+ systemctl status kiln-cgroup.service kiln-host.service kiln-api.service --no-pager --full
● kiln-cgroup.service - Prepare kiln cgroup v2 controllers
     Loaded: loaded (/etc/systemd/system/kiln-cgroup.service; enabled; preset: enabled)
     Active: active (exited) since Mon 2026-10-19 17:12:01 CEST; 40s ago
 Invocation: 5ad128ab716c4e71bdc9c8a892ba2133
   Main PID: 3657210 (code=exited, status=0/SUCCESS)
   Mem peak: 1.7M
        CPU: 5ms

Oct 19 17:12:01 dark-forge.dev systemd[1]: Starting kiln-cgroup.service - Prepare kiln cgroup v2 controllers...
Oct 19 17:12:01 dark-forge.dev systemd[1]: Finished kiln-cgroup.service - Prepare kiln cgroup v2 controllers.

● kiln-host.service - kiln privileged microVM host service
     Loaded: loaded (/etc/systemd/system/kiln-host.service; enabled; preset: enabled)
    Drop-In: /etc/systemd/system/kiln-host.service.d
             └─installer.conf
     Active: active (running) since Mon 2026-10-19 17:12:18 CEST; 23s ago
 Invocation: 93cc735dc78f4333bdc5acaabfc45f2d
   Main PID: 3657433 (kiln-host)
      Tasks: 13 (limit: 77002)
     Memory: 1.2M (peak: 2.1M)
        CPU: 10ms
     CGroup: /system.slice/kiln-host.service
             └─3657433 /usr/local/libexec/kiln-host --config /etc/kiln/host.json

Oct 19 17:12:18 dark-forge.dev systemd[1]: Started kiln-host.service - kiln privileged microVM host service.
Oct 19 17:12:18 dark-forge.dev kiln-host[3657433]: host service listening on /run/kiln/host.sock

● kiln-api.service - kiln unprivileged HTTPS gateway
     Loaded: loaded (/etc/systemd/system/kiln-api.service; enabled; preset: enabled)
    Drop-In: /etc/systemd/system/kiln-api.service.d
             └─installer.conf
     Active: active (running) since Mon 2026-10-19 17:12:18 CEST; 23s ago
 Invocation: 0415bcdbf98a44e49e5cbbf8242de924
   Main PID: 3657448 (kiln-api)
      Tasks: 13 (limit: 77002)
     Memory: 1.4M (peak: 2.6M)
        CPU: 21ms
     CGroup: /system.slice/kiln-api.service
             └─3657448 /usr/local/libexec/kiln-api --listen 127.0.0.1:8443 --host-socket /run/kiln/host.sock --token-file /etc/kiln/admin.token --tls-cert /etc/kiln/tls.crt --tls-key /etc/kiln/tls.key

Oct 19 17:12:18 dark-forge.dev systemd[1]: Started kiln-api.service - kiln unprivileged HTTPS gateway.
Oct 19 17:12:18 dark-forge.dev kiln-api[3657448]: HTTPS gateway listening on 127.0.0.1:8443
status_exit:0
+ journalctl -u kiln-cgroup.service -u kiln-host.service -u kiln-api.service --no-pager
Oct 19 17:12:01 dark-forge.dev systemd[1]: Starting kiln-cgroup.service - Prepare kiln cgroup v2 controllers...
Oct 19 17:12:01 dark-forge.dev systemd[1]: Finished kiln-cgroup.service - Prepare kiln cgroup v2 controllers.
Oct 19 17:12:18 dark-forge.dev systemd[1]: Started kiln-host.service - kiln privileged microVM host service.
Oct 19 17:12:18 dark-forge.dev systemd[1]: Started kiln-api.service - kiln unprivileged HTTPS gateway.
Oct 19 17:12:18 dark-forge.dev kiln-api[3657448]: HTTPS gateway listening on 127.0.0.1:8443
Oct 19 17:12:18 dark-forge.dev kiln-host[3657433]: host service listening on /run/kiln/host.sock
journal_exit:0
+ firecracker tarball checksum
/var/tmp/fc-verify.2djJDj/release.tgz: OK
06094a1108ae9e82aa4c23a775aa92758f53f1175d422270d9d6162cb9ade558  /var/tmp/fc-verify.2djJDj/release.tgz
DOCTOR_SECTION_DONE
```

`bash -lc` reset `PATH`, so that `command -v` printed nothing. Doctor still resolved Firecracker 1.17.0 from `PATH=/opt/kiln/bin:$PATH`. `isolated_ready` is false because this doctor command did not pass `--isolation-config`. The installed host service uses `/etc/kiln/isolation.json`.

Full raw doctor output:

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

## 8. Smoke test

```sh
set -u
echo '=== date ==='
date -u
echo '+ PATH=/opt/kiln/bin:$PATH command -v firecracker'
PATH="/opt/kiln/bin:$PATH" command -v firecracker
echo '+ PATH=/opt/kiln/bin:$PATH command -v jailer'
PATH="/opt/kiln/bin:$PATH" command -v jailer
echo '+ PATH=/opt/kiln/bin:$PATH command -v kiln-runtime'
PATH="/opt/kiln/bin:$PATH" command -v kiln-runtime
work=$(mktemp -d /var/tmp/kiln-smoke.XXXXXX)
chmod 700 "$work"
echo "smoke_config_dir=$work"
kiln=/opt/kiln/bin/kiln
echo '+ kiln profile add default --url https://127.0.0.1:8443 --token-file /etc/kiln/admin.token --ca-file /etc/kiln/ca.crt'
"$kiln" --config "$work/profiles.json" profile add default \
  --url https://127.0.0.1:8443 \
  --token-file /etc/kiln/admin.token \
  --ca-file /etc/kiln/ca.crt
echo profile_add_exit:$?
echo '+ kiln templates'
"$kiln" --config "$work/profiles.json" --profile default templates
echo templates_exit:$?
echo '+ kiln create --template ubuntu-4g --name kiln-smoke'
"$kiln" --config "$work/profiles.json" --profile default create --template ubuntu-4g --name kiln-smoke
echo create_exit:$?
echo '+ kiln exec kiln-smoke --timeout-ms 60000 -- /bin/sh -c uname -a; echo kiln-smoke-ok'
"$kiln" --config "$work/profiles.json" --profile default exec kiln-smoke --timeout-ms 60000 -- /bin/sh -c 'uname -a; echo kiln-smoke-ok'
echo exec_exit:$?
echo '+ kiln delete kiln-smoke'
"$kiln" --config "$work/profiles.json" --profile default delete kiln-smoke
echo delete_exit:$?
echo '+ kiln list'
"$kiln" --config "$work/profiles.json" --profile default list
echo list_exit:$?
echo SMOKE_DONE
```

Stdout:

```text
=== date ===
Mon Oct 19 03:12:56 PM UTC 2026
+ PATH=/opt/kiln/bin:$PATH command -v firecracker
/opt/kiln/bin/firecracker
+ PATH=/opt/kiln/bin:$PATH command -v jailer
/opt/kiln/bin/jailer
+ PATH=/opt/kiln/bin:$PATH command -v kiln-runtime
/opt/kiln/bin/kiln-runtime
smoke_config_dir=/var/tmp/kiln-smoke.CFfOY8
+ kiln profile add default --url https://127.0.0.1:8443 --token-file /etc/kiln/admin.token --ca-file /etc/kiln/ca.crt
added profile default
profile_add_exit:0
+ kiln templates
TEMPLATE       MEMORY  VCPUS
ubuntu-4g    4096 MiB  1
templates_exit:0
+ kiln create --template ubuntu-4g --name kiln-smoke
NAME        ID                                STATE           MEMORY  VCPUS
kiln-smoke  bcd64e3589a246ebbabca75bed34db06  running       4096 MiB  1
create_exit:0
+ kiln exec kiln-smoke --timeout-ms 60000 -- /bin/sh -c uname -a; echo kiln-smoke-ok
Linux box-bcd64e3589a2 6.1.155+ #1 SMP PREEMPT_DYNAMIC Thu Dec 18 15:17:16 UTC 2025 x86_64 x86_64 x86_64 GNU/Linux
kiln-smoke-ok
exec_exit:0
+ kiln delete kiln-smoke
deleted bcd64e3589a246ebbabca75bed34db06
delete_exit:0
+ kiln list
No boxes yet. Create one with `kiln create`.
list_exit:0
SMOKE_DONE
```

Stderr:

```text
creating kiln-smoke with template ubuntu-4g
request 06f5f845190d4dbe8a3ee68a4727612f
request d753ca467f0d49fa98e8575a7969c5ad
request 1716a17f3d0a452989abdb349861b112
```

The profile file stores the token path. It does not contain the token. `/etc/kiln/admin.token` and `/etc/kiln/laptop.tar.gz` were not printed.

## 9. Snapshot after install and smoke

```sh
set -u
echo '=== date ==='
date -u
echo '=== identity show ==='
for u in kiln-identity.service kiln-identity-proxy.service kiln-identity-proxy.socket; do
  echo "--- $u ---"
  systemctl show -p ActiveEnterTimestamp,MainPID,NRestarts -- "$u"
done
echo '=== identity and unit hashes ==='
sha256sum \
  /etc/systemd/system/kiln-identity.service \
  /etc/systemd/system/kiln-identity.service.d/10-provider-readiness.conf \
  /etc/systemd/system/kiln-identity-proxy.service \
  /etc/systemd/system/kiln-identity-proxy.socket \
  /etc/kiln-identity/github.secret \
  /etc/kiln-identity/google.secret \
  /etc/kiln-identity/service.env \
  /etc/kiln-identity/signing.pem
if [ -f /etc/kiln/login.token ]; then sha256sum /etc/kiln/login.token; else echo 'ABSENT /etc/kiln/login.token'; fi
echo '=== authorized_keys ==='
sha256sum /root/.ssh/authorized_keys
find /root /home /etc -name authorized_keys -type f -exec sha256sum {} +
echo '=== profile token leak check ==='
python3 - <<'PY'
from pathlib import Path
token = Path("/etc/kiln/admin.token").read_text().strip()
root = Path("/var/tmp")
hits = []
for path in root.rglob("profiles.json"):
    text = path.read_text(errors="replace")
    hits.append(f"{path}: token_embedded={token in text}")
print("\n".join(hits) if hits else "no profiles.json")
print("token_len", len(token))
PY
```

```text
=== date ===
Mon Oct 19 03:13:38 PM UTC 2026
=== identity show ===
--- kiln-identity.service ---
MainPID=3522047
NRestarts=0
ActiveEnterTimestamp=Mon 2026-10-19 12:42:17 CEST
--- kiln-identity-proxy.service ---
MainPID=0
NRestarts=0
ActiveEnterTimestamp=Mon 2026-10-19 16:51:59 CEST
--- kiln-identity-proxy.socket ---
ActiveEnterTimestamp=Sat 2026-10-03 20:14:48 CEST
=== identity and unit hashes ===
31667738c2b275d7b326abcdfa0b6097c947b62005506693d90ab5df80122554  /etc/systemd/system/kiln-identity.service
08aca9fe20443428eed6d902c6d9c47999f8361ea7cb7bb9b22fbd84e739f9e2  /etc/systemd/system/kiln-identity.service.d/10-provider-readiness.conf
f9e0c98316c5f83d12de0abb3b885d6060f8b05a6618ba4f1360e0fbcbd37d6e  /etc/systemd/system/kiln-identity-proxy.service
d04bd26752276f842f93c7f4f6929cdccfacf81abbdc37b35f44519eca6b5141  /etc/systemd/system/kiln-identity-proxy.socket
7508a7c9782a09e3181a4bd6a70938ba033eb826bcb8d6eafd53a3610061e495  /etc/kiln-identity/github.secret
9f7da71cffb6cdf12653975b136703b05dc836742879a194ba089733482a3757  /etc/kiln-identity/google.secret
6e0f03257ca0ed6039f1bd3ba855049a0619beba8789163696836027f8c56abb  /etc/kiln-identity/service.env
b1b7a26a116cb539bd4a19c4f4a8f9554f6896f6e7303567a4c50ebeefd87eb8  /etc/kiln-identity/signing.pem
ABSENT /etc/kiln/login.token
=== authorized_keys ===
4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37  /root/.ssh/authorized_keys
4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37  /root/.ssh/authorized_keys
=== profile token leak check ===
/var/tmp/kiln-smoke.CFfOY8/profiles.json: token_embedded=False
token_len 64
```

`dpkg -l` after the install. 369 lines. sha256 `b63a5c449c27b54383f15d202762cc6c08d03d438ff77ec70448934fa4fd4713`. `diff -u` against the pre-install listing produced no lines.

## Identity untouched

Yes. The same `systemctl show -p ActiveEnterTimestamp,MainPID,NRestarts` output was recorded before and after. Unit and config digests match. No identity unit was restarted, stopped, reloaded, or edited.

| Unit | ActiveEnterTimestamp | MainPID | NRestarts |
| --- | --- | --- | --- |
| kiln-identity.service | Mon 2026-10-19 12:42:17 CEST | 3522047 | 0 |
| kiln-identity-proxy.service | Mon 2026-10-19 16:51:59 CEST | 0 | 0 |
| kiln-identity-proxy.socket | Sat 2026-10-03 20:14:48 CEST | not printed | not printed |

`/etc/kiln/login.token` was absent before and after. It was not created.

## authorized_keys

`/root/.ssh/authorized_keys` sha256 before and after:

`4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37`

The file was not edited. A search under `/root`, `/home`, and `/etc` found only that file.

## Package diff

`dpkg -l` before and after are the same file. sha256 `b63a5c449c27b54383f15d202762cc6c08d03d438ff77ec70448934fa4fd4713`. Unified diff is empty. No apt install and no apt upgrade ran.

## Checksums

| Asset | Result |
| --- | --- |
| `kiln-server-v0.4.4-x86_64-unknown-linux-gnu.tar.gz` | `sha256sum -c` OK, `bbc358efeb977c9a023f6427c4482f8a974eb3b9b0cb714ce07b276eaa15aa1d` |
| Firecracker v1.17.0 x86_64 tarball | `sha256sum -c` OK, `06094a1108ae9e82aa4c23a775aa92758f53f1175d422270d9d6162cb9ade558` |
| Installed `install.py` | main `bf9cb1635423f397933e84cd0bf45aa7c632ff299a8edb9c5377e539d5cb58c7` |
| `fetch-firecracker.sh` | main and archive both `5b806a40333fc128c71072ec91a4172019e5db272b6a36a12db7e335c574d948` |

## Versions

```text
Firecracker v1.17.0
Jailer v1.17.0
```

## Problems

The published v0.4.4 `install.py` rejects Debian 13. This install replaced that file with main `dfd14e176f185fc5a1d852d369db0253500e1157` before `--apply`. A later release should ship the Debian 13 installer so the archive can be installed without an overlay.

The installer prints "Ubuntu / KVM" on Debian. The check itself accepted Debian 13. The message can name the detected OS.

`/etc/kiln/login.token` does not exist on this host. Nothing in this install creates it. Leave it absent unless a separate identity change is requested.

## Cleanup

The temp dirs were removed after Proof's review.

`/var/tmp/kiln-v044-install` was 2.3G. `/var/tmp/kiln-smoke.CFfOY8` was 8.0K. `fuser` is not installed, so each `fuser -v` exited 127 with `command not found`. `lsof +D` on each directory printed nothing and exited 1. `systemctl cat 'kiln*'` exited 0 and `grep -F /var/tmp` exited 1. Grep of `/etc/kiln` and `/opt/kiln` for the two directories, and for `/var/tmp`, exited 1. No unit ExecStart, WorkingDirectory, or config referenced them. `rm -rf` removed only those two paths. No package was installed. The identity units were not restarted or edited.

```sh
exec 2>&1
set -u
echo '=== date ==='
date -u
echo '=== ls -la /var/tmp/kiln-v044-install /var/tmp/kiln-smoke.CFfOY8 ==='
ls -la /var/tmp/kiln-v044-install /var/tmp/kiln-smoke.CFfOY8
ls_exit=$?
echo ls_exit:$ls_exit
echo '=== du -sh /var/tmp/kiln-v044-install /var/tmp/kiln-smoke.CFfOY8 ==='
du -sh /var/tmp/kiln-v044-install /var/tmp/kiln-smoke.CFfOY8
du_exit=$?
echo du_exit:$du_exit
echo '=== fuser -v /var/tmp/kiln-v044-install ==='
fuser -v /var/tmp/kiln-v044-install
fuser_install_exit=$?
echo fuser_install_exit:$fuser_install_exit
echo '=== fuser -v /var/tmp/kiln-smoke.CFfOY8 ==='
fuser -v /var/tmp/kiln-smoke.CFfOY8
fuser_smoke_exit=$?
echo fuser_smoke_exit:$fuser_smoke_exit
echo '=== lsof +D /var/tmp/kiln-v044-install ==='
lsof +D /var/tmp/kiln-v044-install
lsof_install_exit=$?
echo lsof_install_exit:$lsof_install_exit
echo '=== lsof +D /var/tmp/kiln-smoke.CFfOY8 ==='
lsof +D /var/tmp/kiln-smoke.CFfOY8
lsof_smoke_exit=$?
echo lsof_smoke_exit:$lsof_smoke_exit
echo '=== systemctl cat kiln* | grep -F /var/tmp ==='
systemctl cat 'kiln*' | grep -F /var/tmp
st=("${PIPESTATUS[@]}")
systemctl_cat_exit=${st[0]}
grep_units_exit=${st[1]}
echo systemctl_cat_exit:$systemctl_cat_exit
echo grep_units_exit:$grep_units_exit
echo '=== grep -R -n -F -e /var/tmp/kiln-v044-install -e /var/tmp/kiln-smoke.CFfOY8 /etc/kiln /opt/kiln ==='
grep -R -n -F -e /var/tmp/kiln-v044-install -e /var/tmp/kiln-smoke.CFfOY8 /etc/kiln /opt/kiln
grep_paths_exit=$?
echo grep_paths_exit:$grep_paths_exit
echo '=== grep -R -n -F /var/tmp /etc/kiln /opt/kiln ==='
grep -R -n -F /var/tmp /etc/kiln /opt/kiln
grep_vartmp_exit=$?
echo grep_vartmp_exit:$grep_vartmp_exit
abort=0
if [ "$ls_exit" -ne 0 ] || [ "$du_exit" -ne 0 ]; then
  echo ABORT_LISTING
  abort=1
fi
case "$fuser_install_exit" in
  0) echo ABORT_FUSER_INSTALL_IN_USE; abort=1 ;;
  1|127) ;;
  *) echo ABORT_FUSER_INSTALL_UNEXPECTED; abort=1 ;;
esac
case "$fuser_smoke_exit" in
  0) echo ABORT_FUSER_SMOKE_IN_USE; abort=1 ;;
  1|127) ;;
  *) echo ABORT_FUSER_SMOKE_UNEXPECTED; abort=1 ;;
esac
if [ "$lsof_install_exit" -ne 1 ] || [ "$lsof_smoke_exit" -ne 1 ]; then
  echo ABORT_LSOF
  abort=1
fi
if [ "$systemctl_cat_exit" -ne 0 ] || [ "$grep_units_exit" -ne 1 ]; then
  echo ABORT_UNIT_REFERENCE
  abort=1
fi
if [ "$grep_paths_exit" -ne 1 ] || [ "$grep_vartmp_exit" -ne 1 ]; then
  echo ABORT_CONFIG_REFERENCE
  abort=1
fi
if [ "$abort" -ne 0 ]; then
  echo CLEANUP_ABORTED
  exit 2
fi
echo '=== rm -rf -- /var/tmp/kiln-v044-install /var/tmp/kiln-smoke.CFfOY8 ==='
rm -rf -- /var/tmp/kiln-v044-install /var/tmp/kiln-smoke.CFfOY8
echo rm_exit:$?
echo '=== ls -ld /var/tmp/kiln-v044-install /var/tmp/kiln-smoke.CFfOY8 ==='
ls -ld /var/tmp/kiln-v044-install /var/tmp/kiln-smoke.CFfOY8
echo ls_ld_exit:$?
echo '=== command -v kiln-runtime ==='
command -v kiln-runtime
echo command_v_exit:$?
echo '=== PATH=/opt/kiln/bin:$PATH command -v kiln-runtime ==='
PATH="/opt/kiln/bin:$PATH" command -v kiln-runtime
echo '=== PATH=/opt/kiln/bin:$PATH kiln-runtime doctor ==='
PATH="/opt/kiln/bin:$PATH" kiln-runtime doctor
echo doctor_exit:$?
echo '=== systemctl show -p ActiveEnterTimestamp,MainPID,NRestarts ==='
for u in kiln-identity.service kiln-identity-proxy.service kiln-identity-proxy.socket; do
  echo "--- $u ---"
  systemctl show -p ActiveEnterTimestamp,MainPID,NRestarts -- "$u"
done
echo CLEANUP_DONE
```

Output:

```text
=== date ===
Mon Oct 19 03:25:07 PM UTC 2026
=== ls -la /var/tmp/kiln-v044-install /var/tmp/kiln-smoke.CFfOY8 ===
/var/tmp/kiln-smoke.CFfOY8:
total 12
drwx------ 2 root root 4096 Oct 19 17:12 .
drwxrwxrwt 7 root root 4096 Oct 19 17:12 ..
-rw------- 1 root root  166 Oct 19 17:12 profiles.json
-rw-r--r-- 1 root root    0 Oct 19 17:12 .profiles.lock

/var/tmp/kiln-v044-install:
total 194204
drwx------ 3 root root      4096 Oct 19 17:11 .
drwxrwxrwt 7 root root      4096 Oct 19 17:12 ..
-rw------- 1 root root 198843591 Oct 19 17:11 kiln-server-v0.4.4-x86_64-unknown-linux-gnu.tar.gz
-rw------- 1 root root       117 Oct 19 17:11 kiln-server-v0.4.4-x86_64-unknown-linux-gnu.tar.gz.sha256
drwx------ 6 root root      4096 Oct 19 17:11 package
ls_exit:0
=== du -sh /var/tmp/kiln-v044-install /var/tmp/kiln-smoke.CFfOY8 ===
2.3G	/var/tmp/kiln-v044-install
8.0K	/var/tmp/kiln-smoke.CFfOY8
du_exit:0
=== fuser -v /var/tmp/kiln-v044-install ===
bash: line 14: fuser: command not found
fuser_install_exit:127
=== fuser -v /var/tmp/kiln-smoke.CFfOY8 ===
bash: line 18: fuser: command not found
fuser_smoke_exit:127
=== lsof +D /var/tmp/kiln-v044-install ===
lsof_install_exit:1
=== lsof +D /var/tmp/kiln-smoke.CFfOY8 ===
lsof_smoke_exit:1
=== systemctl cat kiln* | grep -F /var/tmp ===
systemctl_cat_exit:0
grep_units_exit:1
=== grep -R -n -F -e /var/tmp/kiln-v044-install -e /var/tmp/kiln-smoke.CFfOY8 /etc/kiln /opt/kiln ===
grep_paths_exit:1
=== grep -R -n -F /var/tmp /etc/kiln /opt/kiln ===
grep_vartmp_exit:1
=== rm -rf -- /var/tmp/kiln-v044-install /var/tmp/kiln-smoke.CFfOY8 ===
rm_exit:0
=== ls -ld /var/tmp/kiln-v044-install /var/tmp/kiln-smoke.CFfOY8 ===
ls: cannot access '/var/tmp/kiln-v044-install': No such file or directory
ls: cannot access '/var/tmp/kiln-smoke.CFfOY8': No such file or directory
ls_ld_exit:2
=== command -v kiln-runtime ===
command_v_exit:1
=== PATH=/opt/kiln/bin:$PATH command -v kiln-runtime ===
/opt/kiln/bin/kiln-runtime
=== PATH=/opt/kiln/bin:$PATH kiln-runtime doctor ===
{
  "architecture": "x86_64",
  "development_ready": true,
  "firecracker_version": "1.17.0",
  "isolated_ready": false,
  "jailer_version": "1.17.0",
  "problems": [],
  "schema_version": 1
}
doctor_exit:0
=== systemctl show -p ActiveEnterTimestamp,MainPID,NRestarts ===
--- kiln-identity.service ---
MainPID=3522047
NRestarts=0
ActiveEnterTimestamp=Mon 2026-10-19 12:42:17 CEST
--- kiln-identity-proxy.service ---
MainPID=0
NRestarts=0
ActiveEnterTimestamp=Mon 2026-10-19 16:51:59 CEST
--- kiln-identity-proxy.socket ---
ActiveEnterTimestamp=Sat 2026-10-03 20:14:48 CEST
CLEANUP_DONE
```

`ls -ld` printed `No such file or directory` for both paths. `kiln-runtime` is not on root's default `PATH`. `PATH="/opt/kiln/bin:$PATH" kiln-runtime doctor` exited 0 and printed `"development_ready": true`.

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

`kiln-identity.service` stayed `MainPID=3522047` and `NRestarts=0`. `kiln-identity-proxy.service` stayed `MainPID=0` and `NRestarts=0`. `kiln-identity-proxy.socket` printed the same `ActiveEnterTimestamp` as the pre-install snapshot, `Sat 2026-10-03 20:14:48 CEST`.

# dark-forge.dev upgrade to Kiln v0.4.5

Record of the attempt to upgrade dark-forge.dev from Kiln 0.4.4 to the published release v0.4.5. Tag `v0.4.5` points at main `5a43e41214a018d87aebd8575a0db3647b49c83d`. The asset is `kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz` from https://github.com/Jabbslad/kiln/releases/tag/v0.4.5. No app code on the host was edited. The only repo change is this runbook.

The upgrade did not finish. The host is still Kiln 0.4.4. Firecracker and jailer are 1.17.0, the same versions recorded before the attempt. `PATH=/opt/kiln/bin:$PATH kiln-runtime doctor` still prints `"development_ready": true` for that 0.4.4 install. The smoke test did not run.

## Result

| Check | Result |
| --- | --- |
| Install `install.sh --apply` | Fail. Exit 1. |
| Doctor `development_ready` | True on the unchanged 0.4.4 install. Exit 0. This is not a v0.4.5 result. |
| Firecracker and jailer | 1.17.0 before and after. |
| Smoke test | Not run. |
| `dpkg` diff | Empty. Both lists share one sha256. |
| Login service unchanged | Yes. |
| `authorized_keys` unchanged | Yes. |
| `apt-get update` | Not run. |
| Deploy key file | Removed with `shred -u`. |

## Why the upgrade stopped

`deploy/install.sh` at tag v0.4.5 upgrades an existing server when root runs it with `--apply`. The copy inside the published archive has the same usage. The script does not install packages. It exits if `sqlite3` is missing, and that check happens before it writes `/opt/kiln/pending-upgrade.json` or stops `kiln-host` and `kiln-api`.

`sqlite3` was not on `PATH`. `libsqlite3-0` `3.46.1-7+deb13u1` was already installed. The approved package command, from `install_sqlite3_if_missing` in `deploy/bootstrap/install.sh` and from `deploy/github/install-remote.sh` at the same tag, is:

```sh
NEEDRESTART_MODE=l apt-get install -y --no-install-recommends --no-upgrade sqlite3
```

That command ran. `apt-get update` did not. Apt used the existing package lists. Those lists name `sqlite3` `3.46.1-7+deb13u1`. Both configured mirrors returned 404 for `sqlite3_3.46.1-7+deb13u1_amd64.deb`. Apt exited 100. It reported `0 upgraded, 1 newly installed, 0 to remove and 16 not upgraded`, then fetched nothing, so dpkg did not unpack a package. `install.sh --apply` then exited 1 with `missing prerequisite: sqlite3`.

There was no retry and no rollback. The installer had not replaced binaries.

## Login service and authorized_keys

`kiln-identity.service` stayed active and running. MainPID `3522047`. NRestarts `0`. ActiveEnterTimestamp `Mon 2026-10-19 12:42:17 CEST`.

`kiln-identity-proxy.service` stayed inactive and dead. MainPID `0`. NRestarts `0`. ActiveEnterTimestamp `Mon 2026-10-19 18:59:45 CEST`.

`kiln-identity-proxy.socket` stayed active and listening. ActiveEnterTimestamp `Sat 2026-10-03 20:14:48 CEST`.

`/usr/local/libexec/kiln-identity` stayed `54c89f144ef0407b0abef11a8ffe60c18acdacc82b2f9c90bdf647c1df1d76ea`. `/usr/local/libexec/kiln-identity.rollback-v033` stayed `9e36ca9ae505da5b3704915114e4b39565099871e407ff915f4103d9729eda59`.

`/root/.ssh/authorized_keys` stayed `4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37`.

`kiln-host.service` stayed MainPID `3657433`, active since `Mon 2026-10-19 17:12:18 CEST`, NRestarts `0`. `kiln-api.service` stayed MainPID `3657448` with the same timestamp and NRestarts `0`. `kiln-cgroup.service` stayed active.

## Proxy journal from PR #13

PR #13 left open why `kiln-identity-proxy.service` started at 17:42:16 CEST. The server timezone is `Europe/Berlin` (CEST, +0200). The requested window `2026-10-19 17:35` to `2026-10-19 17:50` is local time, which is 15:35 to 15:50 UTC.

`journalctl` printed one line.

```text
Oct 19 17:42:16 dark-forge.dev systemd[1]: Started kiln-identity-proxy.service - Private Caddy bridge to loopback Kiln identity.
```

That line shows systemd started the unit at 17:42:16 CEST. It does not name a client. The current ActiveEnterTimestamp is 18:59:45 CEST, outside this window, so this command does not explain that later start. At both checks the service was already inactive, MainPID 0. A later ActiveEnterTimestamp with MainPID 0 matches the socket-activated proxy exiting after idle time. This session started at 19:21 CEST, after that timestamp, and the before and after values match.

## Package diff

`diff -u /root/dpkg-before-v045.txt /root/dpkg-after-v045.txt` exited 0 and printed no lines. Both files are sha256 `b63a5c449c27b54383f15d202762cc6c08d03d438ff77ec70448934fa4fd4713` and are 369 lines. The local copy of each file has the same sha256.

The expected difference was the `sqlite3` package, plus `libsqlite3-0` only if it had been missing. `libsqlite3-0` was already installed. `sqlite3` is still absent. The diff is empty because the fetch failed before dpkg unpacked a package. Apt also reported 16 installed packages that were not upgraded. None of them changed.

## Checksums

The release checksum file is one line.

```text
9a304c273ae7c1d4de4c370f169866f5fdc3879c88f42d4b1cb2e9a125452c23  kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz
```

`sha256sum` of the tarball printed the same digest. `sha256sum -c` printed `OK`. `sha256sum` of the checksum file printed `f296af74e45a9e17a6a22be948339680d14a6b4acc1a430c37348310c5083188`, which is the GitHub asset digest for `kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz.sha256`.

`release.json` in the archive:

```text
{"version": "v0.4.5", "target": "x86_64-unknown-linux-gnu", "min_upgrade_from": "0.3.0"}
```

## Upgrade command

The exact command, run as root, was:

```sh
bash /var/tmp/kiln-v045.h0bedT/extract.gOUVHV/install.sh --apply
```

It exited 1. Full output is in the install transcript below.

## SSH

The remote prefix was `ssh -i ~/.ssh/kiln_deploy -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile=<pinned known_hosts> -o BatchMode=yes -o ConnectTimeout=15 root@dark-forge.dev`. The ssh client wrapped each remote script in `timeout`. The before and after scripts also wrap each command in `timeout`. The download and install scripts wrap the long commands in `timeout` and run inside that outer `timeout`.

The deploy key fingerprint is `SHA256:AGD6iqWdOAB1R7LeRTlbIMYyvx+FvkIDw8MTWEVtrd4`. The pinned host key is ED25519 `SHA256:/yIiijBqkbGh9lFju3N9ZhbJGIqv+NjC0R1CyWh9g9s`. The private key is not in this file. After the host work, `shred -u ~/.ssh/kiln_deploy` exited 0. A following existence check found the file gone.

## Smoke test

Not run. `install.sh` failed before it replaced binaries. `kiln create --template ubuntu-4g --name kiln-smoke` would have tested the existing 0.4.4 host, not v0.4.5.

## Files left on the host

The attempt left these paths in place:

- `/root/dpkg-before-v045.txt`
- `/root/dpkg-after-v045.txt`
- `/var/tmp/kiln-v045.h0bedT`, including the verified archive and `extract.gOUVHV`

`/var/lib/kiln/runtime/snapshots` was not listed and was not modified. `/opt/kiln/upgrades` does not exist. `/opt/kiln/pending-upgrade.json` does not exist.

## 1. Before checks

Remote script sent to `bash -s`:

### before.sh

```text
#!/bin/bash
set +e
echo "===== date -u ====="
date -u
echo "===== timedatectl ====="
timedatectl
run() {
  local name=$1
  local secs=$2
  shift 2
  echo
  echo "===== BEGIN $name ====="
  printf '+ '
  printf '%q ' "$@"
  echo
  timeout "$secs" "$@"
  local ec=$?
  echo
  echo "===== END $name exit=$ec ====="
  return 0
}
run systemctl-show 30 systemctl show kiln-identity.service kiln-identity-proxy.service kiln-identity-proxy.socket -p Id,ActiveState,SubState,MainPID,NRestarts,ActiveEnterTimestamp
run authorized-keys-sha 20 sha256sum /root/.ssh/authorized_keys
run identity-sha 20 bash -c 'sha256sum /usr/local/libexec/kiln-identity*'
run dpkg-before 60 bash -c 'dpkg -l > /root/dpkg-before-v045.txt; echo dpkg_write_exit:$?; sha256sum /root/dpkg-before-v045.txt; wc -l /root/dpkg-before-v045.txt'
run dpkg-sqlite 30 dpkg -l libsqlite3-0 sqlite3
run sqlite3-which 15 bash -c 'command -v sqlite3; echo $?'
run apt-cache-policy 30 apt-cache policy sqlite3 libsqlite3-0
run versions 30 bash -c 'for b in /opt/kiln/bin/kiln /opt/kiln/bin/kiln-runtime /opt/kiln/bin/firecracker /opt/kiln/bin/jailer /usr/local/libexec/kiln-host /usr/local/libexec/kiln-api; do echo "--- $b ---"; if [ -e "$b" ]; then "$b" --version; echo exit:$?; else echo MISSING; fi; done'
run ls-opt 20 bash -c 'ls -la /opt/kiln /opt/kiln/upgrades 2>&1'
run df 15 df -h /
run install-lock 15 bash -c 'cat /run/lock/kiln-install.lock; echo cat_exit:$?; ls -la /run/lock/kiln-install.lock'
run journalctl-proxy 40 journalctl -u kiln-identity-proxy.service --since '2026-10-19 17:35' --until '2026-10-19 17:50' --no-pager
echo
echo "===== BEFORE CHECKS DONE ====="
```

Stdout:

### before.out

```text
===== date -u =====
Mon Oct 19 05:21:15 PM UTC 2026
===== timedatectl =====
               Local time: Mon 2026-10-19 19:21:15 CEST
           Universal time: Mon 2026-10-19 17:21:15 UTC
                 RTC time: Mon 2026-10-19 17:21:15
                Time zone: Europe/Berlin (CEST, +0200)
System clock synchronized: yes
              NTP service: active
          RTC in local TZ: no

===== BEGIN systemctl-show =====
+ systemctl show kiln-identity.service kiln-identity-proxy.service kiln-identity-proxy.socket -p Id\,ActiveState\,SubState\,MainPID\,NRestarts\,ActiveEnterTimestamp 
MainPID=3522047
NRestarts=0
Id=kiln-identity.service
ActiveState=active
SubState=running
ActiveEnterTimestamp=Mon 2026-10-19 12:42:17 CEST

MainPID=0
NRestarts=0
Id=kiln-identity-proxy.service
ActiveState=inactive
SubState=dead
ActiveEnterTimestamp=Mon 2026-10-19 18:59:45 CEST

Id=kiln-identity-proxy.socket
ActiveState=active
SubState=listening
ActiveEnterTimestamp=Sat 2026-10-03 20:14:48 CEST

===== END systemctl-show exit=0 =====

===== BEGIN authorized-keys-sha =====
+ sha256sum /root/.ssh/authorized_keys 
4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37  /root/.ssh/authorized_keys

===== END authorized-keys-sha exit=0 =====

===== BEGIN identity-sha =====
+ bash -c sha256sum\ /usr/local/libexec/kiln-identity\* 
54c89f144ef0407b0abef11a8ffe60c18acdacc82b2f9c90bdf647c1df1d76ea  /usr/local/libexec/kiln-identity
9e36ca9ae505da5b3704915114e4b39565099871e407ff915f4103d9729eda59  /usr/local/libexec/kiln-identity.rollback-v033

===== END identity-sha exit=0 =====

===== BEGIN dpkg-before =====
+ bash -c dpkg\ -l\ \>\ /root/dpkg-before-v045.txt\;\ echo\ dpkg_write_exit:\$\?\;\ sha256sum\ /root/dpkg-before-v045.txt\;\ wc\ -l\ /root/dpkg-before-v045.txt 
dpkg_write_exit:0
b63a5c449c27b54383f15d202762cc6c08d03d438ff77ec70448934fa4fd4713  /root/dpkg-before-v045.txt
369 /root/dpkg-before-v045.txt

===== END dpkg-before exit=0 =====

===== BEGIN dpkg-sqlite =====
+ dpkg -l libsqlite3-0 sqlite3 
Desired=Unknown/Install/Remove/Purge/Hold
| Status=Not/Inst/Conf-files/Unpacked/halF-conf/Half-inst/trig-aWait/Trig-pend
|/ Err?=(none)/Reinst-required (Status,Err: uppercase=bad)
||/ Name               Version          Architecture Description
+++-==================-================-============-=================================
ii  libsqlite3-0:amd64 3.46.1-7+deb13u1 amd64        SQLite 3 shared library

===== END dpkg-sqlite exit=1 =====

===== BEGIN sqlite3-which =====
+ bash -c command\ -v\ sqlite3\;\ echo\ \$\? 
1

===== END sqlite3-which exit=0 =====

===== BEGIN apt-cache-policy =====
+ apt-cache policy sqlite3 libsqlite3-0 
sqlite3:
  Installed: (none)
  Candidate: 3.46.1-7+deb13u1
  Version table:
     3.46.1-7+deb13u1 500
        500 http://deb.debian.org/debian trixie/main amd64 Packages
        500 http://mirror.hetzner.com/debian/packages trixie/main amd64 Packages
libsqlite3-0:
  Installed: 3.46.1-7+deb13u1
  Candidate: 3.46.1-7+deb13u1
  Version table:
 *** 3.46.1-7+deb13u1 500
        500 http://deb.debian.org/debian trixie/main amd64 Packages
        500 http://mirror.hetzner.com/debian/packages trixie/main amd64 Packages
        100 /var/lib/dpkg/status

===== END apt-cache-policy exit=0 =====

===== BEGIN versions =====
+ bash -c for\ b\ in\ /opt/kiln/bin/kiln\ /opt/kiln/bin/kiln-runtime\ /opt/kiln/bin/firecracker\ /opt/kiln/bin/jailer\ /usr/local/libexec/kiln-host\ /usr/local/libexec/kiln-api\;\ do\ echo\ \"---\ \$b\ ---\"\;\ if\ \[\ -e\ \"\$b\"\ \]\;\ then\ \"\$b\"\ --version\;\ echo\ exit:\$\?\;\ else\ echo\ MISSING\;\ fi\;\ done 
--- /opt/kiln/bin/kiln ---
kiln 0.4.4
exit:0
--- /opt/kiln/bin/kiln-runtime ---
kiln-runtime 0.4.4
exit:0
--- /opt/kiln/bin/firecracker ---
Firecracker v1.17.0

2026-10-19T19:21:15.900730726 [anonymous-instance:main] Firecracker exiting successfully. exit_code=0
exit:0
--- /opt/kiln/bin/jailer ---
Jailer v1.17.0

exit:0
--- /usr/local/libexec/kiln-host ---
kiln-server 0.4.4
exit:0
--- /usr/local/libexec/kiln-api ---
kiln-server 0.4.4
exit:0

===== END versions exit=0 =====

===== BEGIN ls-opt =====
+ bash -c ls\ -la\ /opt/kiln\ /opt/kiln/upgrades\ 2\>\&1 
ls: cannot access '/opt/kiln/upgrades': No such file or directory
/opt/kiln:
total 12
drwxr-xr-x 3 root root 4096 Oct 19 17:11 .
drwxr-xr-x 5 root root 4096 Oct 19 17:11 ..
drwxr-xr-x 2 root root 4096 Oct 19 17:11 bin

===== END ls-opt exit=2 =====

===== BEGIN df =====
+ df -h / 
Filesystem      Size  Used Avail Use% Mounted on
/dev/md2        436G   53G  362G  13% /

===== END df exit=0 =====

===== BEGIN install-lock =====
+ bash -c cat\ /run/lock/kiln-install.lock\;\ echo\ cat_exit:\$\?\;\ ls\ -la\ /run/lock/kiln-install.lock 
cat_exit:0
-rw------- 1 root root 0 Oct 19 17:11 /run/lock/kiln-install.lock

===== END install-lock exit=0 =====

===== BEGIN journalctl-proxy =====
+ journalctl -u kiln-identity-proxy.service --since 2026-10-19\ 17:35 --until 2026-10-19\ 17:50 --no-pager 
Oct 19 17:42:16 dark-forge.dev systemd[1]: Started kiln-identity-proxy.service - Private Caddy bridge to loopback Kiln identity.

===== END journalctl-proxy exit=0 =====

===== BEFORE CHECKS DONE =====
```

Stderr:

### before.err

```text
dpkg-query: no packages found matching sqlite3
```

## 2. Download, checksum, and extract

Remote script sent to `bash -s`:

### download.sh

```text
#!/bin/bash
set -u
echo "===== date -u ====="
date -u
work=$(mktemp -d /var/tmp/kiln-v045.XXXXXX)
chmod 700 "$work"
echo "work=$work"
cd "$work"
base=https://github.com/Jabbslad/kiln/releases/download/v0.4.5
name=kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz
echo "+ curl -fsSL -o $name.sha256 $base/$name.sha256"
timeout 60 curl -fsSL -o "$name.sha256" "$base/$name.sha256"
echo curl_sha_exit:$?
echo "===== checksum file ====="
cat "$name.sha256"
echo
echo "+ sha256sum $name.sha256"
sha256sum "$name.sha256"
echo "+ curl -fL --retry 3 -o $name $base/$name"
timeout 280 curl -fL --retry 3 --retry-delay 2 -o "$name" "$base/$name"
echo curl_tar_exit:$?
echo "+ sha256sum $name"
sha256sum "$name"
echo "+ sha256sum -c $name.sha256"
sha256sum -c "$name.sha256"
echo sha_check_exit:$?
echo "+ tar -tzf $name"
timeout 60 tar -tzf "$name"
echo tar_list_exit:$?
extract=$(mktemp -d "$work/extract.XXXXXX")
chmod 700 "$extract"
echo "extract=$extract"
echo "+ tar -xzf $name -C $extract"
timeout 120 tar -xzf "$name" -C "$extract"
echo tar_extract_exit:$?
echo "+ ls -la $extract"
ls -la "$extract"
echo "===== install.sh usage lines ====="
grep -n 'usage:' "$extract/install.sh" || true
echo "===== apt lines in archive install.sh ====="
grep -n 'apt-get' "$extract/install.sh" || echo 'no apt-get in archive install.sh'
echo "===== sqlite3 prerequisite lines ====="
grep -n 'sqlite3' "$extract/install.sh" || true
echo DOWNLOAD_DONE
```

Stdout:

### download.out

```text
===== date -u =====
Mon Oct 19 05:22:54 PM UTC 2026
work=/var/tmp/kiln-v045.h0bedT
+ curl -fsSL -o kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz.sha256 https://github.com/Jabbslad/kiln/releases/download/v0.4.5/kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz.sha256
curl_sha_exit:0
===== checksum file =====
9a304c273ae7c1d4de4c370f169866f5fdc3879c88f42d4b1cb2e9a125452c23  kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz

+ sha256sum kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz.sha256
f296af74e45a9e17a6a22be948339680d14a6b4acc1a430c37348310c5083188  kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz.sha256
+ curl -fL --retry 3 -o kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz https://github.com/Jabbslad/kiln/releases/download/v0.4.5/kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz
curl_tar_exit:0
+ sha256sum kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz
9a304c273ae7c1d4de4c370f169866f5fdc3879c88f42d4b1cb2e9a125452c23  kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz
+ sha256sum -c kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz.sha256
kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz: OK
sha_check_exit:0
+ tar -tzf kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz
bin/kiln-runtime
bin/kiln-host
bin/kiln-api
bin/kiln
install.sh
bin/kiln-network
fetch-firecracker.sh
README.md
docs/releases.md
docs/remote-client.md
docs/identity-service.md
docs/runtime.md
deploy/kiln-host.service
deploy/kiln-api.service
deploy/host.example.json
image/image.json
image/inputs.json
image/vmlinux
image/rootfs.ext4
release.json
tar_list_exit:0
extract=/var/tmp/kiln-v045.h0bedT/extract.gOUVHV
+ tar -xzf kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz -C /var/tmp/kiln-v045.h0bedT/extract.gOUVHV
tar_extract_exit:0
+ ls -la /var/tmp/kiln-v045.h0bedT/extract.gOUVHV
total 92
drwx------ 6 root root  4096 Oct 19 19:23 .
drwx------ 3 root root  4096 Oct 19 19:23 ..
drwxr-xr-x 2 root root  4096 Oct 19 19:23 bin
drwxr-xr-x 2 root root  4096 Oct 19 19:23 deploy
drwxr-xr-x 2 root root  4096 Oct 19 19:23 docs
-rw-r--r-- 1 root root  1058 Jan  1  1970 fetch-firecracker.sh
drwxr-xr-x 2 root root  4096 Oct 19 19:23 image
-rwxr-xr-x 1 root root 39415 Jan  1  1970 install.sh
-rw-r--r-- 1 root root 16865 Jan  1  1970 README.md
-rw-r--r-- 1 root root    88 Jan  1  1970 release.json
===== install.sh usage lines =====
1121:        [ "$#" -ge 2 ] || die "usage: install.sh [--address IP] [--apply] [--network] [--network-uplink INTERFACE]"
1126:        [ "$#" -ge 2 ] || die "usage: install.sh [--address IP] [--apply] [--network] [--network-uplink INTERFACE]"
1131:        [ "$#" -ge 2 ] || die "usage: install.sh --healthcheck ADDRESS"
1135:      *) die "usage: install.sh [--address IP] [--apply] [--network] [--network-uplink INTERFACE]" ;;
===== apt lines in archive install.sh =====
no apt-get in archive install.sh
===== sqlite3 prerequisite lines =====
283:  for tool in openssl systemctl systemd-analyze groupadd useradd curl bash tar sha256sum install realpath sqlite3 perl flock sync; do
849:  have_tool sqlite3 || return 1
850:  count=$(sqlite3 -readonly "$journal" 'PRAGMA user_version;' 2>/dev/null) || return 1
854:    count=$(sqlite3 -readonly "$journal" 'SELECT count(*) FROM operations WHERE running=1;' 2>/dev/null) || return 1
1018:  have_tool sqlite3 || die "missing prerequisite: sqlite3; install host packages before retrying"
DOWNLOAD_DONE
```

Stderr:

### download.err

```text
  % Total    % Received % Xferd  Average Speed   Time    Time     Time  Current
                                 Dload  Upload   Total   Spent    Left  Speed

  0     0    0     0    0     0      0      0 --:--:-- --:--:-- --:--:--     0
  0     0    0     0    0     0      0      0 --:--:-- --:--:-- --:--:--     0

  4  189M    4 9532k    0     0  16.1M      0  0:00:11 --:--:--  0:00:11 16.1M
 22  189M   22 42.0M    0     0  27.0M      0  0:00:07  0:00:01  0:00:06 33.5M
 36  189M   36 69.7M    0     0  27.4M      0  0:00:06  0:00:02  0:00:04 30.7M
 52  189M   52 99.8M    0     0  28.2M      0  0:00:06  0:00:03  0:00:03 30.5M
 73  189M   73  139M    0     0  30.8M      0  0:00:06  0:00:04  0:00:02 32.9M
 81  189M   81  154M    0     0  27.8M      0  0:00:06  0:00:05  0:00:01 29.2M
100  189M  100  189M    0     0  29.5M      0  0:00:06  0:00:06 --:--:-- 30.3M
```

## 3. sqlite3 install and archive install.sh

Remote script sent to `bash -s`:

### install.sh

```text
#!/bin/bash
set -u
echo "===== date -u ====="
date -u
extract=/var/tmp/kiln-v045.h0bedT/extract.gOUVHV
echo "===== release.json ====="
cat "$extract/release.json"
echo
echo "===== lock probe ====="
python3 - << 'PY'
import fcntl
path = "/run/lock/kiln-install.lock"
f = open(path, "a+")
try:
    fcntl.flock(f.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
except BlockingIOError:
    print("lock_held")
    raise SystemExit(1)
else:
    print("lock_free")
    fcntl.flock(f.fileno(), fcntl.LOCK_UN)
PY
echo lock_probe_exit:$?
if ! command -v sqlite3 >/dev/null 2>&1; then
  echo "+ NEEDRESTART_MODE=l apt-get install -y --no-install-recommends --no-upgrade sqlite3"
  NEEDRESTART_MODE=l apt-get install -y --no-install-recommends --no-upgrade sqlite3
  echo apt_sqlite_exit:$?
else
  echo "sqlite3 already on PATH; apt not run"
fi
echo "+ command -v sqlite3"
command -v sqlite3
echo sqlite_which_exit:$?
echo "+ sqlite3 --version"
sqlite3 --version
echo "+ bash $extract/install.sh --apply"
timeout 240 bash "$extract/install.sh" --apply
echo install_exit:$?
echo INSTALL_SCRIPT_DONE
```

Stdout:

### install.out

```text
===== date -u =====
Mon Oct 19 05:23:57 PM UTC 2026
===== release.json =====
{"version": "v0.4.5", "target": "x86_64-unknown-linux-gnu", "min_upgrade_from": "0.3.0"}
===== lock probe =====
lock_free
lock_probe_exit:0
+ NEEDRESTART_MODE=l apt-get install -y --no-install-recommends --no-upgrade sqlite3
Reading package lists...
Building dependency tree...
Reading state information...
Suggested packages:
  sqlite3-doc
The following NEW packages will be installed:
  sqlite3
0 upgraded, 1 newly installed, 0 to remove and 16 not upgraded.
Need to get 384 kB of archives.
After this operation, 601 kB of additional disk space will be used.
Ign:1 http://deb.debian.org/debian trixie/main amd64 sqlite3 amd64 3.46.1-7+deb13u1
Err:1 http://mirror.hetzner.com/debian/packages trixie/main amd64 sqlite3 amd64 3.46.1-7+deb13u1
  404  Not Found [IP: 2a04:4e42:8e::644 80]
  404  Not Found [IP: 2a01:4ff:ff00::3:3 80]
apt_sqlite_exit:100
+ command -v sqlite3
sqlite_which_exit:1
+ sqlite3 --version
+ bash /var/tmp/kiln-v045.h0bedT/extract.gOUVHV/install.sh --apply
install_exit:1
INSTALL_SCRIPT_DONE
```

Stderr:

### install.err

```text
E: Failed to fetch http://mirror.hetzner.com/debian/packages/pool/main/s/sqlite3/sqlite3_3.46.1-7%2bdeb13u1_amd64.deb  404  Not Found [IP: 2a01:4ff:ff00::3:3 80]
E: Unable to fetch some archives, maybe run apt-get update or try with --fix-missing?
bash: line 35: sqlite3: command not found
Setup stopped: missing prerequisite: sqlite3; install host packages before retrying
Retain state and inspect logs; do not delete VM state or blindly retry an interrupted update.
```

## 4. After checks

`/root/dpkg-before-v045.txt` was copied back before the install attempt. Its local sha256 matched the server sha256 `b63a5c449c27b54383f15d202762cc6c08d03d438ff77ec70448934fa4fd4713`. `/root/dpkg-after-v045.txt` was copied back after the attempt and matched the same sha256. A local `diff -u` of the two copies exited 0 and printed nothing.

Remote script sent to `bash -s`:

### after.sh

```text
#!/bin/bash
set +e
echo "===== date -u ====="
date -u
run() {
  local name=$1
  local secs=$2
  shift 2
  echo
  echo "===== BEGIN $name ====="
  printf '+ '
  printf '%q ' "$@"
  echo
  timeout "$secs" "$@"
  local ec=$?
  echo
  echo "===== END $name exit=$ec ====="
  return 0
}
run versions 30 bash -c 'for b in /opt/kiln/bin/kiln /opt/kiln/bin/kiln-runtime /opt/kiln/bin/firecracker /opt/kiln/bin/jailer /usr/local/libexec/kiln-host /usr/local/libexec/kiln-api; do echo "--- $b ---"; if [ -e "$b" ]; then "$b" --version; echo exit:$?; else echo MISSING; fi; done'
run doctor 60 bash -c 'PATH=/opt/kiln/bin:$PATH kiln-runtime doctor; echo doctor_exit:$?'
run systemctl-show 30 systemctl show kiln-identity.service kiln-identity-proxy.service kiln-identity-proxy.socket -p Id,ActiveState,SubState,MainPID,NRestarts,ActiveEnterTimestamp
run authorized-keys-sha 20 sha256sum /root/.ssh/authorized_keys
run identity-sha 20 bash -c 'sha256sum /usr/local/libexec/kiln-identity*'
run dpkg-after 60 bash -c 'dpkg -l > /root/dpkg-after-v045.txt; echo dpkg_write_exit:$?; sha256sum /root/dpkg-after-v045.txt; echo "+ diff -u /root/dpkg-before-v045.txt /root/dpkg-after-v045.txt"; diff -u /root/dpkg-before-v045.txt /root/dpkg-after-v045.txt; echo diff_exit:$?'
run dpkg-sqlite 20 dpkg -l libsqlite3-0 sqlite3
run sqlite3-which 15 bash -c 'command -v sqlite3; echo $?'
run upgrades 20 bash -c 'ls -la /opt/kiln/upgrades 2>&1; echo; echo "+ ls -la /opt/kiln/pending-upgrade.json"; ls -la /opt/kiln/pending-upgrade.json 2>&1; echo; echo "+ cat /opt/kiln/pending-upgrade.json"; cat /opt/kiln/pending-upgrade.json 2>&1'
run services 20 bash -c 'systemctl is-active kiln-host.service kiln-api.service kiln-cgroup.service; echo; systemctl show kiln-host.service kiln-api.service -p Id,ActiveState,SubState,MainPID,NRestarts,ActiveEnterTimestamp'
echo
echo AFTER_DONE
```

Stdout:

### after.out

```text
===== date -u =====
Mon Oct 19 05:24:40 PM UTC 2026

===== BEGIN versions =====
+ bash -c for\ b\ in\ /opt/kiln/bin/kiln\ /opt/kiln/bin/kiln-runtime\ /opt/kiln/bin/firecracker\ /opt/kiln/bin/jailer\ /usr/local/libexec/kiln-host\ /usr/local/libexec/kiln-api\;\ do\ echo\ \"---\ \$b\ ---\"\;\ if\ \[\ -e\ \"\$b\"\ \]\;\ then\ \"\$b\"\ --version\;\ echo\ exit:\$\?\;\ else\ echo\ MISSING\;\ fi\;\ done 
--- /opt/kiln/bin/kiln ---
kiln 0.4.4
exit:0
--- /opt/kiln/bin/kiln-runtime ---
kiln-runtime 0.4.4
exit:0
--- /opt/kiln/bin/firecracker ---
Firecracker v1.17.0

2026-10-19T19:24:40.723796588 [anonymous-instance:main] Firecracker exiting successfully. exit_code=0
exit:0
--- /opt/kiln/bin/jailer ---
Jailer v1.17.0

exit:0
--- /usr/local/libexec/kiln-host ---
kiln-server 0.4.4
exit:0
--- /usr/local/libexec/kiln-api ---
kiln-server 0.4.4
exit:0

===== END versions exit=0 =====

===== BEGIN doctor =====
+ bash -c PATH=/opt/kiln/bin:\$PATH\ kiln-runtime\ doctor\;\ echo\ doctor_exit:\$\? 
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

===== END doctor exit=0 =====

===== BEGIN systemctl-show =====
+ systemctl show kiln-identity.service kiln-identity-proxy.service kiln-identity-proxy.socket -p Id\,ActiveState\,SubState\,MainPID\,NRestarts\,ActiveEnterTimestamp 
MainPID=3522047
NRestarts=0
Id=kiln-identity.service
ActiveState=active
SubState=running
ActiveEnterTimestamp=Mon 2026-10-19 12:42:17 CEST

MainPID=0
NRestarts=0
Id=kiln-identity-proxy.service
ActiveState=inactive
SubState=dead
ActiveEnterTimestamp=Mon 2026-10-19 18:59:45 CEST

Id=kiln-identity-proxy.socket
ActiveState=active
SubState=listening
ActiveEnterTimestamp=Sat 2026-10-03 20:14:48 CEST

===== END systemctl-show exit=0 =====

===== BEGIN authorized-keys-sha =====
+ sha256sum /root/.ssh/authorized_keys 
4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37  /root/.ssh/authorized_keys

===== END authorized-keys-sha exit=0 =====

===== BEGIN identity-sha =====
+ bash -c sha256sum\ /usr/local/libexec/kiln-identity\* 
54c89f144ef0407b0abef11a8ffe60c18acdacc82b2f9c90bdf647c1df1d76ea  /usr/local/libexec/kiln-identity
9e36ca9ae505da5b3704915114e4b39565099871e407ff915f4103d9729eda59  /usr/local/libexec/kiln-identity.rollback-v033

===== END identity-sha exit=0 =====

===== BEGIN dpkg-after =====
+ bash -c dpkg\ -l\ \>\ /root/dpkg-after-v045.txt\;\ echo\ dpkg_write_exit:\$\?\;\ sha256sum\ /root/dpkg-after-v045.txt\;\ echo\ \"+\ diff\ -u\ /root/dpkg-before-v045.txt\ /root/dpkg-after-v045.txt\"\;\ diff\ -u\ /root/dpkg-before-v045.txt\ /root/dpkg-after-v045.txt\;\ echo\ diff_exit:\$\? 
dpkg_write_exit:0
b63a5c449c27b54383f15d202762cc6c08d03d438ff77ec70448934fa4fd4713  /root/dpkg-after-v045.txt
+ diff -u /root/dpkg-before-v045.txt /root/dpkg-after-v045.txt
diff_exit:0

===== END dpkg-after exit=0 =====

===== BEGIN dpkg-sqlite =====
+ dpkg -l libsqlite3-0 sqlite3 
Desired=Unknown/Install/Remove/Purge/Hold
| Status=Not/Inst/Conf-files/Unpacked/halF-conf/Half-inst/trig-aWait/Trig-pend
|/ Err?=(none)/Reinst-required (Status,Err: uppercase=bad)
||/ Name               Version          Architecture Description
+++-==================-================-============-=================================
ii  libsqlite3-0:amd64 3.46.1-7+deb13u1 amd64        SQLite 3 shared library

===== END dpkg-sqlite exit=1 =====

===== BEGIN sqlite3-which =====
+ bash -c command\ -v\ sqlite3\;\ echo\ \$\? 
1

===== END sqlite3-which exit=0 =====

===== BEGIN upgrades =====
+ bash -c ls\ -la\ /opt/kiln/upgrades\ 2\>\&1\;\ echo\;\ echo\ \"+\ ls\ -la\ /opt/kiln/pending-upgrade.json\"\;\ ls\ -la\ /opt/kiln/pending-upgrade.json\ 2\>\&1\;\ echo\;\ echo\ \"+\ cat\ /opt/kiln/pending-upgrade.json\"\;\ cat\ /opt/kiln/pending-upgrade.json\ 2\>\&1 
ls: cannot access '/opt/kiln/upgrades': No such file or directory

+ ls -la /opt/kiln/pending-upgrade.json
ls: cannot access '/opt/kiln/pending-upgrade.json': No such file or directory

+ cat /opt/kiln/pending-upgrade.json
cat: /opt/kiln/pending-upgrade.json: No such file or directory

===== END upgrades exit=1 =====

===== BEGIN services =====
+ bash -c systemctl\ is-active\ kiln-host.service\ kiln-api.service\ kiln-cgroup.service\;\ echo\;\ systemctl\ show\ kiln-host.service\ kiln-api.service\ -p\ Id\,ActiveState\,SubState\,MainPID\,NRestarts\,ActiveEnterTimestamp 
active
active
active

MainPID=3657433
NRestarts=0
Id=kiln-host.service
ActiveState=active
SubState=running
ActiveEnterTimestamp=Mon 2026-10-19 17:12:18 CEST

MainPID=3657448
NRestarts=0
Id=kiln-api.service
ActiveState=active
SubState=running
ActiveEnterTimestamp=Mon 2026-10-19 17:12:18 CEST

===== END services exit=0 =====

AFTER_DONE
```

Stderr:

### after.err

```text
dpkg-query: no packages found matching sqlite3
```

## 5. dpkg diff

Server command, also present in the after transcript:

```sh
diff -u /root/dpkg-before-v045.txt /root/dpkg-after-v045.txt
```

Stdout is empty. Exit code is 0. The two files are identical.

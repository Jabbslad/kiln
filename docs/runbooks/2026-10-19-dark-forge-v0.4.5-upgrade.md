# dark-forge.dev upgrade to Kiln v0.4.5

Attempt 2 upgraded dark-forge.dev to Kiln 0.4.5. That record is the section "Attempt 2: apt-get update then upgrade" at the end of this file. The text from the next paragraph through the attempt 1 package diff is the first attempt, which stopped before any package change.

Record of the attempt to upgrade dark-forge.dev from Kiln 0.4.4 to the published release v0.4.5. Tag `v0.4.5` points at main `5a43e41214a018d87aebd8575a0db3647b49c83d`. The asset is `kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz` from https://github.com/Jabbslad/kiln/releases/tag/v0.4.5. No app code on the host was edited. The only repo change is this runbook.

The upgrade did not finish. The host is still Kiln 0.4.4. Firecracker and jailer are 1.17.0, the same versions recorded before the attempt. `PATH=/opt/kiln/bin:$PATH kiln-runtime doctor` still prints `"development_ready": true` for that 0.4.4 install. The smoke test did not run.

## Attempt 1 result

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

## Attempt 2: apt-get update then upgrade

Jamie approved `apt-get update`, then installing `sqlite3`, and upgrading `libsqlite3-0` if `sqlite3` required the newer library. This attempt ran `apt-get update` with that approval. The install used the published v0.4.5 archive only. No app code on the host was edited.

The remote prefix was `ssh -i ~/.ssh/kiln_deploy -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile=<pinned known_hosts> -o BatchMode=yes -o ConnectTimeout=15 root@dark-forge.dev`. Each ssh invocation wrapped the remote script in `timeout`. The scripts below wrap each command in `timeout` as well. The deploy key fingerprint was `SHA256:AGD6iqWdOAB1R7LeRTlbIMYyvx+FvkIDw8MTWEVtrd4`. The pinned host key was ED25519 `SHA256:/yIiijBqkbGh9lFju3N9ZhbJGIqv+NjC0R1CyWh9g9s`. The private key is not in this file. After the host work, `shred -u ~/.ssh/kiln_deploy` exited 0. A following existence check found the file gone.

### Result

| Check | Result |
| --- | --- |
| `sqlite3` install | Pass. `sqlite3` 3.46.1-7+deb13u2 installed. `libsqlite3-0` upgraded from 3.46.1-7+deb13u1 to 3.46.1-7+deb13u2. Apt exit 0. |
| `install.sh --apply` | Pass. Exit 0. |
| Kiln versions | `kiln`, `kiln-runtime`, `kiln-host`, and `kiln-api` report 0.4.5. |
| Firecracker and jailer | Both 1.17.0. |
| Doctor `development_ready` | True. Exit 0. |
| Smoke test | Pass. Guest stdout was `kiln-smoke-ok-6.1.155+`. The box was deleted and a later list was empty. |
| `dpkg` diff | Only `libsqlite3-0` and the new `sqlite3` package. |
| Login service unchanged | Yes. |
| `authorized_keys` unchanged | Yes. |
| `apt-get update` | Run, with Jamie's approval. Exit 0. |
| Deploy key file | Removed with `shred -u`. |

`kiln-host.service` MainPID moved from 3657433 to 3739721. `kiln-api.service` MainPID moved from 3657448 to 3739725. Both were active and running after the upgrade, NRestarts 0, ActiveEnterTimestamp `Mon 2026-10-19 19:52:11 CEST`. `install.sh` stops and starts those two units. Their new pids are the expected result of `--apply`.

`/opt/kiln/upgrades/0.4.4-8DDh4B` holds the previous binaries. `/opt/kiln/pending-upgrade.json` is absent. The installer removes that file after a successful swap.

### Package gate

After `apt-get update`, `apt-cache policy` showed `sqlite3` not installed, candidate `3.46.1-7+deb13u2`, and `libsqlite3-0` installed at `3.46.1-7+deb13u1` with candidate `3.46.1-7+deb13u2`.

`apt-get install -s --no-install-recommends --no-upgrade sqlite3` exited 0. The plan installed `sqlite3` and upgraded `libsqlite3-0`. It removed nothing. The summary line was `1 upgraded, 1 newly installed, 0 to remove and 72 not upgraded`.

`apt-get install -s --no-install-recommends sqlite3 libsqlite3-0` exited 0 with the same two-package plan.

Nothing outside `sqlite3` and `libsqlite3-0` would be installed, upgraded, or removed, so the gate passed. The `--no-upgrade` simulation was satisfiable on its own, so the install command was:

```sh
NEEDRESTART_MODE=l DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends --no-upgrade sqlite3
```

That command upgraded `libsqlite3-0` and installed `sqlite3`. `sqlite3 --version` printed:

```text
3.46.1 2024-08-13 09:16:08 c9c2ab54ba1f5f46360f1b4f35d849cd3f080e6fc2b6c60e91b16c63f69aalt1 (64-bit)
```

The 72 other upgradable packages were left alone. There was no `apt-get upgrade` and no `dist-upgrade`.

### Upgrade command

The archive was downloaded from `https://github.com/Jabbslad/kiln/releases/download/v0.4.5/`. `sha256sum -c` printed `OK`. The tarball digest was `9a304c273ae7c1d4de4c370f169866f5fdc3879c88f42d4b1cb2e9a125452c23`. The checksum file digest was `f296af74e45a9e17a6a22be948339680d14a6b4acc1a430c37348310c5083188`.

The exact command, run as root, was:

```sh
bash /var/tmp/kiln-v045-a2.Y72RmH/extract.sbwGaB/install.sh --apply
```

It exited 0. Full output is in the install transcript below.

### dpkg diff

```text
--- /root/dpkg-before-v045-a2.txt	2026-10-19 19:50:41.091076056 +0200
+++ /root/dpkg-after-v045-a2.txt	2026-10-19 19:53:04.000248029 +0200
@@ -240,7 +240,7 @@
 ii  libslang2:amd64                  2.3.3-5+b2                           amd64        S-Lang programming library - runtime version
 ii  libsmartcols1:amd64              2.41-5                               amd64        smart column output alignment library
 ii  libsodium23:amd64                1.0.18-1+deb13u1                     amd64        Network communication, cryptography and signaturing library
-ii  libsqlite3-0:amd64               3.46.1-7+deb13u1                     amd64        SQLite 3 shared library
+ii  libsqlite3-0:amd64               3.46.1-7+deb13u2                     amd64        SQLite 3 shared library
 ii  libss2:amd64                     1.47.2-3+b11                         amd64        command-line interface parsing library
 ii  libssh2-1t64:amd64               1.11.1-1+deb13u1                     amd64        SSH2 client-side library
 ii  libssl3t64:amd64                 3.5.6-1~deb13u2                      amd64        Secure Sockets Layer toolkit - shared libraries
@@ -335,6 +335,7 @@
 ii  sed                              4.9-2+deb13u1                        amd64        GNU stream editor for filtering/transforming text
 ii  sensible-utils                   0.0.25                               all          Utilities for sensible alternative selection
 ii  shared-mime-info                 2.4-5+b2                             amd64        FreeDesktop.org shared MIME database and spec
+ii  sqlite3                          3.46.1-7+deb13u2                     amd64        Command line interface for SQLite 3
 ii  sqv                              1.3.0-3+b2                           amd64        OpenPGP signature verification program from Sequoia
 ii  ssh-import-id                    5.10-3                               all          securely retrieve an SSH public key and install it locally
 ii  sudo                             1.9.16p2-3+deb13u2                   amd64        Provide limited super user privileges to specific users
```

`diff -u` exited 1 because the files differ. Before sha256 `b63a5c449c27b54383f15d202762cc6c08d03d438ff77ec70448934fa4fd4713`. After sha256 `d388be45b10aa31fa9ab14681dd30f24fa77e122363f6acb724f6f507703807d`.

### Smoke test

`kiln` was not on root's default `PATH` (`/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin`). The smoke commands prepended `/opt/kiln/bin`. `kiln --help` shows `create --template` and `--name`, default issuer `https://dark-forge.dev`, and a profile via `--config` or the default config path. Root had no `/root/.config/kiln/profiles.json`. `KILN_IDENTITY_URL` was unset. The API unit listens on `127.0.0.1:8443`.

The working profile was a temporary file, not a copy of the admin token. `kiln profile add` stored paths to `/etc/kiln/admin.token` and `/etc/kiln/ca.crt`. The file was removed after the test, along with `/var/tmp/.profiles.lock`, which `profile add` created beside that config.

The first smoke script created that config with `mktemp`, so the file existed and was empty. `kiln profile add` then failed with `EOF while parsing a value at line 1 column 0`. No box was created. The retry used a path that did not exist yet.

The successful commands were:

```sh
export PATH=/opt/kiln/bin:$PATH
kiln --config /var/tmp/kiln-smoke-profile-a2.json profile add default --url https://127.0.0.1:8443 --token-file /etc/kiln/admin.token --ca-file /etc/kiln/ca.crt
kiln --config /var/tmp/kiln-smoke-profile-a2.json create --template ubuntu-4g --name kiln-smoke
kiln --config /var/tmp/kiln-smoke-profile-a2.json exec kiln-smoke -- sh -c 'echo kiln-smoke-ok-$(uname -r)'
kiln --config /var/tmp/kiln-smoke-profile-a2.json delete kiln-smoke
kiln --config /var/tmp/kiln-smoke-profile-a2.json list
```

Create printed box `b181377b5cd74a90bb615f080ea018b8` in state `running`. Exec stdout was `kiln-smoke-ok-6.1.155+`. Delete printed `deleted b181377b5cd74a90bb615f080ea018b8`. The following list printed `No boxes yet. Create one with kiln create.`

### Files left on the host

- `/root/dpkg-before-v045-a2.txt`
- `/root/dpkg-after-v045-a2.txt`
- `/var/tmp/kiln-v045-a2.Y72RmH`, including the verified archive and `extract.sbwGaB`
- `/opt/kiln/upgrades/0.4.4-8DDh4B`
- The attempt 1 files `/root/dpkg-before-v045.txt`, `/root/dpkg-after-v045.txt`, and `/var/tmp/kiln-v045.h0bedT`

`/opt/kiln/pending-upgrade.json` does not exist. `/var/lib/kiln/runtime/snapshots` was not listed and was not modified. The temporary profile `/var/tmp/kiln-smoke-profile-a2.json` was removed.

### 1. Before checks

Remote script sent to `bash -s`. The outer ssh `timeout` was 90 seconds. ssh exit 0.

### before.sh

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
run systemctl-show 30 systemctl show kiln-identity.service kiln-identity-proxy.service kiln-identity-proxy.socket -p Id,ActiveState,SubState,MainPID,NRestarts,ActiveEnterTimestamp
run authorized-keys-sha 20 sha256sum /root/.ssh/authorized_keys
run identity-sha 20 bash -c 'sha256sum /usr/local/libexec/kiln-identity*'
run dpkg-before 60 bash -c 'dpkg -l > /root/dpkg-before-v045-a2.txt; echo dpkg_write_exit:$?; sha256sum /root/dpkg-before-v045-a2.txt; wc -l /root/dpkg-before-v045-a2.txt'
run dpkg-sqlite 30 dpkg -l sqlite3 libsqlite3-0
run sqlite3-which 15 bash -c 'command -v sqlite3; echo command_v_exit:$?'
run versions 40 bash -c 'for b in /opt/kiln/bin/kiln /opt/kiln/bin/kiln-runtime /opt/kiln/bin/firecracker /opt/kiln/bin/jailer /usr/local/libexec/kiln-host /usr/local/libexec/kiln-api; do echo "--- $b ---"; if [ -e "$b" ]; then "$b" --version; echo exit:$?; else echo MISSING; fi; done'
run host-api-show 30 systemctl show kiln-host.service kiln-api.service -p Id,ActiveState,SubState,MainPID,NRestarts,ActiveEnterTimestamp
echo
echo "===== BEFORE CHECKS DONE ====="
```

Stdout:

### before.out

```text
===== date -u =====
Mon Oct 19 05:50:41 PM UTC 2026

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
+ bash -c dpkg\ -l\ \>\ /root/dpkg-before-v045-a2.txt\;\ echo\ dpkg_write_exit:\$\?\;\ sha256sum\ /root/dpkg-before-v045-a2.txt\;\ wc\ -l\ /root/dpkg-before-v045-a2.txt 
dpkg_write_exit:0
b63a5c449c27b54383f15d202762cc6c08d03d438ff77ec70448934fa4fd4713  /root/dpkg-before-v045-a2.txt
369 /root/dpkg-before-v045-a2.txt

===== END dpkg-before exit=0 =====

===== BEGIN dpkg-sqlite =====
+ dpkg -l sqlite3 libsqlite3-0 
Desired=Unknown/Install/Remove/Purge/Hold
| Status=Not/Inst/Conf-files/Unpacked/halF-conf/Half-inst/trig-aWait/Trig-pend
|/ Err?=(none)/Reinst-required (Status,Err: uppercase=bad)
||/ Name               Version          Architecture Description
+++-==================-================-============-=================================
ii  libsqlite3-0:amd64 3.46.1-7+deb13u1 amd64        SQLite 3 shared library

===== END dpkg-sqlite exit=1 =====

===== BEGIN sqlite3-which =====
+ bash -c command\ -v\ sqlite3\;\ echo\ command_v_exit:\$\? 
command_v_exit:1

===== END sqlite3-which exit=0 =====

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

2026-10-19T19:50:41.112528754 [anonymous-instance:main] Firecracker exiting successfully. exit_code=0
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

===== BEGIN host-api-show =====
+ systemctl show kiln-host.service kiln-api.service -p Id\,ActiveState\,SubState\,MainPID\,NRestarts\,ActiveEnterTimestamp 
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

===== END host-api-show exit=0 =====

===== BEFORE CHECKS DONE =====
```

Stderr:

### before.err

```text
dpkg-query: no packages found matching sqlite3
```

ssh exit 0. `dpkg -l sqlite3` wrote the stderr line because the package was not installed.

### 2. apt-get update

Jamie approved this command. Remote script sent to `bash -s`. The outer ssh `timeout` was 330 seconds. ssh exit 0.

### apt-update.sh

```text
#!/bin/bash
set +e
echo "===== date -u ====="
date -u
echo "+ timeout 300 apt-get update"
timeout 300 apt-get update
echo apt_update_exit:$?
echo "===== APT UPDATE DONE ====="
```

Stdout:

### apt-update.out

```text
===== date -u =====
Mon Oct 19 05:50:48 PM UTC 2026
+ timeout 300 apt-get update
Get:1 http://mirror.hetzner.com/debian/packages trixie InRelease [140 kB]
Get:2 http://deb.debian.org/debian trixie InRelease [140 kB]
Get:3 http://deb.debian.org/debian trixie-updates InRelease [47.3 kB]
Get:4 http://mirror.hetzner.com/debian/packages trixie-updates InRelease [47.3 kB]
Get:5 http://mirror.hetzner.com/debian/packages trixie-backports InRelease [54.0 kB]
Get:6 http://mirror.hetzner.com/debian/security trixie-security InRelease [43.4 kB]
Get:7 https://download.docker.com/linux/debian trixie InRelease [32.5 kB]
Get:8 http://mirror.hetzner.com/debian/packages trixie/main amd64 Packages [9,678 kB]
Get:9 http://deb.debian.org/debian trixie-backports InRelease [54.0 kB]
Get:10 http://deb.debian.org/debian-security trixie-security InRelease [43.4 kB]
Get:11 https://download.docker.com/linux/debian trixie/stable amd64 Packages [49.8 kB]
Get:12 http://deb.debian.org/debian trixie/main Sources [10.5 MB]
Get:13 http://mirror.hetzner.com/debian/packages trixie/main Translation-en [6,486 kB]
Get:14 http://deb.debian.org/debian trixie/contrib Sources [52.4 kB]
Get:15 http://deb.debian.org/debian trixie/main amd64 Packages [9,678 kB]
Get:16 http://mirror.hetzner.com/debian/packages trixie/contrib amd64 Packages [53.9 kB]
Get:17 http://mirror.hetzner.com/debian/packages trixie-backports/main amd64 Packages [289 kB]
Get:18 http://mirror.hetzner.com/debian/packages trixie-backports/main Translation-en [226 kB]
Get:19 http://mirror.hetzner.com/debian/packages trixie-backports/contrib amd64 Packages [7,976 B]
Get:20 http://mirror.hetzner.com/debian/packages trixie-backports/non-free amd64 Packages [7,956 B]
Get:21 http://mirror.hetzner.com/debian/packages trixie-backports/non-free-firmware amd64 Packages [6,756 B]
Get:22 http://mirror.hetzner.com/debian/packages trixie-backports/non-free-firmware Translation-en [4,568 B]
Get:23 http://mirror.hetzner.com/debian/security trixie-security/main amd64 Packages [269 kB]
Get:24 http://mirror.hetzner.com/debian/security trixie-security/main Translation-en [166 kB]
Get:25 http://mirror.hetzner.com/debian/security trixie-security/contrib amd64 Packages [3,152 B]
Get:26 http://mirror.hetzner.com/debian/security trixie-security/contrib Translation-en [2,564 B]
Get:27 http://deb.debian.org/debian trixie/main Translation-en [6,486 kB]
Get:28 http://deb.debian.org/debian trixie/contrib amd64 Packages [53.9 kB]
Get:29 http://deb.debian.org/debian trixie-backports/non-free Sources [1,452 B]
Get:30 http://deb.debian.org/debian trixie-backports/non-free-firmware Sources [3,436 B]
Get:31 http://deb.debian.org/debian trixie-backports/contrib Sources [7,448 B]
Get:32 http://deb.debian.org/debian trixie-backports/main Sources [327 kB]
Get:33 http://deb.debian.org/debian trixie-backports/main amd64 Packages [289 kB]
Get:34 http://deb.debian.org/debian trixie-backports/main Translation-en [226 kB]
Get:35 http://deb.debian.org/debian trixie-backports/contrib amd64 Packages [7,976 B]
Get:36 http://deb.debian.org/debian trixie-backports/non-free amd64 Packages [7,956 B]
Get:37 http://deb.debian.org/debian trixie-backports/non-free-firmware amd64 Packages [6,756 B]
Get:38 http://deb.debian.org/debian trixie-backports/non-free-firmware Translation-en [4,568 B]
Get:39 http://deb.debian.org/debian-security trixie-security/main Sources [243 kB]
Get:40 http://deb.debian.org/debian-security trixie-security/contrib Sources [1,572 B]
Get:41 http://deb.debian.org/debian-security trixie-security/main amd64 Packages [269 kB]
Get:42 http://deb.debian.org/debian-security trixie-security/main Translation-en [166 kB]
Get:43 http://deb.debian.org/debian-security trixie-security/contrib amd64 Packages [3,152 B]
Get:44 http://deb.debian.org/debian-security trixie-security/contrib Translation-en [2,564 B]
Fetched 46.2 MB in 2s (20.9 MB/s)
Reading package lists...
apt_update_exit:0
===== APT UPDATE DONE =====
```

Stderr:

### apt-update.err

```text
(empty)
```

ssh exit 0. `apt_update_exit` is 0.

### 3. Dry run

Remote script sent to `bash -s`. The outer ssh `timeout` was 180 seconds. ssh exit 0. The gate in the result section used this output. No install ran until that gate passed.

### dry-run.sh

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
run apt-cache-policy 40 apt-cache policy sqlite3 libsqlite3-0
run apt-sim-no-upgrade 60 apt-get install -s --no-install-recommends --no-upgrade sqlite3
run apt-sim-with-lib 60 apt-get install -s --no-install-recommends sqlite3 libsqlite3-0
echo
echo "===== DRY RUN DONE ====="
```

Stdout:

### dry-run.out

```text
===== date -u =====
Mon Oct 19 05:50:58 PM UTC 2026

===== BEGIN apt-cache-policy =====
+ apt-cache policy sqlite3 libsqlite3-0 
sqlite3:
  Installed: (none)
  Candidate: 3.46.1-7+deb13u2
  Version table:
     3.46.1-7+deb13u2 500
        500 http://deb.debian.org/debian trixie/main amd64 Packages
        500 http://mirror.hetzner.com/debian/packages trixie/main amd64 Packages
libsqlite3-0:
  Installed: 3.46.1-7+deb13u1
  Candidate: 3.46.1-7+deb13u2
  Version table:
     3.46.1-7+deb13u2 500
        500 http://deb.debian.org/debian trixie/main amd64 Packages
        500 http://mirror.hetzner.com/debian/packages trixie/main amd64 Packages
 *** 3.46.1-7+deb13u1 100
        100 /var/lib/dpkg/status

===== END apt-cache-policy exit=0 =====

===== BEGIN apt-sim-no-upgrade =====
+ apt-get install -s --no-install-recommends --no-upgrade sqlite3 
Reading package lists...
Building dependency tree...
Reading state information...
The following additional packages will be installed:
  libsqlite3-0
Suggested packages:
  sqlite3-doc
The following NEW packages will be installed:
  sqlite3
The following packages will be upgraded:
  libsqlite3-0
1 upgraded, 1 newly installed, 0 to remove and 72 not upgraded.
Inst libsqlite3-0 [3.46.1-7+deb13u1] (3.46.1-7+deb13u2 Debian:13.7/stable [amd64])
Conf libsqlite3-0 (3.46.1-7+deb13u2 Debian:13.7/stable [amd64])
Inst sqlite3 (3.46.1-7+deb13u2 Debian:13.7/stable [amd64])
Conf sqlite3 (3.46.1-7+deb13u2 Debian:13.7/stable [amd64])

===== END apt-sim-no-upgrade exit=0 =====

===== BEGIN apt-sim-with-lib =====
+ apt-get install -s --no-install-recommends sqlite3 libsqlite3-0 
Reading package lists...
Building dependency tree...
Reading state information...
Suggested packages:
  sqlite3-doc
The following NEW packages will be installed:
  sqlite3
The following packages will be upgraded:
  libsqlite3-0
1 upgraded, 1 newly installed, 0 to remove and 72 not upgraded.
Inst libsqlite3-0 [3.46.1-7+deb13u1] (3.46.1-7+deb13u2 Debian:13.7/stable [amd64])
Conf libsqlite3-0 (3.46.1-7+deb13u2 Debian:13.7/stable [amd64])
Inst sqlite3 (3.46.1-7+deb13u2 Debian:13.7/stable [amd64])
Conf sqlite3 (3.46.1-7+deb13u2 Debian:13.7/stable [amd64])

===== END apt-sim-with-lib exit=0 =====

===== DRY RUN DONE =====
```

Stderr:

### dry-run.err

```text
(empty)
```

ssh exit 0. Both simulations exited 0.

### 4. sqlite3 install

Remote script sent to `bash -s`. The outer ssh `timeout` was 240 seconds. ssh exit 0.

### apt-install.sh

```text
#!/bin/bash
set +e
echo "===== date -u ====="
date -u
echo "+ NEEDRESTART_MODE=l DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends --no-upgrade sqlite3"
timeout 180 env NEEDRESTART_MODE=l DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends --no-upgrade sqlite3
echo apt_install_exit:$?
echo "+ command -v sqlite3"
command -v sqlite3
echo command_v_exit:$?
echo "+ sqlite3 --version"
sqlite3 --version
echo sqlite_version_exit:$?
echo "+ dpkg -l sqlite3 libsqlite3-0"
dpkg -l sqlite3 libsqlite3-0
echo dpkg_l_exit:$?
echo "===== APT INSTALL DONE ====="
```

Stdout:

### apt-install.out

```text
===== date -u =====
Mon Oct 19 05:51:39 PM UTC 2026
+ NEEDRESTART_MODE=l DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends --no-upgrade sqlite3
Reading package lists...
Building dependency tree...
Reading state information...
The following additional packages will be installed:
  libsqlite3-0
Suggested packages:
  sqlite3-doc
The following NEW packages will be installed:
  sqlite3
The following packages will be upgraded:
  libsqlite3-0
1 upgraded, 1 newly installed, 0 to remove and 72 not upgraded.
Need to get 1,299 kB of archives.
After this operation, 601 kB of additional disk space will be used.
Get:1 http://deb.debian.org/debian trixie/main amd64 libsqlite3-0 amd64 3.46.1-7+deb13u2 [915 kB]
Get:2 http://deb.debian.org/debian trixie/main amd64 sqlite3 amd64 3.46.1-7+deb13u2 [384 kB]
Fetched 1,299 kB in 0s (12.1 MB/s)
(Reading database ... 
(Reading database ... 5%
(Reading database ... 10%
(Reading database ... 15%
(Reading database ... 20%
(Reading database ... 25%
(Reading database ... 30%
(Reading database ... 35%
(Reading database ... 40%
(Reading database ... 45%
(Reading database ... 50%
(Reading database ... 55%
(Reading database ... 60%
(Reading database ... 65%
(Reading database ... 70%
(Reading database ... 75%
(Reading database ... 80%
(Reading database ... 85%
(Reading database ... 90%
(Reading database ... 95%
(Reading database ... 100%
(Reading database ... 37152 files and directories currently installed.)
Preparing to unpack .../libsqlite3-0_3.46.1-7+deb13u2_amd64.deb ...
Unpacking libsqlite3-0:amd64 (3.46.1-7+deb13u2) over (3.46.1-7+deb13u1) ...
Setting up libsqlite3-0:amd64 (3.46.1-7+deb13u2) ...
Selecting previously unselected package sqlite3.
(Reading database ... 
(Reading database ... 5%
(Reading database ... 10%
(Reading database ... 15%
(Reading database ... 20%
(Reading database ... 25%
(Reading database ... 30%
(Reading database ... 35%
(Reading database ... 40%
(Reading database ... 45%
(Reading database ... 50%
(Reading database ... 55%
(Reading database ... 60%
(Reading database ... 65%
(Reading database ... 70%
(Reading database ... 75%
(Reading database ... 80%
(Reading database ... 85%
(Reading database ... 90%
(Reading database ... 95%
(Reading database ... 100%
(Reading database ... 37152 files and directories currently installed.)
Preparing to unpack .../sqlite3_3.46.1-7+deb13u2_amd64.deb ...
Unpacking sqlite3 (3.46.1-7+deb13u2) ...
Setting up sqlite3 (3.46.1-7+deb13u2) ...
Processing triggers for man-db (2.13.1-1) ...
Processing triggers for libc-bin (2.41-12+deb13u3) ...
apt_install_exit:0
+ command -v sqlite3
/usr/bin/sqlite3
command_v_exit:0
+ sqlite3 --version
3.46.1 2024-08-13 09:16:08 c9c2ab54ba1f5f46360f1b4f35d849cd3f080e6fc2b6c60e91b16c63f69aalt1 (64-bit)
sqlite_version_exit:0
+ dpkg -l sqlite3 libsqlite3-0
Desired=Unknown/Install/Remove/Purge/Hold
| Status=Not/Inst/Conf-files/Unpacked/halF-conf/Half-inst/trig-aWait/Trig-pend
|/ Err?=(none)/Reinst-required (Status,Err: uppercase=bad)
||/ Name               Version          Architecture Description
+++-==================-================-============-===================================
ii  libsqlite3-0:amd64 3.46.1-7+deb13u2 amd64        SQLite 3 shared library
ii  sqlite3            3.46.1-7+deb13u2 amd64        Command line interface for SQLite 3
dpkg_l_exit:0
===== APT INSTALL DONE =====
```

Stderr:

### apt-install.err

```text
(empty)
```

ssh exit 0. `apt_install_exit` is 0.

### 5. Download and install.sh --apply

Remote script sent to `bash -s`. The outer ssh `timeout` was 980 seconds. ssh exit 0.

### kiln-install.sh

```text
#!/bin/bash
set -u
echo "===== date -u ====="
date -u
work=$(mktemp -d /var/tmp/kiln-v045-a2.XXXXXX)
echo "work=$work"
cd "$work"
name=kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz
base=https://github.com/Jabbslad/kiln/releases/download/v0.4.5
echo "+ timeout 60 curl -fsSL -o $name.sha256 $base/$name.sha256"
timeout 60 curl -fsSL -o "$name.sha256" "$base/$name.sha256"
echo curl_sha_exit:$?
echo "===== checksum file ====="
cat "$name.sha256"
echo
echo "+ sha256sum $name.sha256"
sha256sum "$name.sha256"
echo "+ timeout 280 curl -fL --retry 3 --retry-delay 2 -o $name $base/$name"
timeout 280 curl -fL --retry 3 --retry-delay 2 -o "$name" "$base/$name"
echo curl_tar_exit:$?
echo "+ sha256sum $name"
sha256sum "$name"
echo "+ sha256sum -c $name.sha256"
sha256sum -c "$name.sha256"
echo sha_check_exit:$?
actual=$(sha256sum "$name" | awk '{print $1}')
expect=9a304c273ae7c1d4de4c370f169866f5fdc3879c88f42d4b1cb2e9a125452c23
echo "expect=$expect"
echo "actual=$actual"
if [ "$actual" != "$expect" ]; then
  echo "CHECKSUM_MISMATCH"
  echo INSTALL_SCRIPT_DONE
  exit 0
fi
extract=$(mktemp -d "$work/extract.XXXXXX")
echo "extract=$extract"
echo "+ timeout 60 tar -xzf $name -C $extract"
timeout 60 tar -xzf "$name" -C "$extract"
echo tar_exit:$?
echo "===== release.json ====="
cat "$extract/release.json"
echo
echo "+ bash $extract/install.sh --apply"
timeout 600 bash "$extract/install.sh" --apply
echo install_exit:$?
echo INSTALL_SCRIPT_DONE
```

Stdout:

### kiln-install.out

```text
===== date -u =====
Mon Oct 19 05:51:58 PM UTC 2026
work=/var/tmp/kiln-v045-a2.Y72RmH
+ timeout 60 curl -fsSL -o kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz.sha256 https://github.com/Jabbslad/kiln/releases/download/v0.4.5/kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz.sha256
curl_sha_exit:0
===== checksum file =====
9a304c273ae7c1d4de4c370f169866f5fdc3879c88f42d4b1cb2e9a125452c23  kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz

+ sha256sum kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz.sha256
f296af74e45a9e17a6a22be948339680d14a6b4acc1a430c37348310c5083188  kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz.sha256
+ timeout 280 curl -fL --retry 3 --retry-delay 2 -o kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz https://github.com/Jabbslad/kiln/releases/download/v0.4.5/kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz
curl_tar_exit:0
+ sha256sum kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz
9a304c273ae7c1d4de4c370f169866f5fdc3879c88f42d4b1cb2e9a125452c23  kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz
+ sha256sum -c kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz.sha256
kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz: OK
sha_check_exit:0
expect=9a304c273ae7c1d4de4c370f169866f5fdc3879c88f42d4b1cb2e9a125452c23
actual=9a304c273ae7c1d4de4c370f169866f5fdc3879c88f42d4b1cb2e9a125452c23
extract=/var/tmp/kiln-v045-a2.Y72RmH/extract.sbwGaB
+ timeout 60 tar -xzf kiln-server-v0.4.5-x86_64-unknown-linux-gnu.tar.gz -C /var/tmp/kiln-v045-a2.Y72RmH/extract.sbwGaB
tar_exit:0
===== release.json =====
{"version": "v0.4.5", "target": "x86_64-unknown-linux-gnu", "min_upgrade_from": "0.3.0"}
+ bash /var/tmp/kiln-v045-a2.Y72RmH/extract.sbwGaB/install.sh --apply
Kiln 0.4.4 -> 0.4.5: preserve configuration and VM state; briefly disconnect API/SSH sessions.
Updated to Kiln 0.4.5. Ready: https://127.0.0.1:8443
Previous binaries: /opt/kiln/upgrades/0.4.4-8DDh4B
One-time browser enrollment (does not restart guest VMs):
  sudo /usr/local/libexec/kiln-api enroll --url https://127.0.0.1:8443
Approve using GitHub or Google, then run kiln login on your laptop using the same provider.
Installation never enrolls a server or opens a browser automatically.
install_exit:0
INSTALL_SCRIPT_DONE
```

Stderr:

### kiln-install.err

```text
  % Total    % Received % Xferd  Average Speed   Time    Time     Time  Current
                                 Dload  Upload   Total   Spent    Left  Speed

  0     0    0     0    0     0      0      0 --:--:-- --:--:-- --:--:--     0
  0     0    0     0    0     0      0      0 --:--:-- --:--:-- --:--:--     0

 26  189M   26 49.4M    0     0  60.9M      0  0:00:03 --:--:--  0:00:03 60.9M
 81  189M   81  154M    0     0  85.2M      0  0:00:02  0:00:01  0:00:01  104M
100  189M  100  189M    0     0  86.0M      0  0:00:02  0:00:02 --:--:--  100M
```

ssh exit 0. `install_exit` is 0. Curl's progress meter is the stderr.

### 6. After checks

Remote script sent to `bash -s`. The outer ssh `timeout` was 220 seconds. ssh exit 0.

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
run versions 40 bash -c 'for b in /opt/kiln/bin/kiln /opt/kiln/bin/kiln-runtime /opt/kiln/bin/firecracker /opt/kiln/bin/jailer /usr/local/libexec/kiln-host /usr/local/libexec/kiln-api; do echo "--- $b ---"; if [ -e "$b" ]; then "$b" --version; echo exit:$?; else echo MISSING; fi; done'
run doctor 60 bash -c 'PATH=/opt/kiln/bin:$PATH kiln-runtime doctor; echo doctor_exit:$?'
run host-api-show 30 systemctl show kiln-host.service kiln-api.service -p Id,ActiveState,SubState,MainPID,NRestarts,ActiveEnterTimestamp
run upgrades 20 bash -c 'ls -la /opt/kiln/upgrades; echo ls_upgrades_exit:$?; ls -la /opt/kiln/pending-upgrade.json; echo ls_pending_exit:$?'
run systemctl-show 30 systemctl show kiln-identity.service kiln-identity-proxy.service kiln-identity-proxy.socket -p Id,ActiveState,SubState,MainPID,NRestarts,ActiveEnterTimestamp
run authorized-keys-sha 20 sha256sum /root/.ssh/authorized_keys
run identity-sha 20 bash -c 'sha256sum /usr/local/libexec/kiln-identity*'
run dpkg-after 60 bash -c 'dpkg -l > /root/dpkg-after-v045-a2.txt; echo dpkg_write_exit:$?; sha256sum /root/dpkg-before-v045-a2.txt /root/dpkg-after-v045-a2.txt; echo "+ diff -u /root/dpkg-before-v045-a2.txt /root/dpkg-after-v045-a2.txt"; diff -u /root/dpkg-before-v045-a2.txt /root/dpkg-after-v045-a2.txt; echo diff_exit:$?'
run kiln-help 30 bash -c 'PATH=/opt/kiln/bin:$PATH kiln --help; echo help_exit:$?'
run kiln-create-help 20 bash -c 'PATH=/opt/kiln/bin:$PATH kiln create --help; echo create_help_exit:$?'
run kiln-exec-help 20 bash -c 'PATH=/opt/kiln/bin:$PATH kiln exec --help; echo exec_help_exit:$?'
run profile-probe 20 python3 -c 'import json, os
p="/root/.config/kiln/profiles.json"
print("profiles_exists", os.path.exists(p))
print("KILN_IDENTITY_URL", os.environ.get("KILN_IDENTITY_URL", ""))
print("XDG_CONFIG_HOME", os.environ.get("XDG_CONFIG_HOME", ""))
if os.path.exists(p):
    data=json.load(open(p))
    profiles=data.get("profiles", {})
    print("profile_count", len(profiles))
    for name, prof in profiles.items():
        kind="central" if "issuer" in prof else "direct" if "url" in prof else "unknown"
        origin=""
        server=prof.get("server")
        if isinstance(server, dict):
            origin=server.get("origin", "")
        print("profile", name, "kind", kind, "issuer", prof.get("issuer", ""), "url", prof.get("url", ""), "server_origin", origin)
'
echo
echo "===== AFTER CHECKS DONE ====="
```

Stdout:

### after.out

```text
===== date -u =====
Mon Oct 19 05:53:03 PM UTC 2026

===== BEGIN versions =====
+ bash -c for\ b\ in\ /opt/kiln/bin/kiln\ /opt/kiln/bin/kiln-runtime\ /opt/kiln/bin/firecracker\ /opt/kiln/bin/jailer\ /usr/local/libexec/kiln-host\ /usr/local/libexec/kiln-api\;\ do\ echo\ \"---\ \$b\ ---\"\;\ if\ \[\ -e\ \"\$b\"\ \]\;\ then\ \"\$b\"\ --version\;\ echo\ exit:\$\?\;\ else\ echo\ MISSING\;\ fi\;\ done 
--- /opt/kiln/bin/kiln ---
kiln 0.4.5
exit:0
--- /opt/kiln/bin/kiln-runtime ---
kiln-runtime 0.4.5
exit:0
--- /opt/kiln/bin/firecracker ---
Firecracker v1.17.0

2026-10-19T19:53:03.941631057 [anonymous-instance:main] Firecracker exiting successfully. exit_code=0
exit:0
--- /opt/kiln/bin/jailer ---
Jailer v1.17.0

exit:0
--- /usr/local/libexec/kiln-host ---
kiln-server 0.4.5
exit:0
--- /usr/local/libexec/kiln-api ---
kiln-server 0.4.5
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

===== BEGIN host-api-show =====
+ systemctl show kiln-host.service kiln-api.service -p Id\,ActiveState\,SubState\,MainPID\,NRestarts\,ActiveEnterTimestamp 
MainPID=3739721
NRestarts=0
Id=kiln-host.service
ActiveState=active
SubState=running
ActiveEnterTimestamp=Mon 2026-10-19 19:52:11 CEST

MainPID=3739725
NRestarts=0
Id=kiln-api.service
ActiveState=active
SubState=running
ActiveEnterTimestamp=Mon 2026-10-19 19:52:11 CEST

===== END host-api-show exit=0 =====

===== BEGIN upgrades =====
+ bash -c ls\ -la\ /opt/kiln/upgrades\;\ echo\ ls_upgrades_exit:\$\?\;\ ls\ -la\ /opt/kiln/pending-upgrade.json\;\ echo\ ls_pending_exit:\$\? 
total 12
drwx------ 3 root root 4096 Oct 19 19:52 .
drwxr-xr-x 4 root root 4096 Oct 19 19:52 ..
drwx------ 2 root root 4096 Oct 19 19:52 0.4.4-8DDh4B
ls_upgrades_exit:0
ls_pending_exit:2

===== END upgrades exit=0 =====

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
+ bash -c dpkg\ -l\ \>\ /root/dpkg-after-v045-a2.txt\;\ echo\ dpkg_write_exit:\$\?\;\ sha256sum\ /root/dpkg-before-v045-a2.txt\ /root/dpkg-after-v045-a2.txt\;\ echo\ \"+\ diff\ -u\ /root/dpkg-before-v045-a2.txt\ /root/dpkg-after-v045-a2.txt\"\;\ diff\ -u\ /root/dpkg-before-v045-a2.txt\ /root/dpkg-after-v045-a2.txt\;\ echo\ diff_exit:\$\? 
dpkg_write_exit:0
b63a5c449c27b54383f15d202762cc6c08d03d438ff77ec70448934fa4fd4713  /root/dpkg-before-v045-a2.txt
d388be45b10aa31fa9ab14681dd30f24fa77e122363f6acb724f6f507703807d  /root/dpkg-after-v045-a2.txt
+ diff -u /root/dpkg-before-v045-a2.txt /root/dpkg-after-v045-a2.txt
--- /root/dpkg-before-v045-a2.txt	2026-10-19 19:50:41.091076056 +0200
+++ /root/dpkg-after-v045-a2.txt	2026-10-19 19:53:04.000248029 +0200
@@ -240,7 +240,7 @@
 ii  libslang2:amd64                  2.3.3-5+b2                           amd64        S-Lang programming library - runtime version
 ii  libsmartcols1:amd64              2.41-5                               amd64        smart column output alignment library
 ii  libsodium23:amd64                1.0.18-1+deb13u1                     amd64        Network communication, cryptography and signaturing library
-ii  libsqlite3-0:amd64               3.46.1-7+deb13u1                     amd64        SQLite 3 shared library
+ii  libsqlite3-0:amd64               3.46.1-7+deb13u2                     amd64        SQLite 3 shared library
 ii  libss2:amd64                     1.47.2-3+b11                         amd64        command-line interface parsing library
 ii  libssh2-1t64:amd64               1.11.1-1+deb13u1                     amd64        SSH2 client-side library
 ii  libssl3t64:amd64                 3.5.6-1~deb13u2                      amd64        Secure Sockets Layer toolkit - shared libraries
@@ -335,6 +335,7 @@
 ii  sed                              4.9-2+deb13u1                        amd64        GNU stream editor for filtering/transforming text
 ii  sensible-utils                   0.0.25                               all          Utilities for sensible alternative selection
 ii  shared-mime-info                 2.4-5+b2                             amd64        FreeDesktop.org shared MIME database and spec
+ii  sqlite3                          3.46.1-7+deb13u2                     amd64        Command line interface for SQLite 3
 ii  sqv                              1.3.0-3+b2                           amd64        OpenPGP signature verification program from Sequoia
 ii  ssh-import-id                    5.10-3                               all          securely retrieve an SSH public key and install it locally
 ii  sudo                             1.9.16p2-3+deb13u2                   amd64        Provide limited super user privileges to specific users
diff_exit:1

===== END dpkg-after exit=0 =====

===== BEGIN kiln-help =====
+ bash -c PATH=/opt/kiln/bin:\$PATH\ kiln\ --help\;\ echo\ help_exit:\$\? 
Manage Kiln microVMs over authenticated HTTPS

Usage: kiln [OPTIONS] <COMMAND>

Commands:
  login       
  logout      
  auth        
  ssh         Open a shell or run a command in a box
  cp          Copy a file using BOX:/absolute/path for the remote endpoint
  ssh-config  
  profile     
  templates   
  list        
  inspect     
  create      Create a box; defaults to the only available template and a generated name
  pause       
  resume      
  stop        
  start       
  delete      
  exec        
  operation   
  help        Print this message or the help of the given subcommand(s)

Options:
      --config <CONFIG>
          
      --profile <PROFILE>
          
      --issuer <ISSUER>
          Override the identity service (default: https://dark-forge.dev)
      --auth-token-file <AUTH_TOKEN_FILE>
          
      --json
          
      --request-id <REQUEST_ID>
          Reuse only for an identical request; never generates a duplicate operation
      --no-wait
          Return accepted operation immediately; the server continues working
      --wait-seconds <WAIT_SECONDS>
          [default: 300]
  -h, --help
          Print help
  -V, --version
          Print version
help_exit:0

===== END kiln-help exit=0 =====

===== BEGIN kiln-create-help =====
+ bash -c PATH=/opt/kiln/bin:\$PATH\ kiln\ create\ --help\;\ echo\ create_help_exit:\$\? 
Create a box; defaults to the only available template and a generated name

Usage: kiln create [OPTIONS]

Options:
      --config <CONFIG>
          
      --template <TEMPLATE>
          
      --name <NAME>
          
      --profile <PROFILE>
          
      --issuer <ISSUER>
          Override the identity service (default: https://dark-forge.dev)
      --auth-token-file <AUTH_TOKEN_FILE>
          
      --json
          
      --request-id <REQUEST_ID>
          Reuse only for an identical request; never generates a duplicate operation
      --no-wait
          Return accepted operation immediately; the server continues working
      --wait-seconds <WAIT_SECONDS>
          [default: 300]
  -h, --help
          Print help
create_help_exit:0

===== END kiln-create-help exit=0 =====

===== BEGIN kiln-exec-help =====
+ bash -c PATH=/opt/kiln/bin:\$PATH\ kiln\ exec\ --help\;\ echo\ exec_help_exit:\$\? 
Usage: kiln exec [OPTIONS] <BOX> -- <ARGV>...

Arguments:
  <BOX>      
  <ARGV>...  

Options:
      --config <CONFIG>
          
      --timeout-ms <TIMEOUT_MS>
          [default: 10000]
      --cwd <CWD>
          
      --profile <PROFILE>
          
      --env <ENV>
          
      --issuer <ISSUER>
          Override the identity service (default: https://dark-forge.dev)
      --auth-token-file <AUTH_TOKEN_FILE>
          
      --json
          
      --request-id <REQUEST_ID>
          Reuse only for an identical request; never generates a duplicate operation
      --no-wait
          Return accepted operation immediately; the server continues working
      --wait-seconds <WAIT_SECONDS>
          [default: 300]
  -h, --help
          Print help
exec_help_exit:0

===== END kiln-exec-help exit=0 =====

===== BEGIN profile-probe =====
+ python3 -c $'import json, os\np="/root/.config/kiln/profiles.json"\nprint("profiles_exists", os.path.exists(p))\nprint("KILN_IDENTITY_URL", os.environ.get("KILN_IDENTITY_URL", ""))\nprint("XDG_CONFIG_HOME", os.environ.get("XDG_CONFIG_HOME", ""))\nif os.path.exists(p):\n    data=json.load(open(p))\n    profiles=data.get("profiles", {})\n    print("profile_count", len(profiles))\n    for name, prof in profiles.items():\n        kind="central" if "issuer" in prof else "direct" if "url" in prof else "unknown"\n        origin=""\n        server=prof.get("server")\n        if isinstance(server, dict):\n            origin=server.get("origin", "")\n        print("profile", name, "kind", kind, "issuer", prof.get("issuer", ""), "url", prof.get("url", ""), "server_origin", origin)\n' 
profiles_exists False
KILN_IDENTITY_URL 
XDG_CONFIG_HOME 

===== END profile-probe exit=0 =====

===== AFTER CHECKS DONE =====
```

Stderr:

### after.err

```text
ls: cannot access '/opt/kiln/pending-upgrade.json': No such file or directory
```

ssh exit 0. Stderr is `ls` reporting that `/opt/kiln/pending-upgrade.json` is absent.

### 7. Smoke test

`kiln --help`, `kiln create --help`, and `kiln exec --help` are in the after transcript. Root's `PATH` did not include `/opt/kiln/bin`. The issuer default in help is `https://dark-forge.dev`. No `KILN_IDENTITY_URL` was set. The profile file is the config the client needs.

The first script created an empty config file, so `profile add` failed before it created a box. The local copy of that script was later overwritten by the retry. The text below is the script that was sent. Stdout and stderr are the original captures. The outer ssh `timeout` was 720 seconds. ssh exit 0.

### smoke-first.sh

```text
#!/bin/bash
set +e
echo "===== date -u ====="
date -u
echo "===== PATH before ====="
printf '%s\n' "$PATH"
echo "===== command -v kiln before ====="
command -v kiln
echo command_v_exit:$?
export PATH=/opt/kiln/bin:$PATH
echo "===== PATH after prepend ====="
printf '%s\n' "$PATH"
echo "+ command -v kiln"
command -v kiln
cfg=$(mktemp /var/tmp/kiln-smoke-profile.XXXXXX.json)
echo "cfg=$cfg"
echo "+ ls -l /etc/kiln/admin.token /etc/kiln/ca.crt"
ls -l /etc/kiln/admin.token /etc/kiln/ca.crt
echo "+ systemctl show kiln-api.service -p FragmentPath"
systemctl show kiln-api.service -p FragmentPath
echo "+ listen address from unit"
awk 'match($0, /--listen [^ ]+/) { print substr($0, RSTART, RLENGTH) }' /etc/systemd/system/kiln-api.service
echo "+ kiln profile add"
kiln --config "$cfg" profile add default --url https://127.0.0.1:8443 --token-file /etc/kiln/admin.token --ca-file /etc/kiln/ca.crt
echo profile_add_exit:$?
echo "+ profile file keys"
python3 - "$cfg" << 'PY'
import json, sys
data = json.load(open(sys.argv[1]))
for name, prof in data.get("profiles", {}).items():
    print("profile", name)
    for key, value in prof.items():
        print(f"  {key}={value}")
PY
echo "+ kiln list"
timeout 40 kiln --config "$cfg" list
echo list_before_exit:$?
echo "+ kiln create --template ubuntu-4g --name kiln-smoke"
timeout 360 kiln --config "$cfg" create --template ubuntu-4g --name kiln-smoke
echo create_exit:$?
echo "+ kiln list after create"
timeout 40 kiln --config "$cfg" list
echo list_after_create_exit:$?
echo "+ kiln exec kiln-smoke -- sh -c 'echo kiln-smoke-ok-\$(uname -r)'"
timeout 60 kiln --config "$cfg" exec kiln-smoke -- sh -c 'echo kiln-smoke-ok-$(uname -r)'
echo exec_exit:$?
echo "+ kiln delete kiln-smoke"
timeout 180 kiln --config "$cfg" delete kiln-smoke
echo delete_exit:$?
echo "+ kiln list after delete"
timeout 40 kiln --config "$cfg" list
echo list_after_delete_exit:$?
echo "+ rm profile config"
rm -f -- "$cfg"
if [ -e "$cfg" ]; then echo profile_config_still_present; else echo profile_config_removed; fi
echo "+ identity recheck"
systemctl show kiln-identity.service kiln-identity-proxy.service kiln-identity-proxy.socket -p Id,ActiveState,SubState,MainPID,NRestarts,ActiveEnterTimestamp
sha256sum /root/.ssh/authorized_keys
sha256sum /usr/local/libexec/kiln-identity*
echo "===== SMOKE DONE ====="
```

Stdout:

### smoke.out

```text
===== date -u =====
Mon Oct 19 05:54:00 PM UTC 2026
===== PATH before =====
/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
===== command -v kiln before =====
command_v_exit:1
===== PATH after prepend =====
/opt/kiln/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
+ command -v kiln
/opt/kiln/bin/kiln
cfg=/var/tmp/kiln-smoke-profile.hzQwaz.json
+ ls -l /etc/kiln/admin.token /etc/kiln/ca.crt
-rw------- 1 kiln-api kiln-api   65 Oct 19 17:11 /etc/kiln/admin.token
-rw-r--r-- 1 root     root     1501 Oct 19 17:11 /etc/kiln/ca.crt
+ systemctl show kiln-api.service -p FragmentPath
FragmentPath=/etc/systemd/system/kiln-api.service
+ listen address from unit
--listen with
--listen 127.0.0.1:8443
+ kiln profile add
profile_add_exit:1
+ profile file keys
+ kiln list
list_before_exit:1
+ kiln create --template ubuntu-4g --name kiln-smoke
create_exit:1
+ kiln list after create
list_after_create_exit:1
+ kiln exec kiln-smoke -- sh -c 'echo kiln-smoke-ok-$(uname -r)'
exec_exit:1
+ kiln delete kiln-smoke
delete_exit:1
+ kiln list after delete
list_after_delete_exit:1
+ rm profile config
profile_config_removed
+ identity recheck
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
4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37  /root/.ssh/authorized_keys
54c89f144ef0407b0abef11a8ffe60c18acdacc82b2f9c90bdf647c1df1d76ea  /usr/local/libexec/kiln-identity
9e36ca9ae505da5b3704915114e4b39565099871e407ff915f4103d9729eda59  /usr/local/libexec/kiln-identity.rollback-v033
===== SMOKE DONE =====
```

Stderr:

### smoke.err

```text
error: EOF while parsing a value at line 1 column 0
Traceback (most recent call last):
  File "<stdin>", line 2, in <module>
  File "/usr/lib/python3.13/json/__init__.py", line 293, in load
    return loads(fp.read(),
        cls=cls, object_hook=object_hook,
        parse_float=parse_float, parse_int=parse_int,
        parse_constant=parse_constant, object_pairs_hook=object_pairs_hook, **kw)
  File "/usr/lib/python3.13/json/__init__.py", line 346, in loads
    return _default_decoder.decode(s)
           ~~~~~~~~~~~~~~~~~~~~~~~^^^
  File "/usr/lib/python3.13/json/decoder.py", line 345, in decode
    obj, end = self.raw_decode(s, idx=_w(s, 0).end())
               ~~~~~~~~~~~~~~~^^^^^^^^^^^^^^^^^^^^^^^
  File "/usr/lib/python3.13/json/decoder.py", line 363, in raw_decode
    raise JSONDecodeError("Expecting value", s, err.value) from None
json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0)
error: EOF while parsing a value at line 1 column 0
error: EOF while parsing a value at line 1 column 0
error: EOF while parsing a value at line 1 column 0
error: EOF while parsing a value at line 1 column 0
error: EOF while parsing a value at line 1 column 0
error: EOF while parsing a value at line 1 column 0
```

ssh exit 0. `profile_add_exit` is 1. No box was created. The empty config file was removed.

The retry used a config path that did not exist. The outer ssh `timeout` was 720 seconds. ssh exit 0.

### smoke.sh

```text
#!/bin/bash
set +e
echo "===== date -u ====="
date -u
export PATH=/opt/kiln/bin:$PATH
cfg=/var/tmp/kiln-smoke-profile-a2.json
rm -f -- "$cfg"
echo "cfg=$cfg"
echo "cfg_exists_before=$( [ -e "$cfg" ] && echo yes || echo no )"
echo "+ kiln --config $cfg profile add default --url https://127.0.0.1:8443 --token-file /etc/kiln/admin.token --ca-file /etc/kiln/ca.crt"
kiln --config "$cfg" profile add default --url https://127.0.0.1:8443 --token-file /etc/kiln/admin.token --ca-file /etc/kiln/ca.crt
echo profile_add_exit:$?
echo "+ profile file keys"
if [ -s "$cfg" ]; then
  python3 - "$cfg" << 'PY'
import json, sys
data = json.load(open(sys.argv[1]))
for name, prof in data.get("profiles", {}).items():
    print("profile", name)
    for key, value in prof.items():
        text = str(value)
        if len(text) > 200:
            text = text[:40] + "...(truncated)"
        print(f"  {key}={text}")
PY
else
  echo "profile file missing or empty"
fi
echo "+ kiln list"
timeout 40 kiln --config "$cfg" list
echo list_before_exit:$?
echo "+ kiln create --template ubuntu-4g --name kiln-smoke"
timeout 360 kiln --config "$cfg" create --template ubuntu-4g --name kiln-smoke
echo create_exit:$?
echo "+ kiln list after create"
timeout 40 kiln --config "$cfg" list
echo list_after_create_exit:$?
echo "+ kiln exec kiln-smoke -- sh -c 'echo kiln-smoke-ok-\$(uname -r)'"
timeout 60 kiln --config "$cfg" exec kiln-smoke -- sh -c 'echo kiln-smoke-ok-$(uname -r)'
echo exec_exit:$?
echo "+ kiln delete kiln-smoke"
timeout 180 kiln --config "$cfg" delete kiln-smoke
echo delete_exit:$?
echo "+ kiln list after delete"
timeout 40 kiln --config "$cfg" list
echo list_after_delete_exit:$?
rm -f -- "$cfg" /var/tmp/.profiles.lock
if [ -e "$cfg" ]; then echo profile_config_still_present; else echo profile_config_removed; fi
echo "===== SMOKE DONE ====="
```

Stdout:

### smoke2.out

```text
===== date -u =====
Mon Oct 19 05:54:36 PM UTC 2026
cfg=/var/tmp/kiln-smoke-profile-a2.json
cfg_exists_before=no
+ kiln --config /var/tmp/kiln-smoke-profile-a2.json profile add default --url https://127.0.0.1:8443 --token-file /etc/kiln/admin.token --ca-file /etc/kiln/ca.crt
added profile default
profile_add_exit:0
+ profile file keys
profile default
  url=https://127.0.0.1:8443
  token_file=/etc/kiln/admin.token
  ca_file=/etc/kiln/ca.crt
+ kiln list
No boxes yet. Create one with `kiln create`.
list_before_exit:0
+ kiln create --template ubuntu-4g --name kiln-smoke
NAME        ID                                STATE           MEMORY  VCPUS
kiln-smoke  b181377b5cd74a90bb615f080ea018b8  running       4096 MiB  1
create_exit:0
+ kiln list after create
NAME        ID        STATE           MEMORY  VCPUS
kiln-smoke  b181377b  running       4096 MiB  1
list_after_create_exit:0
+ kiln exec kiln-smoke -- sh -c 'echo kiln-smoke-ok-$(uname -r)'
kiln-smoke-ok-6.1.155+
exec_exit:0
+ kiln delete kiln-smoke
deleted b181377b5cd74a90bb615f080ea018b8
delete_exit:0
+ kiln list after delete
No boxes yet. Create one with `kiln create`.
list_after_delete_exit:0
profile_config_removed
===== SMOKE DONE =====
```

Stderr:

### smoke2.err

```text
creating kiln-smoke with template ubuntu-4g
request 90d4deb5e62d4ea58ecac28bb8a691f4
request 4f000924fad24eaa84dd6b7f9f8250bf
request d61b4d7c8635400c9084d6b66a85276b
```

ssh exit 0. Create, exec, and delete exited 0. Guest stdout is `kiln-smoke-ok-6.1.155+`.

### 8. Identity recheck and key removal

This recheck ran after the smoke test. The outer ssh `timeout` was 40 seconds. ssh exit 0. The identity units, `authorized_keys`, and `kiln-identity*` hashes match the before checks. `kiln-host` and `kiln-api` were still the post-upgrade pids.

### recheck.sh

```text
#!/bin/bash
set +e
echo "===== date -u ====="
date -u
systemctl show kiln-identity.service kiln-identity-proxy.service kiln-identity-proxy.socket -p Id,ActiveState,SubState,MainPID,NRestarts,ActiveEnterTimestamp
echo "+ sha256sum authorized_keys and identity"
sha256sum /root/.ssh/authorized_keys
sha256sum /usr/local/libexec/kiln-identity*
echo "+ host api still active"
systemctl show kiln-host.service kiln-api.service -p Id,ActiveState,SubState,MainPID
echo "+ smoke box gone"
export PATH=/opt/kiln/bin:$PATH
# no profile; list via a throwaway config would recreate state. skip.
echo "RECHECK_DONE"
```

Stdout:

### recheck.out

```text
===== date -u =====
Mon Oct 19 05:55:25 PM UTC 2026
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
+ sha256sum authorized_keys and identity
4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37  /root/.ssh/authorized_keys
54c89f144ef0407b0abef11a8ffe60c18acdacc82b2f9c90bdf647c1df1d76ea  /usr/local/libexec/kiln-identity
9e36ca9ae505da5b3704915114e4b39565099871e407ff915f4103d9729eda59  /usr/local/libexec/kiln-identity.rollback-v033
+ host api still active
MainPID=3739721
Id=kiln-host.service
ActiveState=active
SubState=running

MainPID=3739725
Id=kiln-api.service
ActiveState=active
SubState=running
+ smoke box gone
RECHECK_DONE
```

Stderr:

### recheck.err

```text
(empty)
```

ssh exit 0.

On the operator machine, after that recheck:

```text
shred -u ~/.ssh/kiln_deploy
shred_exit:0
KEY_GONE
```

The key file was absent after `shred -u`.

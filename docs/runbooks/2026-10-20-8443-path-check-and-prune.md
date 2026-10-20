# dark-forge.dev: 443 path check and 0.4.6 rollback prune

Record of the 2026-10-20 check on dark-forge.dev after the off-host v0.4.9 client test. No application source on the host was edited. `kiln-identity`, `kiln-identity-proxy`, and their sockets were not restarted and their unit files were not edited. `authorized_keys` was not edited. No secret or token was printed. Caddy was not recreated and `docker compose` was not run. No systemd unit was changed, so no unit-file backup was taken.

The saved client config that held the server URL was deleted before Proof asked for it. This record does not have that file. The port 443 evidence is the server-side logs below.

## Decision

Public tcp/8443 was left open. The close commands in `docs/runbooks/2026-10-19-door-public-cert.md` were not run:

```
systemctl disable --now kiln-door-proxy-public.socket
systemctl restart kiln-door-proxy.service
nft add rule inet filter input tcp dport 8443 drop
```

Proof's gate was to close `kiln-door-proxy-public.socket` on `78.46.98.73:8443` only if the off-host create, exec, and delete reached the door through Caddy on `cli.dark-forge.dev:443`, and nothing for those requests came through the 8443 proxy. The server logs do not show which path those request IDs used.

`/opt/paseo/Caddyfile` says `No access logs or debug logging`. `docker logs` for the `caddy` container from `2026-10-20T10:51:00Z` to `2026-10-20T10:54:30Z` is empty (0 bytes). A wider window, `10:40:00Z` to `11:10:00Z`, is also empty. The last Caddy log line before that gap is an unrelated HTTP-challenge warning at `2026-10-20T10:23:44Z`. Caddy's metrics endpoint has no HTTP request counter. `caddy_reverse_proxy_upstreams_healthy{upstream="172.18.0.1:8444"}` is 1, which only says the upstream probe is healthy.

`journalctl` for `kiln-door-proxy-public.socket`, `kiln-door-proxy.service`, `kiln-door.service`, `kiln-api.service`, and `kiln-host.service` has no entries in the test window. A search of those units from `08:00:00Z` to `11:10:00Z` does not contain the request IDs or the box id. The proxy process (pid 47723) has no journal lines after it started. The door does not log a peer address.

`/var/lib/kiln/journal/operations.sqlite` does contain the three operations. They succeeded. The rows have no proxy path and no peer. Their timestamps are inside the 11:53 UTC+1 minute (10:53 UTC):

| id | box_id | created_utc | state |
| --- | --- | --- | --- |
| `0ddf22b59ebb482f87386c605ec25e09` | `fa0a7f2a1b3744b4b547db3438e683d9` | 2026-10-20 10:53:02 | succeeded |
| `eb922e3f96cf4db7b1c1d27223180864` | `fa0a7f2a1b3744b4b547db3438e683d9` | 2026-10-20 10:53:11 | succeeded |
| `22ace0b43b0444c693d5b93d6a5db1e0` | `fa0a7f2a1b3744b4b547db3438e683d9` | 2026-10-20 10:53:12 | succeeded |

The exec row's `result.stdout` bytes are `[107,105,108,110,45,101,120,101,99,45,112,111,108,108,10]`, which is `kiln-exec-poll` plus a newline. That matches the client output. It does not identify the listener.

`/opt/kiln/upgrades/0.4.6-3vO6d0` is the directory named by `docs/runbooks/deploy-2026-10-20-v0.4.9.md` as the v0.4.6 rollback copy. Nothing on the host had it mapped. It was removed with `rm -rf -- /opt/kiln/upgrades/0.4.6-3vO6d0`. `/opt/kiln/upgrades/0.4.4-8DDh4B` and `/opt/kiln/upgrades/0.4.5-RSNr4F` are still there. `/usr/local/libexec/kiln-identity.rollback-v033` was not touched.

## Off-host test output

Raw client output from Kiln Ops' machine. The client config holding the saved server URL was deleted before Proof asked for it, so this block is not a server-URL proof. The `:443` evidence is the server-side logs in the transcripts.

```
++ ./kiln --config /workspace/kiln-client/config.toml --version
kiln 0.4.9
++ ./kiln --config /workspace/kiln-client/config.toml list
No boxes yet. Create one with `kiln create`.
++ ./kiln --config /workspace/kiln-client/config.toml create --name door-443
creating door-443 with template ubuntu-4g
request 0ddf22b59ebb482f87386c605ec25e09
NAME      ID                                STATE           MEMORY  VCPUS
door-443  fa0a7f2a1b3744b4b547db3438e683d9  running       4096 MiB  1
++ ./kiln --config /workspace/kiln-client/config.toml exec door-443 -- /bin/echo kiln-exec-poll
request eb922e3f96cf4db7b1c1d27223180864
kiln-exec-poll
++ ./kiln --config /workspace/kiln-client/config.toml delete door-443
request 22ace0b43b0444c693d5b93d6a5db1e0
deleted fa0a7f2a1b3744b4b547db3438e683d9
++ ./kiln --config /workspace/kiln-client/config.toml list
No boxes yet. Create one with `kiln create`.
```

## Result

| Check | Result |
| --- | --- |
| Request path is Caddy `:443` and not the 8443 proxy | Not proven. 8443 was left open. |
| `78.46.98.73:8443` no longer listening, and refused off-host | Fail. Still listening. Off-host TCP connected. The certificate is still the door leaf, sha256 `D4:BA:F2:89:F2:62:5D:E8:67:57:2D:4F:97:F8:CB:B4:D7:8B:27:BE:FA:63:0B:53:A8:DB:3E:0C:74:C2:7D:3A`. |
| `https://cli.dark-forge.dev/` without auth | Pass. HTTP 401 `{"code":"unauthorized","message":"A valid credential is required."}`. Response has `via: 2.0 Caddy`. |
| `https://cli.dark-forge.dev/v1/templates` without auth | Pass. Same 401. |
| `https://dark-forge.dev/login` | Pass. HTTP 200. Body contains `Light the way in.` |
| `kiln-runtime doctor` | Pass. Exit 0. `development_ready` is true. Full output is below. |
| On-host smoke test | Pass on the second attempt. Create, exec (`kiln-prune-ok`), and delete exited 0. A later list was empty. The first attempt failed before creating a box because the profile path was an empty file. |
| kiln-identity unchanged during this run | Pass. `MainPID=3522047`, `ActiveState=active`, `SubState=running`, `UnitFileState=enabled`, `NRestarts=0`, `ActiveEnterTimestamp=Mon 2026-10-19 12:42:17 CEST` before and after. |
| kiln-identity-proxy.service unchanged during this run | Pass. `MainPID=60490`, `ActiveState=active`, `SubState=running`, `UnitFileState=static`, `NRestarts=0`, `ActiveEnterTimestamp=Tue 2026-10-20 12:51:08 CEST` before and after. |
| kiln-identity-proxy.socket unchanged during this run | Pass. `ActiveState=active`, `SubState=running`, `UnitFileState=enabled`, `ActiveEnterTimestamp=Sat 2026-10-03 20:14:48 CEST` before and after. |
| `authorized_keys` | Pass. sha256 `4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37` before and after. Mode stayed `600`. The file was not edited. |
| Prune | Pass for the named directory only. `/opt/kiln/upgrades/0.4.6-3vO6d0` is gone. `0.4.4-8DDh4B` and `0.4.5-RSNr4F` remain. |

## Unexpected

`kiln-identity-proxy.service` had already stopped and started again before this session. The journal says `Deactivated successfully` at `2026-10-20T12:48:07+02:00` and `Started` at `2026-10-20T12:51:08+02:00`. The v0.4.9 deploy record had `MainPID=4161400` and `ActiveEnterTimestamp=Tue 2026-10-20 09:56:22 CEST`. This run found `MainPID=60490` and did not change it. The unit is `systemd-socket-proxyd` with `--exit-idle-time=5min`, and the journal line is the idle exit, not a unit edit.

`kiln-door-proxy.service` had the same idle pattern before this session: `Deactivated successfully` at `2026-10-20T12:23:30+02:00`, `Started` at `2026-10-20T12:25:44+02:00`, `MainPID=47723`. This run did not restart it. That process holds both `172.18.0.1:8444` and `78.46.98.73:8443`.

`nftables.service` is disabled and inactive. `nft list table inet filter` still shows only `tcp dport 8443 accept` with policy accept. No rule was added or deleted. The live accept rule is not what this run relied on, because the socket was left up.

The first on-host smoke attempt used `mktemp`, which created an empty profile file. `kiln` then printed `error: EOF while parsing a value at line 1 column 0` and exited 1 for profile add, list, create, exec, delete, and the second list. No box was created. The profile file was removed with `shred -u`. The retry used a path that did not exist yet and passed.

## SSH

The remote prefix was `ssh -i <key> -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile=<pinned known_hosts> -o BatchMode=yes -o ConnectTimeout=15 root@dark-forge.dev`. The pinned host key is ED25519 `SHA256:/yIiijBqkbGh9lFju3N9ZhbJGIqv+NjC0R1CyWh9g9s`. `ssh-keyscan -t ed25519` matched that fingerprint before the first connection. The deploy key fingerprint is `SHA256:AGD6iqWdOAB1R7LeRTlbIMYyvx+FvkIDw8MTWEVtrd4` (`kiln-deploy@dark-forge.dev`). The private key is not in this file. After the host commands finished, `shred -u` removed the key file. A following existence check found the file gone.

Host clock at the start of the read-only collection: `Tue Oct 20 10:57:22 AM UTC 2026`, `Europe/Berlin (CEST, +0200)`. 11:52–11:53 UTC+1 is 10:52–10:53 UTC.

## Transcripts

Commands below are the scripts passed to `bash -s` on the host, followed by the raw stdout and stderr. The first smoke attempt is included. It did not create a box.


## 1. Read-only collection script

```text
set -u
echo "===== date ====="
date -u
date
timedatectl 2>/dev/null || true
echo "===== identity before ====="
systemctl show kiln-identity.service -p Id -p ActiveState -p SubState -p UnitFileState -p MainPID -p NRestarts -p ActiveEnterTimestamp
echo "--- kiln-identity-proxy.service ---"
systemctl show kiln-identity-proxy.service -p Id -p ActiveState -p SubState -p UnitFileState -p MainPID -p NRestarts -p ActiveEnterTimestamp
echo "--- kiln-identity-proxy.socket ---"
systemctl show kiln-identity-proxy.socket -p Id -p ActiveState -p SubState -p UnitFileState -p ActiveEnterTimestamp
echo "===== authorized_keys before ====="
sha256sum /root/.ssh/authorized_keys
stat -c '%n %a %u %g %s %y' /root/.ssh/authorized_keys
echo "===== listeners ====="
ss -ltnp
echo "===== upgrades ====="
ls -la /opt/kiln/upgrades/ || true
echo "===== upgrades find ====="
find /opt/kiln/upgrades -maxdepth 2 -printf '%y %m %u:%g %s %TY-%Tm-%TdT%TH:%TM %p\n' || true
echo "===== door units ====="
systemctl show kiln-door-proxy-public.socket -p Id -p ActiveState -p SubState -p UnitFileState -p ActiveEnterTimestamp -p Listen -p FragmentPath -p Triggers
echo "--- kiln-door-proxy.service ---"
systemctl show kiln-door-proxy.service -p Id -p ActiveState -p SubState -p UnitFileState -p MainPID -p NRestarts -p ActiveEnterTimestamp -p FragmentPath
echo "--- kiln-door-proxy.socket ---"
systemctl show kiln-door-proxy.socket -p Id -p ActiveState -p SubState -p UnitFileState -p ActiveEnterTimestamp -p Listen
echo "--- kiln-door.service ---"
systemctl show kiln-door.service -p Id -p ActiveState -p SubState -p UnitFileState -p MainPID -p NRestarts -p ActiveEnterTimestamp
echo "===== caddy ps ====="
docker ps -a --filter name=caddy --no-trunc --format 'id={{.ID}} name={{.Names}} image={{.Image}} status={{.Status}} ports={{.Ports}}'
echo "===== caddyfile log directives ====="
grep -n -E 'log |access|cli\.dark-forge' /opt/paseo/Caddyfile || true
echo "===== docker logs exact window 10:51:00Z-10:54:30Z ====="
docker logs --since 2026-10-20T10:51:00Z --until 2026-10-20T10:54:30Z --timestamps caddy > /tmp/caddy-window.log 2>&1 || true
wc -c /tmp/caddy-window.log
echo "===== docker logs window grep ====="
grep -n -E 'cli\.dark-forge|0ddf22b59ebb482f87386c605ec25e09|eb922e3f96cf4db7b1c1d27223180864|22ace0b43b0444c693d5b93d6a5db1e0|fa0a7f2a1b3744b4b547db3438e683d9|8443|8444' /tmp/caddy-window.log || echo 'NO_MATCH_IN_WINDOW'
echo "===== docker logs wider 10:40Z-11:10Z count and matches ====="
docker logs --since 2026-10-20T10:40:00Z --until 2026-10-20T11:10:00Z --timestamps caddy > /tmp/caddy-wide.log 2>&1 || true
wc -c /tmp/caddy-wide.log
grep -n -E 'cli\.dark-forge|0ddf22b59ebb482f87386c605ec25e09|eb922e3f96cf4db7b1c1d27223180864|22ace0b43b0444c693d5b93d6a5db1e0|fa0a7f2a1b3744b4b547db3438e683d9' /tmp/caddy-wide.log || echo 'NO_MATCH_IN_WIDE'
echo "===== journal public socket exact ====="
journalctl -u kiln-door-proxy-public.socket --since '2026-10-20 10:51:00 UTC' --until '2026-10-20 10:54:30 UTC' --no-pager -o short-iso
echo "PUBLIC_SOCKET_JOURNAL_EXIT:$?"
echo "===== journal public socket wide ====="
journalctl -u kiln-door-proxy-public.socket --since '2026-10-20 10:40:00 UTC' --until '2026-10-20 11:10:00 UTC' --no-pager -o short-iso
echo "===== journal proxy service exact ====="
journalctl -u kiln-door-proxy.service --since '2026-10-20 10:51:00 UTC' --until '2026-10-20 10:54:30 UTC' --no-pager -o short-iso
echo "===== journal proxy service wide ====="
journalctl -u kiln-door-proxy.service --since '2026-10-20 10:40:00 UTC' --until '2026-10-20 11:10:00 UTC' --no-pager -o short-iso
echo "===== journal door exact ====="
journalctl -u kiln-door.service --since '2026-10-20 10:51:00 UTC' --until '2026-10-20 10:54:30 UTC' --no-pager -o short-iso
echo "===== journal door wide ====="
journalctl -u kiln-door.service --since '2026-10-20 10:40:00 UTC' --until '2026-10-20 11:10:00 UTC' --no-pager -o short-iso
echo "===== journal kiln-api exact ====="
journalctl -u kiln-api.service --since '2026-10-20 10:51:00 UTC' --until '2026-10-20 10:54:30 UTC' --no-pager -o short-iso
echo "===== journal kiln-host exact ====="
journalctl -u kiln-host.service --since '2026-10-20 10:51:00 UTC' --until '2026-10-20 10:54:30 UTC' --no-pager -o short-iso
echo "===== request id search units ====="
journalctl -u kiln-door.service -u kiln-door-proxy.service -u kiln-door-proxy-public.socket -u kiln-door-proxy.socket -u kiln-api.service -u kiln-host.service --since '2026-10-20 08:00:00 UTC' --until '2026-10-20 11:10:00 UTC' --no-pager -o short-iso | grep -F -e 0ddf22b59ebb482f87386c605ec25e09 -e eb922e3f96cf4db7b1c1d27223180864 -e 22ace0b43b0444c693d5b93d6a5db1e0 -e fa0a7f2a1b3744b4b547db3438e683d9 -e door-443 || echo 'NO_ID_IN_UNIT_JOURNALS'
echo "===== caddy access files ====="
docker exec caddy sh -c 'ls -la /var/log /data/caddy 2>/dev/null; find /var/log /data -name "*access*" -o -name "*.log" 2>/dev/null | head -100' || true
echo "COLLECT_DONE"
```

## 1. stdout

```text
===== date =====
Tue Oct 20 10:57:22 AM UTC 2026
Tue Oct 20 12:57:22 PM CEST 2026
               Local time: Tue 2026-10-20 12:57:22 CEST
           Universal time: Tue 2026-10-20 10:57:22 UTC
                 RTC time: Tue 2026-10-20 10:57:23
                Time zone: Europe/Berlin (CEST, +0200)
System clock synchronized: yes
              NTP service: active
          RTC in local TZ: no
===== identity before =====
MainPID=3522047
NRestarts=0
Id=kiln-identity.service
ActiveState=active
SubState=running
UnitFileState=enabled
ActiveEnterTimestamp=Mon 2026-10-19 12:42:17 CEST
--- kiln-identity-proxy.service ---
MainPID=60490
NRestarts=0
Id=kiln-identity-proxy.service
ActiveState=active
SubState=running
UnitFileState=static
ActiveEnterTimestamp=Tue 2026-10-20 12:51:08 CEST
--- kiln-identity-proxy.socket ---
Id=kiln-identity-proxy.socket
ActiveState=active
SubState=running
UnitFileState=enabled
ActiveEnterTimestamp=Sat 2026-10-03 20:14:48 CEST
===== authorized_keys before =====
4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37  /root/.ssh/authorized_keys
/root/.ssh/authorized_keys 600 0 0 592 2026-10-19 16:54:10.656267246 +0200
===== listeners =====
State  Recv-Q Send-Q Local Address:Port Peer Address:PortProcess                                                             
LISTEN 0      4096      172.18.0.1:8092      0.0.0.0:*    users:(("systemd-socket-",pid=60490,fd=3),("systemd",pid=1,fd=150))
LISTEN 0      4096      172.18.0.1:8444      0.0.0.0:*    users:(("systemd-socket-",pid=47723,fd=3),("systemd",pid=1,fd=110))
LISTEN 0      4096     78.46.98.73:8443      0.0.0.0:*    users:(("systemd-socket-",pid=47723,fd=4),("systemd",pid=1,fd=135))
LISTEN 0      128        127.0.0.1:8091      0.0.0.0:*    users:(("kiln-identity",pid=3522047,fd=12))                        
LISTEN 0      4096       127.0.0.1:6767      0.0.0.0:*    users:(("docker-proxy",pid=3699036,fd=8))                          
LISTEN 0      128          0.0.0.0:22        0.0.0.0:*    users:(("sshd",pid=1022,fd=6))                                     
LISTEN 0      4096         0.0.0.0:80        0.0.0.0:*    users:(("docker-proxy",pid=4399,fd=8))                             
LISTEN 0      4096         0.0.0.0:443       0.0.0.0:*    users:(("docker-proxy",pid=4422,fd=8))                             
LISTEN 0      128        127.0.0.1:8444      0.0.0.0:*    users:(("kiln-api",pid=4167939,fd=9))                              
LISTEN 0      128        127.0.0.1:8443      0.0.0.0:*    users:(("kiln-api",pid=4165125,fd=9))                              
LISTEN 0      128             [::]:22           [::]:*    users:(("sshd",pid=1022,fd=7))                                     
LISTEN 0      4096            [::]:80           [::]:*    users:(("docker-proxy",pid=4405,fd=8))                             
LISTEN 0      4096            [::]:443          [::]:*    users:(("docker-proxy",pid=4429,fd=8))                             
===== upgrades =====
total 20
drwx------ 5 root root 4096 Oct 20 10:02 .
drwxr-xr-x 4 root root 4096 Oct 20 10:02 ..
drwx------ 2 root root 4096 Oct 19 19:52 0.4.4-8DDh4B
drwx------ 2 root root 4096 Oct 19 23:27 0.4.5-RSNr4F
drwx------ 2 root root 4096 Oct 20 10:02 0.4.6-3vO6d0
===== upgrades find =====
d 700 root:root 4096 2026-10-20T10:02 /opt/kiln/upgrades
d 700 root:root 4096 2026-10-19T19:52 /opt/kiln/upgrades/0.4.4-8DDh4B
f 755 root:root 8166808 2026-10-19T19:52 /opt/kiln/upgrades/0.4.4-8DDh4B/kiln
f 755 root:root 6077 2026-10-19T19:52 /opt/kiln/upgrades/0.4.4-8DDh4B/kiln-network
f 755 root:root 11390504 2026-10-19T19:52 /opt/kiln/upgrades/0.4.4-8DDh4B/kiln-host
f 755 root:root 8492816 2026-10-19T19:52 /opt/kiln/upgrades/0.4.4-8DDh4B/kiln-runtime
f 755 root:root 10438672 2026-10-19T19:52 /opt/kiln/upgrades/0.4.4-8DDh4B/kiln-api
d 700 root:root 4096 2026-10-20T10:02 /opt/kiln/upgrades/0.4.6-3vO6d0
f 755 root:root 8167552 2026-10-20T10:02 /opt/kiln/upgrades/0.4.6-3vO6d0/kiln
f 755 root:root 6077 2026-10-20T10:02 /opt/kiln/upgrades/0.4.6-3vO6d0/kiln-network
f 755 root:root 11421536 2026-10-20T10:02 /opt/kiln/upgrades/0.4.6-3vO6d0/kiln-host
f 755 root:root 8492800 2026-10-20T10:02 /opt/kiln/upgrades/0.4.6-3vO6d0/kiln-runtime
f 755 root:root 10597496 2026-10-20T10:02 /opt/kiln/upgrades/0.4.6-3vO6d0/kiln-api
d 700 root:root 4096 2026-10-19T23:27 /opt/kiln/upgrades/0.4.5-RSNr4F
f 755 root:root 8166976 2026-10-19T23:27 /opt/kiln/upgrades/0.4.5-RSNr4F/kiln
f 755 root:root 6077 2026-10-19T23:27 /opt/kiln/upgrades/0.4.5-RSNr4F/kiln-network
f 755 root:root 11390800 2026-10-19T23:27 /opt/kiln/upgrades/0.4.5-RSNr4F/kiln-host
f 755 root:root 8492736 2026-10-19T23:27 /opt/kiln/upgrades/0.4.5-RSNr4F/kiln-runtime
f 755 root:root 10439448 2026-10-19T23:27 /opt/kiln/upgrades/0.4.5-RSNr4F/kiln-api
===== door units =====
Listen=78.46.98.73:8443 (Stream)
Id=kiln-door-proxy-public.socket
Triggers=kiln-door-proxy.service
ActiveState=active
SubState=running
FragmentPath=/etc/systemd/system/kiln-door-proxy-public.socket
UnitFileState=enabled
ActiveEnterTimestamp=Tue 2026-10-20 10:06:13 CEST
--- kiln-door-proxy.service ---
MainPID=47723
NRestarts=0
Id=kiln-door-proxy.service
ActiveState=active
SubState=running
FragmentPath=/etc/systemd/system/kiln-door-proxy.service
UnitFileState=static
ActiveEnterTimestamp=Tue 2026-10-20 12:25:44 CEST
--- kiln-door-proxy.socket ---
Listen=172.18.0.1:8444 (Stream)
Id=kiln-door-proxy.socket
ActiveState=active
SubState=running
UnitFileState=enabled
ActiveEnterTimestamp=Tue 2026-10-20 10:06:13 CEST
--- kiln-door.service ---
MainPID=4167939
NRestarts=0
Id=kiln-door.service
ActiveState=active
SubState=running
UnitFileState=enabled
ActiveEnterTimestamp=Tue 2026-10-20 10:06:14 CEST
===== caddy ps =====
id=14b7fcb812c35e320a4817a2731ef003eec77df247270945c72f6ca21188bd8b name=caddy image=caddy:2 status=Up 2 hours ports=0.0.0.0:80->80/tcp, [::]:80->80/tcp, 0.0.0.0:443->443/tcp, [::]:443->443/tcp, 443/udp, 2019/tcp
===== caddyfile log directives =====
11:# No access logs or debug logging: OAuth URLs contain one-use credentials.
35:cli.dark-forge.dev {
38:			tls_server_name cli.dark-forge.dev
===== docker logs exact window 10:51:00Z-10:54:30Z =====
0 /tmp/caddy-window.log
===== docker logs window grep =====
NO_MATCH_IN_WINDOW
===== docker logs wider 10:40Z-11:10Z count and matches =====
0 /tmp/caddy-wide.log
NO_MATCH_IN_WIDE
===== journal public socket exact =====
-- No entries --
PUBLIC_SOCKET_JOURNAL_EXIT:0
===== journal public socket wide =====
-- No entries --
===== journal proxy service exact =====
-- No entries --
===== journal proxy service wide =====
-- No entries --
===== journal door exact =====
-- No entries --
===== journal door wide =====
-- No entries --
===== journal kiln-api exact =====
-- No entries --
===== journal kiln-host exact =====
-- No entries --
===== request id search units =====
NO_ID_IN_UNIT_JOURNALS
===== caddy access files =====
/data/caddy:
total 28
drwxrwxrwt    5 root     root          4096 Oct 20 08:04 .
drwxr-xr-x    3 root     root          4096 Aug 22 22:11 ..
drwx------    3 root     root          4096 Aug 22 22:11 acme
drwx------    3 root     root          4096 Aug 22 22:11 certificates
-rw-------    1 root     root            36 Aug 22 22:11 instance.uuid
-rw-------    1 root     root           107 Oct 19 18:15 last_clean.json
drwx------    2 root     root          4096 Oct 20 09:04 locks

/var/log:
total 12
drwxr-xr-x    1 root     root          4096 Jun 22 20:09 .
drwxr-xr-x    1 root     root          4096 Jun 21 18:51 ..
-rw-r--r--    1 root     root           888 Jun 22 20:09 apk.log
/var/log/apk.log
COLLECT_DONE
```

## 1. stderr

```text
(empty)
```

## 2. Journal, sqlite, and nft script

```text
set -u
echo "===== docker inspect log config ====="
docker inspect caddy --format 'logtype={{.HostConfig.LogConfig.Type}} logpath={{.LogPath}} started={{.State.StartedAt}}'
echo "===== docker logs tail 40 ====="
docker logs --tail 40 --timestamps caddy 2>&1 || true
echo "===== journal proxy pid 47723 window ====="
journalctl _PID=47723 --since '2026-10-20 10:40:00 UTC' --until '2026-10-20 11:10:00 UTC' --no-pager -o short-iso || true
echo "===== journal proxy pid since restart ====="
journalctl _PID=47723 --since '2026-10-20 10:20:00 UTC' --no-pager -o short-iso | head -c 20000 || true
echo "===== journal identity-proxy around 10:51 UTC ====="
journalctl -u kiln-identity-proxy.service -u kiln-identity-proxy.socket -u kiln-identity.service --since '2026-10-20 10:45:00 UTC' --until '2026-10-20 11:05:00 UTC' --no-pager -o short-iso || true
echo "===== journal door-proxy restart 10:25 UTC ====="
journalctl -u kiln-door-proxy.service -u kiln-door-proxy.socket -u kiln-door-proxy-public.socket --since '2026-10-20 10:20:00 UTC' --until '2026-10-20 10:35:00 UTC' --no-pager -o short-iso || true
echo "===== find operations.sqlite ====="
find /var/lib/kiln /var/lib/kiln-api -name 'operations.sqlite' -printf '%p %s %TY-%Tm-%TdT%TH:%TM\n' 2>/dev/null || true
echo "===== sqlite request rows ====="
DB=$(find /var/lib/kiln -name 'operations.sqlite' -print -quit 2>/dev/null || true)
echo "db=$DB"
if [ -n "$DB" ]; then
  sqlite3 -readonly "$DB" << 'SQL'
.headers on
.mode column
SELECT id, box_id, created_at, datetime(created_at, 'unixepoch') AS created_utc, running
FROM operations
WHERE id IN (
  '0ddf22b59ebb482f87386c605ec25e09',
  'eb922e3f96cf4db7b1c1d27223180864',
  '22ace0b43b0444c693d5b93d6a5db1e0'
)
OR box_id = 'fa0a7f2a1b3744b4b547db3438e683d9';
SQL
  echo "===== sqlite response keys only ====="
  sqlite3 -readonly "$DB" << 'SQL'
.headers on
SELECT id, box_id, json_extract(response, '$.state') AS state,
       json_extract(response, '$.error') AS error,
       substr(json_extract(response, '$.result.stdout'), 1, 80) AS stdout_prefix
FROM operations
WHERE id IN (
  '0ddf22b59ebb482f87386c605ec25e09',
  'eb922e3f96cf4db7b1c1d27223180864',
  '22ace0b43b0444c693d5b93d6a5db1e0'
);
SQL
fi
echo "===== other 0.4.6 rollback names ====="
find /opt/kiln /usr/local/libexec /var/lib/kiln -iname '*0.4.6*' -o -iname '*rollback*' 2>/dev/null | head -100
echo "===== nft inet filter ====="
nft list table inet filter || true
echo "===== nftables service ====="
systemctl is-enabled nftables.service || true
systemctl is-active nftables.service || true
systemctl cat nftables.service 2>/dev/null | head -80 || true
echo "COLLECT2_DONE"
```

## 2. stdout

```text
===== docker inspect log config =====
logtype=json-file logpath=/var/lib/docker/containers/14b7fcb812c35e320a4817a2731ef003eec77df247270945c72f6ca21188bd8b/14b7fcb812c35e320a4817a2731ef003eec77df247270945c72f6ca21188bd8b-json.log started=2026-10-20T09:04:01.860181622Z
===== docker logs tail 40 =====
2026-10-20T09:04:02.051865592Z {"level":"info","ts":1792487042.0517995,"msg":"maxprocs: Leaving GOMAXPROCS=12: CPU quota undefined"}
2026-10-20T09:04:02.051897991Z {"level":"info","ts":1792487042.0518095,"msg":"GOMEMLIMIT is updated","GOMEMLIMIT":60615578419,"previous":9223372036854775807}
2026-10-20T09:04:02.051901001Z {"level":"info","ts":1792487042.051812,"msg":"using config from file","file":"/etc/caddy/Caddyfile"}
2026-10-20T09:04:02.051903401Z {"level":"warn","ts":1792487042.0518138,"msg":"Unnecessary header_up X-Forwarded-For: the reverse proxy's default behavior is to pass headers to the upstream"}
2026-10-20T09:04:02.051905571Z {"level":"info","ts":1792487042.0518155,"msg":"adapted config to JSON","adapter":"caddyfile"}
2026-10-20T09:04:02.052284514Z {"level":"info","ts":1792487042.052252,"logger":"admin","msg":"admin endpoint started","address":"localhost:2019","enforce_origin":false,"origins":["//localhost:2019","//[::1]:2019","//127.0.0.1:2019"]}
2026-10-20T09:04:02.052379152Z {"level":"info","ts":1792487042.0523582,"logger":"http.auto_https","msg":"server is listening only on the HTTPS port but has no TLS connection policies; adding one to enable TLS","server_name":"srv0","https_port":443}
2026-10-20T09:04:02.052386242Z {"level":"info","ts":1792487042.052368,"logger":"http.auto_https","msg":"enabling automatic HTTP->HTTPS redirects","server_name":"srv0"}
2026-10-20T09:04:02.052393951Z {"level":"warn","ts":1792487042.0523775,"logger":"http.auto_https","msg":"server is listening only on the HTTP port, so no automatic HTTPS will be applied to this server","server_name":"srv1","http_port":80}
2026-10-20T09:04:02.052460310Z {"level":"info","ts":1792487042.0524359,"logger":"tls.cache.maintenance","msg":"started background certificate maintenance","cache":"0x1a49d56a5600"}
2026-10-20T09:04:02.052979490Z {"level":"warn","ts":1792487042.0529485,"logger":"http","msg":"HTTP/2 skipped because it requires TLS","network":"tcp","addr":":80"}
2026-10-20T09:04:02.052985640Z {"level":"warn","ts":1792487042.0529554,"logger":"http","msg":"HTTP/3 skipped because it requires TLS","network":"tcp","addr":":80"}
2026-10-20T09:04:02.052988420Z {"level":"info","ts":1792487042.0529575,"logger":"http.log","msg":"server running","name":"srv1","protocols":["h1","h2","h3"]}
2026-10-20T09:04:02.053000810Z {"level":"info","ts":1792487042.0529847,"logger":"http","msg":"enabling HTTP/3 listener","addr":":443"}
2026-10-20T09:04:02.053058389Z {"level":"info","ts":1792487042.0530393,"msg":"failed to sufficiently increase receive buffer size (was: 208 kiB, wanted: 7168 kiB, got: 416 kiB). See https://github.com/quic-go/quic-go/wiki/UDP-Buffer-Sizes for details."}
2026-10-20T09:04:02.053568319Z {"level":"info","ts":1792487042.053118,"logger":"http.log","msg":"server running","name":"srv0","protocols":["h1","h2","h3"]}
2026-10-20T09:04:02.053577379Z {"level":"info","ts":1792487042.0531285,"logger":"http","msg":"enabling automatic TLS certificate management","domains":["cli.dark-forge.dev","dark-forge.dev"]}
2026-10-20T09:04:02.053580088Z {"level":"info","ts":1792487042.0533507,"msg":"autosaved config (load with --resume flag)","file":"/config/caddy/autosave.json"}
2026-10-20T09:04:02.053588078Z {"level":"info","ts":1792487042.0533733,"msg":"serving initial configuration"}
2026-10-20T09:04:02.057970444Z {"level":"info","ts":1792487042.0579383,"logger":"tls","msg":"storage cleaning happened too recently; skipping for now","storage":"FileStorage:/data/caddy","instance":"b811679a-a72d-4c83-a5af-4c9f196d1ad5","try_again":1792573442.0579379,"try_again_in":86399.99999972}
2026-10-20T09:04:02.058011703Z {"level":"info","ts":1792487042.0579937,"logger":"tls","msg":"finished cleaning storage units"}
2026-10-20T09:17:33.930665709Z {"level":"warn","ts":1792487853.9305315,"logger":"http","msg":"looking up info for HTTP challenge","uri":"/.well-known/acme-challenge/Ug9Lxat5BfamFYA8EToHClyCewKOYK-P6Fr4n5lpqBA","identifier":"web1605.login-11.loginserver.ch","remote_addr":"23.178.112.103:53123","user_agent":"Mozilla/5.0 (compatible; Let's Encrypt validation server; +https://www.letsencrypt.org)","error":"no information found to solve challenge for identifier: web1605.login-11.loginserver.ch"}
2026-10-20T09:17:39.168268988Z {"level":"warn","ts":1792487859.1681595,"logger":"http","msg":"looking up info for HTTP challenge","uri":"/.well-known/acme-challenge/RukbFIEUxJC8wn5UEIj1Vz2QCn8LYsRLrudI7LBxrCg","identifier":"web1066.login-11.loginserver.ch","remote_addr":"23.178.112.104:65275","user_agent":"Mozilla/5.0 (compatible; Let's Encrypt validation server; +https://www.letsencrypt.org)","error":"no information found to solve challenge for identifier: web1066.login-11.loginserver.ch"}
2026-10-20T09:17:44.496923394Z {"level":"warn","ts":1792487864.496825,"logger":"http","msg":"looking up info for HTTP challenge","uri":"/.well-known/acme-challenge/5cCwXH3Ogfw4R0H0nuZf7rCL4DeODMYkrVNyak6OhnM","identifier":"web1373.login-11.loginserver.ch","remote_addr":"23.178.112.103:54051","user_agent":"Mozilla/5.0 (compatible; Let's Encrypt validation server; +https://www.letsencrypt.org)","error":"no information found to solve challenge for identifier: web1373.login-11.loginserver.ch"}
2026-10-20T09:17:51.593596770Z {"level":"warn","ts":1792487871.5935159,"logger":"http","msg":"looking up info for HTTP challenge","uri":"/.well-known/acme-challenge/xcNcsLeOWtOSeBBCjc3Uy5_5sbQfTP84QmJxYkVnuow","identifier":"web1580.login-11.loginserver.ch","remote_addr":"23.178.112.104:49173","user_agent":"Mozilla/5.0 (compatible; Let's Encrypt validation server; +https://www.letsencrypt.org)","error":"no information found to solve challenge for identifier: web1580.login-11.loginserver.ch"}
2026-10-20T09:18:03.998375263Z {"level":"warn","ts":1792487883.9982848,"logger":"http","msg":"looking up info for HTTP challenge","uri":"/.well-known/acme-challenge/B9uRcWjt5eCr2iIarb9ZNJftpUOHIRF_i08A5_WDDus","identifier":"web1674.login-11.loginserver.ch","remote_addr":"23.178.112.211:53549","user_agent":"Mozilla/5.0 (compatible; Let's Encrypt validation server; +https://www.letsencrypt.org)","error":"no information found to solve challenge for identifier: web1674.login-11.loginserver.ch"}
2026-10-20T09:18:24.141193441Z {"level":"warn","ts":1792487904.1409767,"logger":"http","msg":"looking up info for HTTP challenge","uri":"/.well-known/acme-challenge/5MBZYYi6G9r9OKHpikQrLUd8n6E5JAmvOokt0Bg0YkQ","identifier":"web1676.login-11.loginserver.ch","remote_addr":"23.178.112.107:37131","user_agent":"Mozilla/5.0 (compatible; Let's Encrypt validation server; +https://www.letsencrypt.org)","error":"no information found to solve challenge for identifier: web1676.login-11.loginserver.ch"}
2026-10-20T09:18:29.292533409Z {"level":"warn","ts":1792487909.2924552,"logger":"http","msg":"looking up info for HTTP challenge","uri":"/.well-known/acme-challenge/kF3r1WmKEqM4BqrgYoMgPeNigE2zbuLDXxW4tYN0WkA","identifier":"web1727.login-11.loginserver.ch","remote_addr":"23.178.112.103:49501","user_agent":"Mozilla/5.0 (compatible; Let's Encrypt validation server; +https://www.letsencrypt.org)","error":"no information found to solve challenge for identifier: web1727.login-11.loginserver.ch"}
2026-10-20T09:18:35.260870479Z {"level":"warn","ts":1792487915.2607424,"logger":"http","msg":"looking up info for HTTP challenge","uri":"/.well-known/acme-challenge/J6xMunir-21sf8Oz4YPx_0vXTRUkxFRosvjJm22eWgk","identifier":"web192.login-11.loginserver.ch","remote_addr":"23.178.112.219:60395","user_agent":"Mozilla/5.0 (compatible; Let's Encrypt validation server; +https://www.letsencrypt.org)","error":"no information found to solve challenge for identifier: web192.login-11.loginserver.ch"}
2026-10-20T09:18:41.721330014Z {"level":"warn","ts":1792487921.721261,"logger":"http","msg":"looking up info for HTTP challenge","uri":"/.well-known/acme-challenge/N7MFs3QhOWtG-FEtT8oYKUP3LxQ-3wveJ0B3-thU568","identifier":"web1739.login-11.loginserver.ch","remote_addr":"23.178.112.213:58955","user_agent":"Mozilla/5.0 (compatible; Let's Encrypt validation server; +https://www.letsencrypt.org)","error":"no information found to solve challenge for identifier: web1739.login-11.loginserver.ch"}
2026-10-20T09:18:47.874911518Z {"level":"warn","ts":1792487927.8748333,"logger":"http","msg":"looking up info for HTTP challenge","uri":"/.well-known/acme-challenge/1vAZEHUZo11_Y8TjDR8gUcufkvZGNQQ-xGyFR-Em82M","identifier":"web873.login-11.loginserver.ch","remote_addr":"23.178.112.107:36901","user_agent":"Mozilla/5.0 (compatible; Let's Encrypt validation server; +https://www.letsencrypt.org)","error":"no information found to solve challenge for identifier: web873.login-11.loginserver.ch"}
2026-10-20T09:18:53.887327133Z {"level":"warn","ts":1792487933.8872633,"logger":"http","msg":"looking up info for HTTP challenge","uri":"/.well-known/acme-challenge/heqW7Zv4YK70tSm18-Y1zYrX02zVd-Z6tppxf3o7CmA","identifier":"web1732.login-11.loginserver.ch","remote_addr":"23.178.112.212:50521","user_agent":"Mozilla/5.0 (compatible; Let's Encrypt validation server; +https://www.letsencrypt.org)","error":"no information found to solve challenge for identifier: web1732.login-11.loginserver.ch"}
2026-10-20T09:19:04.228609743Z {"level":"warn","ts":1792487944.2285354,"logger":"http","msg":"looking up info for HTTP challenge","uri":"/.well-known/acme-challenge/TN0yVmIDicfVUJRZVsBTdm1sGjnFhi2ZODW8rk0SfF0","identifier":"web921.login-11.loginserver.ch","remote_addr":"23.178.112.210:51239","user_agent":"Mozilla/5.0 (compatible; Let's Encrypt validation server; +https://www.letsencrypt.org)","error":"no information found to solve challenge for identifier: web921.login-11.loginserver.ch"}
2026-10-20T09:19:10.918333935Z {"level":"warn","ts":1792487950.9182434,"logger":"http","msg":"looking up info for HTTP challenge","uri":"/.well-known/acme-challenge/bzCDH0om3m6ExSeTFBnhQxkZ7g5aREbMu2iSLViWleY","identifier":"web819.login-11.loginserver.ch","remote_addr":"23.178.112.107:57197","user_agent":"Mozilla/5.0 (compatible; Let's Encrypt validation server; +https://www.letsencrypt.org)","error":"no information found to solve challenge for identifier: web819.login-11.loginserver.ch"}
2026-10-20T09:19:30.895652484Z {"level":"warn","ts":1792487970.8955514,"logger":"http","msg":"looking up info for HTTP challenge","uri":"/.well-known/acme-challenge/_5W68ICSYDDIOG9EQD75UD6GT2X2PPEV","identifier":"www.zurich-limousine.com","remote_addr":"195.16.72.5:49623","user_agent":"Cpanel-HTTP-Client/1.0","error":"no information found to solve challenge for identifier: www.zurich-limousine.com"}
2026-10-20T10:07:35.744224174Z {"level":"warn","ts":1792490855.744122,"logger":"http","msg":"looking up info for HTTP challenge","uri":"/.well-known/acme-challenge/gF0ttAhrjyWHL5tEo6GqnCspIjJHmxazOLxhPiHibMM","identifier":"web1268.login-11.loginserver.ch","remote_addr":"23.178.112.105:61225","user_agent":"Mozilla/5.0 (compatible; Let's Encrypt validation server; +https://www.letsencrypt.org)","error":"no information found to solve challenge for identifier: web1268.login-11.loginserver.ch"}
2026-10-20T10:16:52.210827328Z {"level":"warn","ts":1792491412.2107575,"logger":"http","msg":"looking up info for HTTP challenge","uri":"/.well-known/acme-challenge/_bivM4evFo72RbqG1rtwHDPPGp7XWr0lcApRJH5VAg0","identifier":"web1412.login-11.loginserver.ch","remote_addr":"23.178.112.219:36091","user_agent":"Mozilla/5.0 (compatible; Let's Encrypt validation server; +https://www.letsencrypt.org)","error":"no information found to solve challenge for identifier: web1412.login-11.loginserver.ch"}
2026-10-20T10:17:15.138959554Z {"level":"warn","ts":1792491435.1388624,"logger":"http","msg":"looking up info for HTTP challenge","uri":"/.well-known/acme-challenge/HVcFDow_Vrf5DoPDY-4j8oqWKYbt-7fpYPtRcdURg94","identifier":"web1332.login-11.loginserver.ch","remote_addr":"23.178.112.103:48359","user_agent":"Mozilla/5.0 (compatible; Let's Encrypt validation server; +https://www.letsencrypt.org)","error":"no information found to solve challenge for identifier: web1332.login-11.loginserver.ch"}
2026-10-20T10:21:20.014833712Z {"level":"warn","ts":1792491680.0147343,"logger":"http","msg":"looking up info for HTTP challenge","uri":"/.well-known/acme-challenge/check","identifier":"longwitz.net","remote_addr":"178.254.50.42:60350","user_agent":"curl/7.52.1","error":"no information found to solve challenge for identifier: longwitz.net"}
2026-10-20T10:23:44.721497640Z {"level":"warn","ts":1792491824.7214272,"logger":"http","msg":"looking up info for HTTP challenge","uri":"/.well-known/acme-challenge/check","identifier":"longwitz.net","remote_addr":"178.254.50.71:38656","user_agent":"curl/7.52.1","error":"no information found to solve challenge for identifier: longwitz.net"}
===== journal proxy pid 47723 window =====
-- No entries --
===== journal proxy pid since restart =====
-- No entries --
===== journal identity-proxy around 10:51 UTC =====
2026-10-20T12:48:07+02:00 dark-forge.dev systemd[1]: kiln-identity-proxy.service: Deactivated successfully.
2026-10-20T12:51:08+02:00 dark-forge.dev systemd[1]: Started kiln-identity-proxy.service - Private Caddy bridge to loopback Kiln identity.
===== journal door-proxy restart 10:25 UTC =====
2026-10-20T12:23:30+02:00 dark-forge.dev systemd[1]: kiln-door-proxy.service: Deactivated successfully.
2026-10-20T12:25:44+02:00 dark-forge.dev systemd[1]: Started kiln-door-proxy.service - Private Caddy bridge to the Kiln door.
===== find operations.sqlite =====
/var/lib/kiln/journal/operations.sqlite 16384 2026-10-19T17:12
===== sqlite request rows =====
db=/var/lib/kiln/journal/operations.sqlite
id                                box_id                            created_at  created_utc          running
--------------------------------  --------------------------------  ----------  -------------------  -------
0ddf22b59ebb482f87386c605ec25e09  fa0a7f2a1b3744b4b547db3438e683d9  1792493582  2026-10-20 10:53:02  0      
22ace0b43b0444c693d5b93d6a5db1e0  fa0a7f2a1b3744b4b547db3438e683d9  1792493592  2026-10-20 10:53:12  0      
eb922e3f96cf4db7b1c1d27223180864  fa0a7f2a1b3744b4b547db3438e683d9  1792493591  2026-10-20 10:53:11  0      
===== sqlite response keys only =====
id|box_id|state|error|stdout_prefix
0ddf22b59ebb482f87386c605ec25e09|fa0a7f2a1b3744b4b547db3438e683d9|succeeded||
22ace0b43b0444c693d5b93d6a5db1e0|fa0a7f2a1b3744b4b547db3438e683d9|succeeded||
eb922e3f96cf4db7b1c1d27223180864|fa0a7f2a1b3744b4b547db3438e683d9|succeeded||[107,105,108,110,45,101,120,101,99,45,112,111,108,108,10]
===== other 0.4.6 rollback names =====
/opt/kiln/upgrades/0.4.6-3vO6d0
/usr/local/libexec/kiln-identity.rollback-v033
===== nft inet filter =====
table inet filter {
	chain input {
		type filter hook input priority filter; policy accept;
		tcp dport 8443 accept
	}
}
===== nftables service =====
disabled
inactive
# /usr/lib/systemd/system/nftables.service
[Unit]
Description=nftables
Documentation=man:nft(8) http://wiki.nftables.org
Wants=network-pre.target
Before=network-pre.target shutdown.target
Conflicts=shutdown.target
DefaultDependencies=no

[Service]
Type=oneshot
RemainAfterExit=yes
StandardInput=null
ProtectSystem=full
ProtectHome=true
ExecStart=/usr/sbin/nft -f /etc/nftables.conf
ExecReload=/usr/sbin/nft -f /etc/nftables.conf
ExecStop=/usr/sbin/nft flush ruleset

[Install]
WantedBy=sysinit.target
COLLECT2_DONE
```

## 2. stderr

```text
(empty)
```

## 3. Caddy metric names

```text
# HELP caddy_admin_http_requests_total Counter of requests made to the Admin API's HTTP endpoints.
# HELP caddy_config_last_reload_success_timestamp_seconds Timestamp of the last successful configuration reload.
# HELP caddy_config_last_reload_successful Whether the last configuration reload attempt was successful.
# HELP caddy_reverse_proxy_upstreams_healthy Health status of reverse proxy upstreams.
# HELP go_build_info Build information about the main Go module.
# HELP go_gc_duration_seconds A summary of the wall-time pause (stop-the-world) duration in garbage collection cycles.
# HELP go_gc_gogc_percent Heap size target percentage configured by the user, otherwise 100. This value is set by the GOGC environment variable, and the runtime/debug.SetGCPercent function. Sourced from /gc/gogc:percent.
# HELP go_gc_gomemlimit_bytes Go runtime memory limit configured by the user, otherwise math.MaxInt64. This value is set by the GOMEMLIMIT environment variable, and the runtime/debug.SetMemoryLimit function. Sourced from /gc/gomemlimit:bytes.
# HELP go_goroutines Number of goroutines that currently exist.
# HELP go_info Information about the Go environment.
# HELP go_memstats_alloc_bytes Number of bytes allocated in heap and currently in use. Equals to /memory/classes/heap/objects:bytes.
# HELP go_memstats_alloc_bytes_total Total number of bytes allocated in heap until now, even if released already. Equals to /gc/heap/allocs:bytes.
# HELP go_memstats_buck_hash_sys_bytes Number of bytes used by the profiling bucket hash table. Equals to /memory/classes/profiling/buckets:bytes.
# HELP go_memstats_frees_total Total number of heap objects frees. Equals to /gc/heap/frees:objects + /gc/heap/tiny/allocs:objects.
# HELP go_memstats_gc_sys_bytes Number of bytes used for garbage collection system metadata. Equals to /memory/classes/metadata/other:bytes.
# HELP go_memstats_heap_alloc_bytes Number of heap bytes allocated and currently in use, same as go_memstats_alloc_bytes. Equals to /memory/classes/heap/objects:bytes.
# HELP go_memstats_heap_idle_bytes Number of heap bytes waiting to be used. Equals to /memory/classes/heap/released:bytes + /memory/classes/heap/free:bytes.
# HELP go_memstats_heap_inuse_bytes Number of heap bytes that are in use. Equals to /memory/classes/heap/objects:bytes + /memory/classes/heap/unused:bytes
# HELP go_memstats_heap_objects Number of currently allocated objects. Equals to /gc/heap/objects:objects.
# HELP go_memstats_heap_released_bytes Number of heap bytes released to OS. Equals to /memory/classes/heap/released:bytes.
# HELP go_memstats_heap_sys_bytes Number of heap bytes obtained from system. Equals to /memory/classes/heap/objects:bytes + /memory/classes/heap/unused:bytes + /memory/classes/heap/released:bytes + /memory/classes/heap/free:bytes.
# HELP go_memstats_last_gc_time_seconds Number of seconds since 1970 of last garbage collection.
# HELP go_memstats_mallocs_total Total number of heap objects allocated, both live and gc-ed. Semantically a counter version for go_memstats_heap_objects gauge. Equals to /gc/heap/allocs:objects + /gc/heap/tiny/allocs:objects.
# HELP go_memstats_mcache_inuse_bytes Number of bytes in use by mcache structures. Equals to /memory/classes/metadata/mcache/inuse:bytes.
# HELP go_memstats_mcache_sys_bytes Number of bytes used for mcache structures obtained from system. Equals to /memory/classes/metadata/mcache/inuse:bytes + /memory/classes/metadata/mcache/free:bytes.
# HELP go_memstats_mspan_inuse_bytes Number of bytes in use by mspan structures. Equals to /memory/classes/metadata/mspan/inuse:bytes.
# HELP go_memstats_mspan_sys_bytes Number of bytes used for mspan structures obtained from system. Equals to /memory/classes/metadata/mspan/inuse:bytes + /memory/classes/metadata/mspan/free:bytes.
# HELP go_memstats_next_gc_bytes Number of heap bytes when next garbage collection will take place. Equals to /gc/heap/goal:bytes.
# HELP go_memstats_other_sys_bytes Number of bytes used for other system allocations. Equals to /memory/classes/other:bytes.
# HELP go_memstats_stack_inuse_bytes Number of bytes obtained from system for stack allocator in non-CGO environments. Equals to /memory/classes/heap/stacks:bytes.
# HELP go_memstats_stack_sys_bytes Number of bytes obtained from system for stack allocator. Equals to /memory/classes/heap/stacks:bytes + /memory/classes/os-stacks:bytes.
# HELP go_memstats_sys_bytes Number of bytes obtained from system. Equals to /memory/classes/total:byte.
# HELP go_sched_gomaxprocs_threads The current runtime.GOMAXPROCS setting, or the number of operating system threads that can execute user-level Go code simultaneously. Sourced from /sched/gomaxprocs:threads.
# HELP go_threads Number of OS threads created.
# HELP process_cpu_seconds_total Total user and system CPU time spent in seconds.
# HELP process_max_fds Maximum number of open file descriptors.
# HELP process_network_receive_bytes_total Number of bytes received by the process over the network.
# HELP process_network_transmit_bytes_total Number of bytes sent by the process over the network.
# HELP process_open_fds Number of open file descriptors.
# HELP process_resident_memory_bytes Resident memory size in bytes.
# HELP process_start_time_seconds Start time of the process since unix epoch in seconds.
# HELP process_virtual_memory_bytes Virtual memory size in bytes.
# HELP process_virtual_memory_max_bytes Maximum amount of virtual memory available in bytes.
# HELP promhttp_metric_handler_requests_in_flight Current number of scrapes being served.
# HELP promhttp_metric_handler_requests_total Total number of scrapes by HTTP status code.
```

## 3. Caddy metrics filtered

```text
===== caddy metrics matching door/cli =====
# HELP caddy_reverse_proxy_upstreams_healthy Health status of reverse proxy upstreams.
# TYPE caddy_reverse_proxy_upstreams_healthy gauge
caddy_reverse_proxy_upstreams_healthy{upstream="172.18.0.1:8092"} 1
caddy_reverse_proxy_upstreams_healthy{upstream="172.18.0.1:8444"} 1
caddy_reverse_proxy_upstreams_healthy{upstream="hub:3000"} 1
caddy_reverse_proxy_upstreams_healthy{upstream="paseo:6767"} 1
METRICS_DONE
```

## 3. metrics stderr

```text
(empty)
```

## 4. Prune script

```text
set -u
echo "===== identity immediately before prune ====="
systemctl show kiln-identity.service -p Id -p ActiveState -p SubState -p UnitFileState -p MainPID -p NRestarts -p ActiveEnterTimestamp
echo "--- kiln-identity-proxy.service ---"
systemctl show kiln-identity-proxy.service -p Id -p ActiveState -p SubState -p UnitFileState -p MainPID -p NRestarts -p ActiveEnterTimestamp
echo "--- kiln-identity-proxy.socket ---"
systemctl show kiln-identity-proxy.socket -p Id -p ActiveState -p SubState -p UnitFileState -p ActiveEnterTimestamp
echo "===== authorized_keys immediately before prune ====="
sha256sum /root/.ssh/authorized_keys
stat -c '%n %a %u %g %s %y' /root/.ssh/authorized_keys
echo "===== listeners before prune ====="
ss -ltnp | awk 'NR==1 || /:8443/ || /:8444/ || /:443/ || /:8091/ || /:8092/'
echo "===== planned delete listing ====="
echo "+ find /opt/kiln/upgrades/0.4.6-3vO6d0 -printf '%p\n'"
find /opt/kiln/upgrades/0.4.6-3vO6d0 -printf '%p\n'
echo "+ ls -la /opt/kiln/upgrades /opt/kiln/upgrades/0.4.6-3vO6d0"
ls -la /opt/kiln/upgrades /opt/kiln/upgrades/0.4.6-3vO6d0
echo "+ sha256sum /opt/kiln/upgrades/0.4.6-3vO6d0/*"
sha256sum /opt/kiln/upgrades/0.4.6-3vO6d0/*
echo "===== mapped by live processes? ====="
echo "+ grep -l 0.4.6-3vO6d0 /proc/[0-9]*/maps"
grep -l '0.4.6-3vO6d0' /proc/[0-9]*/maps 2>/dev/null || echo 'no-live-map'
echo "===== delete only the named 0.4.6 directory ====="
echo "+ rm -rf -- /opt/kiln/upgrades/0.4.6-3vO6d0"
rm -rf -- /opt/kiln/upgrades/0.4.6-3vO6d0
echo rm_exit:$?
echo "===== upgrades after delete ====="
echo "+ ls -la /opt/kiln/upgrades"
ls -la /opt/kiln/upgrades
echo "+ test ! -e /opt/kiln/upgrades/0.4.6-3vO6d0"
if test ! -e /opt/kiln/upgrades/0.4.6-3vO6d0; then echo 'absent:0.4.6-3vO6d0'; else echo 'STILL_PRESENT'; fi
echo "+ test -d /opt/kiln/upgrades/0.4.4-8DDh4B"
if test -d /opt/kiln/upgrades/0.4.4-8DDh4B; then echo 'kept:0.4.4-8DDh4B'; else echo 'MISSING_0.4.4'; fi
echo "+ test -d /opt/kiln/upgrades/0.4.5-RSNr4F"
if test -d /opt/kiln/upgrades/0.4.5-RSNr4F; then echo 'kept:0.4.5-RSNr4F'; else echo 'MISSING_0.4.5'; fi
echo "===== doctor ====="
echo "+ env PATH=/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin /opt/kiln/bin/kiln-runtime --state-dir /var/lib/kiln/runtime --isolation-config /etc/kiln/isolation.json doctor"
env PATH=/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin /opt/kiln/bin/kiln-runtime --state-dir /var/lib/kiln/runtime --isolation-config /etc/kiln/isolation.json doctor
echo doctor_exit:$?
echo "===== smoke ====="
cfg=$(mktemp /var/tmp/kiln-smoke-prune046.XXXXXX.json)
cleanup() {
  if [ -n "${cfg:-}" ] && [ -f "$cfg" ]; then
    shred -u "$cfg" || rm -f "$cfg"
  fi
}
trap cleanup EXIT
echo "+ kiln --config $cfg profile add default --url https://127.0.0.1:8443 --token-file /etc/kiln/admin.token --ca-file /etc/kiln/ca.crt"
env PATH=/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin kiln --config "$cfg" profile add default --url https://127.0.0.1:8443 --token-file /etc/kiln/admin.token --ca-file /etc/kiln/ca.crt
echo profile_add_exit:$?
python3 - "$cfg" << 'PY'
import json, sys
text = open(sys.argv[1]).read()
print(text)
print("contains_bearer", "Bearer " in text)
print("contains_admin_token_path", "/etc/kiln/admin.token" in text)
PY
echo "+ kiln --config $cfg list"
env PATH=/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin kiln --config "$cfg" list
echo list_before_exit:$?
echo "+ kiln --config $cfg create --template ubuntu-4g --name kiln-smoke-prune046"
env PATH=/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin kiln --config "$cfg" create --template ubuntu-4g --name kiln-smoke-prune046
echo create_exit:$?
echo "+ kiln --config $cfg exec kiln-smoke-prune046 -- /bin/echo kiln-prune-ok"
env PATH=/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin kiln --config "$cfg" exec kiln-smoke-prune046 -- /bin/echo kiln-prune-ok
echo exec_exit:$?
echo "+ kiln --config $cfg delete kiln-smoke-prune046"
env PATH=/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin kiln --config "$cfg" delete kiln-smoke-prune046
echo delete_exit:$?
echo "+ kiln --config $cfg list"
env PATH=/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin kiln --config "$cfg" list
echo list_after_exit:$?
cleanup
trap - EXIT
echo "+ ls $cfg"
ls -l "$cfg" 2>&1 || true
echo "===== listeners after ====="
ss -ltnp | awk 'NR==1 || /:8443/ || /:8444/ || /:443/ || /:8091/ || /:8092/'
echo "===== identity after ====="
systemctl show kiln-identity.service -p Id -p ActiveState -p SubState -p UnitFileState -p MainPID -p NRestarts -p ActiveEnterTimestamp
echo "--- kiln-identity-proxy.service ---"
systemctl show kiln-identity-proxy.service -p Id -p ActiveState -p SubState -p UnitFileState -p MainPID -p NRestarts -p ActiveEnterTimestamp
echo "--- kiln-identity-proxy.socket ---"
systemctl show kiln-identity-proxy.socket -p Id -p ActiveState -p SubState -p UnitFileState -p ActiveEnterTimestamp
echo "===== authorized_keys after ====="
sha256sum /root/.ssh/authorized_keys
stat -c '%n %a %u %g %s %y' /root/.ssh/authorized_keys
echo "===== public socket still enabled ====="
systemctl is-enabled kiln-door-proxy-public.socket
systemctl is-active kiln-door-proxy-public.socket
systemctl show kiln-door-proxy-public.socket -p Id -p ActiveState -p SubState -p UnitFileState -p ActiveEnterTimestamp -p Listen
systemctl show kiln-door-proxy.service -p Id -p ActiveState -p SubState -p MainPID -p ActiveEnterTimestamp
echo "PRUNE_DONE"
```

## 4. stdout

```text
===== identity immediately before prune =====
MainPID=3522047
NRestarts=0
Id=kiln-identity.service
ActiveState=active
SubState=running
UnitFileState=enabled
ActiveEnterTimestamp=Mon 2026-10-19 12:42:17 CEST
--- kiln-identity-proxy.service ---
MainPID=60490
NRestarts=0
Id=kiln-identity-proxy.service
ActiveState=active
SubState=running
UnitFileState=static
ActiveEnterTimestamp=Tue 2026-10-20 12:51:08 CEST
--- kiln-identity-proxy.socket ---
Id=kiln-identity-proxy.socket
ActiveState=active
SubState=running
UnitFileState=enabled
ActiveEnterTimestamp=Sat 2026-10-03 20:14:48 CEST
===== authorized_keys immediately before prune =====
4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37  /root/.ssh/authorized_keys
/root/.ssh/authorized_keys 600 0 0 592 2026-10-19 16:54:10.656267246 +0200
===== listeners before prune =====
State  Recv-Q Send-Q Local Address:Port Peer Address:PortProcess                                                             
LISTEN 0      4096      172.18.0.1:8092      0.0.0.0:*    users:(("systemd-socket-",pid=60490,fd=3),("systemd",pid=1,fd=150))
LISTEN 0      4096      172.18.0.1:8444      0.0.0.0:*    users:(("systemd-socket-",pid=47723,fd=3),("systemd",pid=1,fd=110))
LISTEN 0      4096     78.46.98.73:8443      0.0.0.0:*    users:(("systemd-socket-",pid=47723,fd=4),("systemd",pid=1,fd=135))
LISTEN 0      128        127.0.0.1:8091      0.0.0.0:*    users:(("kiln-identity",pid=3522047,fd=12))                        
LISTEN 0      4096         0.0.0.0:443       0.0.0.0:*    users:(("docker-proxy",pid=4422,fd=8))                             
LISTEN 0      128        127.0.0.1:8444      0.0.0.0:*    users:(("kiln-api",pid=4167939,fd=9))                              
LISTEN 0      128        127.0.0.1:8443      0.0.0.0:*    users:(("kiln-api",pid=4165125,fd=9))                              
LISTEN 0      4096            [::]:443          [::]:*    users:(("docker-proxy",pid=4429,fd=8))                             
===== planned delete listing =====
+ find /opt/kiln/upgrades/0.4.6-3vO6d0 -printf '%p\n'
/opt/kiln/upgrades/0.4.6-3vO6d0
/opt/kiln/upgrades/0.4.6-3vO6d0/kiln
/opt/kiln/upgrades/0.4.6-3vO6d0/kiln-network
/opt/kiln/upgrades/0.4.6-3vO6d0/kiln-host
/opt/kiln/upgrades/0.4.6-3vO6d0/kiln-runtime
/opt/kiln/upgrades/0.4.6-3vO6d0/kiln-api
+ ls -la /opt/kiln/upgrades /opt/kiln/upgrades/0.4.6-3vO6d0
/opt/kiln/upgrades:
total 20
drwx------ 5 root root 4096 Oct 20 10:02 .
drwxr-xr-x 4 root root 4096 Oct 20 10:02 ..
drwx------ 2 root root 4096 Oct 19 19:52 0.4.4-8DDh4B
drwx------ 2 root root 4096 Oct 19 23:27 0.4.5-RSNr4F
drwx------ 2 root root 4096 Oct 20 10:02 0.4.6-3vO6d0

/opt/kiln/upgrades/0.4.6-3vO6d0:
total 37800
drwx------ 2 root root     4096 Oct 20 10:02 .
drwx------ 5 root root     4096 Oct 20 10:02 ..
-rwxr-xr-x 1 root root  8167552 Oct 20 10:02 kiln
-rwxr-xr-x 1 root root 10597496 Oct 20 10:02 kiln-api
-rwxr-xr-x 1 root root 11421536 Oct 20 10:02 kiln-host
-rwxr-xr-x 1 root root     6077 Oct 20 10:02 kiln-network
-rwxr-xr-x 1 root root  8492800 Oct 20 10:02 kiln-runtime
+ sha256sum /opt/kiln/upgrades/0.4.6-3vO6d0/*
53823917c8b199ecf708fcb548e262b6ef02538ef97012e4d556865dfceb8ae8  /opt/kiln/upgrades/0.4.6-3vO6d0/kiln
05dfe191b9e86e4f5a430c199b485ebad0bf8edafbf4594a2b133d7b6d549797  /opt/kiln/upgrades/0.4.6-3vO6d0/kiln-api
69764fb84927c53ad10d0a29f3ef2dade2aff780228604b15c7ced23c30691e1  /opt/kiln/upgrades/0.4.6-3vO6d0/kiln-host
6aa537753ef5ee4c1b307b3a13d9639b52ebb7c56b02d86f811119c975146d1a  /opt/kiln/upgrades/0.4.6-3vO6d0/kiln-network
f75b1b1e627fb897e8a22d1977a661a3f40ccaba08092884098a405dfd4f05a6  /opt/kiln/upgrades/0.4.6-3vO6d0/kiln-runtime
===== mapped by live processes? =====
+ grep -l 0.4.6-3vO6d0 /proc/[0-9]*/maps
no-live-map
===== delete only the named 0.4.6 directory =====
+ rm -rf -- /opt/kiln/upgrades/0.4.6-3vO6d0
rm_exit:0
===== upgrades after delete =====
+ ls -la /opt/kiln/upgrades
total 16
drwx------ 4 root root 4096 Oct 20 12:59 .
drwxr-xr-x 4 root root 4096 Oct 20 10:02 ..
drwx------ 2 root root 4096 Oct 19 19:52 0.4.4-8DDh4B
drwx------ 2 root root 4096 Oct 19 23:27 0.4.5-RSNr4F
+ test ! -e /opt/kiln/upgrades/0.4.6-3vO6d0
absent:0.4.6-3vO6d0
+ test -d /opt/kiln/upgrades/0.4.4-8DDh4B
kept:0.4.4-8DDh4B
+ test -d /opt/kiln/upgrades/0.4.5-RSNr4F
kept:0.4.5-RSNr4F
===== doctor =====
+ env PATH=/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin /opt/kiln/bin/kiln-runtime --state-dir /var/lib/kiln/runtime --isolation-config /etc/kiln/isolation.json doctor
{
  "architecture": "x86_64",
  "development_ready": true,
  "firecracker_version": "1.17.0",
  "isolated_ready": true,
  "jailer_version": "1.17.0",
  "problems": [],
  "schema_version": 1
}
doctor_exit:0
===== smoke =====
+ kiln --config /var/tmp/kiln-smoke-prune046.wBkIO2.json profile add default --url https://127.0.0.1:8443 --token-file /etc/kiln/admin.token --ca-file /etc/kiln/ca.crt
profile_add_exit:1

contains_bearer False
contains_admin_token_path False
+ kiln --config /var/tmp/kiln-smoke-prune046.wBkIO2.json list
list_before_exit:1
+ kiln --config /var/tmp/kiln-smoke-prune046.wBkIO2.json create --template ubuntu-4g --name kiln-smoke-prune046
create_exit:1
+ kiln --config /var/tmp/kiln-smoke-prune046.wBkIO2.json exec kiln-smoke-prune046 -- /bin/echo kiln-prune-ok
exec_exit:1
+ kiln --config /var/tmp/kiln-smoke-prune046.wBkIO2.json delete kiln-smoke-prune046
delete_exit:1
+ kiln --config /var/tmp/kiln-smoke-prune046.wBkIO2.json list
list_after_exit:1
+ ls /var/tmp/kiln-smoke-prune046.wBkIO2.json
ls: cannot access '/var/tmp/kiln-smoke-prune046.wBkIO2.json': No such file or directory
===== listeners after =====
State  Recv-Q Send-Q Local Address:Port Peer Address:PortProcess                                                             
LISTEN 0      4096      172.18.0.1:8092      0.0.0.0:*    users:(("systemd-socket-",pid=60490,fd=3),("systemd",pid=1,fd=150))
LISTEN 0      4096      172.18.0.1:8444      0.0.0.0:*    users:(("systemd-socket-",pid=47723,fd=3),("systemd",pid=1,fd=110))
LISTEN 0      4096     78.46.98.73:8443      0.0.0.0:*    users:(("systemd-socket-",pid=47723,fd=4),("systemd",pid=1,fd=135))
LISTEN 0      128        127.0.0.1:8091      0.0.0.0:*    users:(("kiln-identity",pid=3522047,fd=12))                        
LISTEN 0      4096         0.0.0.0:443       0.0.0.0:*    users:(("docker-proxy",pid=4422,fd=8))                             
LISTEN 0      128        127.0.0.1:8444      0.0.0.0:*    users:(("kiln-api",pid=4167939,fd=9))                              
LISTEN 0      128        127.0.0.1:8443      0.0.0.0:*    users:(("kiln-api",pid=4165125,fd=9))                              
LISTEN 0      4096            [::]:443          [::]:*    users:(("docker-proxy",pid=4429,fd=8))                             
===== identity after =====
MainPID=3522047
NRestarts=0
Id=kiln-identity.service
ActiveState=active
SubState=running
UnitFileState=enabled
ActiveEnterTimestamp=Mon 2026-10-19 12:42:17 CEST
--- kiln-identity-proxy.service ---
MainPID=60490
NRestarts=0
Id=kiln-identity-proxy.service
ActiveState=active
SubState=running
UnitFileState=static
ActiveEnterTimestamp=Tue 2026-10-20 12:51:08 CEST
--- kiln-identity-proxy.socket ---
Id=kiln-identity-proxy.socket
ActiveState=active
SubState=running
UnitFileState=enabled
ActiveEnterTimestamp=Sat 2026-10-03 20:14:48 CEST
===== authorized_keys after =====
4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37  /root/.ssh/authorized_keys
/root/.ssh/authorized_keys 600 0 0 592 2026-10-19 16:54:10.656267246 +0200
===== public socket still enabled =====
enabled
active
Listen=78.46.98.73:8443 (Stream)
Id=kiln-door-proxy-public.socket
ActiveState=active
SubState=running
UnitFileState=enabled
ActiveEnterTimestamp=Tue 2026-10-20 10:06:13 CEST
MainPID=47723
Id=kiln-door-proxy.service
ActiveState=active
SubState=running
ActiveEnterTimestamp=Tue 2026-10-20 12:25:44 CEST
PRUNE_DONE
```

## 4. stderr

```text
error: EOF while parsing a value at line 1 column 0
error: EOF while parsing a value at line 1 column 0
error: EOF while parsing a value at line 1 column 0
error: EOF while parsing a value at line 1 column 0
error: EOF while parsing a value at line 1 column 0
error: EOF while parsing a value at line 1 column 0
```

## 5. Smoke retry script

```text
set -u
cfg=/var/tmp/kiln-smoke-prune046.json
rm -f "$cfg"
cleanup() {
  if [ -f "$cfg" ]; then
    shred -u "$cfg" || rm -f "$cfg"
  fi
}
trap cleanup EXIT
echo "+ test ! -e $cfg"
if test ! -e "$cfg"; then echo 'config_absent_before'; else echo 'config_exists_before'; fi
echo "+ kiln --config $cfg profile add default --url https://127.0.0.1:8443 --token-file /etc/kiln/admin.token --ca-file /etc/kiln/ca.crt"
env PATH=/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin kiln --config "$cfg" profile add default --url https://127.0.0.1:8443 --token-file /etc/kiln/admin.token --ca-file /etc/kiln/ca.crt
echo profile_add_exit:$?
echo "+ python3 profile inspection"
python3 - "$cfg" << 'PY'
import sys
text = open(sys.argv[1]).read()
print(text)
print("contains_bearer", "Bearer " in text)
print("contains_admin_token_path", "/etc/kiln/admin.token" in text)
PY
echo "+ kiln --config $cfg list"
env PATH=/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin kiln --config "$cfg" list
echo list_before_exit:$?
echo "+ kiln --config $cfg create --template ubuntu-4g --name kiln-smoke-prune046"
env PATH=/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin kiln --config "$cfg" create --template ubuntu-4g --name kiln-smoke-prune046
echo create_exit:$?
echo "+ kiln --config $cfg exec kiln-smoke-prune046 -- /bin/echo kiln-prune-ok"
env PATH=/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin kiln --config "$cfg" exec kiln-smoke-prune046 -- /bin/echo kiln-prune-ok
echo exec_exit:$?
echo "+ kiln --config $cfg delete kiln-smoke-prune046"
env PATH=/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin kiln --config "$cfg" delete kiln-smoke-prune046
echo delete_exit:$?
echo "+ kiln --config $cfg list"
env PATH=/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin kiln --config "$cfg" list
echo list_after_exit:$?
cleanup
trap - EXIT
echo "+ ls $cfg"
ls -l "$cfg" 2>&1 || true
echo "===== doctor after smoke ====="
echo "+ env PATH=/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin /opt/kiln/bin/kiln-runtime --state-dir /var/lib/kiln/runtime --isolation-config /etc/kiln/isolation.json doctor"
env PATH=/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin /opt/kiln/bin/kiln-runtime --state-dir /var/lib/kiln/runtime --isolation-config /etc/kiln/isolation.json doctor
echo doctor_exit:$?
echo "===== identity after smoke ====="
systemctl show kiln-identity.service -p Id -p ActiveState -p SubState -p UnitFileState -p MainPID -p NRestarts -p ActiveEnterTimestamp
echo "--- kiln-identity-proxy.service ---"
systemctl show kiln-identity-proxy.service -p Id -p ActiveState -p SubState -p UnitFileState -p MainPID -p NRestarts -p ActiveEnterTimestamp
echo "--- kiln-identity-proxy.socket ---"
systemctl show kiln-identity-proxy.socket -p Id -p ActiveState -p SubState -p UnitFileState -p ActiveEnterTimestamp
echo "===== authorized_keys after smoke ====="
sha256sum /root/.ssh/authorized_keys
stat -c '%n %a %u %g %s %y' /root/.ssh/authorized_keys
echo "===== listeners after smoke ====="
ss -ltnp | awk 'NR==1 || /:8443/ || /:8444/ || /:443 / || /:8091/ || /:8092/'
echo "SMOKE_DONE"
```

## 5. stdout

```text
+ test ! -e /var/tmp/kiln-smoke-prune046.json
config_absent_before
+ kiln --config /var/tmp/kiln-smoke-prune046.json profile add default --url https://127.0.0.1:8443 --token-file /etc/kiln/admin.token --ca-file /etc/kiln/ca.crt
added profile default
profile_add_exit:0
+ python3 profile inspection
{
  "profiles": {
    "default": {
      "url": "https://127.0.0.1:8443",
      "token_file": "/etc/kiln/admin.token",
      "ca_file": "/etc/kiln/ca.crt"
    }
  }
}
contains_bearer False
contains_admin_token_path True
+ kiln --config /var/tmp/kiln-smoke-prune046.json list
No boxes yet. Create one with `kiln create`.
list_before_exit:0
+ kiln --config /var/tmp/kiln-smoke-prune046.json create --template ubuntu-4g --name kiln-smoke-prune046
NAME                 ID                                STATE           MEMORY  VCPUS
kiln-smoke-prune046  3ec21a87917b475e8e2d8c4007ed3c8e  running       4096 MiB  1
create_exit:0
+ kiln --config /var/tmp/kiln-smoke-prune046.json exec kiln-smoke-prune046 -- /bin/echo kiln-prune-ok
kiln-prune-ok
exec_exit:0
+ kiln --config /var/tmp/kiln-smoke-prune046.json delete kiln-smoke-prune046
deleted 3ec21a87917b475e8e2d8c4007ed3c8e
delete_exit:0
+ kiln --config /var/tmp/kiln-smoke-prune046.json list
No boxes yet. Create one with `kiln create`.
list_after_exit:0
+ ls /var/tmp/kiln-smoke-prune046.json
ls: cannot access '/var/tmp/kiln-smoke-prune046.json': No such file or directory
===== doctor after smoke =====
+ env PATH=/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin /opt/kiln/bin/kiln-runtime --state-dir /var/lib/kiln/runtime --isolation-config /etc/kiln/isolation.json doctor
{
  "architecture": "x86_64",
  "development_ready": true,
  "firecracker_version": "1.17.0",
  "isolated_ready": true,
  "jailer_version": "1.17.0",
  "problems": [],
  "schema_version": 1
}
doctor_exit:0
===== identity after smoke =====
MainPID=3522047
NRestarts=0
Id=kiln-identity.service
ActiveState=active
SubState=running
UnitFileState=enabled
ActiveEnterTimestamp=Mon 2026-10-19 12:42:17 CEST
--- kiln-identity-proxy.service ---
MainPID=60490
NRestarts=0
Id=kiln-identity-proxy.service
ActiveState=active
SubState=running
UnitFileState=static
ActiveEnterTimestamp=Tue 2026-10-20 12:51:08 CEST
--- kiln-identity-proxy.socket ---
Id=kiln-identity-proxy.socket
ActiveState=active
SubState=running
UnitFileState=enabled
ActiveEnterTimestamp=Sat 2026-10-03 20:14:48 CEST
===== authorized_keys after smoke =====
4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37  /root/.ssh/authorized_keys
/root/.ssh/authorized_keys 600 0 0 592 2026-10-19 16:54:10.656267246 +0200
===== listeners after smoke =====
State  Recv-Q Send-Q Local Address:Port Peer Address:PortProcess                                                             
LISTEN 0      4096      172.18.0.1:8092      0.0.0.0:*    users:(("systemd-socket-",pid=60490,fd=3),("systemd",pid=1,fd=150))
LISTEN 0      4096      172.18.0.1:8444      0.0.0.0:*    users:(("systemd-socket-",pid=47723,fd=3),("systemd",pid=1,fd=110))
LISTEN 0      4096     78.46.98.73:8443      0.0.0.0:*    users:(("systemd-socket-",pid=47723,fd=4),("systemd",pid=1,fd=135))
LISTEN 0      128        127.0.0.1:8091      0.0.0.0:*    users:(("kiln-identity",pid=3522047,fd=12))                        
LISTEN 0      4096         0.0.0.0:443       0.0.0.0:*    users:(("docker-proxy",pid=4422,fd=8))                             
LISTEN 0      128        127.0.0.1:8444      0.0.0.0:*    users:(("kiln-api",pid=4167939,fd=9))                              
LISTEN 0      128        127.0.0.1:8443      0.0.0.0:*    users:(("kiln-api",pid=4165125,fd=9))                              
LISTEN 0      4096            [::]:443          [::]:*    users:(("docker-proxy",pid=4429,fd=8))                             
SMOKE_DONE
```

## 5. stderr

```text
creating kiln-smoke-prune046 with template ubuntu-4g
request 2282fa54e7234bfba13ed5be7ef84854
request 6252699400ef40538dee53af68f1f574
request 5db90a9f6d0d41968d81c5da339fbdc4
```

## 6. Off-host checks from the operator VM

```text
===== curl https://cli.dark-forge.dev/ =====
http_code:401
===== door body =====
{"code":"unauthorized","message":"A valid credential is required."}
===== door headers =====
HTTP/2 401 
alt-svc: h3=":443"; ma=2592000
cache-control: no-store
content-security-policy: default-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'
content-type: application/json
date: Tue, 20 Oct 2026 10:59:58 GMT
referrer-policy: strict-origin
via: 2.0 Caddy
x-content-type-options: nosniff
content-length: 67

===== curl https://cli.dark-forge.dev/v1/templates =====
http_code:401
===== templates body =====
{"code":"unauthorized","message":"A valid credential is required."}
===== templates headers =====
HTTP/2 401 
alt-svc: h3=":443"; ma=2592000
cache-control: no-store
content-security-policy: default-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'
content-type: application/json
date: Tue, 20 Oct 2026 10:59:58 GMT
referrer-policy: strict-origin
via: 2.0 Caddy
x-content-type-options: nosniff
content-length: 67

===== curl https://dark-forge.dev/login =====
http_code:200
===== login grep =====
1:<!doctype html><html><head><meta charset=utf-8><meta name=viewport content="width=device-width"><title>Kiln identity</title><link rel=stylesheet href=/static/auth.css></head><body><main class=card><div class=mark>Kiln identity</div><h1>Light the way in.</h1><p>Choose the same identity provider you used before. Accounts are never merged by email.</p><a class=button href=/oauth/github>Continue with GitHub</a><a class="button secondary" href=/oauth/google>Continue with Google</a></main></body></html>
===== 8443 tcp from off-host =====
tcp_8443:connected ('78.46.98.73', 8443)
===== openssl 8443 brief =====
sha256 Fingerprint=D4:BA:F2:89:F2:62:5D:E8:67:57:2D:4F:97:F8:CB:B4:D7:8B:27:BE:FA:63:0B:53:A8:DB:3E:0C:74:C2:7D:3A
issuer=CN = kiln private CA
subject=CN = cli.dark-forge.dev
X509v3 Subject Alternative Name: 
    DNS:cli.dark-forge.dev
OFFHOST_DONE
```

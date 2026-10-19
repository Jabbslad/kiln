# dark-forge.dev upgrade to Kiln v0.4.6

Phase 1 record of the upgrade of dark-forge.dev from Kiln 0.4.5 to the published release v0.4.6, tag `v0.4.6` at main `e34af50e7f6f122474d5bb39a1d8f0e3582c438e`. The server archive is `kiln-server-v0.4.6-x86_64-unknown-linux-gnu.tar.gz` from https://github.com/Jabbslad/kiln/releases/tag/v0.4.6. The installer that ran is `install.sh` inside that archive. No app source on the host was edited. The laptop client on the operator VM is the published `kiln-v0.4.6-x86_64-unknown-linux-gnu.tar.gz`, checked against its `.sha256` file. The Jabbslad/kiln-install one-line installer was not used.

Phase 2, the off-server `kiln list` / `create` / `exec` / `delete` proof after Jamie approves, is not in this revision. The deploy key is still on the operator VM and will be shredded at the end of phase 2. Device codes are omitted from this public file. They were printed for the operator in the phase 1 report, and each pending row expires at the UTC time recorded below.

## Phase 1 result

| Check | Result |
| --- | --- |
| Pre-check | Host was Kiln 0.4.5. Firecracker and jailer were 1.17.0. `kiln-identity` was 0.4.1, MainPID 3522047. No enrollment file existed. `authorized_keys` was `4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37`. |
| Archive checksum | `sha256sum -c` printed `OK`. Digest `d3f4e26f014bb295fb5ebb4286b2d6d3a0ac6947bc8ff606f81ceff6d105ea40`. |
| `install.sh --apply` | Pass. Exit 0. Previous binaries: `/opt/kiln/upgrades/0.4.5-RSNr4F`. |
| Doctor `development_ready` | True. Exit 0. |
| Versions | `kiln`, `kiln-runtime`, `kiln-host`, and `kiln-api` report 0.4.6. `kiln-identity` stayed 0.4.1. |
| Firecracker and jailer | Both 1.17.0. |
| On-server smoke test | Pass. Guest `uname -r` printed `6.1.155+`. The box was deleted and a later list was empty. |
| `dpkg` diff | Empty. Both lists share sha256 `d388be45b10aa31fa9ab14681dd30f24fa77e122363f6acb724f6f507703807d`. |
| Login unit files and `kiln-identity` process | Unchanged. MainPID stayed 3522047. Config hashes stayed the same. |
| Identity proxy | Socket activation started `kiln-identity-proxy.service` when the device flows reached the identity service. The unit file was not edited. |
| `authorized_keys` | Unchanged. |
| Firewall | Host filter is nftables via iptables-nft. `ufw` is absent. IPv4 INPUT policy was already ACCEPT. Added only `inet filter` / `input` / `tcp dport 8443 accept`, policy accept. Docker filter and nat rules were not edited. Packet counters moved with live traffic. |
| Off-server probe of tcp/8443 | Connection refused in about 110 ms, before and after the nft rule. The path is not dropped by a provider firewall. Nothing is listening on the public address yet. |
| HTTP 401 | Not yet. `kiln-api door` refuses to start without `/etc/kiln/identity.json`. `deploy/enroll-dark-forge.sh` installs `kiln-door.service` only after the enrollment check passes. |
| Client `kiln --version` | `kiln 0.4.6`, before `kiln login`. |
| Device flows | Both are waiting, parented to PID 1. They outlive this run until the pending rows expire. |

## Client version

The binary is `/home/ubuntu/.local/bin/kiln`, copied from the verified v0.4.6 linux x86_64 release archive. This command ran before `kiln login`.

```text
+ /home/ubuntu/.local/bin/kiln --version
kiln 0.4.6
version_exit:0
```

A later check with that directory on `PATH` printed the same line:

```text
kiln 0.4.6
```

## Rollback copies

`/opt/kiln/upgrades/0.4.4-8DDh4B` and `/opt/kiln/upgrades/0.4.5-RSNr4F` are intentional. The 0.4.4 directory is the rollback copy the v0.4.5 installer wrote. The 0.4.5 directory is the rollback copy this `install.sh --apply` wrote. Neither was deleted.

Keep `0.4.5-RSNr4F` until phase 2's off-server proof has passed and the operator accepts that v0.4.6 will not be rolled back. `0.4.4-8DDh4B` can be pruned once a return to 0.4.4 is no longer required. The v0.4.5 copy is the rollback target for this deploy, so the 0.4.4 copy is not needed for a rollback to 0.4.5. This phase did not prune either directory.

## SSH

The remote prefix was `ssh -i <key> -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile=<pinned known_hosts> -o BatchMode=yes -o ConnectTimeout=15 root@dark-forge.dev`. Each ssh invocation wrapped the remote script in `timeout`. The scripts below also wrap each command in `timeout`. The deploy key fingerprint is `SHA256:AGD6iqWdOAB1R7LeRTlbIMYyvx+FvkIDw8MTWEVtrd4`. The pinned host key is ED25519 `SHA256:/yIiijBqkbGh9lFju3N9ZhbJGIqv+NjC0R1CyWh9g9s`. The private key is not in this file. It is still present for phase 2.

## 1. Before checks

Remote script sent to `bash -s`. The outer ssh `timeout` was 180 seconds. ssh exit 0.

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
run versions 40 bash -c 'for b in /opt/kiln/bin/kiln /opt/kiln/bin/kiln-runtime /opt/kiln/bin/firecracker /opt/kiln/bin/jailer /usr/local/libexec/kiln-host /usr/local/libexec/kiln-api /usr/local/libexec/kiln-identity; do echo "--- $b ---"; if [ -e "$b" ]; then "$b" --version; echo exit:$?; else echo MISSING; fi; done'
run units 30 systemctl show kiln-host.service kiln-api.service kiln-cgroup.service kiln-door.service kiln-identity.service kiln-identity-proxy.service kiln-identity-proxy.socket -p Id,ActiveState,SubState,MainPID,NRestarts,ActiveEnterTimestamp,FragmentPath,UnitFileState
run unit-files 20 bash -c 'systemctl list-unit-files "kiln*" --no-pager; echo; systemctl list-units "kiln*" --all --no-pager'
run authorized-keys-sha 20 sha256sum /root/.ssh/authorized_keys
run identity-bins 20 bash -c 'sha256sum /usr/local/libexec/kiln-identity*'
run identity-units 20 bash -c 'sha256sum /etc/systemd/system/kiln-identity.service /etc/systemd/system/kiln-identity.service.d/10-provider-readiness.conf /etc/systemd/system/kiln-identity-proxy.service /etc/systemd/system/kiln-identity-proxy.socket 2>&1'
run identity-config-hashes 20 bash -c 'if [ -d /etc/kiln-identity ]; then find /etc/kiln-identity -type f -print0 | sort -z | xargs -0 sha256sum; else echo ABSENT /etc/kiln-identity; fi'
run caddy-files 20 bash -c 'systemctl cat kiln-identity-proxy.service kiln-identity-proxy.socket; echo "===== find caddy ====="; find /etc /usr/local /opt -name "Caddyfile*" -o -name "*caddy*" -type f 2>/dev/null | head -100'
run enrollment-files 20 bash -c 'for p in /opt/kiln/identity.json /etc/kiln/identity.json /etc/kiln/directory.token /etc/kiln/enrollment.pending.json /etc/systemd/system/kiln-api.service.d/identity.conf /etc/kiln/door.crt /etc/kiln/door.key /etc/kiln/door.env /etc/systemd/system/kiln-door.service; do if [ -e "$p" ]; then echo "EXISTS $p"; ls -la "$p"; else echo "ABSENT $p"; fi; done'
run upgrades 20 bash -c 'ls -la /opt/kiln /opt/kiln/bin /opt/kiln/upgrades 2>&1; echo; ls -la /opt/kiln/pending-upgrade.json 2>&1'
run listen 20 bash -c 'ss -lntup'
run firewall-which 20 bash -c 'echo "command -v nft: $(command -v nft || echo none)"; echo "command -v ufw: $(command -v ufw || echo none)"; echo "command -v iptables: $(command -v iptables || echo none)"; echo "command -v ip6tables: $(command -v ip6tables || echo none)"; echo "command -v firewall-cmd: $(command -v firewall-cmd || echo none)"; echo "===== ufw status ====="; if command -v ufw >/dev/null; then ufw status verbose; else echo ufw_absent; fi; echo "===== nft list ruleset ====="; if command -v nft >/dev/null; then nft list ruleset; echo nft_exit:$?; else echo nft_absent; fi; echo "===== iptables-save ====="; if command -v iptables-save >/dev/null; then iptables-save; else echo iptables_save_absent; fi; echo "===== ip6tables-save ====="; if command -v ip6tables-save >/dev/null; then ip6tables-save; else echo ip6tables_save_absent; fi'
run dpkg-before 60 bash -c 'dpkg -l > /root/dpkg-before-v046.txt; echo dpkg_write_exit:$?; sha256sum /root/dpkg-before-v046.txt; wc -l /root/dpkg-before-v046.txt'
run sqlite 15 bash -c 'command -v sqlite3; echo command_v_exit:$?; sqlite3 --version; dpkg -l sqlite3 libsqlite3-0'
run addrs 20 bash -c 'echo "===== ip -4 addr ====="; ip -4 addr; echo "===== ip route get ====="; ip -4 route get 1.1.1.1; echo "===== dns ====="; getent ahostsv4 cli.dark-forge.dev; echo getent_cli_exit:$?; getent ahostsv4 dark-forge.dev; echo getent_dark_exit:$?'
run api-argv 20 bash -c 'systemctl show kiln-api.service kiln-host.service -p ExecStart --no-pager; echo "===== unit listen ====="; awk "match(\$0, /--listen [^ ]+/) { print substr(\$0, RSTART, RLENGTH) }" /etc/systemd/system/kiln-api.service'
echo
echo "===== BEFORE CHECKS DONE ====="
```

Stdout:

### before.out

```text
===== date -u =====
Mon Oct 19 09:26:02 PM UTC 2026
===== timedatectl =====
               Local time: Mon 2026-10-19 23:26:02 CEST
           Universal time: Mon 2026-10-19 21:26:02 UTC
                 RTC time: Mon 2026-10-19 21:26:03
                Time zone: Europe/Berlin (CEST, +0200)
System clock synchronized: yes
              NTP service: active
          RTC in local TZ: no

===== BEGIN versions =====
+ bash -c for\ b\ in\ /opt/kiln/bin/kiln\ /opt/kiln/bin/kiln-runtime\ /opt/kiln/bin/firecracker\ /opt/kiln/bin/jailer\ /usr/local/libexec/kiln-host\ /usr/local/libexec/kiln-api\ /usr/local/libexec/kiln-identity\;\ do\ echo\ \"---\ \$b\ ---\"\;\ if\ \[\ -e\ \"\$b\"\ \]\;\ then\ \"\$b\"\ --version\;\ echo\ exit:\$\?\;\ else\ echo\ MISSING\;\ fi\;\ done 
--- /opt/kiln/bin/kiln ---
kiln 0.4.5
exit:0
--- /opt/kiln/bin/kiln-runtime ---
kiln-runtime 0.4.5
exit:0
--- /opt/kiln/bin/firecracker ---
Firecracker v1.17.0

2026-10-19T23:26:02.989170127 [anonymous-instance:main] Firecracker exiting successfully. exit_code=0
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
--- /usr/local/libexec/kiln-identity ---
kiln-identity 0.4.1
exit:0

===== END versions exit=0 =====

===== BEGIN units =====
+ systemctl show kiln-host.service kiln-api.service kiln-cgroup.service kiln-door.service kiln-identity.service kiln-identity-proxy.service kiln-identity-proxy.socket -p Id\,ActiveState\,SubState\,MainPID\,NRestarts\,ActiveEnterTimestamp\,FragmentPath\,UnitFileState 
MainPID=3739721
NRestarts=0
Id=kiln-host.service
ActiveState=active
SubState=running
FragmentPath=/etc/systemd/system/kiln-host.service
UnitFileState=enabled
ActiveEnterTimestamp=Mon 2026-10-19 19:52:11 CEST

MainPID=3739725
NRestarts=0
Id=kiln-api.service
ActiveState=active
SubState=running
FragmentPath=/etc/systemd/system/kiln-api.service
UnitFileState=enabled
ActiveEnterTimestamp=Mon 2026-10-19 19:52:11 CEST

MainPID=0
NRestarts=0
Id=kiln-cgroup.service
ActiveState=active
SubState=exited
FragmentPath=/etc/systemd/system/kiln-cgroup.service
UnitFileState=enabled
ActiveEnterTimestamp=Mon 2026-10-19 17:12:01 CEST

MainPID=0
NRestarts=0
Id=kiln-door.service
ActiveState=inactive
SubState=dead
FragmentPath=
UnitFileState=
ActiveEnterTimestamp=

MainPID=3522047
NRestarts=0
Id=kiln-identity.service
ActiveState=active
SubState=running
FragmentPath=/etc/systemd/system/kiln-identity.service
UnitFileState=enabled
ActiveEnterTimestamp=Mon 2026-10-19 12:42:17 CEST

MainPID=0
NRestarts=0
Id=kiln-identity-proxy.service
ActiveState=inactive
SubState=dead
FragmentPath=/etc/systemd/system/kiln-identity-proxy.service
UnitFileState=static
ActiveEnterTimestamp=Mon 2026-10-19 22:26:22 CEST

Id=kiln-identity-proxy.socket
ActiveState=active
SubState=listening
FragmentPath=/etc/systemd/system/kiln-identity-proxy.socket
UnitFileState=enabled
ActiveEnterTimestamp=Sat 2026-10-03 20:14:48 CEST

===== END units exit=0 =====

===== BEGIN unit-files =====
+ bash -c systemctl\ list-unit-files\ \"kiln\*\"\ --no-pager\;\ echo\;\ systemctl\ list-units\ \"kiln\*\"\ --all\ --no-pager 
UNIT FILE                   STATE   PRESET
kiln-api.service            enabled enabled
kiln-cgroup.service         enabled enabled
kiln-host.service           enabled enabled
kiln-identity-proxy.service static  -
kiln-identity.service       enabled enabled
kiln-identity-proxy.socket  enabled enabled

6 unit files listed.

  UNIT                        LOAD   ACTIVE   SUB       DESCRIPTION
  kiln-api.service            loaded active   running   kiln unprivileged HTTPS gateway
  kiln-cgroup.service         loaded active   exited    Prepare kiln cgroup v2 controllers
  kiln-host.service           loaded active   running   kiln privileged microVM host service
  kiln-identity-proxy.service loaded inactive dead      Private Caddy bridge to loopback Kiln identity
  kiln-identity.service       loaded active   running   Kiln identity and server discovery
  kiln-identity-proxy.socket  loaded active   listening Private Caddy bridge socket for Kiln identity

Legend: LOAD   → Reflects whether the unit definition was properly loaded.
        ACTIVE → The high-level unit activation state, i.e. generalization of SUB.
        SUB    → The low-level unit activation state, values depend on unit type.

6 loaded units listed.
To show all installed unit files use 'systemctl list-unit-files'.

===== END unit-files exit=0 =====

===== BEGIN authorized-keys-sha =====
+ sha256sum /root/.ssh/authorized_keys 
4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37  /root/.ssh/authorized_keys

===== END authorized-keys-sha exit=0 =====

===== BEGIN identity-bins =====
+ bash -c sha256sum\ /usr/local/libexec/kiln-identity\* 
54c89f144ef0407b0abef11a8ffe60c18acdacc82b2f9c90bdf647c1df1d76ea  /usr/local/libexec/kiln-identity
9e36ca9ae505da5b3704915114e4b39565099871e407ff915f4103d9729eda59  /usr/local/libexec/kiln-identity.rollback-v033

===== END identity-bins exit=0 =====

===== BEGIN identity-units =====
+ bash -c sha256sum\ /etc/systemd/system/kiln-identity.service\ /etc/systemd/system/kiln-identity.service.d/10-provider-readiness.conf\ /etc/systemd/system/kiln-identity-proxy.service\ /etc/systemd/system/kiln-identity-proxy.socket\ 2\>\&1 
31667738c2b275d7b326abcdfa0b6097c947b62005506693d90ab5df80122554  /etc/systemd/system/kiln-identity.service
08aca9fe20443428eed6d902c6d9c47999f8361ea7cb7bb9b22fbd84e739f9e2  /etc/systemd/system/kiln-identity.service.d/10-provider-readiness.conf
f9e0c98316c5f83d12de0abb3b885d6060f8b05a6618ba4f1360e0fbcbd37d6e  /etc/systemd/system/kiln-identity-proxy.service
d04bd26752276f842f93c7f4f6929cdccfacf81abbdc37b35f44519eca6b5141  /etc/systemd/system/kiln-identity-proxy.socket

===== END identity-units exit=0 =====

===== BEGIN identity-config-hashes =====
+ bash -c if\ \[\ -d\ /etc/kiln-identity\ \]\;\ then\ find\ /etc/kiln-identity\ -type\ f\ -print0\ \|\ sort\ -z\ \|\ xargs\ -0\ sha256sum\;\ else\ echo\ ABSENT\ /etc/kiln-identity\;\ fi 
7508a7c9782a09e3181a4bd6a70938ba033eb826bcb8d6eafd53a3610061e495  /etc/kiln-identity/github.secret
9f7da71cffb6cdf12653975b136703b05dc836742879a194ba089733482a3757  /etc/kiln-identity/google.secret
6e0f03257ca0ed6039f1bd3ba855049a0619beba8789163696836027f8c56abb  /etc/kiln-identity/service.env
b1b7a26a116cb539bd4a19c4f4a8f9554f6896f6e7303567a4c50ebeefd87eb8  /etc/kiln-identity/signing.pem

===== END identity-config-hashes exit=0 =====

===== BEGIN caddy-files =====
+ bash -c systemctl\ cat\ kiln-identity-proxy.service\ kiln-identity-proxy.socket\;\ echo\ \"=====\ find\ caddy\ =====\"\;\ find\ /etc\ /usr/local\ /opt\ -name\ \"Caddyfile\*\"\ -o\ -name\ \"\*caddy\*\"\ -type\ f\ 2\>/dev/null\ \|\ head\ -100 
# /etc/systemd/system/kiln-identity-proxy.service
[Unit]
Description=Private Caddy bridge to loopback Kiln identity
Requires=kiln-identity.service
After=kiln-identity.service

[Service]
ExecStart=/usr/lib/systemd/systemd-socket-proxyd --connections-max=256 --exit-idle-time=5min 127.0.0.1:8091
DynamicUser=yes
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
PrivateDevices=true
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectControlGroups=true
RestrictSUIDSGID=true
RestrictAddressFamilies=AF_UNIX AF_INET
CapabilityBoundingSet=
IPAddressDeny=any
IPAddressAllow=localhost

# /etc/systemd/system/kiln-identity-proxy.socket
[Unit]
Description=Private Caddy bridge socket for Kiln identity

[Socket]
ListenStream=172.18.0.1:8092
# Bind before Docker recreates the bridge without ordering sockets after Docker.
FreeBind=yes
NoDelay=true
IPAddressDeny=any
IPAddressAllow=172.18.0.3/32

[Install]
WantedBy=sockets.target
===== find caddy =====
/opt/paseo/Caddyfile

===== END caddy-files exit=0 =====

===== BEGIN enrollment-files =====
+ bash -c for\ p\ in\ /opt/kiln/identity.json\ /etc/kiln/identity.json\ /etc/kiln/directory.token\ /etc/kiln/enrollment.pending.json\ /etc/systemd/system/kiln-api.service.d/identity.conf\ /etc/kiln/door.crt\ /etc/kiln/door.key\ /etc/kiln/door.env\ /etc/systemd/system/kiln-door.service\;\ do\ if\ \[\ -e\ \"\$p\"\ \]\;\ then\ echo\ \"EXISTS\ \$p\"\;\ ls\ -la\ \"\$p\"\;\ else\ echo\ \"ABSENT\ \$p\"\;\ fi\;\ done 
ABSENT /opt/kiln/identity.json
ABSENT /etc/kiln/identity.json
ABSENT /etc/kiln/directory.token
ABSENT /etc/kiln/enrollment.pending.json
ABSENT /etc/systemd/system/kiln-api.service.d/identity.conf
ABSENT /etc/kiln/door.crt
ABSENT /etc/kiln/door.key
ABSENT /etc/kiln/door.env
ABSENT /etc/systemd/system/kiln-door.service

===== END enrollment-files exit=0 =====

===== BEGIN upgrades =====
+ bash -c ls\ -la\ /opt/kiln\ /opt/kiln/bin\ /opt/kiln/upgrades\ 2\>\&1\;\ echo\;\ ls\ -la\ /opt/kiln/pending-upgrade.json\ 2\>\&1 
/opt/kiln:
total 16
drwxr-xr-x 4 root root 4096 Oct 19 19:52 .
drwxr-xr-x 5 root root 4096 Oct 19 17:11 ..
drwxr-xr-x 2 root root 4096 Oct 19 19:52 bin
drwx------ 3 root root 4096 Oct 19 19:52 upgrades

/opt/kiln/bin:
total 22072
drwxr-xr-x 2 root root    4096 Oct 19 19:52 .
drwxr-xr-x 4 root root    4096 Oct 19 19:52 ..
-rwxr-xr-x 1 root root 3676680 Oct 19 17:11 firecracker
-rwxr-xr-x 1 root root 2243000 Oct 19 17:11 jailer
-rwxr-xr-x 1 root root 8166976 Oct 19 19:52 kiln
-rwxr-xr-x 1 root root    6077 Oct 19 19:52 kiln-network
-rwxr-xr-x 1 root root 8492736 Oct 19 19:52 kiln-runtime

/opt/kiln/upgrades:
total 12
drwx------ 3 root root 4096 Oct 19 19:52 .
drwxr-xr-x 4 root root 4096 Oct 19 19:52 ..
drwx------ 2 root root 4096 Oct 19 19:52 0.4.4-8DDh4B

ls: cannot access '/opt/kiln/pending-upgrade.json': No such file or directory

===== END upgrades exit=2 =====

===== BEGIN listen =====
+ bash -c ss\ -lntup 
Netid State  Recv-Q Send-Q Local Address:Port Peer Address:PortProcess                                     
tcp   LISTEN 0      4096      172.18.0.1:8092      0.0.0.0:*    users:(("systemd",pid=1,fd=159))           
tcp   LISTEN 0      128        127.0.0.1:8091      0.0.0.0:*    users:(("kiln-identity",pid=3522047,fd=12))
tcp   LISTEN 0      4096       127.0.0.1:6767      0.0.0.0:*    users:(("docker-proxy",pid=3699036,fd=8))  
tcp   LISTEN 0      128          0.0.0.0:22        0.0.0.0:*    users:(("sshd",pid=1022,fd=6))             
tcp   LISTEN 0      4096         0.0.0.0:80        0.0.0.0:*    users:(("docker-proxy",pid=1302820,fd=8))  
tcp   LISTEN 0      4096         0.0.0.0:443       0.0.0.0:*    users:(("docker-proxy",pid=1302843,fd=8))  
tcp   LISTEN 0      128        127.0.0.1:8443      0.0.0.0:*    users:(("kiln-api",pid=3739725,fd=9))      
tcp   LISTEN 0      128             [::]:22           [::]:*    users:(("sshd",pid=1022,fd=7))             
tcp   LISTEN 0      4096            [::]:80           [::]:*    users:(("docker-proxy",pid=1302827,fd=8))  
tcp   LISTEN 0      4096            [::]:443          [::]:*    users:(("docker-proxy",pid=1302851,fd=8))  

===== END listen exit=0 =====

===== BEGIN firewall-which =====
+ bash -c echo\ \"command\ -v\ nft:\ \$\(command\ -v\ nft\ \|\|\ echo\ none\)\"\;\ echo\ \"command\ -v\ ufw:\ \$\(command\ -v\ ufw\ \|\|\ echo\ none\)\"\;\ echo\ \"command\ -v\ iptables:\ \$\(command\ -v\ iptables\ \|\|\ echo\ none\)\"\;\ echo\ \"command\ -v\ ip6tables:\ \$\(command\ -v\ ip6tables\ \|\|\ echo\ none\)\"\;\ echo\ \"command\ -v\ firewall-cmd:\ \$\(command\ -v\ firewall-cmd\ \|\|\ echo\ none\)\"\;\ echo\ \"=====\ ufw\ status\ =====\"\;\ if\ command\ -v\ ufw\ \>/dev/null\;\ then\ ufw\ status\ verbose\;\ else\ echo\ ufw_absent\;\ fi\;\ echo\ \"=====\ nft\ list\ ruleset\ =====\"\;\ if\ command\ -v\ nft\ \>/dev/null\;\ then\ nft\ list\ ruleset\;\ echo\ nft_exit:\$\?\;\ else\ echo\ nft_absent\;\ fi\;\ echo\ \"=====\ iptables-save\ =====\"\;\ if\ command\ -v\ iptables-save\ \>/dev/null\;\ then\ iptables-save\;\ else\ echo\ iptables_save_absent\;\ fi\;\ echo\ \"=====\ ip6tables-save\ =====\"\;\ if\ command\ -v\ ip6tables-save\ \>/dev/null\;\ then\ ip6tables-save\;\ else\ echo\ ip6tables_save_absent\;\ fi 
command -v nft: /usr/sbin/nft
command -v ufw: none
command -v iptables: /usr/sbin/iptables
command -v ip6tables: /usr/sbin/ip6tables
command -v firewall-cmd: none
===== ufw status =====
ufw_absent
===== nft list ruleset =====
table ip nat {
	chain DOCKER {
		iifname != "br-164dac86db63" tcp dport 80 counter packets 1595908 bytes 93942879 dnat to 172.18.0.3:80
		iifname != "br-164dac86db63" tcp dport 443 counter packets 2361940 bytes 139289443 dnat to 172.18.0.3:443
		ip daddr 127.0.0.1 iifname != "br-164dac86db63" tcp dport 6767 counter packets 0 bytes 0 dnat to 172.18.0.2:6767
	}

	chain PREROUTING {
		type nat hook prerouting priority dstnat; policy accept;
		fib daddr type local counter packets 19260888 bytes 1236637706 jump DOCKER
	}

	chain OUTPUT {
		type nat hook output priority dstnat; policy accept;
		ip daddr != 127.0.0.0/8 fib daddr type local counter packets 2 bytes 120 jump DOCKER
	}

	chain POSTROUTING {
		type nat hook postrouting priority srcnat; policy accept;
		ip saddr 172.18.0.0/16 oifname != "br-164dac86db63" counter packets 69144 bytes 4148620 masquerade
		ip saddr 172.17.0.0/16 oifname != "docker0" counter packets 458 bytes 27564 masquerade
	}
}
table ip filter {
	chain DOCKER {
		ip daddr 172.18.0.2 iifname != "br-164dac86db63" oifname "br-164dac86db63" tcp dport 6767 counter packets 0 bytes 0 accept
		ip daddr 172.18.0.3 iifname != "br-164dac86db63" oifname "br-164dac86db63" tcp dport 443 counter packets 2361940 bytes 139289443 accept
		ip daddr 172.18.0.3 iifname != "br-164dac86db63" oifname "br-164dac86db63" tcp dport 80 counter packets 1595908 bytes 93942879 accept
		iifname != "docker0" oifname "docker0" counter packets 0 bytes 0 drop
		iifname != "br-164dac86db63" oifname "br-164dac86db63" counter packets 0 bytes 0 drop
	}

	chain DOCKER-FORWARD {
		counter packets 38248558 bytes 13365486449 jump DOCKER-CT
		counter packets 28983204 bytes 4665362921 jump DOCKER-INTERNAL
		counter packets 28983204 bytes 4665362921 jump DOCKER-BRIDGE
		iifname "docker0" counter packets 447013 bytes 31000591 accept
		iifname "br-164dac86db63" counter packets 23157497 bytes 4322387347 accept
	}

	chain DOCKER-BRIDGE {
		oifname "docker0" counter packets 0 bytes 0 jump DOCKER
		oifname "br-164dac86db63" counter packets 4434453 bytes 261341332 jump DOCKER
	}

	chain DOCKER-CT {
		oifname "docker0" ct state related,established counter packets 715346 bytes 3733100098 accept
		oifname "br-164dac86db63" ct state related,established counter packets 8548872 bytes 4953909852 accept
	}

	chain DOCKER-INTERNAL {
	}

	chain FORWARD {
		type filter hook forward priority filter; policy drop;
		counter packets 38248558 bytes 13365486449 jump DOCKER-USER
		counter packets 38248558 bytes 13365486449 jump DOCKER-FORWARD
	}

	chain DOCKER-USER {
	}
}
table ip6 nat {
	chain DOCKER {
	}

	chain PREROUTING {
		type nat hook prerouting priority dstnat; policy accept;
		fib daddr type local counter packets 0 bytes 0 jump DOCKER
	}

	chain OUTPUT {
		type nat hook output priority dstnat; policy accept;
		ip6 daddr != ::1 fib daddr type local counter packets 0 bytes 0 jump DOCKER
	}
}
table ip6 filter {
	chain DOCKER {
	}

	chain DOCKER-FORWARD {
		counter packets 0 bytes 0 jump DOCKER-CT
		counter packets 0 bytes 0 jump DOCKER-INTERNAL
		counter packets 0 bytes 0 jump DOCKER-BRIDGE
	}

	chain DOCKER-BRIDGE {
	}

	chain DOCKER-CT {
	}

	chain DOCKER-INTERNAL {
	}

	chain FORWARD {
		type filter hook forward priority filter; policy accept;
		counter packets 0 bytes 0 jump DOCKER-USER
		counter packets 0 bytes 0 jump DOCKER-FORWARD
	}

	chain DOCKER-USER {
	}
}
table ip raw {
	chain PREROUTING {
		type filter hook prerouting priority raw; policy accept;
		ip daddr 172.18.0.4 iifname != "br-164dac86db63" counter packets 0 bytes 0 drop
		ip daddr 172.18.0.3 iifname != "br-164dac86db63" counter packets 0 bytes 0 drop
		ip daddr 172.18.0.5 iifname != "br-164dac86db63" counter packets 0 bytes 0 drop
		ip daddr 172.18.0.2 iifname != "br-164dac86db63" counter packets 0 bytes 0 drop
		ip daddr 127.0.0.1 iifname != "lo" tcp dport 6767 counter packets 0 bytes 0 drop
	}
}
nft_exit:0
===== iptables-save =====
# Generated by iptables-save v1.8.11 (nf_tables) on Mon Oct 19 23:26:03 2026
*raw
:PREROUTING ACCEPT [60870763:16282804174]
:OUTPUT ACCEPT [0:0]
-A PREROUTING -d 172.18.0.4/32 ! -i br-164dac86db63 -j DROP
-A PREROUTING -d 172.18.0.3/32 ! -i br-164dac86db63 -j DROP
-A PREROUTING -d 172.18.0.5/32 ! -i br-164dac86db63 -j DROP
-A PREROUTING -d 172.18.0.2/32 ! -i br-164dac86db63 -j DROP
-A PREROUTING -d 127.0.0.1/32 ! -i lo -p tcp -m tcp --dport 6767 -j DROP
COMMIT
# Completed on Mon Oct 19 23:26:03 2026
# Generated by iptables-save v1.8.11 (nf_tables) on Mon Oct 19 23:26:03 2026
*filter
:INPUT ACCEPT [0:0]
:FORWARD DROP [943729:50579299]
:OUTPUT ACCEPT [0:0]
:DOCKER - [0:0]
:DOCKER-BRIDGE - [0:0]
:DOCKER-CT - [0:0]
:DOCKER-FORWARD - [0:0]
:DOCKER-INTERNAL - [0:0]
:DOCKER-USER - [0:0]
-A FORWARD -j DOCKER-USER
-A FORWARD -j DOCKER-FORWARD
-A DOCKER -d 172.18.0.2/32 ! -i br-164dac86db63 -o br-164dac86db63 -p tcp -m tcp --dport 6767 -j ACCEPT
-A DOCKER -d 172.18.0.3/32 ! -i br-164dac86db63 -o br-164dac86db63 -p tcp -m tcp --dport 443 -j ACCEPT
-A DOCKER -d 172.18.0.3/32 ! -i br-164dac86db63 -o br-164dac86db63 -p tcp -m tcp --dport 80 -j ACCEPT
-A DOCKER ! -i docker0 -o docker0 -j DROP
-A DOCKER ! -i br-164dac86db63 -o br-164dac86db63 -j DROP
-A DOCKER-BRIDGE -o docker0 -j DOCKER
-A DOCKER-BRIDGE -o br-164dac86db63 -j DOCKER
-A DOCKER-CT -o docker0 -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT
-A DOCKER-CT -o br-164dac86db63 -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT
-A DOCKER-FORWARD -j DOCKER-CT
-A DOCKER-FORWARD -j DOCKER-INTERNAL
-A DOCKER-FORWARD -j DOCKER-BRIDGE
-A DOCKER-FORWARD -i docker0 -j ACCEPT
-A DOCKER-FORWARD -i br-164dac86db63 -j ACCEPT
COMMIT
# Completed on Mon Oct 19 23:26:03 2026
# Generated by iptables-save v1.8.11 (nf_tables) on Mon Oct 19 23:26:03 2026
*nat
:PREROUTING ACCEPT [15836156:1029877161]
:INPUT ACCEPT [0:0]
:OUTPUT ACCEPT [97520:6236457]
:POSTROUTING ACCEPT [4531972:267577729]
:DOCKER - [0:0]
-A PREROUTING -m addrtype --dst-type LOCAL -j DOCKER
-A OUTPUT ! -d 127.0.0.0/8 -m addrtype --dst-type LOCAL -j DOCKER
-A POSTROUTING -s 172.18.0.0/16 ! -o br-164dac86db63 -j MASQUERADE
-A POSTROUTING -s 172.17.0.0/16 ! -o docker0 -j MASQUERADE
-A DOCKER ! -i br-164dac86db63 -p tcp -m tcp --dport 80 -j DNAT --to-destination 172.18.0.3:80
-A DOCKER ! -i br-164dac86db63 -p tcp -m tcp --dport 443 -j DNAT --to-destination 172.18.0.3:443
-A DOCKER -d 127.0.0.1/32 ! -i br-164dac86db63 -p tcp -m tcp --dport 6767 -j DNAT --to-destination 172.18.0.2:6767
COMMIT
# Completed on Mon Oct 19 23:26:03 2026
===== ip6tables-save =====
# Generated by ip6tables-save v1.8.11 (nf_tables) on Mon Oct 19 23:26:03 2026
*filter
:INPUT ACCEPT [0:0]
:FORWARD ACCEPT [0:0]
:OUTPUT ACCEPT [0:0]
:DOCKER - [0:0]
:DOCKER-BRIDGE - [0:0]
:DOCKER-CT - [0:0]
:DOCKER-FORWARD - [0:0]
:DOCKER-INTERNAL - [0:0]
:DOCKER-USER - [0:0]
-A FORWARD -j DOCKER-USER
-A FORWARD -j DOCKER-FORWARD
-A DOCKER-FORWARD -j DOCKER-CT
-A DOCKER-FORWARD -j DOCKER-INTERNAL
-A DOCKER-FORWARD -j DOCKER-BRIDGE
COMMIT
# Completed on Mon Oct 19 23:26:03 2026
# Generated by ip6tables-save v1.8.11 (nf_tables) on Mon Oct 19 23:26:03 2026
*nat
:PREROUTING ACCEPT [0:0]
:INPUT ACCEPT [0:0]
:OUTPUT ACCEPT [0:0]
:POSTROUTING ACCEPT [0:0]
:DOCKER - [0:0]
-A PREROUTING -m addrtype --dst-type LOCAL -j DOCKER
-A OUTPUT ! -d ::1/128 -m addrtype --dst-type LOCAL -j DOCKER
COMMIT
# Completed on Mon Oct 19 23:26:03 2026

===== END firewall-which exit=0 =====

===== BEGIN dpkg-before =====
+ bash -c dpkg\ -l\ \>\ /root/dpkg-before-v046.txt\;\ echo\ dpkg_write_exit:\$\?\;\ sha256sum\ /root/dpkg-before-v046.txt\;\ wc\ -l\ /root/dpkg-before-v046.txt 
dpkg_write_exit:0
d388be45b10aa31fa9ab14681dd30f24fa77e122363f6acb724f6f507703807d  /root/dpkg-before-v046.txt
370 /root/dpkg-before-v046.txt

===== END dpkg-before exit=0 =====

===== BEGIN sqlite =====
+ bash -c command\ -v\ sqlite3\;\ echo\ command_v_exit:\$\?\;\ sqlite3\ --version\;\ dpkg\ -l\ sqlite3\ libsqlite3-0 
/usr/bin/sqlite3
command_v_exit:0
3.46.1 2024-08-13 09:16:08 c9c2ab54ba1f5f46360f1b4f35d849cd3f080e6fc2b6c60e91b16c63f69aalt1 (64-bit)
Desired=Unknown/Install/Remove/Purge/Hold
| Status=Not/Inst/Conf-files/Unpacked/halF-conf/Half-inst/trig-aWait/Trig-pend
|/ Err?=(none)/Reinst-required (Status,Err: uppercase=bad)
||/ Name               Version          Architecture Description
+++-==================-================-============-===================================
ii  libsqlite3-0:amd64 3.46.1-7+deb13u2 amd64        SQLite 3 shared library
ii  sqlite3            3.46.1-7+deb13u2 amd64        Command line interface for SQLite 3

===== END sqlite exit=0 =====

===== BEGIN addrs =====
+ bash -c echo\ \"=====\ ip\ -4\ addr\ =====\"\;\ ip\ -4\ addr\;\ echo\ \"=====\ ip\ route\ get\ =====\"\;\ ip\ -4\ route\ get\ 1.1.1.1\;\ echo\ \"=====\ dns\ =====\"\;\ getent\ ahostsv4\ cli.dark-forge.dev\;\ echo\ getent_cli_exit:\$\?\;\ getent\ ahostsv4\ dark-forge.dev\;\ echo\ getent_dark_exit:\$\? 
===== ip -4 addr =====
1: lo: <LOOPBACK,UP,LOWER_UP> mtu 65536 qdisc noqueue state UNKNOWN group default qlen 1000
    inet 127.0.0.1/8 scope host lo
       valid_lft forever preferred_lft forever
2: enp41s0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc mq state UP group default qlen 1000
    altname enx9c6b0006b226
    inet 78.46.98.73/27 brd 78.46.98.95 scope global enp41s0
       valid_lft forever preferred_lft forever
3: docker0: <NO-CARRIER,BROADCAST,MULTICAST,UP> mtu 1500 qdisc noqueue state DOWN group default 
    inet 172.17.0.1/16 brd 172.17.255.255 scope global docker0
       valid_lft forever preferred_lft forever
6: br-164dac86db63: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc noqueue state UP group default 
    inet 172.18.0.1/16 brd 172.18.255.255 scope global br-164dac86db63
       valid_lft forever preferred_lft forever
===== ip route get =====
1.1.1.1 via 78.46.98.65 dev enp41s0 src 78.46.98.73 uid 0 
    cache 
===== dns =====
78.46.98.73     STREAM cli.dark-forge.dev
78.46.98.73     DGRAM  
78.46.98.73     RAW    
getent_cli_exit:0
78.46.98.73     STREAM dark-forge.dev
78.46.98.73     DGRAM  
78.46.98.73     RAW    
getent_dark_exit:0

===== END addrs exit=0 =====

===== BEGIN api-argv =====
+ bash -c systemctl\ show\ kiln-api.service\ kiln-host.service\ -p\ ExecStart\ --no-pager\;\ echo\ \"=====\ unit\ listen\ =====\"\;\ awk\ \"match\(\\\$0\,\ /--listen\ \[\^\ \]+/\)\ \{\ print\ substr\(\\\$0\,\ RSTART\,\ RLENGTH\)\ \}\"\ /etc/systemd/system/kiln-api.service 
ExecStart={ path=/usr/local/libexec/kiln-api ; argv[]=/usr/local/libexec/kiln-api --listen 127.0.0.1:8443 --host-socket /run/kiln/host.sock --token-file /etc/kiln/admin.token --tls-cert /etc/kiln/tls.crt --tls-key /etc/kiln/tls.key ; ignore_errors=no ; start_time=[Mon 2026-10-19 19:52:11 CEST] ; stop_time=[n/a] ; pid=3739725 ; code=(null) ; status=0/0 }

ExecStart={ path=/usr/local/libexec/kiln-host ; argv[]=/usr/local/libexec/kiln-host --config /etc/kiln/host.json ; ignore_errors=no ; start_time=[Mon 2026-10-19 19:52:11 CEST] ; stop_time=[n/a] ; pid=3739721 ; code=(null) ; status=0/0 }
===== unit listen =====
--listen with
--listen 127.0.0.1:8443

===== END api-argv exit=0 =====

===== BEFORE CHECKS DONE =====
```

Stderr:

### before.err

```text
# Warning: table ip nat is managed by iptables-nft, do not touch!
# Warning: table ip filter is managed by iptables-nft, do not touch!
# Warning: table ip6 nat is managed by iptables-nft, do not touch!
```

The extra read-only snapshot saved the nftables and iptables rules, hashed `/opt/paseo/Caddyfile`, and listed the runtime directory. Outer ssh `timeout` was 40 seconds. ssh exit 0.

### extra-before.out

```text
===== date -u =====
Mon Oct 19 09:26:57 PM UTC 2026
+ nft list ruleset > /root/nft-before-v046.txt
nft_save_exit:0
0756e3b7baea87c99856d640735a7b18baa9f2b6013d5a819710247e7e2fdb4e  /root/nft-before-v046.txt
114 /root/nft-before-v046.txt
+ iptables-save > /root/iptables-before-v046.txt
iptables_save_exit:0
c906eab8878c00b93901789bffe9ff2744c803fdfde77bb47ddae8859c50dcc2  /root/iptables-before-v046.txt
+ ip6tables-save > /root/ip6tables-before-v046.txt
ip6tables_save_exit:0
7ed05539a143a083c99fd0de1c1c14a157f97a8b25be54e8ecdc96c4cf668239  /root/ip6tables-before-v046.txt
+ caddyfile hash
acd7c4ecf177cc065547e3e82e5ffb04202c1a9c101f56a195c3e162a8761eb9  /opt/paseo/Caddyfile
33 /opt/paseo/Caddyfile
+ grep 8443 or cli.dark-forge
no 8443 or cli.dark-forge in Caddyfile
+ runtime dir
/var/lib/kiln:
total 20
drwx------  5 root kiln-api 4096 Oct 19 17:12 .
drwxr-xr-x 26 root root     4096 Oct 19 17:11 ..
drwx------  2 root kiln-api 4096 Oct 19 17:12 image
drwx------  2 root kiln-api 4096 Oct 19 17:12 journal
drwx------  4 root kiln-api 4096 Oct 19 17:12 runtime

/var/lib/kiln/runtime:
total 20
drwx------ 4 root kiln-api 4096 Oct 19 17:12 .
drwx------ 5 root kiln-api 4096 Oct 19 17:12 ..
drwx------ 2 root kiln-api 4096 Oct 19 19:54 boxes
-rw------- 1 root kiln-api  132 Oct 19 17:12 isolation.json
-rw------- 1 root kiln-api    0 Oct 19 17:12 .lock
-rw------- 1 root kiln-api    0 Oct 19 17:12 .service.lock
drwx------ 3 root kiln-api 4096 Oct 19 17:12 snapshots
EXTRA_BEFORE_DONE
```

Stderr:

### extra-before.err

```text
# Warning: table ip nat is managed by iptables-nft, do not touch!
# Warning: table ip filter is managed by iptables-nft, do not touch!
# Warning: table ip6 nat is managed by iptables-nft, do not touch!
```

From the operator VM, before any firewall edit, `curl` to `https://cli.dark-forge.dev:8443/v1/templates` exited 7. Stderr:

```text
* Host cli.dark-forge.dev:8443 was resolved.
* IPv6: (none)
* IPv4: 78.46.98.73
*   Trying 78.46.98.73:8443...
* connect to 78.46.98.73 port 8443 from 172.30.0.2 port 39572 failed: Connection refused
* Failed to connect to cli.dark-forge.dev port 8443 after 136 ms: Couldn't connect to server
* Closing connection
```

`cli.dark-forge.dev` resolved to `78.46.98.73`. The host answered with connection refused. The public address was not filtered.

## 2. Download, checksum, and install.sh --apply

Remote script sent to `bash -s`. The outer ssh `timeout` was 980 seconds. ssh exit 0. No `apt-get` command ran. `sqlite3` was already installed.

### install.sh

```text
#!/bin/bash
set -u
echo "===== date -u ====="
date -u
echo "+ ls boxes"
timeout 15 bash -c 'ls -la /var/lib/kiln/runtime/boxes; echo ls_exit:$?'
work=$(mktemp -d /var/tmp/kiln-v046.XXXXXX)
chmod 700 "$work"
echo "work=$work"
cd "$work"
name=kiln-server-v0.4.6-x86_64-unknown-linux-gnu.tar.gz
base=https://github.com/Jabbslad/kiln/releases/download/v0.4.6
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
expect=d3f4e26f014bb295fb5ebb4286b2d6d3a0ac6947bc8ff606f81ceff6d105ea40
echo "expect=$expect"
echo "actual=$actual"
if [ "$actual" != "$expect" ]; then
  echo "CHECKSUM_MISMATCH"
  echo INSTALL_SCRIPT_DONE
  exit 0
fi
echo "+ timeout 30 tar -tzf $name"
timeout 30 tar -tzf "$name"
echo tar_list_exit:$?
extract=$(mktemp -d "$work/extract.XXXXXX")
chmod 700 "$extract"
echo "extract=$extract"
echo "+ timeout 120 tar -xzf $name -C $extract"
timeout 120 tar -xzf "$name" -C "$extract"
echo tar_exit:$?
echo "===== release.json ====="
cat "$extract/release.json"
echo
echo "+ test -f enroll and door unit"
ls -la "$extract/deploy/enroll-dark-forge.sh" "$extract/deploy/kiln-door.service" "$extract/install.sh"
echo "+ bash $extract/install.sh --apply"
timeout 600 bash "$extract/install.sh" --apply
echo install_exit:$?
echo INSTALL_SCRIPT_DONE
```

Stdout:

### install.out

```text
===== date -u =====
Mon Oct 19 09:27:08 PM UTC 2026
+ ls boxes
total 8
drwx------ 2 root kiln-api 4096 Oct 19 19:54 .
drwx------ 4 root kiln-api 4096 Oct 19 17:12 ..
ls_exit:0
work=/var/tmp/kiln-v046.CHz3Mi
+ timeout 60 curl -fsSL -o kiln-server-v0.4.6-x86_64-unknown-linux-gnu.tar.gz.sha256 https://github.com/Jabbslad/kiln/releases/download/v0.4.6/kiln-server-v0.4.6-x86_64-unknown-linux-gnu.tar.gz.sha256
curl_sha_exit:0
===== checksum file =====
d3f4e26f014bb295fb5ebb4286b2d6d3a0ac6947bc8ff606f81ceff6d105ea40  kiln-server-v0.4.6-x86_64-unknown-linux-gnu.tar.gz

+ sha256sum kiln-server-v0.4.6-x86_64-unknown-linux-gnu.tar.gz.sha256
63156f58adfbcbc40f3eab41d29e3af10286a9ca10c394261d43c4ae9ab96d7a  kiln-server-v0.4.6-x86_64-unknown-linux-gnu.tar.gz.sha256
+ timeout 280 curl -fL --retry 3 --retry-delay 2 -o kiln-server-v0.4.6-x86_64-unknown-linux-gnu.tar.gz https://github.com/Jabbslad/kiln/releases/download/v0.4.6/kiln-server-v0.4.6-x86_64-unknown-linux-gnu.tar.gz
curl_tar_exit:0
+ sha256sum kiln-server-v0.4.6-x86_64-unknown-linux-gnu.tar.gz
d3f4e26f014bb295fb5ebb4286b2d6d3a0ac6947bc8ff606f81ceff6d105ea40  kiln-server-v0.4.6-x86_64-unknown-linux-gnu.tar.gz
+ sha256sum -c kiln-server-v0.4.6-x86_64-unknown-linux-gnu.tar.gz.sha256
kiln-server-v0.4.6-x86_64-unknown-linux-gnu.tar.gz: OK
sha_check_exit:0
expect=d3f4e26f014bb295fb5ebb4286b2d6d3a0ac6947bc8ff606f81ceff6d105ea40
actual=d3f4e26f014bb295fb5ebb4286b2d6d3a0ac6947bc8ff606f81ceff6d105ea40
+ timeout 30 tar -tzf kiln-server-v0.4.6-x86_64-unknown-linux-gnu.tar.gz
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
docs/runbooks/2026-10-19-public-door.md
deploy/kiln-host.service
deploy/kiln-api.service
deploy/kiln-door.service
deploy/enroll-dark-forge.sh
deploy/host.example.json
image/image.json
image/inputs.json
image/vmlinux
image/rootfs.ext4
release.json
tar_list_exit:0
extract=/var/tmp/kiln-v046.CHz3Mi/extract.MRBb3h
+ timeout 120 tar -xzf kiln-server-v0.4.6-x86_64-unknown-linux-gnu.tar.gz -C /var/tmp/kiln-v046.CHz3Mi/extract.MRBb3h
tar_exit:0
===== release.json =====
{"version": "v0.4.6", "target": "x86_64-unknown-linux-gnu", "min_upgrade_from": "0.3.0"}
+ test -f enroll and door unit
-rw-r--r-- 1 root root 13821 Jan  1  1970 /var/tmp/kiln-v046.CHz3Mi/extract.MRBb3h/deploy/enroll-dark-forge.sh
-rw-r--r-- 1 root root   844 Jan  1  1970 /var/tmp/kiln-v046.CHz3Mi/extract.MRBb3h/deploy/kiln-door.service
-rwxr-xr-x 1 root root 39609 Jan  1  1970 /var/tmp/kiln-v046.CHz3Mi/extract.MRBb3h/install.sh
+ bash /var/tmp/kiln-v046.CHz3Mi/extract.MRBb3h/install.sh --apply
Kiln 0.4.5 -> 0.4.6: preserve configuration and VM state; briefly disconnect API/SSH sessions.
Updated to Kiln 0.4.6. Ready: https://127.0.0.1:8443
Previous binaries: /opt/kiln/upgrades/0.4.5-RSNr4F
One-time browser enrollment (does not restart guest VMs):
  sudo /usr/local/libexec/kiln-api enroll --url https://127.0.0.1:8443
Approve using GitHub or Google, then run kiln login on your laptop using the same provider.
Installation never enrolls a server or opens a browser automatically.
That URL is only on this machine. For dark-forge, follow docs/runbooks/2026-10-19-public-door.md and run deploy/enroll-dark-forge.sh.
install_exit:0
INSTALL_SCRIPT_DONE
```

Stderr is curl's progress meter:

### install.err

```text
  % Total    % Received % Xferd  Average Speed   Time    Time     Time  Current
                                 Dload  Upload   Total   Spent    Left  Speed

  0     0    0     0    0     0      0      0 --:--:-- --:--:-- --:--:--     0
  0     0    0     0    0     0      0      0 --:--:-- --:--:-- --:--:--     0

  1  189M    1 3805k    0     0  8462k      0  0:00:22 --:--:--  0:00:22 8462k
 23  189M   23 44.6M    0     0  30.9M      0  0:00:06  0:00:01  0:00:05 41.2M
 42  189M   42 80.0M    0     0  32.4M      0  0:00:05  0:00:02  0:00:03 37.8M
 63  189M   63  120M    0     0  33.4M      0  0:00:05  0:00:03  0:00:02 37.0M
 73  189M   73  139M    0     0  31.3M      0  0:00:06  0:00:04  0:00:02 33.9M
 80  189M   80  152M    0     0  28.0M      0  0:00:06  0:00:05  0:00:01 29.8M
100  189M  100  189M    0     0  29.5M      0  0:00:06  0:00:06 --:--:-- 29.1M
```

`install_exit` is 0. `release.json` in the archive is `{"version": "v0.4.6", "target": "x86_64-unknown-linux-gnu", "min_upgrade_from": "0.3.0"}`. The checksum file digest is `63156f58adfbcbc40f3eab41d29e3af10286a9ca10c394261d43c4ae9ab96d7a`. The archive contains `deploy/enroll-dark-forge.sh` and `deploy/kiln-door.service`. Their sha256 values match tag v0.4.6: `ca5cc44b47ac58471ca09978cf1dbcec9e904b4572cd86729b65839b56f36d5d` and `ac923d3219ad16fcd6fbbbd3fe9966c6fef9c88767095ad5606c770e2d92aa85`.

## 3. Doctor and versions

`PATH` is the installer's `DOCTOR_PATH`, `/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin`. Outer ssh `timeout` was 90 seconds. ssh exit 0.

### doctor.out

```text
===== date -u =====
Mon Oct 19 09:27:43 PM UTC 2026
+ PATH=/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin /opt/kiln/bin/kiln-runtime doctor
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
+ versions
--- /opt/kiln/bin/kiln ---
kiln 0.4.6
exit:0
--- /opt/kiln/bin/kiln-runtime ---
kiln-runtime 0.4.6
exit:0
--- /opt/kiln/bin/firecracker ---
Firecracker v1.17.0

2026-10-19T23:27:43.106354205 [anonymous-instance:main] Firecracker exiting successfully. exit_code=0
exit:0
--- /opt/kiln/bin/jailer ---
Jailer v1.17.0

exit:0
--- /usr/local/libexec/kiln-host ---
kiln-server 0.4.6
exit:0
--- /usr/local/libexec/kiln-api ---
kiln-server 0.4.6
exit:0
+ door help
Serve the public door. Identity JWTs are checked before loopback kiln-api is dialed

Usage: kiln-api door [OPTIONS] --listen <LISTEN> --upstream <UPSTREAM> --tls-cert <TLS_CERT> --tls-key <TLS_KEY> --upstream-ca <UPSTREAM_CA>

Options:
      --listen <LISTEN>                    
      --upstream <UPSTREAM>                
      --tls-cert <TLS_CERT>                
      --tls-key <TLS_KEY>                  
      --upstream-ca <UPSTREAM_CA>          
      --enrollment-file <ENROLLMENT_FILE>  [default: /etc/kiln/identity.json]
      --jwks-cache <JWKS_CACHE>            [default: /var/lib/kiln-api/jwks.json]
  -h, --help                               Print help
door_help_exit:0
+ upgrades
total 16
drwx------ 4 root root 4096 Oct 19 23:27 .
drwxr-xr-x 4 root root 4096 Oct 19 23:27 ..
drwx------ 2 root root 4096 Oct 19 19:52 0.4.4-8DDh4B
drwx------ 2 root root 4096 Oct 19 23:27 0.4.5-RSNr4F

pending_ls:2
+ services
MainPID=3848070
NRestarts=0
Id=kiln-host.service
ActiveState=active
SubState=running
ActiveEnterTimestamp=Mon 2026-10-19 23:27:34 CEST

MainPID=3848074
NRestarts=0
Id=kiln-api.service
ActiveState=active
SubState=running
ActiveEnterTimestamp=Mon 2026-10-19 23:27:34 CEST
DOCTOR_DONE
```

Stderr:

### doctor.err

```text
ls: cannot access '/opt/kiln/pending-upgrade.json': No such file or directory
```

`doctor_exit` is 0. `"development_ready": true`. Firecracker and jailer report 1.17.0. `kiln-host` MainPID moved from 3739721 to 3848070. `kiln-api` MainPID moved from 3739725 to 3848074. Both were active and running, NRestarts 0, ActiveEnterTimestamp `Mon 2026-10-19 23:27:34 CEST`. `install.sh` stops and starts those two units. `/opt/kiln/pending-upgrade.json` is absent.

## 4. On-server smoke test

The config path `/var/tmp/kiln-smoke-profile-v046.json` did not exist. It was not created as an empty file. `profile add` created it. Outer ssh `timeout` was 720 seconds. ssh exit 0.

### smoke.out

```text
===== date -u =====
Mon Oct 19 09:27:52 PM UTC 2026
cfg=/var/tmp/kiln-smoke-profile-v046.json
cfg_exists_before=no
+ kiln --config /var/tmp/kiln-smoke-profile-v046.json profile add default --url https://127.0.0.1:8443 --token-file /etc/kiln/admin.token --ca-file /etc/kiln/ca.crt
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
kiln-smoke  85e581aeac68481394b7d45cdd7e23c3  running       4096 MiB  1
create_exit:0
+ kiln exec kiln-smoke -- uname -r
6.1.155+
exec_exit:0
+ kiln delete kiln-smoke
deleted 85e581aeac68481394b7d45cdd7e23c3
delete_exit:0
+ kiln list after delete
No boxes yet. Create one with `kiln create`.
list_after_delete_exit:0
profile_config_removed
===== SMOKE DONE =====
```

Stderr:

### smoke.err

```text
creating kiln-smoke with template ubuntu-4g
request 599a8ca4cde343469a48c2cdf61c0001
request 51ba0edfe50b4dafa5557bccf2e30083
request 45d903fbe523488ba0720856e3545370
```

Create, exec, and delete exited 0. Guest stdout is `6.1.155+`. The profile file was removed. A later list printed `No boxes yet. Create one with \`kiln create\`.`

## 5. Firewall

The host has `/usr/sbin/nft` and `/usr/sbin/iptables`. `ufw` and `firewall-cmd` are absent. `iptables-save` reports `nf_tables`. The IPv4 filter INPUT policy was ACCEPT with no rules. There was no `inet filter` table. Docker owns `ip filter`, `ip nat`, and `ip6 nat`. Those tables were not edited.

The only added rule is `tcp dport 8443 accept` on a new `inet filter` `input` chain whose policy is accept. Outer ssh `timeout` was 60 seconds. ssh exit 0. A following ssh session succeeded, so ssh on tcp/22 still worked.

### firewall.out

```text
===== date -u =====
Mon Oct 19 09:28:47 PM UTC 2026
+ existing inet filter
inet_filter_before_exit:1
+ add table inet filter
add_table_exit:0
+ add chain inet filter input policy accept
add_chain_exit:0
+ add rule tcp dport 8443 accept
add_rule_exit:0
+ nft list table inet filter
table inet filter {
	chain input {
		type filter hook input priority filter; policy accept;
		tcp dport 8443 accept
	}
}
list_table_exit:0
+ save after rules
nft_after_exit:0
0756e3b7baea87c99856d640735a7b18baa9f2b6013d5a819710247e7e2fdb4e  /root/nft-before-v046.txt
56beb5bb4d1e0eb67bdb4e91abbbcba867d89ff87ed78fb863e578128d45909a  /root/nft-after-v046.txt
+ diff -u nft
--- /root/nft-before-v046.txt	2026-10-19 23:26:57.515387141 +0200
+++ /root/nft-after-v046.txt	2026-10-19 23:28:47.953214458 +0200
@@ -1,13 +1,13 @@
 table ip nat {
 	chain DOCKER {
-		iifname != "br-164dac86db63" tcp dport 80 counter packets 1595910 bytes 93942999 dnat to 172.18.0.3:80
-		iifname != "br-164dac86db63" tcp dport 443 counter packets 2361943 bytes 139289611 dnat to 172.18.0.3:443
+		iifname != "br-164dac86db63" tcp dport 80 counter packets 1595912 bytes 93943119 dnat to 172.18.0.3:80
+		iifname != "br-164dac86db63" tcp dport 443 counter packets 2361976 bytes 139291519 dnat to 172.18.0.3:443
 		ip daddr 127.0.0.1 iifname != "br-164dac86db63" tcp dport 6767 counter packets 0 bytes 0 dnat to 172.18.0.2:6767
 	}
 
 	chain PREROUTING {
 		type nat hook prerouting priority dstnat; policy accept;
-		fib daddr type local counter packets 19261010 bytes 1236645122 jump DOCKER
+		fib daddr type local counter packets 19261262 bytes 1236661026 jump DOCKER
 	}
 
 	chain OUTPUT {
@@ -17,35 +17,35 @@
 
 	chain POSTROUTING {
 		type nat hook postrouting priority srcnat; policy accept;
-		ip saddr 172.18.0.0/16 oifname != "br-164dac86db63" counter packets 69144 bytes 4148620 masquerade
+		ip saddr 172.18.0.0/16 oifname != "br-164dac86db63" counter packets 69147 bytes 4148800 masquerade
 		ip saddr 172.17.0.0/16 oifname != "docker0" counter packets 458 bytes 27564 masquerade
 	}
 }
 table ip filter {
 	chain DOCKER {
 		ip daddr 172.18.0.2 iifname != "br-164dac86db63" oifname "br-164dac86db63" tcp dport 6767 counter packets 0 bytes 0 accept
-		ip daddr 172.18.0.3 iifname != "br-164dac86db63" oifname "br-164dac86db63" tcp dport 443 counter packets 2361943 bytes 139289611 accept
-		ip daddr 172.18.0.3 iifname != "br-164dac86db63" oifname "br-164dac86db63" tcp dport 80 counter packets 1595910 bytes 93942999 accept
+		ip daddr 172.18.0.3 iifname != "br-164dac86db63" oifname "br-164dac86db63" tcp dport 443 counter packets 2361976 bytes 139291519 accept
+		ip daddr 172.18.0.3 iifname != "br-164dac86db63" oifname "br-164dac86db63" tcp dport 80 counter packets 1595912 bytes 93943119 accept
 		iifname != "docker0" oifname "docker0" counter packets 0 bytes 0 drop
 		iifname != "br-164dac86db63" oifname "br-164dac86db63" counter packets 0 bytes 0 drop
 	}
 
 	chain DOCKER-FORWARD {
-		counter packets 38248677 bytes 13365528104 jump DOCKER-CT
-		counter packets 28983271 bytes 4665367109 jump DOCKER-INTERNAL
-		counter packets 28983271 bytes 4665367109 jump DOCKER-BRIDGE
+		counter packets 38249155 bytes 13365586686 jump DOCKER-CT
+		counter packets 28983617 bytes 4665398854 jump DOCKER-INTERNAL
+		counter packets 28983617 bytes 4665398854 jump DOCKER-BRIDGE
 		iifname "docker0" counter packets 447013 bytes 31000591 accept
-		iifname "br-164dac86db63" counter packets 23157559 bytes 4322391247 accept
+		iifname "br-164dac86db63" counter packets 23157870 bytes 4322420964 accept
 	}
 
 	chain DOCKER-BRIDGE {
 		oifname "docker0" counter packets 0 bytes 0 jump DOCKER
-		oifname "br-164dac86db63" counter packets 4434458 bytes 261341620 jump DOCKER
+		oifname "br-164dac86db63" counter packets 4434493 bytes 261343648 jump DOCKER
 	}
 
 	chain DOCKER-CT {
 		oifname "docker0" ct state related,established counter packets 715346 bytes 3733100098 accept
-		oifname "br-164dac86db63" ct state related,established counter packets 8548924 bytes 4953947319 accept
+		oifname "br-164dac86db63" ct state related,established counter packets 8549056 bytes 4953974156 accept
 	}
 
 	chain DOCKER-INTERNAL {
@@ -53,8 +53,8 @@
 
 	chain FORWARD {
 		type filter hook forward priority filter; policy drop;
-		counter packets 38248677 bytes 13365528104 jump DOCKER-USER
-		counter packets 38248677 bytes 13365528104 jump DOCKER-FORWARD
+		counter packets 38249155 bytes 13365586686 jump DOCKER-USER
+		counter packets 38249155 bytes 13365586686 jump DOCKER-FORWARD
 	}
 
 	chain DOCKER-USER {
@@ -112,3 +112,9 @@
 		ip daddr 127.0.0.1 iifname != "lo" tcp dport 6767 counter packets 0 bytes 0 drop
 	}
 }
+table inet filter {
+	chain input {
+		type filter hook input priority filter; policy accept;
+		tcp dport 8443 accept
+	}
+}
nft_diff_exit:1
+ diff iptables
--- /root/iptables-before-v046.txt	2026-10-19 23:26:57.523386983 +0200
+++ /root/iptables-after-v046.txt	2026-10-19 23:28:47.957214381 +0200
@@ -1,6 +1,6 @@
-# Generated by iptables-save v1.8.11 (nf_tables) on Mon Oct 19 23:26:57 2026
+# Generated by iptables-save v1.8.11 (nf_tables) on Mon Oct 19 23:28:47 2026
 *raw
-:PREROUTING ACCEPT [60871144:16282881116]
+:PREROUTING ACCEPT [60888315:16483522046]
 :OUTPUT ACCEPT [0:0]
 -A PREROUTING -d 172.18.0.4/32 ! -i br-164dac86db63 -j DROP
 -A PREROUTING -d 172.18.0.3/32 ! -i br-164dac86db63 -j DROP
@@ -8,8 +8,8 @@
 -A PREROUTING -d 172.18.0.2/32 ! -i br-164dac86db63 -j DROP
 -A PREROUTING -d 127.0.0.1/32 ! -i lo -p tcp -m tcp --dport 6767 -j DROP
 COMMIT
-# Completed on Mon Oct 19 23:26:57 2026
-# Generated by iptables-save v1.8.11 (nf_tables) on Mon Oct 19 23:26:57 2026
+# Completed on Mon Oct 19 23:28:47 2026
+# Generated by iptables-save v1.8.11 (nf_tables) on Mon Oct 19 23:28:47 2026
 *filter
 :INPUT ACCEPT [0:0]
 :FORWARD DROP [943729:50579299]
@@ -37,13 +37,13 @@
 -A DOCKER-FORWARD -i docker0 -j ACCEPT
 -A DOCKER-FORWARD -i br-164dac86db63 -j ACCEPT
 COMMIT
-# Completed on Mon Oct 19 23:26:57 2026
-# Generated by iptables-save v1.8.11 (nf_tables) on Mon Oct 19 23:26:57 2026
+# Completed on Mon Oct 19 23:28:47 2026
+# Generated by iptables-save v1.8.11 (nf_tables) on Mon Oct 19 23:28:47 2026
 *nat
-:PREROUTING ACCEPT [15836273:1029884289]
+:PREROUTING ACCEPT [15836493:1029898345]
 :INPUT ACCEPT [0:0]
-:OUTPUT ACCEPT [97521:6236521]
-:POSTROUTING ACCEPT [4531978:267578081]
+:OUTPUT ACCEPT [97542:6237832]
+:POSTROUTING ACCEPT [4532034:267581420]
 :DOCKER - [0:0]
 -A PREROUTING -m addrtype --dst-type LOCAL -j DOCKER
 -A OUTPUT ! -d 127.0.0.0/8 -m addrtype --dst-type LOCAL -j DOCKER
@@ -53,4 +53,4 @@
 -A DOCKER ! -i br-164dac86db63 -p tcp -m tcp --dport 443 -j DNAT --to-destination 172.18.0.3:443
 -A DOCKER -d 127.0.0.1/32 ! -i br-164dac86db63 -p tcp -m tcp --dport 6767 -j DNAT --to-destination 172.18.0.2:6767
 COMMIT
-# Completed on Mon Oct 19 23:26:57 2026
+# Completed on Mon Oct 19 23:28:47 2026
iptables_diff_exit:1
+ diff ip6tables
--- /root/ip6tables-before-v046.txt	2026-10-19 23:26:57.527386905 +0200
+++ /root/ip6tables-after-v046.txt	2026-10-19 23:28:47.961214301 +0200
@@ -1,4 +1,4 @@
-# Generated by ip6tables-save v1.8.11 (nf_tables) on Mon Oct 19 23:26:57 2026
+# Generated by ip6tables-save v1.8.11 (nf_tables) on Mon Oct 19 23:28:47 2026
 *filter
 :INPUT ACCEPT [0:0]
 :FORWARD ACCEPT [0:0]
@@ -15,8 +15,8 @@
 -A DOCKER-FORWARD -j DOCKER-INTERNAL
 -A DOCKER-FORWARD -j DOCKER-BRIDGE
 COMMIT
-# Completed on Mon Oct 19 23:26:57 2026
-# Generated by ip6tables-save v1.8.11 (nf_tables) on Mon Oct 19 23:26:57 2026
+# Completed on Mon Oct 19 23:28:47 2026
+# Generated by ip6tables-save v1.8.11 (nf_tables) on Mon Oct 19 23:28:47 2026
 *nat
 :PREROUTING ACCEPT [0:0]
 :INPUT ACCEPT [0:0]
@@ -26,4 +26,4 @@
 -A PREROUTING -m addrtype --dst-type LOCAL -j DOCKER
 -A OUTPUT ! -d ::1/128 -m addrtype --dst-type LOCAL -j DOCKER
 COMMIT
-# Completed on Mon Oct 19 23:26:57 2026
+# Completed on Mon Oct 19 23:28:47 2026
ip6tables_diff_exit:1
FIREWALL_DONE
```

Stderr:

### firewall.err

```text
Error: No such file or directory
list table inet filter
                ^^^^^^
# Warning: table ip nat is managed by iptables-nft, do not touch!
# Warning: table ip filter is managed by iptables-nft, do not touch!
# Warning: table ip6 nat is managed by iptables-nft, do not touch!
```

`nft list table inet filter` before the add exited 1 because the table did not exist. After the add it is:

```text
table inet filter {
	chain input {
		type filter hook input priority filter; policy accept;
		tcp dport 8443 accept
	}
}
```

The nft ruleset diff is in `firewall.out`. The structural addition is the `inet filter` table. The other hunks are Docker packet and byte counters. `iptables-save` and `ip6tables-save` differ only in timestamps and packet counters. No Docker rule was added or removed.

Saved rulesets:

### /root/nft-before-v046.txt

```text
table ip nat {
	chain DOCKER {
		iifname != "br-164dac86db63" tcp dport 80 counter packets 1595910 bytes 93942999 dnat to 172.18.0.3:80
		iifname != "br-164dac86db63" tcp dport 443 counter packets 2361943 bytes 139289611 dnat to 172.18.0.3:443
		ip daddr 127.0.0.1 iifname != "br-164dac86db63" tcp dport 6767 counter packets 0 bytes 0 dnat to 172.18.0.2:6767
	}

	chain PREROUTING {
		type nat hook prerouting priority dstnat; policy accept;
		fib daddr type local counter packets 19261010 bytes 1236645122 jump DOCKER
	}

	chain OUTPUT {
		type nat hook output priority dstnat; policy accept;
		ip daddr != 127.0.0.0/8 fib daddr type local counter packets 2 bytes 120 jump DOCKER
	}

	chain POSTROUTING {
		type nat hook postrouting priority srcnat; policy accept;
		ip saddr 172.18.0.0/16 oifname != "br-164dac86db63" counter packets 69144 bytes 4148620 masquerade
		ip saddr 172.17.0.0/16 oifname != "docker0" counter packets 458 bytes 27564 masquerade
	}
}
table ip filter {
	chain DOCKER {
		ip daddr 172.18.0.2 iifname != "br-164dac86db63" oifname "br-164dac86db63" tcp dport 6767 counter packets 0 bytes 0 accept
		ip daddr 172.18.0.3 iifname != "br-164dac86db63" oifname "br-164dac86db63" tcp dport 443 counter packets 2361943 bytes 139289611 accept
		ip daddr 172.18.0.3 iifname != "br-164dac86db63" oifname "br-164dac86db63" tcp dport 80 counter packets 1595910 bytes 93942999 accept
		iifname != "docker0" oifname "docker0" counter packets 0 bytes 0 drop
		iifname != "br-164dac86db63" oifname "br-164dac86db63" counter packets 0 bytes 0 drop
	}

	chain DOCKER-FORWARD {
		counter packets 38248677 bytes 13365528104 jump DOCKER-CT
		counter packets 28983271 bytes 4665367109 jump DOCKER-INTERNAL
		counter packets 28983271 bytes 4665367109 jump DOCKER-BRIDGE
		iifname "docker0" counter packets 447013 bytes 31000591 accept
		iifname "br-164dac86db63" counter packets 23157559 bytes 4322391247 accept
	}

	chain DOCKER-BRIDGE {
		oifname "docker0" counter packets 0 bytes 0 jump DOCKER
		oifname "br-164dac86db63" counter packets 4434458 bytes 261341620 jump DOCKER
	}

	chain DOCKER-CT {
		oifname "docker0" ct state related,established counter packets 715346 bytes 3733100098 accept
		oifname "br-164dac86db63" ct state related,established counter packets 8548924 bytes 4953947319 accept
	}

	chain DOCKER-INTERNAL {
	}

	chain FORWARD {
		type filter hook forward priority filter; policy drop;
		counter packets 38248677 bytes 13365528104 jump DOCKER-USER
		counter packets 38248677 bytes 13365528104 jump DOCKER-FORWARD
	}

	chain DOCKER-USER {
	}
}
table ip6 nat {
	chain DOCKER {
	}

	chain PREROUTING {
		type nat hook prerouting priority dstnat; policy accept;
		fib daddr type local counter packets 0 bytes 0 jump DOCKER
	}

	chain OUTPUT {
		type nat hook output priority dstnat; policy accept;
		ip6 daddr != ::1 fib daddr type local counter packets 0 bytes 0 jump DOCKER
	}
}
table ip6 filter {
	chain DOCKER {
	}

	chain DOCKER-FORWARD {
		counter packets 0 bytes 0 jump DOCKER-CT
		counter packets 0 bytes 0 jump DOCKER-INTERNAL
		counter packets 0 bytes 0 jump DOCKER-BRIDGE
	}

	chain DOCKER-BRIDGE {
	}

	chain DOCKER-CT {
	}

	chain DOCKER-INTERNAL {
	}

	chain FORWARD {
		type filter hook forward priority filter; policy accept;
		counter packets 0 bytes 0 jump DOCKER-USER
		counter packets 0 bytes 0 jump DOCKER-FORWARD
	}

	chain DOCKER-USER {
	}
}
table ip raw {
	chain PREROUTING {
		type filter hook prerouting priority raw; policy accept;
		ip daddr 172.18.0.4 iifname != "br-164dac86db63" counter packets 0 bytes 0 drop
		ip daddr 172.18.0.3 iifname != "br-164dac86db63" counter packets 0 bytes 0 drop
		ip daddr 172.18.0.5 iifname != "br-164dac86db63" counter packets 0 bytes 0 drop
		ip daddr 172.18.0.2 iifname != "br-164dac86db63" counter packets 0 bytes 0 drop
		ip daddr 127.0.0.1 iifname != "lo" tcp dport 6767 counter packets 0 bytes 0 drop
	}
}
```

### /root/nft-after-v046.txt

```text
table ip nat {
	chain DOCKER {
		iifname != "br-164dac86db63" tcp dport 80 counter packets 1595912 bytes 93943119 dnat to 172.18.0.3:80
		iifname != "br-164dac86db63" tcp dport 443 counter packets 2361976 bytes 139291519 dnat to 172.18.0.3:443
		ip daddr 127.0.0.1 iifname != "br-164dac86db63" tcp dport 6767 counter packets 0 bytes 0 dnat to 172.18.0.2:6767
	}

	chain PREROUTING {
		type nat hook prerouting priority dstnat; policy accept;
		fib daddr type local counter packets 19261262 bytes 1236661026 jump DOCKER
	}

	chain OUTPUT {
		type nat hook output priority dstnat; policy accept;
		ip daddr != 127.0.0.0/8 fib daddr type local counter packets 2 bytes 120 jump DOCKER
	}

	chain POSTROUTING {
		type nat hook postrouting priority srcnat; policy accept;
		ip saddr 172.18.0.0/16 oifname != "br-164dac86db63" counter packets 69147 bytes 4148800 masquerade
		ip saddr 172.17.0.0/16 oifname != "docker0" counter packets 458 bytes 27564 masquerade
	}
}
table ip filter {
	chain DOCKER {
		ip daddr 172.18.0.2 iifname != "br-164dac86db63" oifname "br-164dac86db63" tcp dport 6767 counter packets 0 bytes 0 accept
		ip daddr 172.18.0.3 iifname != "br-164dac86db63" oifname "br-164dac86db63" tcp dport 443 counter packets 2361976 bytes 139291519 accept
		ip daddr 172.18.0.3 iifname != "br-164dac86db63" oifname "br-164dac86db63" tcp dport 80 counter packets 1595912 bytes 93943119 accept
		iifname != "docker0" oifname "docker0" counter packets 0 bytes 0 drop
		iifname != "br-164dac86db63" oifname "br-164dac86db63" counter packets 0 bytes 0 drop
	}

	chain DOCKER-FORWARD {
		counter packets 38249155 bytes 13365586686 jump DOCKER-CT
		counter packets 28983617 bytes 4665398854 jump DOCKER-INTERNAL
		counter packets 28983617 bytes 4665398854 jump DOCKER-BRIDGE
		iifname "docker0" counter packets 447013 bytes 31000591 accept
		iifname "br-164dac86db63" counter packets 23157870 bytes 4322420964 accept
	}

	chain DOCKER-BRIDGE {
		oifname "docker0" counter packets 0 bytes 0 jump DOCKER
		oifname "br-164dac86db63" counter packets 4434493 bytes 261343648 jump DOCKER
	}

	chain DOCKER-CT {
		oifname "docker0" ct state related,established counter packets 715346 bytes 3733100098 accept
		oifname "br-164dac86db63" ct state related,established counter packets 8549056 bytes 4953974156 accept
	}

	chain DOCKER-INTERNAL {
	}

	chain FORWARD {
		type filter hook forward priority filter; policy drop;
		counter packets 38249155 bytes 13365586686 jump DOCKER-USER
		counter packets 38249155 bytes 13365586686 jump DOCKER-FORWARD
	}

	chain DOCKER-USER {
	}
}
table ip6 nat {
	chain DOCKER {
	}

	chain PREROUTING {
		type nat hook prerouting priority dstnat; policy accept;
		fib daddr type local counter packets 0 bytes 0 jump DOCKER
	}

	chain OUTPUT {
		type nat hook output priority dstnat; policy accept;
		ip6 daddr != ::1 fib daddr type local counter packets 0 bytes 0 jump DOCKER
	}
}
table ip6 filter {
	chain DOCKER {
	}

	chain DOCKER-FORWARD {
		counter packets 0 bytes 0 jump DOCKER-CT
		counter packets 0 bytes 0 jump DOCKER-INTERNAL
		counter packets 0 bytes 0 jump DOCKER-BRIDGE
	}

	chain DOCKER-BRIDGE {
	}

	chain DOCKER-CT {
	}

	chain DOCKER-INTERNAL {
	}

	chain FORWARD {
		type filter hook forward priority filter; policy accept;
		counter packets 0 bytes 0 jump DOCKER-USER
		counter packets 0 bytes 0 jump DOCKER-FORWARD
	}

	chain DOCKER-USER {
	}
}
table ip raw {
	chain PREROUTING {
		type filter hook prerouting priority raw; policy accept;
		ip daddr 172.18.0.4 iifname != "br-164dac86db63" counter packets 0 bytes 0 drop
		ip daddr 172.18.0.3 iifname != "br-164dac86db63" counter packets 0 bytes 0 drop
		ip daddr 172.18.0.5 iifname != "br-164dac86db63" counter packets 0 bytes 0 drop
		ip daddr 172.18.0.2 iifname != "br-164dac86db63" counter packets 0 bytes 0 drop
		ip daddr 127.0.0.1 iifname != "lo" tcp dport 6767 counter packets 0 bytes 0 drop
	}
}
table inet filter {
	chain input {
		type filter hook input priority filter; policy accept;
		tcp dport 8443 accept
	}
}
```

From the operator VM, after the rule, `curl` to `https://cli.dark-forge.dev:8443/v1/templates` again exited 7. Stderr:

```text
* Host cli.dark-forge.dev:8443 was resolved.
* IPv6: (none)
* IPv4: 78.46.98.73
*   Trying 78.46.98.73:8443...
* connect to 78.46.98.73 port 8443 from 172.30.0.2 port 52552 failed: Connection refused
* Failed to connect to cli.dark-forge.dev port 8443 after 110 ms: Couldn't connect to server
* Closing connection
```

Connection refused, not a timeout. A filter outside the host is not dropping tcp/8443. The door is not listening, so the request cannot return 401 yet.

## 6. Identity, authorized_keys, and dpkg after the upgrade

This snapshot was taken after the upgrade, the smoke test, the firewall rule, and the start of the two device flows. Outer ssh `timeout` was 50 seconds. ssh exit 0.

`kiln-identity.service` stayed MainPID 3522047, NRestarts 0, ActiveEnterTimestamp `Mon 2026-10-19 12:42:17 CEST`. Its unit file, drop-in, proxy unit file, socket unit file, `/etc/kiln-identity` file hashes, and `/usr/local/libexec/kiln-identity` hash match the before checks. `/opt/paseo/Caddyfile` stayed `acd7c4ecf177cc065547e3e82e5ffb04202c1a9c101f56a195c3e162a8761eb9`. It does not mention `8443` or `cli.dark-forge.dev`. It was not edited.

`kiln-identity-proxy.service` changed from inactive, MainPID 0, ActiveEnterTimestamp `Mon 2026-10-19 22:26:22 CEST`, to active running, MainPID 3849543, ActiveEnterTimestamp `Mon 2026-10-19 23:29:31 CEST`. The socket's ActiveEnterTimestamp stayed `Sat 2026-10-03 20:14:48 CEST`. Its SubState went from `listening` to `running` because a connection was accepted. That is socket activation from the device-code traffic. The unit files were not edited and `kiln-identity` was not restarted.

`/root/.ssh/authorized_keys` stayed `4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37`.

`diff -u /root/dpkg-before-v046.txt /root/dpkg-after-v046.txt` exited 0 and printed nothing. Both files are sha256 `d388be45b10aa31fa9ab14681dd30f24fa77e122363f6acb724f6f507703807d` and are 370 lines. No package was installed, upgraded, or removed.

### after.out

```text
===== date -u =====
Mon Oct 19 09:30:16 PM UTC 2026

===== BEGIN units =====
+ systemctl show kiln-host.service kiln-api.service kiln-cgroup.service kiln-door.service kiln-identity.service kiln-identity-proxy.service kiln-identity-proxy.socket -p Id\,ActiveState\,SubState\,MainPID\,NRestarts\,ActiveEnterTimestamp\,FragmentPath\,UnitFileState 
MainPID=3848070
NRestarts=0
Id=kiln-host.service
ActiveState=active
SubState=running
FragmentPath=/etc/systemd/system/kiln-host.service
UnitFileState=enabled
ActiveEnterTimestamp=Mon 2026-10-19 23:27:34 CEST

MainPID=3848074
NRestarts=0
Id=kiln-api.service
ActiveState=active
SubState=running
FragmentPath=/etc/systemd/system/kiln-api.service
UnitFileState=enabled
ActiveEnterTimestamp=Mon 2026-10-19 23:27:34 CEST

MainPID=0
NRestarts=0
Id=kiln-cgroup.service
ActiveState=active
SubState=exited
FragmentPath=/etc/systemd/system/kiln-cgroup.service
UnitFileState=enabled
ActiveEnterTimestamp=Mon 2026-10-19 17:12:01 CEST

MainPID=0
NRestarts=0
Id=kiln-door.service
ActiveState=inactive
SubState=dead
FragmentPath=
UnitFileState=
ActiveEnterTimestamp=

MainPID=3522047
NRestarts=0
Id=kiln-identity.service
ActiveState=active
SubState=running
FragmentPath=/etc/systemd/system/kiln-identity.service
UnitFileState=enabled
ActiveEnterTimestamp=Mon 2026-10-19 12:42:17 CEST

MainPID=3849543
NRestarts=0
Id=kiln-identity-proxy.service
ActiveState=active
SubState=running
FragmentPath=/etc/systemd/system/kiln-identity-proxy.service
UnitFileState=static
ActiveEnterTimestamp=Mon 2026-10-19 23:29:31 CEST

Id=kiln-identity-proxy.socket
ActiveState=active
SubState=running
FragmentPath=/etc/systemd/system/kiln-identity-proxy.socket
UnitFileState=enabled
ActiveEnterTimestamp=Sat 2026-10-03 20:14:48 CEST

===== END units exit=0 =====

===== BEGIN authorized-keys-sha =====
+ sha256sum /root/.ssh/authorized_keys 
4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37  /root/.ssh/authorized_keys

===== END authorized-keys-sha exit=0 =====

===== BEGIN identity-bins =====
+ bash -c sha256sum\ /usr/local/libexec/kiln-identity\* 
54c89f144ef0407b0abef11a8ffe60c18acdacc82b2f9c90bdf647c1df1d76ea  /usr/local/libexec/kiln-identity
9e36ca9ae505da5b3704915114e4b39565099871e407ff915f4103d9729eda59  /usr/local/libexec/kiln-identity.rollback-v033

===== END identity-bins exit=0 =====

===== BEGIN identity-units =====
+ bash -c sha256sum\ /etc/systemd/system/kiln-identity.service\ /etc/systemd/system/kiln-identity.service.d/10-provider-readiness.conf\ /etc/systemd/system/kiln-identity-proxy.service\ /etc/systemd/system/kiln-identity-proxy.socket 
31667738c2b275d7b326abcdfa0b6097c947b62005506693d90ab5df80122554  /etc/systemd/system/kiln-identity.service
08aca9fe20443428eed6d902c6d9c47999f8361ea7cb7bb9b22fbd84e739f9e2  /etc/systemd/system/kiln-identity.service.d/10-provider-readiness.conf
f9e0c98316c5f83d12de0abb3b885d6060f8b05a6618ba4f1360e0fbcbd37d6e  /etc/systemd/system/kiln-identity-proxy.service
d04bd26752276f842f93c7f4f6929cdccfacf81abbdc37b35f44519eca6b5141  /etc/systemd/system/kiln-identity-proxy.socket

===== END identity-units exit=0 =====

===== BEGIN identity-config-hashes =====
+ bash -c find\ /etc/kiln-identity\ -type\ f\ -print0\ \|\ sort\ -z\ \|\ xargs\ -0\ sha256sum 
7508a7c9782a09e3181a4bd6a70938ba033eb826bcb8d6eafd53a3610061e495  /etc/kiln-identity/github.secret
9f7da71cffb6cdf12653975b136703b05dc836742879a194ba089733482a3757  /etc/kiln-identity/google.secret
6e0f03257ca0ed6039f1bd3ba855049a0619beba8789163696836027f8c56abb  /etc/kiln-identity/service.env
b1b7a26a116cb539bd4a19c4f4a8f9554f6896f6e7303567a4c50ebeefd87eb8  /etc/kiln-identity/signing.pem

===== END identity-config-hashes exit=0 =====

===== BEGIN caddy-hash =====
+ sha256sum /opt/paseo/Caddyfile 
acd7c4ecf177cc065547e3e82e5ffb04202c1a9c101f56a195c3e162a8761eb9  /opt/paseo/Caddyfile

===== END caddy-hash exit=0 =====

===== BEGIN dpkg-after =====
+ bash -c dpkg\ -l\ \>\ /root/dpkg-after-v046.txt\;\ echo\ dpkg_write_exit:\$\?\;\ sha256sum\ /root/dpkg-before-v046.txt\ /root/dpkg-after-v046.txt\;\ echo\ \"+\ diff\ -u\"\;\ diff\ -u\ /root/dpkg-before-v046.txt\ /root/dpkg-after-v046.txt\;\ echo\ diff_exit:\$\? 
dpkg_write_exit:0
d388be45b10aa31fa9ab14681dd30f24fa77e122363f6acb724f6f507703807d  /root/dpkg-before-v046.txt
d388be45b10aa31fa9ab14681dd30f24fa77e122363f6acb724f6f507703807d  /root/dpkg-after-v046.txt
+ diff -u
diff_exit:0

===== END dpkg-after exit=0 =====

===== BEGIN versions =====
+ bash -c for\ b\ in\ /opt/kiln/bin/kiln\ /opt/kiln/bin/kiln-runtime\ /opt/kiln/bin/firecracker\ /opt/kiln/bin/jailer\ /usr/local/libexec/kiln-host\ /usr/local/libexec/kiln-api\ /usr/local/libexec/kiln-identity\;\ do\ echo\ \"---\ \$b\ ---\"\;\ \"\$b\"\ --version\;\ echo\ exit:\$\?\;\ done 
--- /opt/kiln/bin/kiln ---
kiln 0.4.6
exit:0
--- /opt/kiln/bin/kiln-runtime ---
kiln-runtime 0.4.6
exit:0
--- /opt/kiln/bin/firecracker ---
Firecracker v1.17.0

2026-10-19T23:30:16.760876372 [anonymous-instance:main] Firecracker exiting successfully. exit_code=0
exit:0
--- /opt/kiln/bin/jailer ---
Jailer v1.17.0

exit:0
--- /usr/local/libexec/kiln-host ---
kiln-server 0.4.6
exit:0
--- /usr/local/libexec/kiln-api ---
kiln-server 0.4.6
exit:0
--- /usr/local/libexec/kiln-identity ---
kiln-identity 0.4.1
exit:0

===== END versions exit=0 =====
AFTER_DONE
```

Stderr:

### after.err

```text

```

### /root/dpkg-before-v046.txt

```text
Desired=Unknown/Install/Remove/Purge/Hold
| Status=Not/Inst/Conf-files/Unpacked/halF-conf/Half-inst/trig-aWait/Trig-pend
|/ Err?=(none)/Reinst-required (Status,Err: uppercase=bad)
||/ Name                             Version                              Architecture Description
+++-================================-====================================-============-================================================================================
ii  acl                              2.3.2-2+b1                           amd64        access control list - utilities
ii  acpid                            1:2.0.34-1+b1                        amd64        Advanced Configuration and Power Interface event daemon
ii  adduser                          3.152                                all          add and remove users and groups
ii  amd64-microcode                  3.20250311.1                         amd64        Platform firmware and microcode for AMD CPUs and SoCs
ii  apt                              3.0.3                                amd64        commandline package manager
ii  apt-utils                        3.0.3                                amd64        package management related utility programs
ii  aptitude                         0.8.13-7                             amd64        terminal-based package manager
ii  aptitude-common                  0.8.13-7                             all          architecture independent files for the aptitude package manager
ii  at                               3.2.5-2.2                            amd64        Delayed job execution and batch processing
ii  base-files                       13.8+deb13u6                         amd64        Debian base system miscellaneous files
ii  base-passwd                      3.6.7                                amd64        Debian base system master password and group files
ii  bash                             5.2.37-2+b9                          amd64        GNU Bourne Again SHell
ii  bash-completion                  1:2.16.0-7                           all          programmable completion for the bash shell
ii  bind9-dnsutils                   1:9.20.26-1~deb13u1                  amd64        Clients provided with BIND 9
ii  bind9-host                       1:9.20.26-1~deb13u1                  amd64        DNS Lookup Utility
ii  bind9-libs:amd64                 1:9.20.26-1~deb13u1                  amd64        Shared Libraries used by BIND 9
ii  bsdextrautils                    2.41-5                               amd64        extra utilities from 4.4BSD-Lite
ii  bsdutils                         1:2.41-5                             amd64        basic utilities from 4.4BSD-Lite
ii  btrfs-progs                      6.14-1                               amd64        Checksumming Copy on Write Filesystem utilities
ii  busybox                          1:1.37.0-6+b8                        amd64        Tiny utilities for small and embedded systems
ii  bzip2                            1.0.8-6                              amd64        high-quality block-sorting file compressor - utilities
ii  ca-certificates                  20250419                             all          Common CA certificates
ii  console-setup                    1.242~deb13u1                        all          console font and keymap setup program
ii  console-setup-linux              1.242~deb13u1                        all          Linux specific part of console-setup
ii  containerd.io                    2.3.3-1~debian.13~trixie             amd64        An open and reliable container runtime
ii  coreutils                        9.7-3                                amd64        GNU core utilities
ii  cpio                             2.15+dfsg-2                          amd64        GNU cpio -- a program to manage archives of files
ii  cron                             3.0pl1-197                           amd64        process scheduling daemon
ii  cron-daemon-common               3.0pl1-197                           all          process scheduling daemon's configuration files
ii  cryptsetup                       2:2.7.5-2                            amd64        disk encryption support - startup scripts
ii  cryptsetup-bin                   2:2.7.5-2                            amd64        disk encryption support - command line tools
ii  cryptsetup-initramfs             2:2.7.5-2                            all          disk encryption support - initramfs integration
ii  curl                             8.14.1-2+deb13u4                     amd64        command line tool for transferring data with URL syntax
ii  dash                             0.5.12-12                            amd64        POSIX-compliant shell
ii  dbus                             1.16.2-2                             amd64        simple interprocess messaging system (system message bus)
ii  dbus-bin                         1.16.2-2                             amd64        simple interprocess messaging system (command line utilities)
ii  dbus-daemon                      1.16.2-2                             amd64        simple interprocess messaging system (reference message bus)
ii  dbus-session-bus-common          1.16.2-2                             all          simple interprocess messaging system (session bus configuration)
ii  dbus-system-bus-common           1.16.2-2                             all          simple interprocess messaging system (system bus configuration)
ii  dbus-user-session                1.16.2-2                             amd64        simple interprocess messaging system (systemd --user integration)
ii  debconf                          1.5.91                               all          Debian configuration management system
ii  debconf-i18n                     1.5.91                               all          full internationalization support for debconf
ii  debian-archive-keyring           2025.1                               all          OpenPGP archive certificates of the Debian archive
ii  debianutils                      5.23.2                               amd64        Miscellaneous utilities specific to Debian
ii  dhcpcd-base                      1:10.1.0-11+deb13u3                  amd64        DHCPv4 and DHCPv6 dual-stack client (binaries and exit hooks)
ii  diffutils                        1:3.10-4                             amd64        File comparison utilities
ii  dirmngr                          2.4.7-21+deb13u1+b4                  amd64        GNU privacy guard - network certificate management service
ii  distro-info-data                 0.66+deb13u2                         all          information about the distributions' releases (data files)
ii  dmeventd                         2:1.02.205-2                         amd64        Linux Kernel Device Mapper event daemon
ii  dmidecode                        3.6-2                                amd64        SMBIOS/DMI table decoder
ii  dmsetup                          2:1.02.205-2                         amd64        Linux Kernel Device Mapper userspace library
ii  docker-buildx-plugin             0.36.1-1~debian.13~trixie            amd64        Docker Buildx plugin extends build capabilities with BuildKit.
ii  docker-ce                        5:29.7.2-1~debian.13~trixie          amd64        Docker: the open-source application container engine
ii  docker-ce-cli                    5:29.7.2-1~debian.13~trixie          amd64        Docker CLI: the open-source application container engine
ii  docker-ce-rootless-extras        5:29.7.2-1~debian.13~trixie          amd64        Rootless support for Docker.
ii  docker-compose-plugin            5.5.0-1~debian.13~trixie             amd64        Docker Compose (V2) plugin for the Docker CLI.
ii  docker-model-plugin              1.2.6-1~debian.13~trixie             amd64        Docker Model Runner plugin for the Docker CLI.
ii  dosfstools                       4.2-1.2                              amd64        utilities for making and checking MS-DOS FAT filesystems
ii  dpkg                             1.22.22                              amd64        Debian package management system
ii  dracut-install                   106-6                                amd64        dracut is an event driven initramfs infrastructure (dracut-install)
ii  e2fsprogs                        1.47.2-3+b11                         amd64        ext2/ext3/ext4 file system utilities
ii  efibootmgr                       18-2                                 amd64        Interact with the EFI Boot Manager
ii  ethtool                          1:6.14.2-1                           amd64        display or change Ethernet device settings
ii  fdisk                            2.41-5                               amd64        collection of partitioning utilities
ii  file                             1:5.46-5                             amd64        Recognize the type of data in a file using "magic" numbers
ii  findutils                        4.10.0-3                             amd64        utilities for finding files--find, xargs
ii  firmware-bnx2x                   20250410-2                           all          Binary firmware for Broadcom NetXtreme II 10Gb
ii  gcc-14-base:amd64                14.2.0-19                            amd64        GCC, the GNU Compiler Collection (base package)
ii  gdisk                            1.0.10-2                             amd64        GPT fdisk text-mode partitioning tool
ii  gettext-base                     0.23.1-2                             amd64        GNU Internationalization utilities for the base system
ii  gnupg                            2.4.7-21+deb13u1                     all          GNU privacy guard - a free PGP replacement
ii  gnupg-l10n                       2.4.7-21+deb13u1                     all          GNU privacy guard - localization files
ii  gpg                              2.4.7-21+deb13u1+b4                  amd64        GNU Privacy Guard -- minimalist public key operations
ii  gpg-agent                        2.4.7-21+deb13u1+b4                  amd64        GNU privacy guard - cryptographic agent
ii  gpgconf                          2.4.7-21+deb13u1+b4                  amd64        GNU privacy guard - core configuration utilities
ii  gpgsm                            2.4.7-21+deb13u1+b4                  amd64        GNU privacy guard - S/MIME version
ii  grep                             3.11-4                               amd64        GNU grep, egrep and fgrep
ii  groff-base                       1.23.0-9                             amd64        GNU troff text-formatting system (base system components)
ii  grub-common                      2.12-9+deb13u2                       amd64        GRand Unified Bootloader (common files)
ii  grub-efi-amd64                   2.12-9+deb13u2                       amd64        GRand Unified Bootloader, version 2 (EFI-AMD64 version)
ii  grub-efi-amd64-bin               2.12-9+deb13u2                       amd64        GRand Unified Bootloader, version 2 (EFI-AMD64 modules)
ii  grub-efi-amd64-unsigned          2.12-9+deb13u2                       amd64        GRand Unified Bootloader, version 2 (EFI-AMD64 images)
ii  grub-pc-bin                      2.12-9+deb13u2                       amd64        GRand Unified Bootloader, version 2 (PC/BIOS modules)
ii  grub2-common                     2.12-9+deb13u2                       amd64        GRand Unified Bootloader (common files for version 2)
ii  gzip                             1.13-1                               amd64        GNU compression utilities
ii  hostname                         3.25                                 amd64        utility to set/show the host name or domain name
ii  htop                             3.4.1-5                              amd64        interactive processes viewer
ii  ifupdown                         0.8.44+deb13u1                       amd64        high level tools to configure network interfaces
ii  inetutils-telnet                 2:2.6-3+deb13u3                      amd64        telnet client
ii  init                             1.69~deb13u1                         amd64        metapackage ensuring an init system is installed
ii  init-system-helpers              1.69~deb13u1                         all          helper tools for all init systems
ii  initramfs-tools                  0.148.4                              all          generic modular initramfs generator (automation)
ii  initramfs-tools-bin              0.148.4                              amd64        generic modular initramfs generator (binary tools)
ii  initramfs-tools-core             0.148.4                              all          generic modular initramfs generator (core tools)
ii  intel-microcode                  3.20251111.1~deb13u1                 amd64        Processor microcode firmware for Intel CPUs
ii  iproute2                         6.15.0-1                             amd64        networking and traffic control tools
ii  iptables                         1.8.11-2                             amd64        administration tools for packet filtering and NAT
ii  iputils-ping                     3:20240905-3                         amd64        Tools to test the reachability of network hosts
ii  iucode-tool                      2.3.1-3                              amd64        Intel processor microcode tool
ii  kbd                              2.7.1-2                              amd64        Linux console font and keytable utilities
ii  keyboard-configuration           1.242~deb13u1                        all          system-wide keyboard preferences
ii  klibc-utils                      2.0.14-1                             amd64        small utilities built with klibc for early boot
ii  kmod                             34.2-2                               amd64        tools for managing Linux kernel modules
ii  laptop-detect                    0.16+nmu1                            all          system chassis type checker
ii  less                             668-1                                amd64        pager program similar to more
ii  libacl1:amd64                    2.3.2-2+b1                           amd64        access control list - shared library
ii  libaio1t64:amd64                 0.3.113-8+b1                         amd64        Linux kernel AIO access library - shared library
ii  libapparmor1:amd64               4.1.0-1                              amd64        changehat AppArmor library
ii  libapt-pkg7.0:amd64              3.0.3                                amd64        package management runtime library
ii  libassuan9:amd64                 3.0.2-2                              amd64        IPC library for the GnuPG components
ii  libatomic1:amd64                 14.2.0-19                            amd64        support library providing __atomic built-in functions
ii  libattr1:amd64                   1:2.5.2-3                            amd64        extended attribute handling - shared library
ii  libaudit-common                  1:4.0.2-2                            all          Dynamic library for security auditing - common files
ii  libaudit1:amd64                  1:4.0.2-2+b2                         amd64        Dynamic library for security auditing
ii  libblkid1:amd64                  2.41-5                               amd64        block device ID library
ii  libboost-iostreams1.83.0:amd64   1.83.0-4.2                           amd64        Boost.Iostreams Library
ii  libbpf1:amd64                    1:1.5.0-3                            amd64        eBPF helper library (shared library)
ii  libbrotli1:amd64                 1.1.0-2+b7                           amd64        library implementing brotli encoder and decoder (shared libraries)
ii  libbsd0:amd64                    0.12.2-2                             amd64        utility functions from BSD systems - shared library
ii  libbz2-1.0:amd64                 1.0.8-6                              amd64        high-quality block-sorting file compressor library - runtime
ii  libc-bin                         2.41-12+deb13u3                      amd64        GNU C Library: Binaries
ii  libc-l10n                        2.41-12+deb13u3                      all          GNU C Library: localization files
ii  libc6:amd64                      2.41-12+deb13u3                      amd64        GNU C Library: Shared libraries
ii  libcap-ng0:amd64                 0.8.5-4+b1                           amd64        alternate POSIX capabilities library
ii  libcap2:amd64                    1:2.75-10+deb13u1+b1                 amd64        POSIX 1003.1e capabilities (library)
ii  libcap2-bin                      1:2.75-10+deb13u1+b1                 amd64        POSIX 1003.1e capabilities (utilities)
ii  libcbor0.10:amd64                0.10.2-2                             amd64        library for parsing and generating CBOR (RFC 7049)
ii  libcom-err2:amd64                1.47.2-3+b11                         amd64        common error description library
ii  libcrypt1:amd64                  1:4.4.38-1                           amd64        libcrypt shared library
ii  libcryptsetup12:amd64            2:2.7.5-2                            amd64        disk encryption support - shared library
ii  libcurl4t64:amd64                8.14.1-2+deb13u4                     amd64        easy-to-use client-side URL transfer library (OpenSSL flavour)
ii  libcwidget4:amd64                0.5.18-6+b2                          amd64        high-level terminal interface library for C++ (runtime files)
ii  libdb5.3t64:amd64                5.3.28+dfsg2-9                       amd64        Berkeley v5.3 Database Libraries [runtime]
ii  libdbus-1-3:amd64                1.16.2-2                             amd64        simple interprocess messaging system (library)
ii  libdebconfclient0:amd64          0.280                                amd64        Debian Configuration Management System (C-implementation library)
ii  libdevmapper-event1.02.1:amd64   2:1.02.205-2                         amd64        Linux Kernel Device Mapper event support library
ii  libdevmapper1.02.1:amd64         2:1.02.205-2                         amd64        Linux Kernel Device Mapper userspace library
ii  libedit2:amd64                   3.1-20250104-1                       amd64        BSD editline and history libraries
ii  libefiboot1t64:amd64             38-3.1+b1                            amd64        Library to manage UEFI variables
ii  libefivar1t64:amd64              38-3.1+b1                            amd64        Library to manage UEFI variables
ii  libelf1t64:amd64                 0.192-4                              amd64        library to read and write ELF files
ii  libexpat1:amd64                  2.8.2-1~deb13u1                      amd64        XML parsing C library - runtime library
ii  libext2fs2t64:amd64              1.47.2-3+b11                         amd64        ext2/ext3/ext4 file system libraries
ii  libfdisk1:amd64                  2.41-5                               amd64        fdisk partitioning library
ii  libffi8:amd64                    3.4.8-2                              amd64        Foreign Function Interface library runtime
ii  libfido2-1:amd64                 1.15.0-1+b1                          amd64        library for generating and verifying FIDO 2.0 objects
ii  libfreetype6:amd64               2.13.3+dfsg-1+deb13u1                amd64        FreeType 2 font engine, shared library files
ii  libfstrm0:amd64                  0.6.1-1+b3                           amd64        Frame Streams (fstrm) library
ii  libfuse3-4:amd64                 3.17.2-3                             amd64        Filesystem in Userspace (library) (3.x version)
ii  libgcc-s1:amd64                  14.2.0-19                            amd64        GCC support library
ii  libgcrypt20:amd64                1.11.0-7+deb13u1                     amd64        LGPL Crypto library - runtime library
ii  libgdbm-compat4t64:amd64         1.24-2                               amd64        GNU dbm database routines (legacy support runtime version) 
ii  libgdbm6t64:amd64                1.24-2                               amd64        GNU dbm database routines (runtime version) 
ii  libglib2.0-0t64:amd64            2.84.4-3~deb13u3                     amd64        GLib library of C routines
ii  libglib2.0-data                  2.84.4-3~deb13u3                     all          Common files for GLib library
ii  libgmp10:amd64                   2:6.3.0+dfsg-3                       amd64        Multiprecision arithmetic library
ii  libgnutls30t64:amd64             3.8.9-3+deb13u4                      amd64        GNU TLS library - main runtime library
ii  libgpg-error0:amd64              1.51-4                               amd64        GnuPG development runtime library
ii  libgpm2:amd64                    1.20.7-11+b2                         amd64        General Purpose Mouse - shared library
ii  libgssapi-krb5-2:amd64           1.21.3-5+deb13u1                     amd64        MIT Kerberos runtime libraries - krb5 GSS-API Mechanism
ii  libhogweed6t64:amd64             3.10.1-1                             amd64        low level cryptographic library (public-key cryptos)
ii  libicu76:amd64                   76.1-4                               amd64        International Components for Unicode
ii  libidn2-0:amd64                  2.3.8-2                              amd64        Internationalized domain names (IDNA2008/TR46) library
ii  libinih1:amd64                   59-1                                 amd64        simple .INI file parser
ii  libip4tc2:amd64                  1.8.11-2                             amd64        netfilter libip4tc library
ii  libip6tc2:amd64                  1.8.11-2                             amd64        netfilter libip6tc library
ii  libjansson4:amd64                2.14-2+b3                            amd64        C library for encoding, decoding and manipulating JSON data
ii  libjemalloc2:amd64               5.3.0-3                              amd64        general-purpose scalable concurrent malloc(3) implementation
ii  libjson-c5:amd64                 0.18+ds-1                            amd64        JSON manipulation library - shared library
ii  libk5crypto3:amd64               1.21.3-5+deb13u1                     amd64        MIT Kerberos runtime libraries - Crypto Library
ii  libkeyutils1:amd64               1.6.3-6                              amd64        Linux Key Management Utilities (library)
ii  libklibc:amd64                   2.0.14-1                             amd64        minimal libc subset for use with initramfs
ii  libkmod2:amd64                   34.2-2                               amd64        libkmod shared library
ii  libkrb5-3:amd64                  1.21.3-5+deb13u1                     amd64        MIT Kerberos runtime libraries
ii  libkrb5support0:amd64            1.21.3-5+deb13u1                     amd64        MIT Kerberos runtime libraries - Support library
ii  libksba8:amd64                   1.6.7-2+b1                           amd64        X.509 and CMS support library
ii  liblastlog2-2:amd64              2.41-5                               amd64        lastlog2 database shared library
ii  libldap2:amd64                   2.6.10+dfsg-1                        amd64        OpenLDAP libraries
ii  liblmdb0:amd64                   0.9.31-1+b2                          amd64        Lightning Memory-Mapped Database shared library
ii  liblocale-gettext-perl           1.07-7+b1                            amd64        module using libc functions for internationalization in Perl
ii  liblockfile-bin                  1.17-2                               amd64        support binaries for and cli utilities based on liblockfile
ii  liblsof0                         4.99.4+dfsg-2                        amd64        utility to list open files (shared library)
ii  liblvm2cmd2.03:amd64             2.03.31-2                            amd64        LVM2 command library
ii  liblz4-1:amd64                   1.10.0-4                             amd64        Fast LZ compression algorithm library - runtime
ii  liblzma5:amd64                   5.8.1-1+deb13u1                      amd64        XZ-format compression library
ii  liblzo2-2:amd64                  2.10-3+b1                            amd64        data compression library
ii  libmagic-mgc                     1:5.46-5                             amd64        File type determination library using "magic" numbers (compiled magic file)
ii  libmagic1t64:amd64               1:5.46-5                             amd64        Recognize the type of data in a file using "magic" numbers - library
ii  libmaxminddb0:amd64              1.12.2-1                             amd64        IP geolocation database library
ii  libmd0:amd64                     1.1.0-2+b1                           amd64        message digest functions from BSD systems - shared library
ii  libmnl0:amd64                    1.0.5-3                              amd64        minimalistic Netlink communication library
ii  libmount1:amd64                  2.41-5                               amd64        device mounting library
ii  libncurses6:amd64                6.5+20250216-2                       amd64        shared libraries for terminal handling
ii  libncursesw6:amd64               6.5+20250216-2                       amd64        shared libraries for terminal handling (wide character support)
ii  libnetfilter-conntrack3:amd64    1.1.0-1                              amd64        Netfilter netlink-conntrack library
ii  libnettle8t64:amd64              3.10.1-1                             amd64        low level cryptographic library (symmetric and one-way cryptos)
ii  libnewt0.52:amd64                0.52.25-1                            amd64        Not Erik's Windowing Toolkit - text mode windowing with slang
ii  libnfnetlink0:amd64              1.0.2-3                              amd64        Netfilter netlink library
ii  libnftables1:amd64               1.1.3-1                              amd64        Netfilter nftables high level userspace API library
ii  libnftnl11:amd64                 1.2.9-1                              amd64        Netfilter nftables userspace API library
ii  libnghttp2-14:amd64              1.64.0-1.1+deb13u1                   amd64        library implementing HTTP/2 protocol (shared library)
ii  libnghttp3-9:amd64               1.8.0-1                              amd64        HTTP/3 library with QUIC and QPACK (library)
ii  libnpth0t64:amd64                1.8-3                                amd64        replacement for GNU Pth using system threads
ii  libnss-systemd:amd64             257.13-1~deb13u1                     amd64        nss module providing dynamic user and group name resolution
ii  libp11-kit0:amd64                0.25.5-3                             amd64        library for loading and coordinating access to PKCS#11 modules - runtime
ii  libpam-modules:amd64             1.7.0-5                              amd64        Pluggable Authentication Modules for PAM
ii  libpam-modules-bin               1.7.0-5                              amd64        Pluggable Authentication Modules for PAM - helper binaries
ii  libpam-runtime                   1.7.0-5                              all          Runtime support for the PAM library
ii  libpam-systemd:amd64             257.13-1~deb13u1                     amd64        system and service manager - PAM module
ii  libpam-wtmpdb:amd64              0.73.0-3+deb13u1                     amd64        wtmp database PAM module
ii  libpam0g:amd64                   1.7.0-5                              amd64        Pluggable Authentication Modules library
ii  libpci3:amd64                    1:3.13.0-2                           amd64        PCI utilities (shared library)
ii  libpcre2-8-0:amd64               10.46-1~deb13u1                      amd64        New Perl Compatible Regular Expression Library- 8 bit runtime files
ii  libperl5.40:amd64                5.40.1-6                             amd64        shared Perl library
ii  libpipeline1:amd64               1.5.8-1                              amd64        Unix process pipeline manipulation library
ii  libpng16-16t64:amd64             1.6.48-1+deb13u5                     amd64        PNG library - runtime (version 1.6)
ii  libpopt0:amd64                   1.19+dfsg-2                          amd64        lib for parsing cmdline parameters
ii  libproc2-0:amd64                 2:4.0.4-9                            amd64        library for accessing process information from /proc
ii  libprotobuf-c1:amd64             1.5.1-1                              amd64        Protocol Buffers C shared library (protobuf-c)
ii  libpsl5t64:amd64                 0.21.2-1.1+b1                        amd64        Library for Public Suffix List (shared libraries)
ii  libpython3-stdlib:amd64          3.13.5-1                             amd64        interactive high-level object-oriented language (default python3 version)
ii  libpython3.13-minimal:amd64      3.13.5-2+deb13u4                     amd64        Minimal subset of the Python language (version 3.13)
ii  libpython3.13-stdlib:amd64       3.13.5-2+deb13u4                     amd64        Interactive high-level object-oriented language (standard library, version 3.13)
ii  libreadline8t64:amd64            8.2-6                                amd64        GNU readline and history libraries, run-time libraries
ii  libreiserfscore0t64              1:3.6.27-9                           amd64        ReiserFS core library
ii  librtmp1:amd64                   2.4+20151223.gitfa8646d.1-2+b5       amd64        toolkit for RTMP streams (shared library)
ii  libsasl2-2:amd64                 2.1.28+dfsg1-9                       amd64        Cyrus SASL - authentication abstraction library
ii  libsasl2-modules-db:amd64        2.1.28+dfsg1-9                       amd64        Cyrus SASL - pluggable authentication modules (DB)
ii  libseccomp2:amd64                2.6.0-2                              amd64        high level interface to Linux seccomp filter
ii  libselinux1:amd64                3.8.1-1                              amd64        SELinux runtime shared libraries
ii  libsemanage-common               3.8.1-1                              all          Common files for SELinux policy management libraries
ii  libsemanage2:amd64               3.8.1-1                              amd64        SELinux policy management library
ii  libsepol2:amd64                  3.8.1-1                              amd64        SELinux library for manipulating binary security policies
ii  libsigc++-2.0-0v5:amd64          2.12.1-3                             amd64        type-safe Signal Framework for C++ - runtime
ii  libslang2:amd64                  2.3.3-5+b2                           amd64        S-Lang programming library - runtime version
ii  libsmartcols1:amd64              2.41-5                               amd64        smart column output alignment library
ii  libsodium23:amd64                1.0.18-1+deb13u1                     amd64        Network communication, cryptography and signaturing library
ii  libsqlite3-0:amd64               3.46.1-7+deb13u2                     amd64        SQLite 3 shared library
ii  libss2:amd64                     1.47.2-3+b11                         amd64        command-line interface parsing library
ii  libssh2-1t64:amd64               1.11.1-1+deb13u1                     amd64        SSH2 client-side library
ii  libssl3t64:amd64                 3.5.6-1~deb13u2                      amd64        Secure Sockets Layer toolkit - shared libraries
ii  libstdc++6:amd64                 14.2.0-19                            amd64        GNU Standard C++ Library v3
ii  libsystemd-shared:amd64          257.13-1~deb13u1                     amd64        systemd shared private library
ii  libsystemd0:amd64                257.13-1~deb13u1                     amd64        systemd utility library
ii  libtasn1-6:amd64                 4.20.0-2+deb13u1                     amd64        Manage ASN.1 structures (runtime)
ii  libtext-charwidth-perl:amd64     0.04-11+b4                           amd64        get display widths of characters on the terminal
ii  libtext-iconv-perl:amd64         1.7-8+b4                             amd64        module to convert between character sets in Perl
ii  libtext-wrapi18n-perl            0.06-10                              all          internationalized substitute of Text::Wrap
ii  libtinfo6:amd64                  6.5+20250216-2                       amd64        shared low-level terminfo library for terminal handling
ii  libtirpc-common                  1.3.6+ds-1                           all          transport-independent RPC library - common files
ii  libtirpc3t64:amd64               1.3.6+ds-1                           amd64        transport-independent RPC library
ii  libuchardet0:amd64               0.0.8-1+b2                           amd64        universal charset detection library - shared library
ii  libudev1:amd64                   257.13-1~deb13u1                     amd64        libudev shared library
ii  libunistring5:amd64              1.3-2                                amd64        Unicode string library for C
ii  liburcu8t64:amd64                0.15.2-2                             amd64        userspace RCU (read-copy-update) library
ii  libuuid1:amd64                   2.41-5                               amd64        Universally Unique ID library
ii  libuv1t64:amd64                  1.50.0-2                             amd64        asynchronous event notification library - runtime library
ii  libwrap0:amd64                   7.6.q-36                             amd64        Wietse Venema's TCP wrappers library
ii  libwtmpdb0:amd64                 0.73.0-3+deb13u1                     amd64        wtmp database shared library
ii  libxapian30:amd64                1.4.29-3                             amd64        Search engine library
ii  libxml2:amd64                    2.12.7+dfsg+really2.9.14-2.1+deb13u3 amd64        GNOME XML library
ii  libxtables12:amd64               1.8.11-2                             amd64        netfilter xtables library
ii  libxxhash0:amd64                 0.8.3-2                              amd64        shared library for xxhash
ii  libzstd1:amd64                   1.5.7+dfsg-1                         amd64        fast lossless compression algorithm
ii  linux-base                       4.12.1                               all          Linux image base package
ii  linux-image-6.12.100+deb13-amd64 6.12.100-1                           amd64        Linux 6.12 for 64-bit PCs (signed)
ii  linux-image-6.12.94+deb13-amd64  6.12.94-1                            amd64        Linux 6.12 for 64-bit PCs (signed)
ii  linux-image-amd64                6.12.100-1                           amd64        Linux for 64-bit PCs (meta-package)
ii  linux-sysctl-defaults            4.12.1                               all          default sysctl configuration for Linux
ii  locales                          2.41-12+deb13u3                      all          GNU C Library: National Language (locale) data [support]
ii  login                            1:4.16.0-2+really2.41-5              amd64        system login tools
ii  login.defs                       1:4.17.4-2                           all          system user management configuration
ii  logrotate                        3.22.0-1                             amd64        Log rotation utility
ii  logsave                          1.47.2-3+b11                         amd64        save the output of a command in a log file
ii  lsb-release                      12.1-1                               all          Linux Standard Base version reporting utility (minimal implementation)
ii  lsof                             4.99.4+dfsg-2                        amd64        utility to list open files
ii  lvm2                             2.03.31-2                            amd64        Linux Logical Volume Manager
ii  man-db                           2.13.1-1                             amd64        tools for reading manual pages
ii  manpages                         6.9.1-1                              all          Manual pages about using a GNU/Linux system
ii  mawk                             1.3.4.20250131-1                     amd64        Pattern scanning and text processing language
ii  mdadm                            4.4-11                               amd64        tool for managing Linux MD devices (software RAID)
ii  media-types                      13.0.0                               all          List of standard media types and their usual file extension
ii  mount                            2.41-5                               amd64        tools for mounting and manipulating filesystems
ii  mtr-tiny                         0.95-1.1+b1                          amd64        Full screen ncurses traceroute tool
ii  nano                             8.4-1+deb13u1                        amd64        small, friendly text editor inspired by Pico
ii  ncurses-base                     6.5+20250216-2                       all          basic terminal type definitions
ii  ncurses-bin                      6.5+20250216-2                       amd64        terminal-related programs and man pages
ii  ncurses-term                     6.5+20250216-2                       all          additional terminal type definitions
ii  net-tools                        2.10-1.3                             amd64        NET-3 networking toolkit
ii  netbase                          6.5                                  all          Basic TCP/IP networking system
ii  netcat-traditional               1.10-50                              amd64        TCP/IP swiss army knife
ii  nftables                         1.1.3-1                              amd64        Program to control packet filtering rules by Netfilter project
ii  openssh-client                   1:10.0p1-7+deb13u4                   amd64        secure shell (SSH) client, for secure access to remote machines
ii  openssh-server                   1:10.0p1-7+deb13u4                   amd64        secure shell (SSH) server, for secure access from remote machines
ii  openssh-sftp-server              1:10.0p1-7+deb13u4                   amd64        secure shell (SSH) sftp server module, for SFTP access from remote machines
ii  openssl                          3.5.6-1~deb13u2                      amd64        Secure Sockets Layer toolkit - cryptographic utility
ii  openssl-provider-legacy          3.5.6-1~deb13u2                      amd64        Secure Sockets Layer toolkit - cryptographic utility
ii  os-prober                        1.83                                 amd64        utility to detect other OSes on a set of drives
ii  passwd                           1:4.17.4-2                           amd64        change and administer password and group data
ii  patch                            2.8-2                                amd64        Apply a diff file to an original
ii  pci.ids                          0.0~2025.06.09-1                     all          PCI ID Repository
ii  pciutils                         1:3.13.0-2                           amd64        PCI utilities
ii  perl                             5.40.1-6                             amd64        Larry Wall's Practical Extraction and Report Language
ii  perl-base                        5.40.1-6                             amd64        minimal Perl system
ii  perl-modules-5.40                5.40.1-6                             all          Core Perl modules
ii  pinentry-curses                  1.3.1-2                              amd64        curses-based PIN or pass-phrase entry dialog for GnuPG
ii  procps                           2:4.0.4-9                            amd64        /proc file system utilities
ii  publicsuffix                     20250328.1952-0.1                    all          accurate, machine-readable list of domain name suffixes
ii  python-apt-common                3.0.0                                all          Python interface to libapt-pkg (locales)
ii  python3                          3.13.5-1                             amd64        interactive high-level object-oriented language (default python3 version)
ii  python3-apt                      3.0.0                                amd64        Python 3 interface to libapt-pkg
ii  python3-certifi                  2025.1.31+ds-1                       all          root certificates for validating SSL certs and verifying TLS hosts (python3)
ii  python3-chardet                  5.2.0+dfsg-2                         all          Universal Character Encoding Detector (Python3)
ii  python3-charset-normalizer       3.4.2-1                              amd64        charset, encoding and language detection (Python 3)
ii  python3-dbus                     1.4.0-1                              amd64        simple interprocess messaging system (Python 3 interface)
ii  python3-debian                   1.0.1                                all          Python 3 modules to work with Debian-related data formats
ii  python3-debianbts                4.1.1                                all          Python interface to Debian's Bug Tracking System
ii  python3-distro                   1.9.0-1                              all          Linux OS platform information API
ii  python3-idna                     3.10-1+deb13u1                       all          Python IDNA2008 (RFC 5891) handling (Python 3)
ii  python3-minimal                  3.13.5-1                             amd64        minimal subset of the Python language (default python3 version)
ii  python3-reportbug                13.2.0                               all          Python modules for interacting with bug tracking systems
ii  python3-requests                 2.32.3+dfsg-5+deb13u1                all          elegant and simple HTTP library for Python3, built for human beings
ii  python3-urllib3                  2.3.0-3+deb13u2                      all          HTTP library with thread-safe connection pooling for Python3
ii  python3.13                       3.13.5-2+deb13u4                     amd64        Interactive high-level object-oriented language (version 3.13)
ii  python3.13-minimal               3.13.5-2+deb13u4                     amd64        Minimal subset of the Python language (version 3.13)
ii  readline-common                  8.2-6                                all          GNU readline and history libraries, common files
ii  reportbug                        13.2.0                               all          reports bugs in the Debian distribution
ii  rsync                            3.4.1+ds1-5+deb13u4                  amd64        fast, versatile, remote (and local) file-copying tool
ii  runit-helper                     2.16.4                               all          dh-runit implementation detail
ii  sed                              4.9-2+deb13u1                        amd64        GNU stream editor for filtering/transforming text
ii  sensible-utils                   0.0.25                               all          Utilities for sensible alternative selection
ii  shared-mime-info                 2.4-5+b2                             amd64        FreeDesktop.org shared MIME database and spec
ii  sqlite3                          3.46.1-7+deb13u2                     amd64        Command line interface for SQLite 3
ii  sqv                              1.3.0-3+b2                           amd64        OpenPGP signature verification program from Sequoia
ii  ssh-import-id                    5.10-3                               all          securely retrieve an SSH public key and install it locally
ii  sudo                             1.9.16p2-3+deb13u2                   amd64        Provide limited super user privileges to specific users
ii  systemd                          257.13-1~deb13u1                     amd64        system and service manager
ii  systemd-sysv                     257.13-1~deb13u1                     amd64        system and service manager - SysV compatibility symlinks
ii  systemd-timesyncd                257.13-1~deb13u1                     amd64        minimalistic service to synchronize local time with NTP servers
ii  sysvinit-utils                   3.14-4                               amd64        System-V-like utilities
ii  tar                              1.35+dfsg-3.1                        amd64        GNU version of the tar archiving utility
ii  task-english                     3.81                                 all          General English environment
ii  task-ssh-server                  3.81                                 all          SSH server
ii  tasksel                          3.81                                 all          tool for selecting tasks for installation on Debian systems
ii  tasksel-data                     3.81                                 all          official tasks used for installation of Debian systems
ii  traceroute                       1:2.1.6-1                            amd64        Traces the route taken by packets over an IPv4/IPv6 network
ii  tzdata                           2026b-0+deb13u1                      all          time zone and daylight-saving time data
ii  ucf                              3.0052                               all          Update Configuration File(s): preserve user changes to config files
ii  udev                             257.13-1~deb13u1                     amd64        /dev/ and hotplug management daemon
ii  util-linux                       2.41-5                               amd64        miscellaneous system utilities
ii  util-linux-extra                 2.41-5                               amd64        interactive login tools
ii  util-linux-locales               2.41-5                               all          locales files for util-linux
ii  vim                              2:9.1.1230-2                         amd64        Vi IMproved - enhanced vi editor
ii  vim-common                       2:9.1.1230-2                         all          Vi IMproved - Common files
ii  vim-runtime                      2:9.1.1230-2                         all          Vi IMproved - Runtime files
ii  vim-tiny                         2:9.1.1230-2                         amd64        Vi IMproved - enhanced vi editor - compact version
ii  wget                             1.25.0-2                             amd64        retrieves files from the web
ii  whiptail                         0.52.25-1                            amd64        Displays user-friendly dialog boxes from shell scripts
ii  wtmpdb                           0.73.0-3+deb13u1                     amd64        utility to display login/logout/reboot information
ii  xdg-user-dirs                    0.18-2                               amd64        tool to manage well known user directories
ii  xfsprogs                         6.13.0-2+b1                          amd64        Utilities for managing the XFS filesystem
ii  xkb-data                         2.42-1                               all          X Keyboard Extension (XKB) configuration data
ii  xz-utils                         5.8.1-1+deb13u1                      amd64        XZ-format compression utilities
ii  zlib1g:amd64                     1:1.3.dfsg+really1.3.1-1+b1          amd64        compression library - runtime
ii  zstd                             1.5.7+dfsg-1                         amd64        fast lossless compression algorithm -- CLI tool
```

### /root/dpkg-after-v046.txt

```text
Desired=Unknown/Install/Remove/Purge/Hold
| Status=Not/Inst/Conf-files/Unpacked/halF-conf/Half-inst/trig-aWait/Trig-pend
|/ Err?=(none)/Reinst-required (Status,Err: uppercase=bad)
||/ Name                             Version                              Architecture Description
+++-================================-====================================-============-================================================================================
ii  acl                              2.3.2-2+b1                           amd64        access control list - utilities
ii  acpid                            1:2.0.34-1+b1                        amd64        Advanced Configuration and Power Interface event daemon
ii  adduser                          3.152                                all          add and remove users and groups
ii  amd64-microcode                  3.20250311.1                         amd64        Platform firmware and microcode for AMD CPUs and SoCs
ii  apt                              3.0.3                                amd64        commandline package manager
ii  apt-utils                        3.0.3                                amd64        package management related utility programs
ii  aptitude                         0.8.13-7                             amd64        terminal-based package manager
ii  aptitude-common                  0.8.13-7                             all          architecture independent files for the aptitude package manager
ii  at                               3.2.5-2.2                            amd64        Delayed job execution and batch processing
ii  base-files                       13.8+deb13u6                         amd64        Debian base system miscellaneous files
ii  base-passwd                      3.6.7                                amd64        Debian base system master password and group files
ii  bash                             5.2.37-2+b9                          amd64        GNU Bourne Again SHell
ii  bash-completion                  1:2.16.0-7                           all          programmable completion for the bash shell
ii  bind9-dnsutils                   1:9.20.26-1~deb13u1                  amd64        Clients provided with BIND 9
ii  bind9-host                       1:9.20.26-1~deb13u1                  amd64        DNS Lookup Utility
ii  bind9-libs:amd64                 1:9.20.26-1~deb13u1                  amd64        Shared Libraries used by BIND 9
ii  bsdextrautils                    2.41-5                               amd64        extra utilities from 4.4BSD-Lite
ii  bsdutils                         1:2.41-5                             amd64        basic utilities from 4.4BSD-Lite
ii  btrfs-progs                      6.14-1                               amd64        Checksumming Copy on Write Filesystem utilities
ii  busybox                          1:1.37.0-6+b8                        amd64        Tiny utilities for small and embedded systems
ii  bzip2                            1.0.8-6                              amd64        high-quality block-sorting file compressor - utilities
ii  ca-certificates                  20250419                             all          Common CA certificates
ii  console-setup                    1.242~deb13u1                        all          console font and keymap setup program
ii  console-setup-linux              1.242~deb13u1                        all          Linux specific part of console-setup
ii  containerd.io                    2.3.3-1~debian.13~trixie             amd64        An open and reliable container runtime
ii  coreutils                        9.7-3                                amd64        GNU core utilities
ii  cpio                             2.15+dfsg-2                          amd64        GNU cpio -- a program to manage archives of files
ii  cron                             3.0pl1-197                           amd64        process scheduling daemon
ii  cron-daemon-common               3.0pl1-197                           all          process scheduling daemon's configuration files
ii  cryptsetup                       2:2.7.5-2                            amd64        disk encryption support - startup scripts
ii  cryptsetup-bin                   2:2.7.5-2                            amd64        disk encryption support - command line tools
ii  cryptsetup-initramfs             2:2.7.5-2                            all          disk encryption support - initramfs integration
ii  curl                             8.14.1-2+deb13u4                     amd64        command line tool for transferring data with URL syntax
ii  dash                             0.5.12-12                            amd64        POSIX-compliant shell
ii  dbus                             1.16.2-2                             amd64        simple interprocess messaging system (system message bus)
ii  dbus-bin                         1.16.2-2                             amd64        simple interprocess messaging system (command line utilities)
ii  dbus-daemon                      1.16.2-2                             amd64        simple interprocess messaging system (reference message bus)
ii  dbus-session-bus-common          1.16.2-2                             all          simple interprocess messaging system (session bus configuration)
ii  dbus-system-bus-common           1.16.2-2                             all          simple interprocess messaging system (system bus configuration)
ii  dbus-user-session                1.16.2-2                             amd64        simple interprocess messaging system (systemd --user integration)
ii  debconf                          1.5.91                               all          Debian configuration management system
ii  debconf-i18n                     1.5.91                               all          full internationalization support for debconf
ii  debian-archive-keyring           2025.1                               all          OpenPGP archive certificates of the Debian archive
ii  debianutils                      5.23.2                               amd64        Miscellaneous utilities specific to Debian
ii  dhcpcd-base                      1:10.1.0-11+deb13u3                  amd64        DHCPv4 and DHCPv6 dual-stack client (binaries and exit hooks)
ii  diffutils                        1:3.10-4                             amd64        File comparison utilities
ii  dirmngr                          2.4.7-21+deb13u1+b4                  amd64        GNU privacy guard - network certificate management service
ii  distro-info-data                 0.66+deb13u2                         all          information about the distributions' releases (data files)
ii  dmeventd                         2:1.02.205-2                         amd64        Linux Kernel Device Mapper event daemon
ii  dmidecode                        3.6-2                                amd64        SMBIOS/DMI table decoder
ii  dmsetup                          2:1.02.205-2                         amd64        Linux Kernel Device Mapper userspace library
ii  docker-buildx-plugin             0.36.1-1~debian.13~trixie            amd64        Docker Buildx plugin extends build capabilities with BuildKit.
ii  docker-ce                        5:29.7.2-1~debian.13~trixie          amd64        Docker: the open-source application container engine
ii  docker-ce-cli                    5:29.7.2-1~debian.13~trixie          amd64        Docker CLI: the open-source application container engine
ii  docker-ce-rootless-extras        5:29.7.2-1~debian.13~trixie          amd64        Rootless support for Docker.
ii  docker-compose-plugin            5.5.0-1~debian.13~trixie             amd64        Docker Compose (V2) plugin for the Docker CLI.
ii  docker-model-plugin              1.2.6-1~debian.13~trixie             amd64        Docker Model Runner plugin for the Docker CLI.
ii  dosfstools                       4.2-1.2                              amd64        utilities for making and checking MS-DOS FAT filesystems
ii  dpkg                             1.22.22                              amd64        Debian package management system
ii  dracut-install                   106-6                                amd64        dracut is an event driven initramfs infrastructure (dracut-install)
ii  e2fsprogs                        1.47.2-3+b11                         amd64        ext2/ext3/ext4 file system utilities
ii  efibootmgr                       18-2                                 amd64        Interact with the EFI Boot Manager
ii  ethtool                          1:6.14.2-1                           amd64        display or change Ethernet device settings
ii  fdisk                            2.41-5                               amd64        collection of partitioning utilities
ii  file                             1:5.46-5                             amd64        Recognize the type of data in a file using "magic" numbers
ii  findutils                        4.10.0-3                             amd64        utilities for finding files--find, xargs
ii  firmware-bnx2x                   20250410-2                           all          Binary firmware for Broadcom NetXtreme II 10Gb
ii  gcc-14-base:amd64                14.2.0-19                            amd64        GCC, the GNU Compiler Collection (base package)
ii  gdisk                            1.0.10-2                             amd64        GPT fdisk text-mode partitioning tool
ii  gettext-base                     0.23.1-2                             amd64        GNU Internationalization utilities for the base system
ii  gnupg                            2.4.7-21+deb13u1                     all          GNU privacy guard - a free PGP replacement
ii  gnupg-l10n                       2.4.7-21+deb13u1                     all          GNU privacy guard - localization files
ii  gpg                              2.4.7-21+deb13u1+b4                  amd64        GNU Privacy Guard -- minimalist public key operations
ii  gpg-agent                        2.4.7-21+deb13u1+b4                  amd64        GNU privacy guard - cryptographic agent
ii  gpgconf                          2.4.7-21+deb13u1+b4                  amd64        GNU privacy guard - core configuration utilities
ii  gpgsm                            2.4.7-21+deb13u1+b4                  amd64        GNU privacy guard - S/MIME version
ii  grep                             3.11-4                               amd64        GNU grep, egrep and fgrep
ii  groff-base                       1.23.0-9                             amd64        GNU troff text-formatting system (base system components)
ii  grub-common                      2.12-9+deb13u2                       amd64        GRand Unified Bootloader (common files)
ii  grub-efi-amd64                   2.12-9+deb13u2                       amd64        GRand Unified Bootloader, version 2 (EFI-AMD64 version)
ii  grub-efi-amd64-bin               2.12-9+deb13u2                       amd64        GRand Unified Bootloader, version 2 (EFI-AMD64 modules)
ii  grub-efi-amd64-unsigned          2.12-9+deb13u2                       amd64        GRand Unified Bootloader, version 2 (EFI-AMD64 images)
ii  grub-pc-bin                      2.12-9+deb13u2                       amd64        GRand Unified Bootloader, version 2 (PC/BIOS modules)
ii  grub2-common                     2.12-9+deb13u2                       amd64        GRand Unified Bootloader (common files for version 2)
ii  gzip                             1.13-1                               amd64        GNU compression utilities
ii  hostname                         3.25                                 amd64        utility to set/show the host name or domain name
ii  htop                             3.4.1-5                              amd64        interactive processes viewer
ii  ifupdown                         0.8.44+deb13u1                       amd64        high level tools to configure network interfaces
ii  inetutils-telnet                 2:2.6-3+deb13u3                      amd64        telnet client
ii  init                             1.69~deb13u1                         amd64        metapackage ensuring an init system is installed
ii  init-system-helpers              1.69~deb13u1                         all          helper tools for all init systems
ii  initramfs-tools                  0.148.4                              all          generic modular initramfs generator (automation)
ii  initramfs-tools-bin              0.148.4                              amd64        generic modular initramfs generator (binary tools)
ii  initramfs-tools-core             0.148.4                              all          generic modular initramfs generator (core tools)
ii  intel-microcode                  3.20251111.1~deb13u1                 amd64        Processor microcode firmware for Intel CPUs
ii  iproute2                         6.15.0-1                             amd64        networking and traffic control tools
ii  iptables                         1.8.11-2                             amd64        administration tools for packet filtering and NAT
ii  iputils-ping                     3:20240905-3                         amd64        Tools to test the reachability of network hosts
ii  iucode-tool                      2.3.1-3                              amd64        Intel processor microcode tool
ii  kbd                              2.7.1-2                              amd64        Linux console font and keytable utilities
ii  keyboard-configuration           1.242~deb13u1                        all          system-wide keyboard preferences
ii  klibc-utils                      2.0.14-1                             amd64        small utilities built with klibc for early boot
ii  kmod                             34.2-2                               amd64        tools for managing Linux kernel modules
ii  laptop-detect                    0.16+nmu1                            all          system chassis type checker
ii  less                             668-1                                amd64        pager program similar to more
ii  libacl1:amd64                    2.3.2-2+b1                           amd64        access control list - shared library
ii  libaio1t64:amd64                 0.3.113-8+b1                         amd64        Linux kernel AIO access library - shared library
ii  libapparmor1:amd64               4.1.0-1                              amd64        changehat AppArmor library
ii  libapt-pkg7.0:amd64              3.0.3                                amd64        package management runtime library
ii  libassuan9:amd64                 3.0.2-2                              amd64        IPC library for the GnuPG components
ii  libatomic1:amd64                 14.2.0-19                            amd64        support library providing __atomic built-in functions
ii  libattr1:amd64                   1:2.5.2-3                            amd64        extended attribute handling - shared library
ii  libaudit-common                  1:4.0.2-2                            all          Dynamic library for security auditing - common files
ii  libaudit1:amd64                  1:4.0.2-2+b2                         amd64        Dynamic library for security auditing
ii  libblkid1:amd64                  2.41-5                               amd64        block device ID library
ii  libboost-iostreams1.83.0:amd64   1.83.0-4.2                           amd64        Boost.Iostreams Library
ii  libbpf1:amd64                    1:1.5.0-3                            amd64        eBPF helper library (shared library)
ii  libbrotli1:amd64                 1.1.0-2+b7                           amd64        library implementing brotli encoder and decoder (shared libraries)
ii  libbsd0:amd64                    0.12.2-2                             amd64        utility functions from BSD systems - shared library
ii  libbz2-1.0:amd64                 1.0.8-6                              amd64        high-quality block-sorting file compressor library - runtime
ii  libc-bin                         2.41-12+deb13u3                      amd64        GNU C Library: Binaries
ii  libc-l10n                        2.41-12+deb13u3                      all          GNU C Library: localization files
ii  libc6:amd64                      2.41-12+deb13u3                      amd64        GNU C Library: Shared libraries
ii  libcap-ng0:amd64                 0.8.5-4+b1                           amd64        alternate POSIX capabilities library
ii  libcap2:amd64                    1:2.75-10+deb13u1+b1                 amd64        POSIX 1003.1e capabilities (library)
ii  libcap2-bin                      1:2.75-10+deb13u1+b1                 amd64        POSIX 1003.1e capabilities (utilities)
ii  libcbor0.10:amd64                0.10.2-2                             amd64        library for parsing and generating CBOR (RFC 7049)
ii  libcom-err2:amd64                1.47.2-3+b11                         amd64        common error description library
ii  libcrypt1:amd64                  1:4.4.38-1                           amd64        libcrypt shared library
ii  libcryptsetup12:amd64            2:2.7.5-2                            amd64        disk encryption support - shared library
ii  libcurl4t64:amd64                8.14.1-2+deb13u4                     amd64        easy-to-use client-side URL transfer library (OpenSSL flavour)
ii  libcwidget4:amd64                0.5.18-6+b2                          amd64        high-level terminal interface library for C++ (runtime files)
ii  libdb5.3t64:amd64                5.3.28+dfsg2-9                       amd64        Berkeley v5.3 Database Libraries [runtime]
ii  libdbus-1-3:amd64                1.16.2-2                             amd64        simple interprocess messaging system (library)
ii  libdebconfclient0:amd64          0.280                                amd64        Debian Configuration Management System (C-implementation library)
ii  libdevmapper-event1.02.1:amd64   2:1.02.205-2                         amd64        Linux Kernel Device Mapper event support library
ii  libdevmapper1.02.1:amd64         2:1.02.205-2                         amd64        Linux Kernel Device Mapper userspace library
ii  libedit2:amd64                   3.1-20250104-1                       amd64        BSD editline and history libraries
ii  libefiboot1t64:amd64             38-3.1+b1                            amd64        Library to manage UEFI variables
ii  libefivar1t64:amd64              38-3.1+b1                            amd64        Library to manage UEFI variables
ii  libelf1t64:amd64                 0.192-4                              amd64        library to read and write ELF files
ii  libexpat1:amd64                  2.8.2-1~deb13u1                      amd64        XML parsing C library - runtime library
ii  libext2fs2t64:amd64              1.47.2-3+b11                         amd64        ext2/ext3/ext4 file system libraries
ii  libfdisk1:amd64                  2.41-5                               amd64        fdisk partitioning library
ii  libffi8:amd64                    3.4.8-2                              amd64        Foreign Function Interface library runtime
ii  libfido2-1:amd64                 1.15.0-1+b1                          amd64        library for generating and verifying FIDO 2.0 objects
ii  libfreetype6:amd64               2.13.3+dfsg-1+deb13u1                amd64        FreeType 2 font engine, shared library files
ii  libfstrm0:amd64                  0.6.1-1+b3                           amd64        Frame Streams (fstrm) library
ii  libfuse3-4:amd64                 3.17.2-3                             amd64        Filesystem in Userspace (library) (3.x version)
ii  libgcc-s1:amd64                  14.2.0-19                            amd64        GCC support library
ii  libgcrypt20:amd64                1.11.0-7+deb13u1                     amd64        LGPL Crypto library - runtime library
ii  libgdbm-compat4t64:amd64         1.24-2                               amd64        GNU dbm database routines (legacy support runtime version) 
ii  libgdbm6t64:amd64                1.24-2                               amd64        GNU dbm database routines (runtime version) 
ii  libglib2.0-0t64:amd64            2.84.4-3~deb13u3                     amd64        GLib library of C routines
ii  libglib2.0-data                  2.84.4-3~deb13u3                     all          Common files for GLib library
ii  libgmp10:amd64                   2:6.3.0+dfsg-3                       amd64        Multiprecision arithmetic library
ii  libgnutls30t64:amd64             3.8.9-3+deb13u4                      amd64        GNU TLS library - main runtime library
ii  libgpg-error0:amd64              1.51-4                               amd64        GnuPG development runtime library
ii  libgpm2:amd64                    1.20.7-11+b2                         amd64        General Purpose Mouse - shared library
ii  libgssapi-krb5-2:amd64           1.21.3-5+deb13u1                     amd64        MIT Kerberos runtime libraries - krb5 GSS-API Mechanism
ii  libhogweed6t64:amd64             3.10.1-1                             amd64        low level cryptographic library (public-key cryptos)
ii  libicu76:amd64                   76.1-4                               amd64        International Components for Unicode
ii  libidn2-0:amd64                  2.3.8-2                              amd64        Internationalized domain names (IDNA2008/TR46) library
ii  libinih1:amd64                   59-1                                 amd64        simple .INI file parser
ii  libip4tc2:amd64                  1.8.11-2                             amd64        netfilter libip4tc library
ii  libip6tc2:amd64                  1.8.11-2                             amd64        netfilter libip6tc library
ii  libjansson4:amd64                2.14-2+b3                            amd64        C library for encoding, decoding and manipulating JSON data
ii  libjemalloc2:amd64               5.3.0-3                              amd64        general-purpose scalable concurrent malloc(3) implementation
ii  libjson-c5:amd64                 0.18+ds-1                            amd64        JSON manipulation library - shared library
ii  libk5crypto3:amd64               1.21.3-5+deb13u1                     amd64        MIT Kerberos runtime libraries - Crypto Library
ii  libkeyutils1:amd64               1.6.3-6                              amd64        Linux Key Management Utilities (library)
ii  libklibc:amd64                   2.0.14-1                             amd64        minimal libc subset for use with initramfs
ii  libkmod2:amd64                   34.2-2                               amd64        libkmod shared library
ii  libkrb5-3:amd64                  1.21.3-5+deb13u1                     amd64        MIT Kerberos runtime libraries
ii  libkrb5support0:amd64            1.21.3-5+deb13u1                     amd64        MIT Kerberos runtime libraries - Support library
ii  libksba8:amd64                   1.6.7-2+b1                           amd64        X.509 and CMS support library
ii  liblastlog2-2:amd64              2.41-5                               amd64        lastlog2 database shared library
ii  libldap2:amd64                   2.6.10+dfsg-1                        amd64        OpenLDAP libraries
ii  liblmdb0:amd64                   0.9.31-1+b2                          amd64        Lightning Memory-Mapped Database shared library
ii  liblocale-gettext-perl           1.07-7+b1                            amd64        module using libc functions for internationalization in Perl
ii  liblockfile-bin                  1.17-2                               amd64        support binaries for and cli utilities based on liblockfile
ii  liblsof0                         4.99.4+dfsg-2                        amd64        utility to list open files (shared library)
ii  liblvm2cmd2.03:amd64             2.03.31-2                            amd64        LVM2 command library
ii  liblz4-1:amd64                   1.10.0-4                             amd64        Fast LZ compression algorithm library - runtime
ii  liblzma5:amd64                   5.8.1-1+deb13u1                      amd64        XZ-format compression library
ii  liblzo2-2:amd64                  2.10-3+b1                            amd64        data compression library
ii  libmagic-mgc                     1:5.46-5                             amd64        File type determination library using "magic" numbers (compiled magic file)
ii  libmagic1t64:amd64               1:5.46-5                             amd64        Recognize the type of data in a file using "magic" numbers - library
ii  libmaxminddb0:amd64              1.12.2-1                             amd64        IP geolocation database library
ii  libmd0:amd64                     1.1.0-2+b1                           amd64        message digest functions from BSD systems - shared library
ii  libmnl0:amd64                    1.0.5-3                              amd64        minimalistic Netlink communication library
ii  libmount1:amd64                  2.41-5                               amd64        device mounting library
ii  libncurses6:amd64                6.5+20250216-2                       amd64        shared libraries for terminal handling
ii  libncursesw6:amd64               6.5+20250216-2                       amd64        shared libraries for terminal handling (wide character support)
ii  libnetfilter-conntrack3:amd64    1.1.0-1                              amd64        Netfilter netlink-conntrack library
ii  libnettle8t64:amd64              3.10.1-1                             amd64        low level cryptographic library (symmetric and one-way cryptos)
ii  libnewt0.52:amd64                0.52.25-1                            amd64        Not Erik's Windowing Toolkit - text mode windowing with slang
ii  libnfnetlink0:amd64              1.0.2-3                              amd64        Netfilter netlink library
ii  libnftables1:amd64               1.1.3-1                              amd64        Netfilter nftables high level userspace API library
ii  libnftnl11:amd64                 1.2.9-1                              amd64        Netfilter nftables userspace API library
ii  libnghttp2-14:amd64              1.64.0-1.1+deb13u1                   amd64        library implementing HTTP/2 protocol (shared library)
ii  libnghttp3-9:amd64               1.8.0-1                              amd64        HTTP/3 library with QUIC and QPACK (library)
ii  libnpth0t64:amd64                1.8-3                                amd64        replacement for GNU Pth using system threads
ii  libnss-systemd:amd64             257.13-1~deb13u1                     amd64        nss module providing dynamic user and group name resolution
ii  libp11-kit0:amd64                0.25.5-3                             amd64        library for loading and coordinating access to PKCS#11 modules - runtime
ii  libpam-modules:amd64             1.7.0-5                              amd64        Pluggable Authentication Modules for PAM
ii  libpam-modules-bin               1.7.0-5                              amd64        Pluggable Authentication Modules for PAM - helper binaries
ii  libpam-runtime                   1.7.0-5                              all          Runtime support for the PAM library
ii  libpam-systemd:amd64             257.13-1~deb13u1                     amd64        system and service manager - PAM module
ii  libpam-wtmpdb:amd64              0.73.0-3+deb13u1                     amd64        wtmp database PAM module
ii  libpam0g:amd64                   1.7.0-5                              amd64        Pluggable Authentication Modules library
ii  libpci3:amd64                    1:3.13.0-2                           amd64        PCI utilities (shared library)
ii  libpcre2-8-0:amd64               10.46-1~deb13u1                      amd64        New Perl Compatible Regular Expression Library- 8 bit runtime files
ii  libperl5.40:amd64                5.40.1-6                             amd64        shared Perl library
ii  libpipeline1:amd64               1.5.8-1                              amd64        Unix process pipeline manipulation library
ii  libpng16-16t64:amd64             1.6.48-1+deb13u5                     amd64        PNG library - runtime (version 1.6)
ii  libpopt0:amd64                   1.19+dfsg-2                          amd64        lib for parsing cmdline parameters
ii  libproc2-0:amd64                 2:4.0.4-9                            amd64        library for accessing process information from /proc
ii  libprotobuf-c1:amd64             1.5.1-1                              amd64        Protocol Buffers C shared library (protobuf-c)
ii  libpsl5t64:amd64                 0.21.2-1.1+b1                        amd64        Library for Public Suffix List (shared libraries)
ii  libpython3-stdlib:amd64          3.13.5-1                             amd64        interactive high-level object-oriented language (default python3 version)
ii  libpython3.13-minimal:amd64      3.13.5-2+deb13u4                     amd64        Minimal subset of the Python language (version 3.13)
ii  libpython3.13-stdlib:amd64       3.13.5-2+deb13u4                     amd64        Interactive high-level object-oriented language (standard library, version 3.13)
ii  libreadline8t64:amd64            8.2-6                                amd64        GNU readline and history libraries, run-time libraries
ii  libreiserfscore0t64              1:3.6.27-9                           amd64        ReiserFS core library
ii  librtmp1:amd64                   2.4+20151223.gitfa8646d.1-2+b5       amd64        toolkit for RTMP streams (shared library)
ii  libsasl2-2:amd64                 2.1.28+dfsg1-9                       amd64        Cyrus SASL - authentication abstraction library
ii  libsasl2-modules-db:amd64        2.1.28+dfsg1-9                       amd64        Cyrus SASL - pluggable authentication modules (DB)
ii  libseccomp2:amd64                2.6.0-2                              amd64        high level interface to Linux seccomp filter
ii  libselinux1:amd64                3.8.1-1                              amd64        SELinux runtime shared libraries
ii  libsemanage-common               3.8.1-1                              all          Common files for SELinux policy management libraries
ii  libsemanage2:amd64               3.8.1-1                              amd64        SELinux policy management library
ii  libsepol2:amd64                  3.8.1-1                              amd64        SELinux library for manipulating binary security policies
ii  libsigc++-2.0-0v5:amd64          2.12.1-3                             amd64        type-safe Signal Framework for C++ - runtime
ii  libslang2:amd64                  2.3.3-5+b2                           amd64        S-Lang programming library - runtime version
ii  libsmartcols1:amd64              2.41-5                               amd64        smart column output alignment library
ii  libsodium23:amd64                1.0.18-1+deb13u1                     amd64        Network communication, cryptography and signaturing library
ii  libsqlite3-0:amd64               3.46.1-7+deb13u2                     amd64        SQLite 3 shared library
ii  libss2:amd64                     1.47.2-3+b11                         amd64        command-line interface parsing library
ii  libssh2-1t64:amd64               1.11.1-1+deb13u1                     amd64        SSH2 client-side library
ii  libssl3t64:amd64                 3.5.6-1~deb13u2                      amd64        Secure Sockets Layer toolkit - shared libraries
ii  libstdc++6:amd64                 14.2.0-19                            amd64        GNU Standard C++ Library v3
ii  libsystemd-shared:amd64          257.13-1~deb13u1                     amd64        systemd shared private library
ii  libsystemd0:amd64                257.13-1~deb13u1                     amd64        systemd utility library
ii  libtasn1-6:amd64                 4.20.0-2+deb13u1                     amd64        Manage ASN.1 structures (runtime)
ii  libtext-charwidth-perl:amd64     0.04-11+b4                           amd64        get display widths of characters on the terminal
ii  libtext-iconv-perl:amd64         1.7-8+b4                             amd64        module to convert between character sets in Perl
ii  libtext-wrapi18n-perl            0.06-10                              all          internationalized substitute of Text::Wrap
ii  libtinfo6:amd64                  6.5+20250216-2                       amd64        shared low-level terminfo library for terminal handling
ii  libtirpc-common                  1.3.6+ds-1                           all          transport-independent RPC library - common files
ii  libtirpc3t64:amd64               1.3.6+ds-1                           amd64        transport-independent RPC library
ii  libuchardet0:amd64               0.0.8-1+b2                           amd64        universal charset detection library - shared library
ii  libudev1:amd64                   257.13-1~deb13u1                     amd64        libudev shared library
ii  libunistring5:amd64              1.3-2                                amd64        Unicode string library for C
ii  liburcu8t64:amd64                0.15.2-2                             amd64        userspace RCU (read-copy-update) library
ii  libuuid1:amd64                   2.41-5                               amd64        Universally Unique ID library
ii  libuv1t64:amd64                  1.50.0-2                             amd64        asynchronous event notification library - runtime library
ii  libwrap0:amd64                   7.6.q-36                             amd64        Wietse Venema's TCP wrappers library
ii  libwtmpdb0:amd64                 0.73.0-3+deb13u1                     amd64        wtmp database shared library
ii  libxapian30:amd64                1.4.29-3                             amd64        Search engine library
ii  libxml2:amd64                    2.12.7+dfsg+really2.9.14-2.1+deb13u3 amd64        GNOME XML library
ii  libxtables12:amd64               1.8.11-2                             amd64        netfilter xtables library
ii  libxxhash0:amd64                 0.8.3-2                              amd64        shared library for xxhash
ii  libzstd1:amd64                   1.5.7+dfsg-1                         amd64        fast lossless compression algorithm
ii  linux-base                       4.12.1                               all          Linux image base package
ii  linux-image-6.12.100+deb13-amd64 6.12.100-1                           amd64        Linux 6.12 for 64-bit PCs (signed)
ii  linux-image-6.12.94+deb13-amd64  6.12.94-1                            amd64        Linux 6.12 for 64-bit PCs (signed)
ii  linux-image-amd64                6.12.100-1                           amd64        Linux for 64-bit PCs (meta-package)
ii  linux-sysctl-defaults            4.12.1                               all          default sysctl configuration for Linux
ii  locales                          2.41-12+deb13u3                      all          GNU C Library: National Language (locale) data [support]
ii  login                            1:4.16.0-2+really2.41-5              amd64        system login tools
ii  login.defs                       1:4.17.4-2                           all          system user management configuration
ii  logrotate                        3.22.0-1                             amd64        Log rotation utility
ii  logsave                          1.47.2-3+b11                         amd64        save the output of a command in a log file
ii  lsb-release                      12.1-1                               all          Linux Standard Base version reporting utility (minimal implementation)
ii  lsof                             4.99.4+dfsg-2                        amd64        utility to list open files
ii  lvm2                             2.03.31-2                            amd64        Linux Logical Volume Manager
ii  man-db                           2.13.1-1                             amd64        tools for reading manual pages
ii  manpages                         6.9.1-1                              all          Manual pages about using a GNU/Linux system
ii  mawk                             1.3.4.20250131-1                     amd64        Pattern scanning and text processing language
ii  mdadm                            4.4-11                               amd64        tool for managing Linux MD devices (software RAID)
ii  media-types                      13.0.0                               all          List of standard media types and their usual file extension
ii  mount                            2.41-5                               amd64        tools for mounting and manipulating filesystems
ii  mtr-tiny                         0.95-1.1+b1                          amd64        Full screen ncurses traceroute tool
ii  nano                             8.4-1+deb13u1                        amd64        small, friendly text editor inspired by Pico
ii  ncurses-base                     6.5+20250216-2                       all          basic terminal type definitions
ii  ncurses-bin                      6.5+20250216-2                       amd64        terminal-related programs and man pages
ii  ncurses-term                     6.5+20250216-2                       all          additional terminal type definitions
ii  net-tools                        2.10-1.3                             amd64        NET-3 networking toolkit
ii  netbase                          6.5                                  all          Basic TCP/IP networking system
ii  netcat-traditional               1.10-50                              amd64        TCP/IP swiss army knife
ii  nftables                         1.1.3-1                              amd64        Program to control packet filtering rules by Netfilter project
ii  openssh-client                   1:10.0p1-7+deb13u4                   amd64        secure shell (SSH) client, for secure access to remote machines
ii  openssh-server                   1:10.0p1-7+deb13u4                   amd64        secure shell (SSH) server, for secure access from remote machines
ii  openssh-sftp-server              1:10.0p1-7+deb13u4                   amd64        secure shell (SSH) sftp server module, for SFTP access from remote machines
ii  openssl                          3.5.6-1~deb13u2                      amd64        Secure Sockets Layer toolkit - cryptographic utility
ii  openssl-provider-legacy          3.5.6-1~deb13u2                      amd64        Secure Sockets Layer toolkit - cryptographic utility
ii  os-prober                        1.83                                 amd64        utility to detect other OSes on a set of drives
ii  passwd                           1:4.17.4-2                           amd64        change and administer password and group data
ii  patch                            2.8-2                                amd64        Apply a diff file to an original
ii  pci.ids                          0.0~2025.06.09-1                     all          PCI ID Repository
ii  pciutils                         1:3.13.0-2                           amd64        PCI utilities
ii  perl                             5.40.1-6                             amd64        Larry Wall's Practical Extraction and Report Language
ii  perl-base                        5.40.1-6                             amd64        minimal Perl system
ii  perl-modules-5.40                5.40.1-6                             all          Core Perl modules
ii  pinentry-curses                  1.3.1-2                              amd64        curses-based PIN or pass-phrase entry dialog for GnuPG
ii  procps                           2:4.0.4-9                            amd64        /proc file system utilities
ii  publicsuffix                     20250328.1952-0.1                    all          accurate, machine-readable list of domain name suffixes
ii  python-apt-common                3.0.0                                all          Python interface to libapt-pkg (locales)
ii  python3                          3.13.5-1                             amd64        interactive high-level object-oriented language (default python3 version)
ii  python3-apt                      3.0.0                                amd64        Python 3 interface to libapt-pkg
ii  python3-certifi                  2025.1.31+ds-1                       all          root certificates for validating SSL certs and verifying TLS hosts (python3)
ii  python3-chardet                  5.2.0+dfsg-2                         all          Universal Character Encoding Detector (Python3)
ii  python3-charset-normalizer       3.4.2-1                              amd64        charset, encoding and language detection (Python 3)
ii  python3-dbus                     1.4.0-1                              amd64        simple interprocess messaging system (Python 3 interface)
ii  python3-debian                   1.0.1                                all          Python 3 modules to work with Debian-related data formats
ii  python3-debianbts                4.1.1                                all          Python interface to Debian's Bug Tracking System
ii  python3-distro                   1.9.0-1                              all          Linux OS platform information API
ii  python3-idna                     3.10-1+deb13u1                       all          Python IDNA2008 (RFC 5891) handling (Python 3)
ii  python3-minimal                  3.13.5-1                             amd64        minimal subset of the Python language (default python3 version)
ii  python3-reportbug                13.2.0                               all          Python modules for interacting with bug tracking systems
ii  python3-requests                 2.32.3+dfsg-5+deb13u1                all          elegant and simple HTTP library for Python3, built for human beings
ii  python3-urllib3                  2.3.0-3+deb13u2                      all          HTTP library with thread-safe connection pooling for Python3
ii  python3.13                       3.13.5-2+deb13u4                     amd64        Interactive high-level object-oriented language (version 3.13)
ii  python3.13-minimal               3.13.5-2+deb13u4                     amd64        Minimal subset of the Python language (version 3.13)
ii  readline-common                  8.2-6                                all          GNU readline and history libraries, common files
ii  reportbug                        13.2.0                               all          reports bugs in the Debian distribution
ii  rsync                            3.4.1+ds1-5+deb13u4                  amd64        fast, versatile, remote (and local) file-copying tool
ii  runit-helper                     2.16.4                               all          dh-runit implementation detail
ii  sed                              4.9-2+deb13u1                        amd64        GNU stream editor for filtering/transforming text
ii  sensible-utils                   0.0.25                               all          Utilities for sensible alternative selection
ii  shared-mime-info                 2.4-5+b2                             amd64        FreeDesktop.org shared MIME database and spec
ii  sqlite3                          3.46.1-7+deb13u2                     amd64        Command line interface for SQLite 3
ii  sqv                              1.3.0-3+b2                           amd64        OpenPGP signature verification program from Sequoia
ii  ssh-import-id                    5.10-3                               all          securely retrieve an SSH public key and install it locally
ii  sudo                             1.9.16p2-3+deb13u2                   amd64        Provide limited super user privileges to specific users
ii  systemd                          257.13-1~deb13u1                     amd64        system and service manager
ii  systemd-sysv                     257.13-1~deb13u1                     amd64        system and service manager - SysV compatibility symlinks
ii  systemd-timesyncd                257.13-1~deb13u1                     amd64        minimalistic service to synchronize local time with NTP servers
ii  sysvinit-utils                   3.14-4                               amd64        System-V-like utilities
ii  tar                              1.35+dfsg-3.1                        amd64        GNU version of the tar archiving utility
ii  task-english                     3.81                                 all          General English environment
ii  task-ssh-server                  3.81                                 all          SSH server
ii  tasksel                          3.81                                 all          tool for selecting tasks for installation on Debian systems
ii  tasksel-data                     3.81                                 all          official tasks used for installation of Debian systems
ii  traceroute                       1:2.1.6-1                            amd64        Traces the route taken by packets over an IPv4/IPv6 network
ii  tzdata                           2026b-0+deb13u1                      all          time zone and daylight-saving time data
ii  ucf                              3.0052                               all          Update Configuration File(s): preserve user changes to config files
ii  udev                             257.13-1~deb13u1                     amd64        /dev/ and hotplug management daemon
ii  util-linux                       2.41-5                               amd64        miscellaneous system utilities
ii  util-linux-extra                 2.41-5                               amd64        interactive login tools
ii  util-linux-locales               2.41-5                               all          locales files for util-linux
ii  vim                              2:9.1.1230-2                         amd64        Vi IMproved - enhanced vi editor
ii  vim-common                       2:9.1.1230-2                         all          Vi IMproved - Common files
ii  vim-runtime                      2:9.1.1230-2                         all          Vi IMproved - Runtime files
ii  vim-tiny                         2:9.1.1230-2                         amd64        Vi IMproved - enhanced vi editor - compact version
ii  wget                             1.25.0-2                             amd64        retrieves files from the web
ii  whiptail                         0.52.25-1                            amd64        Displays user-friendly dialog boxes from shell scripts
ii  wtmpdb                           0.73.0-3+deb13u1                     amd64        utility to display login/logout/reboot information
ii  xdg-user-dirs                    0.18-2                               amd64        tool to manage well known user directories
ii  xfsprogs                         6.13.0-2+b1                          amd64        Utilities for managing the XFS filesystem
ii  xkb-data                         2.42-1                               all          X Keyboard Extension (XKB) configuration data
ii  xz-utils                         5.8.1-1+deb13u1                      amd64        XZ-format compression utilities
ii  zlib1g:amd64                     1:1.3.dfsg+really1.3.1-1+b1          amd64        compression library - runtime
ii  zstd                             1.5.7+dfsg-1                         amd64        fast lossless compression algorithm -- CLI tool
```

## 7. Off-server client and device flows

The client archive was downloaded on the operator VM from `https://github.com/Jabbslad/kiln/releases/download/v0.4.6/kiln-v0.4.6-x86_64-unknown-linux-gnu.tar.gz`. `sha256sum -c` printed `OK`. The digest is `2197728c01e330410cdc0d252399d67699e9a6936e73c8862530e83216dd7f4b`. The checksum file is one line:

```text
2197728c01e330410cdc0d252399d67699e9a6936e73c8862530e83216dd7f4b  kiln-v0.4.6-x86_64-unknown-linux-gnu.tar.gz
```

The archive contains only `kiln`. It was installed as `/home/ubuntu/.local/bin/kiln` mode 0755. `kiln --version` then printed `kiln 0.4.6`. The Jabbslad/kiln-install bootstrap was not used. That bootstrap at tag v0.4.6 still pins the published v0.4.5 assets, and the older public installer pin the operator was warned about is 0.4.4.

`bash deploy/enroll-dark-forge.sh --self-test` from the extracted v0.4.6 archive printed `enroll-dark-forge self-test passed` and exited 0. The script hash matches the tag.

The real enrollment was started with `setsid nohup` so it is not a child of the ssh session. PID 3849584, parent PID 1, running `deploy/enroll-dark-forge.sh 78.46.98.73` from the extracted archive. It has started `/usr/local/libexec/kiln-api enroll --url https://cli.dark-forge.dev:8443 --local-url https://127.0.0.1:8443 --ca-file /etc/kiln/ca.crt --name dark-forge`. Stdout:

```text
Approve the next code at https://dark-forge.dev while signed in as GitHub user Jabbslad (id 3084).
The page must show that GitHub login. The stored subject must be 3084.
```

Stderr, with the user code removed:

```text
Approve enrollment at https://dark-forge.dev/device with code [device-code redacted].
Server: https://cli.dark-forge.dev:8443
CA SHA-256: AF:B5:CF:59:AB:00:F1:C2:C7:E1:87:30:D5:68:AA:F8:04:86:59:D2:95:AB:8A:35:9C:64:11:94:C5:E1:91:B0
Only approve the request you started on this host.
```

On the operator VM, `kiln login` was started only after `kiln --version` printed `kiln 0.4.6`. The command was:

```sh
setsid nohup script -q -f -c '/home/ubuntu/.local/bin/kiln --config /home/ubuntu/.config/kiln/dark-forge-v046.json login' /tmp/kiln-login-v046.typescript
```

`script` supplies the terminal `kiln login` requires. The config path did not exist. The `script` process is PID 3288, parent PID 1. `kiln login` is its child. It printed `Open https://dark-forge.dev/device and enter code [device-code redacted]`. Browser launch failed because this VM has no desktop bus. The process kept waiting.

`sqlite3 -readonly /var/lib/kiln-identity/identity.db` showed one pending device row named `kiln CLI` and one pending registration row named `dark-forge` for origin `https://cli.dark-forge.dev:8443`. The device row expires `2026-10-19 21:39:31` UTC. The registration row expires `2026-10-19 21:39:32` UTC. Identity issues both with `expires_in` 600. The user codes are not copied here.

Both processes can outlive this run. They cannot outlive those expiry times. If a row expires first, that process exits and a new code has to be started. Approve the server registration while signed in as GitHub user `Jabbslad`, numeric id `3084`, before approving the client login. The client login looks up the enrolled server after approval. At the time of this snapshot, `/etc/kiln/identity.json`, `/etc/kiln/directory.token`, `/etc/kiln/enrollment.pending.json`, and `/etc/systemd/system/kiln-api.service.d/identity.conf` were still absent. `kiln-door.service` was still absent.

## Files left on the host

- `/root/dpkg-before-v046.txt`
- `/root/dpkg-after-v046.txt`
- `/root/nft-before-v046.txt`
- `/root/nft-after-v046.txt`
- `/root/iptables-before-v046.txt`
- `/root/iptables-after-v046.txt`
- `/root/ip6tables-before-v046.txt`
- `/root/ip6tables-after-v046.txt`
- `/var/tmp/kiln-v046.CHz3Mi`, including the verified archive and `extract.MRBb3h`
- `/var/tmp/enroll-v046.out`, `/var/tmp/enroll-v046.err`, and `/var/tmp/enroll-v046.pid`
- `/opt/kiln/upgrades/0.4.4-8DDh4B`
- `/opt/kiln/upgrades/0.4.5-RSNr4F`
- The v0.4.5 attempt files already on the host were left in place

`/opt/kiln/pending-upgrade.json` does not exist. The smoke profile was removed. The deploy key was not removed in this phase.

# 2026-10-20 paseo Caddy override on dark-forge.dev

Result. The host change from main `7888f28`, PR #34, is in place. It was not rolled back. Paseo's `caddy` service was recreated. It is still `172.18.0.3` on `paseo_default`, image `sha256:df7f1c2fb114453b951de51a98efc010db1655a92c2e86be6706714e2417a78d`, with `/etc/kiln/ca.crt` mounted read-only. `https://cli.dark-forge.dev/v1/templates` returns 401 on the Let's Encrypt certificate. `https://dark-forge.dev/login` returns 200 and the body contains `Light the way in.` `http://app.nowcode.com/` and `http://hub.nowcode.com/` both return 200. Kiln stayed v0.4.9. `kiln-identity` was not restarted. `/root/.ssh/authorized_keys` was not edited.

Jamie's off-host laptop `kiln login` test is still pending. This deploy did not run it.

## What the change does

`deploy/paseo-caddy.override.yaml` from `7888f28` is installed at `/opt/paseo/paseo-caddy.override.yaml`. Its sha256 is `855f22f41c017fdc0720873c9a93a8b6083cd2ed3fb87ef1ba58a3092851e7e6`, the same hash as `git show 7888f28:deploy/paseo-caddy.override.yaml`. The file sets `ipv4_address: 172.18.0.3` on the compose `default` network and bind-mounts `${KILN_CA_CERT:-/etc/kiln/ca.crt}` at `/etc/kiln/ca.crt` with `read_only: true` and `create_host_path: false`. It does not set the image, the ports, the Caddyfile mount, or the named volumes.

The door site in `/opt/paseo/Caddyfile` now uses `tls_trust_pool file` and `pem_file /etc/kiln/ca.crt`. The previous line was `tls_trusted_ca_certs /etc/kiln/ca.crt`. Before the recreate, `/etc/kiln/ca.crt` inside the container was a copied file, mode `0444`, sha256 `f7bb4b2cb0e335047a536b2aeb11eac9489e2d4db87ecc067fdf652dbd8406bf`, and it was not a mount. `caddy run` had no `--watch` flag, so editing the bind-mounted Caddyfile did not reload the running process. The new container is the first process that sees the mount.

## Blast radius

The safe fact is that the merged caddy service keeps image `caddy:2`, published ports 80 and 443, and volumes `caddy_data` and `caddy_config`, and only adds the pin and the read-only CA file. `docker compose config` proved that before the recreate. The live container proved it after. `paseo_default` IPAM Config is `172.18.0.0/16`, which contains `172.18.0.3`, and the only container holding `172.18.0.3` before the recreate was paseo's `caddy`. Those two checks are why the pin was applied. Replacing `paseo_default` was not required and was not done.

Checked and left alone:

- `kiln-identity` MainPID stayed `3522047`. `ActiveEnterTimestamp` stayed `Mon 2026-10-19 12:42:17 CEST`. `NRestarts` stayed 0. The dark-forge `ca_pem` sha256 stayed `f7bb4b2cb0e335047a536b2aeb11eac9489e2d4db87ecc067fdf652dbd8406bf`.
- `/root/.ssh/authorized_keys` sha256 stayed `4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37`.
- `:8443` sha256 fingerprint stayed `D4:BA:F2:89:F2:62:5D:E8:67:57:2D:4F:97:F8:CB:B4:D7:8B:27:BE:FA:63:0B:53:A8:DB:3E:0C:74:C2:7D:3A`. `kiln-door-proxy-public.socket` stayed active. Its `ActiveEnterTimestamp` stayed `Tue 2026-10-20 10:06:13 CEST`.
- `paseo`, `paseo-hub`, and `paseo-hub-db` kept the same container ids and `StartedAt` values. Hub stayed healthy.
- Volume names stayed `paseo_caddy_data` and `paseo_caddy_config`. The sorted file list under `/data` and `/config` matched before and after. Certificate files for `app.nowcode.com`, `cli.dark-forge.dev`, and `dark-forge.dev` are still present. Private key file paths from that list are omitted below. The files were not read.

Risk that was not taken. Hub's compose service sets `pull_policy: always`. On Compose v5.5.0, `docker compose up -d --dry-run` printed a plan that pulls `ghcr.io/getpaseo/hub:latest` and recreates `paseo-hub`. The real `docker compose up -d` was not run. After that dry-run the hub container id, start time, health, and image id were unchanged. A later paseo update that runs a plain `docker compose up -d` can recreate hub. Re-check that caddy is still `172.18.0.3` on `paseo_default` after any paseo update. `docker compose up -d --no-deps caddy` is the command that reapplies only caddy. That command was run after the recreate. It printed `Container caddy Running` and left container `14b7fcb812c3` at `172.18.0.3`.

The bridge socket `172.18.0.1:8444` did not return `ECONNREFUSED` for other sources. From the host, curl exit code was 28, `Connection timed out after 3002 milliseconds`. From the `paseo` container, a TCP connect timed out. From the caddy container, which is `172.18.0.3`, wget reached the TLS handshake and then failed certificate verification because that wget does not trust the Kiln CA. The public 401 through Caddy is the request that is allowed.

`/etc/kiln/door.key` was mode `640` before this deploy. It was not changed. `/etc/kiln/ca.key` and `/etc/kiln/directory.token` stayed mode `600`. None of those files were mounted.

## SSH

The deploy key fingerprint checked before connecting was `SHA256:AGD6iqWdOAB1R7LeRTlbIMYyvx+FvkIDw8MTWEVtrd4`. The pinned ed25519 host key fingerprint was `SHA256:/yIiijBqkbGh9lFju3N9ZhbJGIqv+NjC0R1CyWh9g9s`. The private key is not in this file. After the host work, `shred -u` on the key file exited 0. A following existence check found the file gone, and the temp directory that held it was removed.

The remote prefix was `ssh -i <key> -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile=<pinned known_hosts> -o BatchMode=yes -o ConnectTimeout=15 root@dark-forge.dev`, with the remote script on stdin and an outer `timeout`.

## Versions

```text
Client: Docker Engine - Community
 Version:           29.7.2
 API version:       1.55
Server: Docker Engine - Community
 Engine:
  Version:          29.7.2
  API version:      1.55 (minimum version 1.40)
Docker Compose version v5.5.0
```

```text
Firecracker v1.17.0
Jailer v1.17.0
kiln 0.4.9
kiln-runtime 0.4.9
kiln-server 0.4.9
```

`kiln-runtime doctor`:

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

`isolated_ready` is false because this doctor command did not pass `--isolation-config`. The installed host service uses its own policy file. `development_ready` is true.

The running caddy command was `["caddy","run","--config","/etc/caddy/Caddyfile","--adapter","caddyfile"]`. Network mode was `paseo_default`.

The caddy service in `/opt/paseo/docker-compose.yml`, which was not edited:

```yaml
  caddy:
    image: caddy:2
    container_name: caddy
    restart: unless-stopped
    ports:
      - "80:80"
      - "443:443"
    volumes:
      - ./Caddyfile:/etc/caddy/Caddyfile:ro
      - caddy_data:/data
      - caddy_config:/config
    depends_on:
      - paseo
      - hub
```

Named volumes in that file are `caddy_data`, `caddy_config`, and `postgres-data`. Compose prefixes the caddy volumes as `paseo_caddy_data` and `paseo_caddy_config`.

## Before

`com.docker.compose.project.config_files` was `/opt/paseo/docker-compose.yml`. `com.docker.compose.project.working_dir` was `/opt/paseo`. There was no `/opt/paseo/.env`, so `COMPOSE_FILE` was unset. There was no `docker-compose.override.yml` in `/opt/paseo`.

Caddy container `826e7f164f64c9858a33616f958e94627ebce18d9b672bed62d6852a652d126d`, name `caddy`, image `sha256:df7f1c2fb114453b951de51a98efc010db1655a92c2e86be6706714e2417a78d`, tag `caddy:2` resolved to that same id. Address `172.18.0.3/16` on `paseo_default`, gateway `172.18.0.1`, network id `164dac86db639d885f7341d8bd2ab0ebba82b5cc76d95c2cc60257d5885a7780`. Started `2026-09-05T00:20:11.922786918Z`.

Mounts before:

```text
/opt/paseo/Caddyfile -> /etc/caddy/Caddyfile bind RW=false
paseo_caddy_data -> /data volume RW=true
paseo_caddy_config -> /config volume RW=true
```

`/etc/kiln/ca.crt` was not a mount. `sha256sum` inside the container matched the host file `f7bb4b2cb0e335047a536b2aeb11eac9489e2d4db87ecc067fdf652dbd8406bf`.

IPAM:

```text
[{"Subnet":"172.18.0.0/16","Gateway":"172.18.0.1"}]
subnet_contains_pin 172.18.0.0/16 yes
preflight_subnet True
holder name=caddy id=826e7f164f64 ip=172.18.0.3/16
holder name=paseo-hub-db id=970927c154c9 ip=172.18.0.4/16
holder name=paseo id=a398b31a015f ip=172.18.0.2/16
holder name=paseo-hub id=ef0635c603c3 ip=172.18.0.5/16
pin_holder_count 1
pin_holder name=caddy project=paseo service=caddy
```

Other containers before, and still the same after:

```text
/paseo a398b31a015fdc30ae14322c46c3957ee96c24268df0709386d98d550c4774ab 2026-10-13T21:14:56.680741291Z running
/paseo-hub ef0635c603c33f07e1aacba6735a4d7db2eb6cd6853451d6c7c869e63b227bc7 2026-09-29T21:26:23.879694609Z running
/paseo-hub-db 970927c154c9b6a04b185de08b5d3a5cdaba553554c608db78a2c977a6e631f2 2026-08-22T22:50:09.192488365Z running
```

Identity and `authorized_keys` before:

```text
MainPID=3522047
NRestarts=0
ActiveState=active
SubState=running
ActiveEnterTimestamp=Mon 2026-10-19 12:42:17 CEST
4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37  /root/.ssh/authorized_keys
```

Active server rows. `ca_pem` bytes were hashed and not printed.

```text
id=srv_fcefac0410a846dcb13a1a83c1a9235c name=ser7 origin=https://192.168.50.210:8443 active=1 ca_pem_len=1501 ca_pem_sha256=4c83f0d254a803507fb24e66a25a07e07deaef34998247a7aa74150395d79abb
id=srv_37e8ac65c6524713a9e646efa39b7e2e name=dark-forge origin=https://cli.dark-forge.dev active=1 ca_pem_len=1501 ca_pem_sha256=f7bb4b2cb0e335047a536b2aeb11eac9489e2d4db87ecc067fdf652dbd8406bf
```

`:8443` before:

```text
sha256 Fingerprint=D4:BA:F2:89:F2:62:5D:E8:67:57:2D:4F:97:F8:CB:B4:D7:8B:27:BE:FA:63:0B:53:A8:DB:3E:0C:74:C2:7D:3A
```

Caddyfile sha256 before the edit: `90954669459825dbd89d15c36a586e5f0f56ffd6734978560c1268f4a82061bf`.

Outside the host, before the recreate, `https://cli.dark-forge.dev:443` was already Let's Encrypt `YE2` with `DNS:cli.dark-forge.dev`. Unauthenticated `GET /v1/templates` was HTTP 401. `https://dark-forge.dev/login` was HTTP 200 and contained `Light the way in.` Both nowcode URLs were HTTP 200.

## Backup

Directory `/root/kiln-caddy-backup-20261020T090400Z`, mode `0700`.

- `/opt/paseo/Caddyfile`
- `/opt/paseo/docker-compose.yml`
- marker `ENV_ABSENT` because `/opt/paseo/.env` did not exist
- `docker inspect caddy` saved on the host as `caddy-inspect.json`, mode `0600`. That file was not copied off the host.
- running image id and `caddy:2` tag id, which matched
- volume file names, with private key paths omitted from this runbook
- container id and `StartedAt` for caddy, paseo, paseo-hub, and paseo-hub-db

`/opt/paseo/Caddyfile.bak-door-443` from `Oct 3 20:15` was already present. It was not overwritten.

`docker-compose.yml` sha256 `29da2e7db3edf0a904ecdf584ccb2c4af72cae509322d00722a1ba46426b555b`.

## Apply

`COMPOSE_FILE` was set from the running container's `config_files` label, same order, plus the override:

```text
config_files_label=/opt/paseo/docker-compose.yml
COMPOSE_FILE=/opt/paseo/docker-compose.yml:/opt/paseo/paseo-caddy.override.yaml
```

`/opt/paseo/.env` mode `0600`, sha256 `ef9dccedf3549ec7f4ec24182a8b2a5848b0c4a6295bd0fdb9a1a3c7573491ee`. That file contains only the `COMPOSE_FILE` line.

`docker compose config` gates, printed without service environment:

```text
services caddy hub paseo postgres
image caddy:2
container_name caddy
ports [{"mode": "ingress", "protocol": "tcp", "published": "80", "target": 80}, {"mode": "ingress", "protocol": "tcp", "published": "443", "target": 443}]
networks {"default": {"ipv4_address": "172.18.0.3"}}
volumes [{"read_only": true, "source": "/opt/paseo/Caddyfile", "target": "/etc/caddy/Caddyfile", "type": "bind"}, {"read_only": null, "source": "caddy_data", "target": "/data", "type": "volume"}, {"read_only": null, "source": "caddy_config", "target": "/config", "type": "volume"}, {"read_only": true, "source": "/etc/kiln/ca.crt", "target": "/etc/kiln/ca.crt", "type": "bind"}]
top_volumes caddy_config caddy_data postgres-data
pin_network default 172.18.0.3
COMPOSE_CONFIG_GATES_OK
COMPOSE_CONFIG_GATES_EXIT:0
```

Caddyfile diff:

```diff
--- /root/kiln-caddy-backup-20261020T090400Z/Caddyfile	2026-10-20 10:03:38.561163519 +0200
+++ /opt/paseo/Caddyfile	2026-10-20 11:04:00.633627378 +0200
@@ -36,7 +36,9 @@
 	reverse_proxy https://172.18.0.1:8444 {
 		transport http {
 			tls_server_name cli.dark-forge.dev
-			tls_trusted_ca_certs /etc/kiln/ca.crt
+			tls_trust_pool file {
+				pem_file /etc/kiln/ca.crt
+			}
 		}
 	}
 }
```

New Caddyfile sha256 `7d2c3722f08e360d125c18cdcbb95faceb692032df704a61893657083434475f`.

Validate used the running image id, not a new pull:

```text
docker run --rm \
  --mount type=bind,source=/opt/paseo/Caddyfile,target=/etc/caddy/Caddyfile,readonly \
  --mount type=bind,source=/etc/kiln/ca.crt,target=/etc/kiln/ca.crt,readonly \
  sha256:df7f1c2fb114453b951de51a98efc010db1655a92c2e86be6706714e2417a78d \
  caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
```

Stdout was `Valid configuration`. Stderr:

```text
{"level":"info","ts":1792487040.9594803,"msg":"using config from file","file":"/etc/caddy/Caddyfile"}
{"level":"warn","ts":1792487040.9599726,"logger":"caddyfile","msg":"Unnecessary header_up X-Forwarded-For: the reverse proxy's default behavior is to pass headers to the upstream"}
{"level":"info","ts":1792487040.9605982,"msg":"adapted config to JSON","adapter":"caddyfile"}
{"level":"info","ts":1792487040.9609377,"logger":"http.auto_https","msg":"server is listening only on the HTTPS port but has no TLS connection policies; adding one to enable TLS","server_name":"srv0","https_port":443}
{"level":"info","ts":1792487040.9609487,"logger":"http.auto_https","msg":"enabling automatic HTTP->HTTPS redirects","server_name":"srv0"}
{"level":"warn","ts":1792487040.960958,"logger":"http.auto_https","msg":"server is listening only on the HTTP port, so no automatic HTTPS will be applied to this server","server_name":"srv1","http_port":80}
{"level":"info","ts":1792487040.9610245,"logger":"tls.cache.maintenance","msg":"started background certificate maintenance","cache":"0x382264316200"}
{"level":"info","ts":1792487040.961574,"logger":"http","msg":"servers shutting down with eternal grace period"}
{"level":"info","ts":1792487040.961625,"logger":"tls.cache.maintenance","msg":"stopped background certificate maintenance","cache":"0x382264316200"}
```

Recreate, from `/opt/paseo`. Stderr:

```text
 Container caddy Recreate 
 Container caddy Recreated 
 Container caddy Starting 
 Container caddy Started 
```

Stdout included `RECREATE_OK`. No other service was in that recreate. The command was `docker compose up -d --force-recreate --no-deps caddy`.

## After

```text
caddy_id 14b7fcb812c35e320a4817a2731ef003eec77df247270945c72f6ca21188bd8b
caddy_image sha256:df7f1c2fb114453b951de51a98efc010db1655a92c2e86be6706714e2417a78d
caddy_ip 172.18.0.3
caddy_status running running True
ca_mount {"Destination": "/etc/kiln/ca.crt", "Name": null, "RW": false, "Source": "/etc/kiln/ca.crt", "Type": "bind"}
volume_names paseo_caddy_config paseo_caddy_data
INSPECT_GATES ok
```

`/proc/mounts` inside the container:

```text
/dev/md2 /etc/kiln/ca.crt ext4 ro,relatime 0 0
```

`sha256sum /etc/kiln/ca.crt` inside the container was still `f7bb4b2cb0e335047a536b2aeb11eac9489e2d4db87ecc067fdf652dbd8406bf`.

Mounts after the later `docker compose up -d --no-deps caddy`:

```text
/etc/caddy/Caddyfile RW=false Name= Type=bind
/data RW=true Name=paseo_caddy_data Type=volume
/config RW=true Name=paseo_caddy_config Type=volume
/etc/kiln/ca.crt RW=false Name= Type=bind
```

That second up printed `Container caddy Running`. Container id and `StartedAt` stayed `14b7fcb812c35e320a4817a2731ef003eec77df247270945c72f6ca21188bd8b` and `2026-10-20T09:04:01.860181622Z`. Address stayed `172.18.0.3`.

Volume file diff was `VOLUME_FILES_UNCHANGED`. Private key paths are omitted. The certificate files that were present both times:

```text
/data/caddy/certificates/acme-v02.api.letsencrypt.org-directory/app.nowcode.com/app.nowcode.com.crt
/data/caddy/certificates/acme-v02.api.letsencrypt.org-directory/app.nowcode.com/app.nowcode.com.json
/data/caddy/certificates/acme-v02.api.letsencrypt.org-directory/cli.dark-forge.dev/cli.dark-forge.dev.crt
/data/caddy/certificates/acme-v02.api.letsencrypt.org-directory/cli.dark-forge.dev/cli.dark-forge.dev.json
/data/caddy/certificates/acme-v02.api.letsencrypt.org-directory/dark-forge.dev/dark-forge.dev.crt
/data/caddy/certificates/acme-v02.api.letsencrypt.org-directory/dark-forge.dev/dark-forge.dev.json
/data/caddy/acme/acme-v02.api.letsencrypt.org-directory/users/default/default.json
/data/caddy/instance.uuid
/data/caddy/last_clean.json
/config/caddy/autosave.json
```

The find listing also had four private key paths. They were not printed and were not read. Omitted path count from the apply transcript: 8.

Host checks through `127.0.0.1`, attempt 1:

```text
issuer=C=US, O=Let's Encrypt, CN=YE2
X509v3 Subject Alternative Name: 
    DNS:cli.dark-forge.dev
templates_http_code:401
templates_body:{"code":"unauthorized","message":"A valid credential is required."}
login_http_code:200
login_phrase:yes
nowcode_app:200
nowcode_hub:200
PUBLIC_GATES_OK
```

Administrator token check, on the host, header file mode `0600`, passed with `curl -H @file`, then `shred -u`. The token was not printed. `admin_token_leaked:no`. `admin_header_shredded True`.

```text
admin_http_code:401
admin_body:{"code":"unauthorized","message":"A valid credential is required."}
admin_status:HTTP/2 401 
```

Bridge:

```text
host_bridge errno=None err=None
```

The follow-up made that error explicit:

```text
TimeoutError errno None str timed out
```

```text
*   Trying 172.18.0.1:8444...
* Connection timed out after 3002 milliseconds
* closing connection #0
curl_bridge_exit:28
```

From `paseo`:

```text
timeout
paseo_node_exit:0
```

From caddy, stderr, which shows the TLS handshake started:

```text
289BDA8F707F0000:error:0A000086:SSL routines:tls_post_process_server_certificate:certificate verify failed:ssl/statem/statem_clnt.c:2124:
ssl_client: SSL_connect
wget: error getting response: Connection reset by peer
caddy_wget_exit:1
```

`:8443` after:

```text
sha256 Fingerprint=D4:BA:F2:89:F2:62:5D:E8:67:57:2D:4F:97:F8:CB:B4:D7:8B:27:BE:FA:63:0B:53:A8:DB:3E:0C:74:C2:7D:3A
```

Dry-run of a plain `docker compose up -d`, which was not followed by the real command:

```text
 Image ghcr.io/getpaseo/hub:latest Pulling 
 Image ghcr.io/getpaseo/hub:latest Pulled 
 Container caddy Running 
 Container paseo Running 
 Container paseo-hub-db Running 
 Container paseo-hub Recreate 
 Container paseo-hub Recreated 
 Container paseo-hub-db Waiting 
 Container paseo-hub-db Healthy 
 Container ef0635c603c3_paseo-hub Starting 
 Container ef0635c603c3_paseo-hub Started 
DRY_RUN_EXIT:0
PLAIN_UP_SKIPPED
```

Hub after that dry-run:

```text
id=ef0635c603c33f07e1aacba6735a4d7db2eb6cd6853451d6c7c869e63b227bc7 status=running health=healthy started=2026-09-29T21:26:23.879694609Z image=sha256:090d987c6d1d163f5492439dd9150233c17a8d16fc6d6c5f4e1a570f93a12b88
```

Identity, `ca_pem`, and `authorized_keys` after:

```text
MainPID=3522047
NRestarts=0
ActiveState=active
ActiveEnterTimestamp=Mon 2026-10-19 12:42:17 CEST
4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37  /root/.ssh/authorized_keys
id=srv_fcefac0410a846dcb13a1a83c1a9235c name=ser7 origin=https://192.168.50.210:8443 ca_pem_sha256=4c83f0d254a803507fb24e66a25a07e07deaef34998247a7aa74150395d79abb
id=srv_37e8ac65c6524713a9e646efa39b7e2e name=dark-forge origin=https://cli.dark-forge.dev ca_pem_sha256=f7bb4b2cb0e335047a536b2aeb11eac9489e2d4db87ecc067fdf652dbd8406bf
```

Door sockets after. `ActiveEnterTimestamp` values match the preflight.

```text
Id=kiln-door-proxy.socket
ActiveState=active
SubState=running
ActiveEnterTimestamp=Tue 2026-10-20 10:06:13 CEST

Id=kiln-door-proxy-public.socket
ActiveState=active
SubState=running
ActiveEnterTimestamp=Tue 2026-10-20 10:06:13 CEST

Id=kiln-door.service
ActiveState=active
SubState=running
ActiveEnterTimestamp=Tue 2026-10-20 10:06:14 CEST
LISTEN 0      4096      172.18.0.1:8444      0.0.0.0:*   
LISTEN 0      4096     78.46.98.73:8443      0.0.0.0:*   
LISTEN 0      128        127.0.0.1:8444      0.0.0.0:*   
LISTEN 0      128        127.0.0.1:8443      0.0.0.0:*   
```

`127.0.0.1:8443` is still `kiln-api`. `127.0.0.1:8444` is still the door. The public socket and the bridge socket stayed up. Kiln was not reinstalled. The `0.4.6` tree under `/opt/kiln/upgrades/0.4.6-3vO6d0` was left in place, along with `0.4.4-8DDh4B` and `0.4.5-RSNr4F`.

## Outside the host, after

Date `Tue Oct 20 09:05:10 AM UTC 2026`.

```text
issuer=C = US, O = Let's Encrypt, CN = YE2
X509v3 Subject Alternative Name: 
    DNS:cli.dark-forge.dev
sha256 Fingerprint=F1:EC:A3:00:B5:D6:92:5A:A2:77:B9:E2:F9:6E:E1:A7:C2:7E:FF:6E:2F:06:0A:8C:F8:4C:8A:64:7F:98:F6:46
```

`https://cli.dark-forge.dev/v1/templates` with no credential:

```text
HTTP/2 401 
via: 2.0 Caddy
content-type: application/json
content-length: 67
http_code:401
{"code":"unauthorized","message":"A valid credential is required."}
```

`https://dark-forge.dev/login` was HTTP 200, 502 bytes, and contained `Light the way in.`

`http://app.nowcode.com/` was HTTP 200, `Via: 1.1 Caddy`. `http://hub.nowcode.com/` was HTTP 200, `Via: 1.1 Caddy`.

`:8443` from outside matched the same sha256 fingerprint as before.

## On-host smoke

Fresh config directory `/var/tmp/kiln-smoke.Dunnlf`, removed at the end. Profile `default` pointed at `https://127.0.0.1:8443` with `--token-file /etc/kiln/admin.token` and `--ca-file /etc/kiln/ca.crt`. The profile file stores the token path. `token_embedded False`. The token was not printed.

```text
+ kiln profile add
added profile default
profile_add_exit:0
token_embedded False
+ kiln create
NAME              ID                                STATE           MEMORY  VCPUS
kiln-caddy-smoke  e5dbbcf40b1a404caf71c1aae3b980a9  running       4096 MiB  1
create_exit:0
+ kiln exec uname -a
Linux box-e5dbbcf40b1a 6.1.155+ #1 SMP PREEMPT_DYNAMIC Thu Dec 18 15:17:16 UTC 2025 x86_64 x86_64 x86_64 GNU/Linux
exec_exit:0
+ kiln delete
deleted e5dbbcf40b1a404caf71c1aae3b980a9
delete_exit:0
+ kiln list
No boxes yet. Create one with `kiln create`.
list_exit:0
SMOKE_DIR_REMOVED
```

Stderr:

```text
creating kiln-caddy-smoke with template ubuntu-4g
request 85a8fc6e3e9841ff907c7d642e18f755
request 31f741c5de68404d925b40cc91426ef2
request 0aa3ca2b0d40444fb357400154c35369
```

The exec command was `/bin/sh -c 'uname -a'`.

## Roll back

These rollback steps were not executed on the host. The 2026-10-20 deploy left `/opt/paseo/Caddyfile` on `tls_trust_pool file` and left this line in `/opt/paseo/.env`:

```text
COMPOSE_FILE=/opt/paseo/docker-compose.yml:/opt/paseo/paseo-caddy.override.yaml
```

Do not run `docker compose down` for the whole project. That would stop paseo, hub, and the database. Do not restart `kiln-identity`.

### Revert only the Caddyfile

Keep `paseo-caddy.override.yaml` in `COMPOSE_FILE`. The override is the read-only `/etc/kiln/ca.crt` mount and the `172.18.0.3` pin. The backup `/root/kiln-caddy-backup-20261020T090400Z/Caddyfile` still has the `cli.dark-forge.dev` site and `tls_trusted_ca_certs /etc/kiln/ca.crt`. Caddy loads that file while it builds the reverse proxy. A container started without the mount has no CA file, `caddy run` exits, and sign-in, both nowcode sites, and the door go down together. Do not remove the override on this path, and do not delete `/opt/paseo/.env`.

`/opt/paseo/docker-compose.yml` was not edited. Leave it.

From `/opt/paseo`. Copy the live file aside, then put the backup in place. The running process has no `--watch` flag, so the copy does not reload it.

```sh
cp -a /opt/paseo/Caddyfile /opt/paseo/Caddyfile.pre-rollback
cp -a /root/kiln-caddy-backup-20261020T090400Z/Caddyfile /opt/paseo/Caddyfile
```

Validate before any reload. Use the image the container already runs, `sha256:df7f1c2fb114453b951de51a98efc010db1655a92c2e86be6706714e2417a78d`, and mount the Caddyfile and the CA the same way the service does. The throwaway container is enough. `docker compose exec` reads the same bind mount inside the running container.

```sh
docker run --rm \
  --mount type=bind,source=/opt/paseo/Caddyfile,target=/etc/caddy/Caddyfile,readonly \
  --mount type=bind,source=/etc/kiln/ca.crt,target=/etc/kiln/ca.crt,readonly \
  sha256:df7f1c2fb114453b951de51a98efc010db1655a92c2e86be6706714e2417a78d \
  caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
```

```sh
docker compose exec caddy caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
```

If validate fails, put the live file back and stop. Do not reload and do not recreate.

```sh
cp -a /opt/paseo/Caddyfile.pre-rollback /opt/paseo/Caddyfile
```

If validate prints `Valid configuration`, reload the running process. This keeps the container, the address, and the CA mount.

```sh
docker compose exec caddy caddy reload --config /etc/caddy/Caddyfile --adapter caddyfile
```

If reload fails, copy `/opt/paseo/Caddyfile.pre-rollback` back over `/opt/paseo/Caddyfile` and validate that file before another reload. The known-good file from this deploy has sha256 `7d2c3722f08e360d125c18cdcbb95faceb692032df704a61893657083434475f`.

### Checks after that reload

Record the status codes. Sign-in must be 200 and the body must contain `Light the way in.` Both nowcode sites must be 200. Record the `:443` status. With the backup door site still in the file, and the CA still mounted, `https://cli.dark-forge.dev/v1/templates` with no credential is 401 on the Let's Encrypt certificate.

```sh
curl -sS -o /dev/null -w 'login:%{http_code}\n' --max-time 20 https://dark-forge.dev/login
curl -sS -o /dev/null -w 'app:%{http_code}\n' --max-time 20 http://app.nowcode.com/
curl -sS -o /dev/null -w 'hub:%{http_code}\n' --max-time 20 http://hub.nowcode.com/
curl -sS -o /dev/null -w 'door443:%{http_code}\n' --max-time 20 https://cli.dark-forge.dev/v1/templates
docker inspect caddy --format 'ip={{index .NetworkSettings.Networks.paseo_default.IPAddress}} {{range .Mounts}}{{.Destination}} RW={{.RW}} {{end}}'
```

The address must still be `172.18.0.3`, and `/etc/kiln/ca.crt` must still be `RW=false`. These commands were not run as a rollback. The deploy checks above are the last ones that ran.

### Remove the override

These steps were not executed on the host either. Use them only to drop the pin and the CA mount. Do this after the Caddyfile no longer names `cli.dark-forge.dev` and no longer mentions `/etc/kiln/ca.crt`. The backup `Caddyfile` is the wrong file for this path. It still has `tls_trusted_ca_certs /etc/kiln/ca.crt`. `/opt/paseo/Caddyfile.bak-door-443` was already on disk and was not read. Use it only after confirming it has no `cli.dark-forge.dev` block and no `ca.crt` reference.

Start from the live file. Copy it aside first.

```sh
cp -a /opt/paseo/Caddyfile /opt/paseo/Caddyfile.pre-rollback
```

Delete the `cli.dark-forge.dev` site from `/opt/paseo/Caddyfile`. Leave `dark-forge.dev`, `http://app.nowcode.com`, and `http://hub.nowcode.com` as they are. Keep the override in `COMPOSE_FILE` until validate succeeds. Then:

```sh
grep -nE 'cli\.dark-forge\.dev|ca\.crt|tls_trust_pool|tls_trusted_ca_certs' /opt/paseo/Caddyfile
```

The grep must print nothing. Do not validate, and do not change `COMPOSE_FILE`, while the grep still matches. Then:

```sh
docker run --rm \
  --mount type=bind,source=/opt/paseo/Caddyfile,target=/etc/caddy/Caddyfile,readonly \
  --mount type=bind,source=/etc/kiln/ca.crt,target=/etc/kiln/ca.crt,readonly \
  sha256:df7f1c2fb114453b951de51a98efc010db1655a92c2e86be6706714e2417a78d \
  caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
```

If validate fails, copy `/opt/paseo/Caddyfile.pre-rollback` back and stop. Leave `COMPOSE_FILE` unchanged.

If validate prints `Valid configuration`, remove only the override from `COMPOSE_FILE`. The label file stays:

```text
COMPOSE_FILE=/opt/paseo/docker-compose.yml
```

Recreate caddy alone. Do not recreate paseo, hub, or the database.

```sh
docker compose up -d --no-deps caddy
```

If that recreate fails, put the override back on `COMPOSE_FILE`, restore `/opt/paseo/Caddyfile.pre-rollback`, validate it, and run `docker compose up -d --no-deps caddy` again.

### Checks after removing the override

Record the same status codes. Sign-in must be 200. `http://app.nowcode.com/` and `http://hub.nowcode.com/` must be 200. Record the `:443` status for `https://cli.dark-forge.dev/v1/templates`. That name is no longer a site, so do not expect the door 401. `docker inspect` will not show `172.18.0.3` from the override or the `/etc/kiln/ca.crt` mount. These commands were not run.

```sh
curl -sS -o /dev/null -w 'login:%{http_code}\n' --max-time 20 https://dark-forge.dev/login
curl -sS -o /dev/null -w 'app:%{http_code}\n' --max-time 20 http://app.nowcode.com/
curl -sS -o /dev/null -w 'hub:%{http_code}\n' --max-time 20 http://hub.nowcode.com/
curl -sS -o /dev/null -w 'door443:%{http_code}\n' --max-time 20 https://cli.dark-forge.dev/v1/templates
docker inspect caddy --format 'ip={{index .NetworkSettings.Networks.paseo_default.IPAddress}} {{range .Mounts}}{{.Destination}} RW={{.RW}} {{end}}'
```

Re-check caddy is at `172.18.0.3` after any paseo update while the override stays in `COMPOSE_FILE`. The door socket allows only that address. A plain `docker compose up -d` in `/opt/paseo` keeps the pin only while `COMPOSE_FILE` still lists `paseo-caddy.override.yaml` after every file in the `config_files` label. On this host that plain command also plans to recreate `paseo-hub` because of `pull_policy: always`.

## Preflight transcript

Remote script stdin, `Tue Oct 20 08:59:13 AM UTC 2026`. Exit 0.

```text
=== date -u ===
Tue Oct 20 08:59:13 AM UTC 2026
=== docker version ===
Client: Docker Engine - Community
 Version:           29.7.2
 API version:       1.55
 Go version:        go1.26.5
 Git commit:        a7dcaa6
 Built:             Wed Aug  5 18:29:26 2026
 OS/Arch:           linux/amd64
 Context:           default

Server: Docker Engine - Community
 Engine:
  Version:          29.7.2
  API version:      1.55 (minimum version 1.40)
  Go version:       go1.26.5
  Git commit:       6a43e3d
  Built:            Wed Aug  5 18:29:26 2026
  OS/Arch:          linux/amd64
  Experimental:     false
 containerd:
  Version:          v2.3.3
  GitCommit:        aad11006b869517fcd3009450b6f82da282e1a9b
 runc:
  Version:          1.4.3
  GitCommit:        v1.4.3-0-gbb14dabe
 docker-init:
  Version:          0.19.0
  GitCommit:        de40ad0
docker_version_exit:0
=== docker compose version ===
Docker Compose version v5.5.0
compose_version_exit:0
=== identity ===
MainPID=3522047
NRestarts=0
ActiveState=active
SubState=running
FragmentPath=/etc/systemd/system/kiln-identity.service
ActiveEnterTimestamp=Mon 2026-10-19 12:42:17 CEST
=== identity not restarted check ===
ExecMainStartTimestamp=Mon 2026-10-19 12:42:17 CEST
InvocationID=570fd98dce5f45c2bf70e3a93bce0824
=== authorized_keys sha ===
4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37  /root/.ssh/authorized_keys
=== ca_pem hash ===
active_rows 2
id=srv_fcefac0410a846dcb13a1a83c1a9235c name=ser7 origin=https://192.168.50.210:8443 active=1 ca_pem_len=1501 ca_pem_sha256=4c83f0d254a803507fb24e66a25a07e07deaef34998247a7aa74150395d79abb
id=srv_37e8ac65c6524713a9e646efa39b7e2e name=dark-forge origin=https://cli.dark-forge.dev active=1 ca_pem_len=1501 ca_pem_sha256=f7bb4b2cb0e335047a536b2aeb11eac9489e2d4db87ecc067fdf652dbd8406bf
=== firecracker jailer kiln ===
+ command -v
/opt/kiln/bin/firecracker
/opt/kiln/bin/jailer
/opt/kiln/bin/kiln
/opt/kiln/bin/kiln-runtime
+ firecracker --version
Firecracker v1.17.0

2026-10-20T10:59:13.330581521 [anonymous-instance:main] Firecracker exiting successfully. exit_code=0
firecracker_exit:0
+ jailer --version
Jailer v1.17.0

jailer_exit:0
+ kiln --version
kiln 0.4.9
kiln_exit:0
+ kiln-runtime doctor
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
=== door sockets and listeners ===
MainPID=4167939
NRestarts=0
Id=kiln-door.service
ActiveState=active
SubState=running
UnitFileState=enabled
ActiveEnterTimestamp=Tue 2026-10-20 10:06:14 CEST

MainPID=4187211
NRestarts=0
Id=kiln-door-proxy.service
ActiveState=active
SubState=running
UnitFileState=static
ActiveEnterTimestamp=Tue 2026-10-20 10:43:50 CEST

Id=kiln-door-proxy.socket
ActiveState=active
SubState=running
UnitFileState=enabled
ActiveEnterTimestamp=Tue 2026-10-20 10:06:13 CEST

Id=kiln-door-proxy-public.socket
ActiveState=active
SubState=running
UnitFileState=enabled
ActiveEnterTimestamp=Tue 2026-10-20 10:06:13 CEST
+ ss 8443/8444
Netid State  Recv-Q Send-Q Local Address:Port Peer Address:PortProcess                                                               
tcp   LISTEN 0      4096      172.18.0.1:8444      0.0.0.0:*    users:(("systemd-socket-",pid=4187211,fd=3),("systemd",pid=1,fd=110))
tcp   LISTEN 0      4096     78.46.98.73:8443      0.0.0.0:*    users:(("systemd-socket-",pid=4187211,fd=4),("systemd",pid=1,fd=135))
tcp   LISTEN 0      4096         0.0.0.0:80        0.0.0.0:*    users:(("docker-proxy",pid=1302820,fd=8))                            
tcp   LISTEN 0      4096         0.0.0.0:443       0.0.0.0:*    users:(("docker-proxy",pid=1302843,fd=8))                            
tcp   LISTEN 0      128        127.0.0.1:8444      0.0.0.0:*    users:(("kiln-api",pid=4167939,fd=9))                                
tcp   LISTEN 0      128        127.0.0.1:8443      0.0.0.0:*    users:(("kiln-api",pid=4165125,fd=9))                                
tcp   LISTEN 0      4096            [::]:80           [::]:*    users:(("docker-proxy",pid=1302827,fd=8))                            
tcp   LISTEN 0      4096            [::]:443          [::]:*    users:(("docker-proxy",pid=1302851,fd=8))                            
=== :8443 fingerprint ===
sha256 Fingerprint=D4:BA:F2:89:F2:62:5D:E8:67:57:2D:4F:97:F8:CB:B4:D7:8B:27:BE:FA:63:0B:53:A8:DB:3E:0C:74:C2:7D:3A
fingerprint_exit:0
=== caddy containers ===
CONTAINER ID   NAMES          IMAGE                         STATUS                 PORTS
a398b31a015f   paseo          paseo-with-pi                 Up 6 days (healthy)    127.0.0.1:6767->6767/tcp
ef0635c603c3   paseo-hub      ghcr.io/getpaseo/hub:latest   Up 2 weeks (healthy)   3000/tcp
826e7f164f64   caddy          caddy:2                       Up 6 weeks             0.0.0.0:80->80/tcp, [::]:80->80/tcp, 0.0.0.0:443->443/tcp, [::]:443->443/tcp, 443/udp, 2019/tcp
970927c154c9   paseo-hub-db   postgres:17-alpine            Up 8 weeks (healthy)   5432/tcp
=== caddy selected inspect ===
caddy_ids ['826e7f164f64']
{
  "ConfigImage": "caddy:2",
  "HostConfigBinds": [
    "/opt/paseo/Caddyfile:/etc/caddy/Caddyfile:ro",
    "paseo_caddy_data:/data:rw",
    "paseo_caddy_config:/config:rw"
  ],
  "Id": "826e7f164f64c9858a33616f958e94627ebce18d9b672bed62d6852a652d126d",
  "Image": "sha256:df7f1c2fb114453b951de51a98efc010db1655a92c2e86be6706714e2417a78d",
  "Labels": {
    "com.docker.compose.container-number": "1",
    "com.docker.compose.oneoff": "False",
    "com.docker.compose.project": "paseo",
    "com.docker.compose.project.config_files": "/opt/paseo/docker-compose.yml",
    "com.docker.compose.project.working_dir": "/opt/paseo",
    "com.docker.compose.service": "caddy"
  },
  "Mounts": [
    {
      "Destination": "/etc/caddy/Caddyfile",
      "Mode": "ro",
      "Name": null,
      "Propagation": "rprivate",
      "RW": false,
      "Source": "/opt/paseo/Caddyfile",
      "Type": "bind"
    },
    {
      "Destination": "/data",
      "Mode": "rw",
      "Name": "paseo_caddy_data",
      "Propagation": "",
      "RW": true,
      "Source": "/var/lib/docker/volumes/paseo_caddy_data/_data",
      "Type": "volume"
    },
    {
      "Destination": "/config",
      "Mode": "rw",
      "Name": "paseo_caddy_config",
      "Propagation": "",
      "RW": true,
      "Source": "/var/lib/docker/volumes/paseo_caddy_config/_data",
      "Type": "volume"
    }
  ],
  "Name": "/caddy",
  "Networks": {
    "paseo_default": {
      "Gateway": "172.18.0.1",
      "IPAddress": "172.18.0.3",
      "IPPrefixLen": 16,
      "MacAddress": "7e:8d:7f:5c:b1:c8",
      "NetworkID": "164dac86db639d885f7341d8bd2ab0ebba82b5cc76d95c2cc60257d5885a7780"
    }
  },
  "State": {
    "Pid": 1302766,
    "Running": true,
    "StartedAt": "2026-09-05T00:20:11.922786918Z",
    "Status": "running"
  }
}
=== paseo_default IPAM and containers ===
[{"Subnet":"172.18.0.0/16","Gateway":"172.18.0.1"}]

"164dac86db639d885f7341d8bd2ab0ebba82b5cc76d95c2cc60257d5885a7780" "paseo_default" "bridge"

IPAM [{"Subnet": "172.18.0.0/16", "Gateway": "172.18.0.1"}]
subnet_contains_pin 172.18.0.0/16 yes
preflight_subnet True
holder name=caddy id=826e7f164f64 ip=172.18.0.3/16
holder name=paseo-hub-db id=970927c154c9 ip=172.18.0.4/16
holder name=paseo id=a398b31a015f ip=172.18.0.2/16
holder name=paseo-hub id=ef0635c603c3 ip=172.18.0.5/16
pin_holder_count 1
pin_holder name=caddy project=paseo service=caddy
preflight_only_caddy True
=== paseo .env keys and COMPOSE_FILE ===
exists False
=== compose files on disk ===
total 48
drwxr-xr-x  5 root root 4096 Oct 20 10:03 .
drwxr-xr-x  5 root root 4096 Oct 19 17:11 ..
drwxr-xr-x  2 root root 4096 Sep  5 01:40 backups
-rw-r--r--  1 root root  887 Oct 20 10:03 Caddyfile
-rw-r--r--  1 root root  717 Oct  3 20:15 Caddyfile.bak-door-443
-rw-r--r--  1 root root 1880 Sep 24 01:17 docker-compose.yml
-rw-r--r--  1 root root 1816 Sep  1 14:58 docker-compose.yml.bak
-rw-r--r--  1 root root 1925 Sep  1 15:10 docker-compose.yml.bak.20260904T234203Z
-rw-r--r--  1 root root  676 Oct 13 23:14 Dockerfile
drwxr-xr-x 16 1000 1000 4096 Oct 13 23:20 paseo-home
-rw-------  1 root root  125 Sep 24 01:14 typesafe.env
drwxr-xr-x  9 1000 1000 4096 Sep 24 01:26 workspace
=== Caddyfile sha and sites ===
90954669459825dbd89d15c36a586e5f0f56ffd6734978560c1268f4a82061bf  /opt/paseo/Caddyfile
bytes 887
1: http://app.nowcode.com {
2: 	reverse_proxy paseo:6767
3: }
4: 
5: http://hub.nowcode.com {
6: 	reverse_proxy hub:3000 {
7: 		header_up X-Forwarded-Proto https
8: 	}
9: }
10: 
11: # No access logs or debug logging: OAuth URLs contain one-use credentials.
12: dark-forge.dev {
13: 	header {
14: 		Cache-Control "no-store"
15: 		X-Content-Type-Options "nosniff"
16: 		X-Robots-Tag "noindex, nofollow"
17: 	}
18: 	request_body {
19: 		max_size 80KB
20: 	}
21: 	redir / /login 302
22: 	reverse_proxy 172.18.0.1:8092 {
23: 		header_up X-Forwarded-For {http.request.remote.host}
24: 		transport http {
25: 			dial_timeout 3s
26: 			response_header_timeout 75s
27: 		}
28: 	}
29: 	# Successfully handled errors avoid ERROR-level request/URL logging.
30: 	handle_errors {
31: 		respond "Kiln sign-in is temporarily unavailable." 503
32: 	}
33: }
34: 
35: cli.dark-forge.dev {
36: 	reverse_proxy https://172.18.0.1:8444 {
37: 		transport http {
38: 			tls_server_name cli.dark-forge.dev
39: 			tls_trusted_ca_certs /etc/kiln/ca.crt
40: 		}
41: 	}
42: }
=== ca.crt presence ===
mode=644 size=1501 /etc/kiln/ca.crt
f7bb4b2cb0e335047a536b2aeb11eac9489e2d4db87ecc067fdf652dbd8406bf  /etc/kiln/ca.crt
=== private key modes only ===
mode=600 /etc/kiln/ca.key
mode=640 /etc/kiln/door.key
mode=600 /etc/kiln/directory.token
mode=600 /etc/kiln/admin.token
=== rollback copy marker ===
drwx------ 5 root root 4096 Oct 20 10:02 /opt/kiln/upgrades
/opt/kiln/upgrades/0.4.4-8DDh4B
/opt/kiln/upgrades/0.4.5-RSNr4F
/opt/kiln/upgrades/0.4.6-3vO6d0
PREFLIGHT_DONE
```

## Apply transcript

Remote script stdin, backup directory `/root/kiln-caddy-backup-20261020T090400Z`. Exit 0. Private key paths were removed from this copy. The sorted find lists matched before that removal.

```text
+ mkdir -m 700 /root/kiln-caddy-backup-20261020T090400Z
BACKUP_DIR=/root/kiln-caddy-backup-20261020T090400Z
+ cp -a /opt/paseo/Caddyfile
+ cp -a /opt/paseo/docker-compose.yml
ENV_EXISTED=no
+ docker inspect caddy > backup (on host, mode 600)
+ image id
sha256:df7f1c2fb114453b951de51a98efc010db1655a92c2e86be6706714e2417a78d
+ tag id
sha256:df7f1c2fb114453b951de51a98efc010db1655a92c2e86be6706714e2417a78d
+ volume file names
/config/caddy/autosave.json
/data/caddy/acme/acme-v02.api.letsencrypt.org-directory/users/default/default.json
/data/caddy/certificates/acme-v02.api.letsencrypt.org-directory/app.nowcode.com/app.nowcode.com.crt
/data/caddy/certificates/acme-v02.api.letsencrypt.org-directory/app.nowcode.com/app.nowcode.com.json
/data/caddy/certificates/acme-v02.api.letsencrypt.org-directory/cli.dark-forge.dev/cli.dark-forge.dev.crt
/data/caddy/certificates/acme-v02.api.letsencrypt.org-directory/cli.dark-forge.dev/cli.dark-forge.dev.json
/data/caddy/certificates/acme-v02.api.letsencrypt.org-directory/dark-forge.dev/dark-forge.dev.crt
/data/caddy/certificates/acme-v02.api.letsencrypt.org-directory/dark-forge.dev/dark-forge.dev.json
/data/caddy/instance.uuid
/data/caddy/last_clean.json
+ started at before
/caddy 826e7f164f64c9858a33616f958e94627ebce18d9b672bed62d6852a652d126d 2026-09-05T00:20:11.922786918Z running
/paseo a398b31a015fdc30ae14322c46c3957ee96c24268df0709386d98d550c4774ab 2026-10-13T21:14:56.680741291Z running
/paseo-hub ef0635c603c33f07e1aacba6735a4d7db2eb6cd6853451d6c7c869e63b227bc7 2026-09-29T21:26:23.879694609Z running
/paseo-hub-db 970927c154c9b6a04b185de08b5d3a5cdaba553554c608db78a2c977a6e631f2 2026-08-22T22:50:09.192488365Z running
+ sha256 backups
90954669459825dbd89d15c36a586e5f0f56ffd6734978560c1268f4a82061bf  /root/kiln-caddy-backup-20261020T090400Z/Caddyfile
29da2e7db3edf0a904ecdf584ccb2c4af72cae509322d00722a1ba46426b555b  /root/kiln-caddy-backup-20261020T090400Z/docker-compose.yml
90954669459825dbd89d15c36a586e5f0f56ffd6734978560c1268f4a82061bf  /opt/paseo/Caddyfile
29da2e7db3edf0a904ecdf584ccb2c4af72cae509322d00722a1ba46426b555b  /opt/paseo/docker-compose.yml
IMAGE_MATCH sha256:df7f1c2fb114453b951de51a98efc010db1655a92c2e86be6706714e2417a78d
+ install /opt/paseo/paseo-caddy.override.yaml
855f22f41c017fdc0720873c9a93a8b6083cd2ed3fb87ef1ba58a3092851e7e6  /opt/paseo/paseo-caddy.override.yaml
+ set COMPOSE_FILE from config_files label plus override
config_files_label=/opt/paseo/docker-compose.yml
COMPOSE_FILE=/opt/paseo/docker-compose.yml:/opt/paseo/paseo-caddy.override.yaml
+ docker compose config (saved on host, safe fields only)
services caddy hub paseo postgres
image caddy:2
container_name caddy
ports [{"mode": "ingress", "protocol": "tcp", "published": "80", "target": 80}, {"mode": "ingress", "protocol": "tcp", "published": "443", "target": 443}]
networks {"default": {"ipv4_address": "172.18.0.3"}}
volumes [{"read_only": true, "source": "/opt/paseo/Caddyfile", "target": "/etc/caddy/Caddyfile", "type": "bind"}, {"read_only": null, "source": "caddy_data", "target": "/data", "type": "volume"}, {"read_only": null, "source": "caddy_config", "target": "/config", "type": "volume"}, {"read_only": true, "source": "/etc/kiln/ca.crt", "target": "/etc/kiln/ca.crt", "type": "bind"}]
top_volumes caddy_config caddy_data postgres-data
pin_network default 172.18.0.3
COMPOSE_CONFIG_GATES_OK
COMPOSE_CONFIG_GATES_EXIT:0
+ edit door site to tls_trust_pool file
tls_trusted_ca_certs_lines 1
CADDYFILE_EDITED
CADDYFILE_EDIT_EXIT:0
+ diff Caddyfile
--- /root/kiln-caddy-backup-20261020T090400Z/Caddyfile	2026-10-20 10:03:38.561163519 +0200
+++ /opt/paseo/Caddyfile	2026-10-20 11:04:00.633627378 +0200
@@ -36,7 +36,9 @@
 	reverse_proxy https://172.18.0.1:8444 {
 		transport http {
 			tls_server_name cli.dark-forge.dev
-			tls_trusted_ca_certs /etc/kiln/ca.crt
+			tls_trust_pool file {
+				pem_file /etc/kiln/ca.crt
+			}
 		}
 	}
 }
7d2c3722f08e360d125c18cdcbb95faceb692032df704a61893657083434475f  /opt/paseo/Caddyfile
90954669459825dbd89d15c36a586e5f0f56ffd6734978560c1268f4a82061bf  /root/kiln-caddy-backup-20261020T090400Z/Caddyfile
+ caddy validate with the running image
Valid configuration
VALIDATE_OK
+ docker compose up -d --force-recreate --no-deps caddy
RECREATE_OK
caddy_id 14b7fcb812c35e320a4817a2731ef003eec77df247270945c72f6ca21188bd8b
caddy_image sha256:df7f1c2fb114453b951de51a98efc010db1655a92c2e86be6706714e2417a78d
caddy_ip 172.18.0.3
caddy_status running running True
ca_mount {"Destination": "/etc/kiln/ca.crt", "Name": null, "RW": false, "Source": "/etc/kiln/ca.crt", "Type": "bind"}
volume_names paseo_caddy_config paseo_caddy_data
INSPECT_GATES ok
INSPECT_GATES_EXIT:0
+ mount table line
/dev/md2 /etc/kiln/ca.crt ext4 ro,relatime 0 0
+ ca sha inside
f7bb4b2cb0e335047a536b2aeb11eac9489e2d4db87ecc067fdf652dbd8406bf  /etc/kiln/ca.crt
+ volume files after
/config/caddy/autosave.json
/data/caddy/acme/acme-v02.api.letsencrypt.org-directory/users/default/default.json
/data/caddy/certificates/acme-v02.api.letsencrypt.org-directory/app.nowcode.com/app.nowcode.com.crt
/data/caddy/certificates/acme-v02.api.letsencrypt.org-directory/app.nowcode.com/app.nowcode.com.json
/data/caddy/certificates/acme-v02.api.letsencrypt.org-directory/cli.dark-forge.dev/cli.dark-forge.dev.crt
/data/caddy/certificates/acme-v02.api.letsencrypt.org-directory/cli.dark-forge.dev/cli.dark-forge.dev.json
/data/caddy/certificates/acme-v02.api.letsencrypt.org-directory/dark-forge.dev/dark-forge.dev.crt
/data/caddy/certificates/acme-v02.api.letsencrypt.org-directory/dark-forge.dev/dark-forge.dev.json
/data/caddy/instance.uuid
/data/caddy/last_clean.json
+ volume file diff
VOLUME_FILES_UNCHANGED
public_attempt=1
+ public checks from host via 127.0.0.1
issuer=C=US, O=Let's Encrypt, CN=YE2
X509v3 Subject Alternative Name: 
    DNS:cli.dark-forge.dev
templates_http_code:401
templates_body:{"code":"unauthorized","message":"A valid credential is required."}
login_http_code:200
login_phrase:yes
nowcode_app:200
nowcode_hub:200
PUBLIC_GATES_OK
+ non-caddy source to bridge socket
host_bridge errno=None err=None
paseo_bridge timeout
PASEO_BRIDGE_EXIT:0
+ admin-token 401 on the host
admin_http_code:401
admin_body:{"code":"unauthorized","message":"A valid credential is required."}
admin_status:HTTP/2 401 
admin_token_leaked:no
admin_header_shredded True
+ :8443 fingerprint
sha256 Fingerprint=D4:BA:F2:89:F2:62:5D:E8:67:57:2D:4F:97:F8:CB:B4:D7:8B:27:BE:FA:63:0B:53:A8:DB:3E:0C:74:C2:7D:3A
+ plain compose up dry-run
 Image ghcr.io/getpaseo/hub:latest Pulling 
 Image ghcr.io/getpaseo/hub:latest Pulled 
 Container caddy Running 
 Container paseo Running 
 Container paseo-hub-db Running 
 Container paseo-hub Recreate 
 Container paseo-hub Recreated 
 Container paseo-hub-db Waiting 
 Container paseo-hub-db Healthy 
 Container ef0635c603c3_paseo-hub Starting 
 Container ef0635c603c3_paseo-hub Started 
DRY_RUN_EXIT:0
PLAIN_UP_SKIPPED
after_plain_up_ip=172.18.0.3 id=14b7fcb812c35e320a4817a2731ef003eec77df247270945c72f6ca21188bd8b image=sha256:df7f1c2fb114453b951de51a98efc010db1655a92c2e86be6706714e2417a78d
+ started at after plain up
/caddy 14b7fcb812c35e320a4817a2731ef003eec77df247270945c72f6ca21188bd8b 2026-10-20T09:04:01.860181622Z running
/paseo a398b31a015fdc30ae14322c46c3957ee96c24268df0709386d98d550c4774ab 2026-10-13T21:14:56.680741291Z running
/paseo-hub ef0635c603c33f07e1aacba6735a4d7db2eb6cd6853451d6c7c869e63b227bc7 2026-09-29T21:26:23.879694609Z running
/paseo-hub-db 970927c154c9b6a04b185de08b5d3a5cdaba553554c608db78a2c977a6e631f2 2026-08-22T22:50:09.192488365Z running
+ ca mount still read-only
/config RW=true Name=paseo_caddy_config Source=/var/lib/docker/volumes/paseo_caddy_config/_data
/etc/kiln/ca.crt RW=false Name= Source=/etc/kiln/ca.crt
/etc/caddy/Caddyfile RW=false Name= Source=/opt/paseo/Caddyfile
/data RW=true Name=paseo_caddy_data Source=/var/lib/docker/volumes/paseo_caddy_data/_data

+ identity and authorized_keys after
MainPID=3522047
NRestarts=0
ActiveState=active
ActiveEnterTimestamp=Mon 2026-10-19 12:42:17 CEST
4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37  /root/.ssh/authorized_keys
id=srv_fcefac0410a846dcb13a1a83c1a9235c name=ser7 origin=https://192.168.50.210:8443 ca_pem_sha256=4c83f0d254a803507fb24e66a25a07e07deaef34998247a7aa74150395d79abb
id=srv_37e8ac65c6524713a9e646efa39b7e2e name=dark-forge origin=https://cli.dark-forge.dev ca_pem_sha256=f7bb4b2cb0e335047a536b2aeb11eac9489e2d4db87ecc067fdf652dbd8406bf
+ sockets still up
Id=kiln-door-proxy.socket
ActiveState=active
SubState=running
ActiveEnterTimestamp=Tue 2026-10-20 10:06:13 CEST

Id=kiln-door-proxy-public.socket
ActiveState=active
SubState=running
ActiveEnterTimestamp=Tue 2026-10-20 10:06:13 CEST

Id=kiln-door.service
ActiveState=active
SubState=running
ActiveEnterTimestamp=Tue 2026-10-20 10:06:14 CEST
State  Recv-Q Send-Q Local Address:Port Peer Address:Port
LISTEN 0      4096      172.18.0.1:8444      0.0.0.0:*   
LISTEN 0      4096     78.46.98.73:8443      0.0.0.0:*   
LISTEN 0      128        127.0.0.1:8444      0.0.0.0:*   
LISTEN 0      128        127.0.0.1:8443      0.0.0.0:*   
+ smoke fresh config
smoke_config_dir=/var/tmp/kiln-smoke.Dunnlf
+ kiln profile add
added profile default
profile_add_exit:0
token_embedded False
+ kiln create
NAME              ID                                STATE           MEMORY  VCPUS
kiln-caddy-smoke  e5dbbcf40b1a404caf71c1aae3b980a9  running       4096 MiB  1
create_exit:0
+ kiln exec uname -a
Linux box-e5dbbcf40b1a 6.1.155+ #1 SMP PREEMPT_DYNAMIC Thu Dec 18 15:17:16 UTC 2025 x86_64 x86_64 x86_64 GNU/Linux
exec_exit:0
+ kiln delete
deleted e5dbbcf40b1a404caf71c1aae3b980a9
delete_exit:0
+ kiln list
No boxes yet. Create one with `kiln create`.
list_exit:0
SMOKE_DIR_REMOVED
APPLY_DONE
```

Apply stderr:

```text
{"level":"info","ts":1792487040.9594803,"msg":"using config from file","file":"/etc/caddy/Caddyfile"}
{"level":"warn","ts":1792487040.9599726,"logger":"caddyfile","msg":"Unnecessary header_up X-Forwarded-For: the reverse proxy's default behavior is to pass headers to the upstream"}
{"level":"info","ts":1792487040.9605982,"msg":"adapted config to JSON","adapter":"caddyfile"}
{"level":"info","ts":1792487040.9609377,"logger":"http.auto_https","msg":"server is listening only on the HTTPS port but has no TLS connection policies; adding one to enable TLS","server_name":"srv0","https_port":443}
{"level":"info","ts":1792487040.9609487,"logger":"http.auto_https","msg":"enabling automatic HTTP->HTTPS redirects","server_name":"srv0"}
{"level":"warn","ts":1792487040.960958,"logger":"http.auto_https","msg":"server is listening only on the HTTP port, so no automatic HTTPS will be applied to this server","server_name":"srv1","http_port":80}
{"level":"info","ts":1792487040.9610245,"logger":"tls.cache.maintenance","msg":"started background certificate maintenance","cache":"0x382264316200"}
{"level":"info","ts":1792487040.961574,"logger":"http","msg":"servers shutting down with eternal grace period"}
{"level":"info","ts":1792487040.961625,"logger":"tls.cache.maintenance","msg":"stopped background certificate maintenance","cache":"0x382264316200"}
 Container caddy Recreate 
 Container caddy Recreated 
 Container caddy Starting 
 Container caddy Started 
creating kiln-caddy-smoke with template ubuntu-4g
request 85a8fc6e3e9841ff907c7d642e18f755
request 31f741c5de68404d925b40cc91426ef2
request 0aa3ca2b0d40444fb357400154c35369
```

## Follow-up transcript

`docker compose up -d --no-deps caddy`, bridge errors, and the unchanged hub container. Exit 0.

```text
=== date ===
Tue Oct 20 09:05:12 AM UTC 2026
+ docker compose up -d --no-deps caddy
NO_DEPS_UP_EXIT:0
id=14b7fcb812c35e320a4817a2731ef003eec77df247270945c72f6ca21188bd8b ip=172.18.0.3 status=running started=2026-10-20T09:04:01.860181622Z image=sha256:df7f1c2fb114453b951de51a98efc010db1655a92c2e86be6706714e2417a78d
+ mounts
/etc/caddy/Caddyfile RW=false Name= Type=bind
/data RW=true Name=paseo_caddy_data Type=volume
/config RW=true Name=paseo_caddy_config Type=volume
/etc/kiln/ca.crt RW=false Name= Type=bind

+ bridge from host
TimeoutError errno None str timed out
+ curl bridge from host
curl_bridge_exit:28
+ tcp from caddy container
caddy_wget_exit:1
+ tcp from paseo container
timeout
paseo_node_exit:0
+ hub still the same container
id=ef0635c603c33f07e1aacba6735a4d7db2eb6cd6853451d6c7c869e63b227bc7 status=running health=healthy started=2026-09-29T21:26:23.879694609Z image=sha256:090d987c6d1d163f5492439dd9150233c17a8d16fc6d6c5f4e1a570f93a12b88
+ other services
/paseo a398b31a015fdc30ae14322c46c3957ee96c24268df0709386d98d550c4774ab running 2026-10-13T21:14:56.680741291Z
/paseo-hub-db 970927c154c9b6a04b185de08b5d3a5cdaba553554c608db78a2c977a6e631f2 running 2026-08-22T22:50:09.192488365Z
+ .env and override sha
ef9dccedf3549ec7f4ec24182a8b2a5848b0c4a6295bd0fdb9a1a3c7573491ee  /opt/paseo/.env
855f22f41c017fdc0720873c9a93a8b6083cd2ed3fb87ef1ba58a3092851e7e6  /opt/paseo/paseo-caddy.override.yaml
7d2c3722f08e360d125c18cdcbb95faceb692032df704a61893657083434475f  /opt/paseo/Caddyfile
+ COMPOSE_FILE
COMPOSE_FILE=/opt/paseo/docker-compose.yml:/opt/paseo/paseo-caddy.override.yaml
+ identity authorized_keys
MainPID=3522047
NRestarts=0
ActiveEnterTimestamp=Mon 2026-10-19 12:42:17 CEST
4a625f4d67cf8e7123cf96c78b21863d23789ef933e20ea5b35c1d068e790f37  /root/.ssh/authorized_keys
FOLLOWUP_DONE
```

Follow-up stderr:

```text
 Container caddy Running 
*   Trying 172.18.0.1:8444...
* Connection timed out after 3002 milliseconds
* closing connection #0
289BDA8F707F0000:error:0A000086:SSL routines:tls_post_process_server_certificate:certificate verify failed:ssl/statem/statem_clnt.c:2124:
ssl_client: SSL_connect
wget: error getting response: Connection reset by peer
```

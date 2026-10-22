# Laptop client and single-host service

For prebuilt downloads and the unattended first-install workflow, start with the
[release guide](releases.md). The manual instructions below remain useful
for custom hosts and development; they are not required when installing a release
package on a supported Ubuntu host.

`kiln` manages boxes over **verified HTTPS**. Only the server needs Linux,
Firecracker and KVM. This is a single-administrator, trusted-workload pilot:
lifecycle management, buffered commands, SSH terminals, SFTP and opt-in isolated
IPv4 egress are implemented. Previews and multi-tenant security are not included.
SSH/networking require matching v0.2.0 client/server/guest builds; v0.1.1
packages and existing templates do not include them.

## Everyday commands (v0.4.2 client)

```sh
kiln create                         # the only available template; generated name
kiln create --name my-project       # choose your own name
kiln list
kiln ssh my-project
kiln exec my-project -- uname -a
kiln cp ./README.md my-project:/workspace/README.md
kiln stop my-project
kiln start my-project
```

Box commands accept an exact, case-sensitive name, a full ID, or a unique
lowercase hexadecimal ID prefix of at least eight characters. Full IDs always
address that ID directly. Other inputs match names and ID prefixes together;
ambiguous inputs fail with the candidates' full IDs, never selecting one silently.
Names are labels, not server-enforced unique identifiers. Existing names are
unchanged. Generated names look like `quiet-otter-19a7f302`; the suffix reduces
collisions and the whole name is stable for a given request ID.

Omitting `--template` works only when the server publishes exactly one template.
With multiple templates, choose explicitly using `--template NAME`; the CLI
lists the choices rather than prompting or guessing. No profile migration or
configured default is required.

Human-readable lists show names first, short IDs and column headings. `inspect`
and mutation results retain full IDs. Scripts should use `--json`: its schema,
full IDs, binary exec output and exit status behavior are unchanged. Progress
goes to stderr. Stopped/paused execution and SSH failures suggest `start`/`resume`
without automatically changing the box state.

`kiln create --wait` blocks until the create operation finishes **and** the host
has recorded a running box (the same readiness the dashboard uses). The server
accepts `POST /v1/operations?wait=true&wait_seconds=SECONDS` for create only
(default 300, maximum 3605). Success returns HTTP 200 with the final operation;
timeout returns HTTP 408 with code `wait_timeout`. Without `--wait`, create still
returns 202 and the client polls as before.

After a disconnected mutation, query the printed request ID with
`kiln operation REQUEST_ID --wait` before retrying. If a retry is needed, use
the same `--request-id`, resolved full box ID, and original arguments (including
the printed template/name for creation). Names or catalogs can change between
invocations; full IDs and explicit template/name arguments avoid that ambiguity.
Do not retry an old client's unnamed create with a newer client's defaults.

The existing `kiln-runtime` binary remains the local Linux operator tool. It is not the
laptop client. Do not run local mutations against the runtime while its host
service is managing it; stop the host service first for template administration.

```text
Laptop kiln → HTTPS + bearer token → kiln-api (unprivileged)
                                       ↓ restricted Unix socket
                                    kiln-host (root, isolated profile)
                                       ↓ existing Firecracker/jailer runtime
                                    Linux guest via vsock
```

There is no mandatory SSH tunnel for management. Guest SSH streams use the same
authenticated HTTPS endpoint and a fixed vsock service, without opening port 22
or requiring a guest IP reachable from the laptop. SQLite is embedded; no
PostgreSQL or Redis is required. The service exposes only catalog aliases and sanitized box records,
not host paths, jail identities, host fingerprints or Firecracker sockets.

## Central login in v0.4.0

The installer includes browser login with `https://dark-forge.dev` built in.
After an operator updates and explicitly enrolls a host, run `kiln login`.
The browser asks for GitHub/Google sign-in and a separate device approval. One
owned server is selected automatically. Several servers prompt on a terminal.
An unanswered prompt keeps the approved session and prints `kiln login --server`
with each server id. A later login uses that session instead of a new device code.
No private server address or CA transfer is needed for this flow. The laptop
still needs network access to the selected server.

Interactive first-use commands can start the same flow without a URL flag.
JSON and SSH proxy invocations never launch login. A non-terminal `kiln login`
requests a device code and polls until it is approved or expires.
Renewal occurs before requests, without retrying VM mutations. `kiln auth status`,
device revocation, server-scoped automation keys and logout are documented in the
[identity operator guide](identity-service.md). Repeating login reuses an existing
session; log out to change accounts. Existing direct administrator profiles below
are preserved; use `kiln --profile personal login` for a separate central profile.
No enrolled server means login stops with enrollment instructions, not a usable
VM profile. Use `--issuer` or `KILN_IDENTITY_URL` for another identity service.
Real account consent and full central-authenticated VM acceptance remain
outstanding; this is a pilot release.

## Build the binaries

From this checkout on a laptop with the pinned Rust 1.95.0 toolchain:

```sh
cargo build --release --locked -p kiln-client --bin kiln
# Or install only the client into your Cargo bin directory:
cargo install --locked --path crates/kiln-client
```

Build natively on the laptop OS/architecture. The client crate does not depend on
`kiln-runtime`, SQLite, Firecracker or Linux/vsock APIs. This repository does not
yet provide signed installers. Release automation builds downloadable binaries
once the source is published to GitHub and the workflow succeeds. Unix builds
enforce mode 0600/0400 on token files. The client uses bundled public CA roots or an explicit private CA file,
not an insecure certificate-verification switch. It connects directly (proxy
environment variables are not used).

On the Linux x86_64 host:

```sh
cargo build --release --locked -p kiln-runtime --bin kiln-runtime \
  -p kiln-server --bin kiln-host --bin kiln-api
```

## Prepare the server before exposing the API

The commands below are installation instructions, **not actions performed by
building this feature**. They change host users, files or services and require
operator approval. A private install stays on a private address. dark-forge is
the public exception, and only through the identity-checked door in
`docs/runbooks/2026-10-19-public-door.md`.

1. Follow the [isolated-runtime setup](runtime.md#experimental-isolated-profile):
   matching root-owned Firecracker/jailer 1.17.0, KVM, pre-enabled cgroup v2
   controllers, a dedicated unused eight-identity UID/GID range and trusted images.
   Keep these binaries in `/opt/kiln/bin`; the isolation policy and service PATH
   must select exactly those binaries. Optional snapshot overlays require the
   separately documented storage setup. The services never provision host routing,
   firewall rules, cgroup controllers or storage pools automatically.
2. Create an unprivileged system account/group named `kiln-api`. Install
   `kiln-host` and `kiln-api` as root-owned executables in `/usr/local/libexec`,
   and `kiln-runtime` in `/opt/kiln/bin`. Create root-owned `/etc/kiln` (0755) and
   `/var/lib/kiln` (0700). Keep all runtime/image path ancestors root-owned and
   not group/other writable; a directory under your home is not suitable.
3. Put the trusted isolation policy in `/etc/kiln/isolation.json`. Copy
   [`deploy/host.example.json`](../deploy/host.example.json) to
   `/etc/kiln/host.json` (root-owned, 0600). The initially empty catalog is valid.
4. Build templates **on this host**, in this runtime store, using the local tool.
   Snapshots are host-specific; copying a template from the development runner
   does not make it portable. For a prepared, root-owned Ubuntu image:

   ```sh
   sudo env PATH=/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin \
     /opt/kiln/bin/kiln-runtime --state-dir /var/lib/kiln/runtime \
     --isolation-config /etc/kiln/isolation.json doctor
   sudo env PATH=/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin \
     /opt/kiln/bin/kiln-runtime --state-dir /var/lib/kiln/runtime \
     --isolation-config /etc/kiln/isolation.json template build \
     --image /var/lib/kiln/images/ubuntu/image.json \
     --memory-mib 4096 --vcpus 1 --profile isolated
   ```

   Add the returned template ID to `host.json`:

   ```json
   "templates": { "ubuntu-4g": "THE_32_HEX_TEMPLATE_ID_FROM_THE_BUILD" }
   ```

   Aliases use 1–63 ASCII letters, digits, hyphens or underscores. Templates
   determine CPU/RAM; publish another template for another size. Remote clients
   cannot override quotas or supply host image paths. Current runtime limits are
   eight boxes and 16 GiB of allocated guest RAM, including stopped boxes.
5. Provision a TLS certificate/key whose SAN matches the hostname used by the
   laptop. Use a trusted issuer or a private CA whose public certificate you copy
   to the laptop. The server certificate must be a leaf (`CA:FALSE`), not the CA's
   signing certificate itself. Store `tls.crt` and `tls.key` in `/etc/kiln`, readable by
   `kiln-api`; keep the key owner-only. Certificate issuance/renewal is an operator
   responsibility. The service has no HTTP fallback and does not obtain ACME
   certificates automatically.
6. Generate a 256-bit random token without displaying it:

   ```sh
   sudo sh -c 'umask 077; openssl rand -hex 32 > /etc/kiln/admin.token'
   sudo chown kiln-api:kiln-api /etc/kiln/admin.token
   sudo chmod 600 /etc/kiln/admin.token
   ```

   Transfer it securely to a private file on your laptop (not via chat, command
   arguments or a URL). It grants **administrator access to all service-managed
   boxes** in this single workspace. This direct token is not scoped; central
   development builds separately support server-scoped `read`/`operate` keys.
7. Check the configuration while the host service is stopped:

   ```sh
   sudo env PATH=/opt/kiln/bin:/usr/sbin:/usr/bin:/sbin:/bin \
     /usr/local/libexec/kiln-host --config /etc/kiln/host.json --check
   ```

   This opens/initializes private state, verifies catalog references and recovers
   the operation journal; it does not bind a socket or launch a VM. Use `box
   doctor` above for host capability checks. Existing templates are fully checked
   by the runtime when launched. A second host service using the same runtime or
   journal refuses to start.
8. Review and install the two [example systemd units](../deploy/). The host unit
   creates `/run/kiln` as root:`kiln-api` 0750 and a socket with mode 0660. The
   gateway cannot read root-owned VM state. Its sample listener is loopback-only;
   replace `--listen` with the chosen private-interface address and review the
   firewall before enabling laptop access. Then, with deployment approval:

   ```sh
   sudo install -m 644 deploy/kiln-host.service deploy/kiln-api.service /etc/systemd/system/
   sudo systemctl daemon-reload
   sudo systemctl enable --now kiln-host kiln-api
   ```

   Do not remove `KillMode=process` from the host unit without changing the VM
   shutdown contract. Manager restart intentionally leaves owned VMMs running;
   stop/delete the boxes explicitly before decommissioning the host.

## Configure the laptop and use it

```sh
chmod 600 "$HOME/.config/kiln/server.token"
kiln profile add default --url https://boxes.example.net:8443 \
  --token-file "$HOME/.config/kiln/server.token" \
  --ca-file "$HOME/.config/kiln/server-ca.pem"
# Omit --ca-file for a certificate signed by a bundled public CA.
kiln templates
kiln create --template ubuntu-4g --name my-dev-box
kiln list
kiln inspect BOX_ID
kiln exec BOX_ID -- /bin/sh -c 'printf hello; exit 37'
# Shell exit status above is 37, not merely success because HTTP succeeded.
kiln pause BOX_ID
kiln resume BOX_ID
kiln stop BOX_ID
kiln start BOX_ID
kiln delete BOX_ID
```

Use `--profile NAME` to select another server and `--config PATH` for a separate
profile store. Default location is `$XDG_CONFIG_HOME/kiln/profiles.json`,
`%APPDATA%/kiln/profiles.json`, or `$HOME/.config/kiln/profiles.json`. Profiles
contain URL and absolute token/CA **file references**, not token bytes. Existing
names are not overwritten by `profile add`; edit the JSON or use another name.

### Interactive terminal, files and editors

On Linux/macOS, install the platform's OpenSSH client (`ssh`, `scp`, `ssh-keygen`).
The profile directory must be private (0700). Use the full box ID:

```sh
kiln ssh BOX_ID
kiln ssh BOX_ID -- 'uname -a; exit 37'
kiln cp ./local-file BOX_ID:/workspace/remote-file
kiln cp BOX_ID:/workspace/remote-file ./downloaded-file
kiln ssh-config BOX_ID > "$HOME/.ssh/kiln-config"
ssh -F "$HOME/.ssh/kiln-config" kiln-default-BOX_ID
```

Add `Include ~/.ssh/kiln-config` to `~/.ssh/config` to use the generated host
entry in VS Code Remote-SSH or another OpenSSH-based editor. This supports
loopback-only guest TCP forwarding for editor servers; agent, X11 and remote
forwarding are disabled. Regenerate the entry when changing profiles or moving
the client binary. Paths with shell/config expansion characters are rejected.
`cp` uses SFTP and accepts one local endpoint and one `BOX_ID:/absolute/path`.

The client creates a private per-profile Ed25519 key. It retrieves the guest's
host key through verified HTTPS and uses strict host-key checking, never an
insecure first-connect prompt. Guest host keys are generated after clone
initialization and persist on the box disk. Authentication authorizes root
access inside the selected box. The administrator token is never sent to the
guest; terminal/file payloads do not enter the operation journal.

On `https://cli.dark-forge.dev`, `kiln ssh` and `kiln ssh-config` use the full
box id and send `Upgrade: kiln-ssh` to `GET /v1/boxes/<id>/ssh`. The public door
accepts that upgrade only for an operate-scope identity JWT whose subject is the
enrolled owner, then proxies the byte stream to loopback `kiln-api` with that
same JWT. It does not accept the administrator token and does not add one. A
missing credential, an invalid JWT, a read-scope token, or another owner's JWT
is rejected before the door dials loopback. Each accepted session is audited.
Linux and macOS ship this client. There is still no public port 22.

SSH sessions are separate from buffered `exec`: Ctrl-C and terminal resizing
work through OpenSSH, with no automatic command replay or session reconnect.
There are at most 32 tunnels per service, eight sessions per guest, and a 24-hour
tunnel lifetime. Native macOS editor execution still needs validation; Linux
OpenSSH protocol integration is tested.

Existing images/templates do not acquire a new agent when the host is updated.
Build a new Ubuntu image and template for SSH. Internet access additionally
requires an explicitly provisioned network-enabled isolated runtime; see
[network setup and limits](runtime-networking.md). It is not necessary for SSH.

`--json` emits structured JSON on stdout. Text-mode exec preserves byte-exact
stdout/stderr; JSON encodes them as byte arrays (including non-UTF-8 data).
Timeout returns exit 124; an absent/unrepresentable guest exit status returns
125. Truncation is explicit. Exec is buffered, not a PTY, and has no stdin stream.
Its guest deadline is `--timeout-ms` (1–3600000, default 10000). Client wait is
separate: `--wait-seconds` (default 300); exceeding it does not cancel the server
operation. Request IDs and diagnostics go to stderr.

### Repair login readiness in an existing guest

Older images can report "System is booting up" after the agent is ready because
their minimal target never starts `systemd-user-sessions`. As root **inside the
guest**, install this dependency and start the oneshot (no guest reboot needed):

```sh
mkdir -p /etc/systemd/system/kiln-guest.service.d
test ! -e /etc/systemd/system/kiln-guest.service.d/login-readiness.conf &&
printf '%s\n' '[Unit]' 'Requires=systemd-user-sessions.service' 'After=systemd-user-sessions.service' > /etc/systemd/system/kiln-guest.service.d/login-readiness.conf
systemctl daemon-reload
systemctl start systemd-user-sessions.service
test ! -e /run/nologin
```

This preserves PAM's login policy, including shutdown-time login restrictions;
do not disable `pam_nologin` or just delete its marker at every login. New v0.2.1
images include the dependency and run the oneshot before warm snapshot creation.
The repair does not enable internet access: networkless VMs have no virtual NIC,
and their immutable store policy cannot be changed in place. Preserve old boxes
and explicitly plan a migration to network-enabled replacements instead of
editing runtime metadata or deleting existing state.

## Reconnect without accidentally repeating work

Every mutation prints a 32-hex request ID **before submission**. To submit and
disconnect immediately, use `--no-wait`. Create can instead use `--wait` so the
command does not return until the box is running and visible to the control
plane. Resume observation for other mutations with:

```sh
kiln operation REQUEST_ID --wait
kiln --json operation REQUEST_ID
```

If submission loses its response, check that ID first. A 404 means it has not
been recorded at the time of the lookup, not proof that an in-flight submission
can never arrive. Retry only with `--request-id REQUEST_ID` and **identical
arguments**. The server returns the same operation; changed arguments return
409. Records are retained even after deletion so a late create retry cannot
resurrect a box. Neither the gateway nor client automatically retries mutations
or follows HTTP redirects with credentials.

On host-service restart, unfinished operations become `unknown`. The runtime
reconciles the associated VM when inspected; the service does not replay the
command. A guest command may have run, partially run or completed despite a lost
response. Inspect its effects before explicitly deciding on another request ID.
A failed lifecycle operation may retain a stopped box/disk for recovery; its
operation contains the assigned box ID. Runtime details stay in the host log.

The operation journal is not a second inventory database. It holds request
fingerprints, resource associations, timestamps and bounded results; the runtime
owns actual VM state and quotas. Raw argv/environment and API tokens are not
journaled. **Exec output may contain secrets**, so protect the SQLite files and
backups. This slice does not encrypt result data at rest or provide OS keychain
integration. Never delete/reset the journal while keeping the runtime: doing so
loses ownership and idempotency history. Back up both together with services
stopped (and VM disks handled using the runtime's checkpoint contract).

There are eight in-flight host operations, 32 in-flight gateway requests, 128 KiB
request bodies, and 1 MiB response bounds. The operation journal uses a
10,000-row high-water mark inside `kiln-host`. Before refusing a new row,
`kiln-host` prunes terminal operations (succeeded, failed, or unknown) that are
older than 30 days and outside the 8,000 newest finished rows. Running operations
are never pruned. Create rows for boxes that still exist on disk are never
pruned, so the dashboard and API box list stay aligned with live VMs. Retries
with the same `--request-id` and identical arguments still dedupe while the row
remains; changed arguments still return 409. If the journal is still at the cap
after pruning, new mutations return HTTP 503 with code `journal_full` and an
actionable message rather than failing silently. Catalog changes and token
rotation require service restart; token rotation means securely replacing the
file on server and laptop.

## Verification

Ordinary tests need neither KVM nor root:

```sh
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo fmt --all -- --check
```

The ignored service integration test launches real VMs, sends requests through
verified TLS and the Unix socket, kills/restarts the host process during exec,
and checks no replay. It retains unfinished VM state on failure for diagnosis:

```sh
PATH="$PWD/.tools/firecracker-1.17.0:$PATH" \
KILN_TEST_IMAGE="$PWD/images/output/fixture-kiln/image.json" \
cargo test --locked -p kiln-server --test lifecycle -- --ignored --nocapture --test-threads=1
```

For approved isolated tests, additionally set `KILN_TEST_ISOLATION_CONFIG` and a
trusted `KILN_TEST_STATE_PARENT`, and use a root-owned `KILN_TEST_HOST` executable.
Run only one state store at a time with a given isolated UID/GID allocation.

Validated on `ser7` on 2026-10-01: 82 ordinary tests, Clippy with warnings denied,
formatting, release builds, development and isolated Ubuntu KVM service lifecycle
tests, and all three release binaries together against a disposable VM. No
persistent service was installed. Native macOS execution is unverified;
an Apple Silicon cross-check was blocked by missing Apple SDK headers in ring's
C build. The systemd examples still require installation-time validation on the
destination host.

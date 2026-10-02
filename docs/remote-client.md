# Laptop client and single-host service

For prebuilt downloads and the guided first-install workflow, start with the
[private release guide](releases.md). The manual instructions below remain useful
for custom hosts and development; they are not required when installing a release
package on a supported Ubuntu host.

`boxctl` manages boxes over **verified HTTPS**. Only the server needs Linux,
Firecracker and KVM. This is a single-administrator, trusted-workload pilot:
lifecycle management and buffered commands work; guest SSH, interactive terminals,
files, internet networking, previews and multi-tenant security are not included.

The existing `box` binary remains the local Linux operator tool. It is not the
laptop client. Do not run local mutations against the runtime while its host
service is managing it; stop the host service first for template administration.

```text
Laptop boxctl → HTTPS + bearer token → boxd-api (unprivileged)
                                       ↓ restricted Unix socket
                                    boxd-host (root, isolated profile)
                                       ↓ existing Firecracker/jailer runtime
                                    Linux guest via vsock
```

There is no mandatory SSH tunnel for management. Guest SSH/editor integration is
a subsequent networking slice. SQLite is embedded; no PostgreSQL or Redis is
required. The service exposes only catalog aliases and sanitized box records,
not host paths, jail identities, host fingerprints or Firecracker sockets.

## Build the binaries

From this checkout on a laptop with the pinned Rust 1.95.0 toolchain:

```sh
cargo build --release --locked -p box-client --bin boxctl
# Or install only the client into your Cargo bin directory:
cargo install --locked --path crates/box-client
```

Build natively on the laptop OS/architecture. The client crate does not depend on
`box-runtime`, SQLite, Firecracker or Linux/vsock APIs. This repository does not
yet provide signed installers. Release automation builds downloadable binaries
once the source is published to GitHub and the workflow succeeds. On Windows,
restrict token-file ACLs to your account; Unix builds additionally enforce mode
0600/0400. The client uses bundled public CA roots or an explicit private CA file,
not an insecure certificate-verification switch. It connects directly (proxy
environment variables are not used).

On the Linux x86_64 host:

```sh
cargo build --release --locked -p box-runtime --bin box \
  -p box-server --bin boxd-host --bin boxd-api
```

## Prepare the server before exposing the API

The commands below are installation instructions, **not actions performed by
building this feature**. They change host users, files or services and require
operator approval. Use a private/VPN interface for this pilot, not an unrestricted
public Internet endpoint.

1. Follow the [isolated-runtime setup](runtime.md#experimental-isolated-profile):
   matching root-owned Firecracker/jailer 1.17.0, KVM, pre-enabled cgroup v2
   controllers, a dedicated unused eight-identity UID/GID range and trusted images.
   Keep these binaries in `/opt/boxd/bin`; the isolation policy and service PATH
   must select exactly those binaries. Optional snapshot overlays require the
   separately documented storage setup. The services never provision host routing,
   firewall rules, cgroup controllers or storage pools automatically.
2. Create an unprivileged system account/group named `boxd-api`. Install
   `boxd-host` and `boxd-api` as root-owned executables in `/usr/local/libexec`,
   and `box` in `/opt/boxd/bin`. Create root-owned `/etc/boxd` (0755) and
   `/var/lib/boxd` (0700). Keep all runtime/image path ancestors root-owned and
   not group/other writable; a directory under your home is not suitable.
3. Put the trusted isolation policy in `/etc/boxd/isolation.json`. Copy
   [`deploy/host.example.json`](../deploy/host.example.json) to
   `/etc/boxd/host.json` (root-owned, 0600). The initially empty catalog is valid.
4. Build templates **on this host**, in this runtime store, using the local tool.
   Snapshots are host-specific; copying a template from the development runner
   does not make it portable. For a prepared, root-owned Ubuntu image:

   ```sh
   sudo env PATH=/opt/boxd/bin:/usr/sbin:/usr/bin:/sbin:/bin \
     /opt/boxd/bin/box --state-dir /var/lib/boxd/runtime \
     --isolation-config /etc/boxd/isolation.json doctor
   sudo env PATH=/opt/boxd/bin:/usr/sbin:/usr/bin:/sbin:/bin \
     /opt/boxd/bin/box --state-dir /var/lib/boxd/runtime \
     --isolation-config /etc/boxd/isolation.json template build \
     --image /var/lib/boxd/images/ubuntu/image.json \
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
   signing certificate itself. Store `tls.crt` and `tls.key` in `/etc/boxd`, readable by
   `boxd-api`; keep the key owner-only. Certificate issuance/renewal is an operator
   responsibility. The service has no HTTP fallback and does not obtain ACME
   certificates automatically.
6. Generate a 256-bit random token without displaying it:

   ```sh
   sudo sh -c 'umask 077; openssl rand -hex 32 > /etc/boxd/admin.token'
   sudo chown boxd-api:boxd-api /etc/boxd/admin.token
   sudo chmod 600 /etc/boxd/admin.token
   ```

   Transfer it securely to a private file on your laptop (not via chat, command
   arguments or a URL). It grants **administrator access to all service-managed
   boxes** in this single workspace. There are no user roles or scoped tokens yet.
7. Check the configuration while the host service is stopped:

   ```sh
   sudo env PATH=/opt/boxd/bin:/usr/sbin:/usr/bin:/sbin:/bin \
     /usr/local/libexec/boxd-host --config /etc/boxd/host.json --check
   ```

   This opens/initializes private state, verifies catalog references and recovers
   the operation journal; it does not bind a socket or launch a VM. Use `box
   doctor` above for host capability checks. Existing templates are fully checked
   by the runtime when launched. A second host service using the same runtime or
   journal refuses to start.
8. Review and install the two [example systemd units](../deploy/). The host unit
   creates `/run/boxd` as root:`boxd-api` 0750 and a socket with mode 0660. The
   gateway cannot read root-owned VM state. Its sample listener is loopback-only;
   replace `--listen` with the chosen private-interface address and review the
   firewall before enabling laptop access. Then, with deployment approval:

   ```sh
   sudo install -m 644 deploy/boxd-host.service deploy/boxd-api.service /etc/systemd/system/
   sudo systemctl daemon-reload
   sudo systemctl enable --now boxd-host boxd-api
   ```

   Do not remove `KillMode=process` from the host unit without changing the VM
   shutdown contract. Manager restart intentionally leaves owned VMMs running;
   stop/delete the boxes explicitly before decommissioning the host.

## Configure the laptop and use it

```sh
chmod 600 "$HOME/.config/boxd/server.token"
boxctl profile add default --url https://boxes.example.net:8443 \
  --token-file "$HOME/.config/boxd/server.token" \
  --ca-file "$HOME/.config/boxd/server-ca.pem"
# Omit --ca-file for a certificate signed by a bundled public CA.
boxctl templates
boxctl create --template ubuntu-4g --name my-dev-box
boxctl list
boxctl inspect BOX_ID
boxctl exec BOX_ID -- /bin/sh -c 'printf hello; exit 37'
# Shell exit status above is 37, not merely success because HTTP succeeded.
boxctl pause BOX_ID
boxctl resume BOX_ID
boxctl stop BOX_ID
boxctl start BOX_ID
boxctl delete BOX_ID
```

Use `--profile NAME` to select another server and `--config PATH` for a separate
profile store. Default location is `$XDG_CONFIG_HOME/boxd/profiles.json`,
`%APPDATA%/boxd/profiles.json`, or `$HOME/.config/boxd/profiles.json`. Profiles
contain URL and absolute token/CA **file references**, not token bytes. Existing
names are not overwritten by `profile add`; edit the JSON or use another name.

`--json` emits structured JSON on stdout. Text-mode exec preserves byte-exact
stdout/stderr; JSON encodes them as byte arrays (including non-UTF-8 data).
Timeout returns exit 124; an absent/unrepresentable guest exit status returns
125. Truncation is explicit. Exec is buffered, not a PTY, and has no stdin stream.
Its guest deadline is `--timeout-ms` (1–3600000, default 10000). Client wait is
separate: `--wait-seconds` (default 300); exceeding it does not cancel the server
operation. Request IDs and diagnostics go to stderr.

## Reconnect without accidentally repeating work

Every mutation prints a 32-hex request ID **before submission**. To submit and
disconnect immediately, use `--no-wait`. Resume observation with:

```sh
boxctl operation REQUEST_ID --wait
boxctl --json operation REQUEST_ID
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
request bodies, and 1 MiB response bounds. A private pilot journal accepts at most
10,000 requests and then fails closed; do not prune idempotency keys to bypass
that bound. Automated retention/tombstone compaction is a follow-up before
long-running production use. Catalog changes and token rotation require service
restart; token rotation means securely replacing the file on server and laptop.

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
BOXD_TEST_IMAGE="$PWD/images/output/fixture-v3/image.json" \
cargo test --locked -p box-server --test lifecycle -- --ignored --nocapture --test-threads=1
```

For approved isolated tests, additionally set `BOXD_TEST_ISOLATION_CONFIG` and a
trusted `BOXD_TEST_STATE_PARENT`, and use a root-owned `BOXD_TEST_HOST` executable.
Run only one state store at a time with a given isolated UID/GID allocation.

Validated on `ser7` on 2026-10-01: 82 ordinary tests, Clippy with warnings denied,
formatting, release builds, development and isolated Ubuntu KVM service lifecycle
tests, and all three release binaries together against a disposable VM. No
persistent service was installed. Native macOS/Windows execution is unverified;
an Apple Silicon cross-check was blocked by missing Apple SDK headers in ring's
C build. The systemd examples still require installation-time validation on the
destination host.

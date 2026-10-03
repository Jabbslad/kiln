# Public builds and unattended installation

GitHub Actions builds the client and server; neither destination needs Rust.
Source and published release downloads are public. No GitHub account, PAT, or
terminal is needed to install. Access to a running Kiln server still requires
browser login to an enrolled host or a direct administrator profile; public
downloads do not disable API authentication.

## v0.4.0 browser-login pilot

Rerun the normal installer, then run `kiln login`. The client includes the
`https://dark-forge.dev` default, browser/device approval, owner-bound server
discovery, private rotating credentials, logout/revocation and automation keys.
VM API, SSH, SFTP and editor access remain direct to the private host. Existing
direct profiles are preserved; use `kiln --profile personal login` alongside one.

The server package includes `kiln-api enroll`; its installer prints the one-time
command with the correct private endpoint. Enroll with the same provider/account
used on the laptop. No server address, token or CA transfer is needed on the
laptop. An unenrolled server does not appear in discovery, and login stops with
enrollment instructions rather than creating a usable profile. The installer
does not approve ownership, open a browser, or enroll a host automatically.

Binary-only updates from standard v0.3.0, v0.3.2 and v0.3.3 hosts preserve existing
configuration, enrollment, guest images and disks. Guest/runtime wire formats and
host unit contracts are unchanged. This release does not migrate guest images.

**Acceptance status:** public HTTPS, actual GitHub/Google sign-in pages, browser
rendering, protocol/security tests and native client builds are verified. Real
account consent and complete central-authenticated VM/SSH/SFTP/PTY, revocation
and outage acceptance remain outstanding. The owner requested distributing this
pilot through the installer to exercise the actual laptop flow. Publishing this
pilot is not a claim that those remaining tests passed, and does not enroll or
restart an existing server. See [identity operations](identity-service.md).

This is a trusted-workload, single-administrator pilot, not a hosted multi-tenant
sandbox. The automation is checked in; that alone does not mean a GitHub build,
release or real server installation has succeeded. Check the Actions run for the
version you download. macOS binaries are not signed/notarized, and Windows
binaries are not Authenticode-signed.

## v0.3.0 compatibility

Kiln v0.3.0 renames the client to `kiln`, the operator tool to `kiln-runtime`,
and the host/API services to `kiln-host` and `kiln-api`. Rust crates, release
archives, guest units, SSH upgrade headers and `KILN_*` environment variables
use the new identity. Source is `Jabbslad/kiln`; the bootstrap
and its README are also published in `Jabbslad/kiln-install`.

This is **not an in-place upgrade from v0.2.x**. Use matching v0.3.0 client,
server and rebuilt guest images/templates. New installs use `/etc/kiln`,
`/var/lib/kiln`, `/opt/kiln`, `/run/kiln`, and client profiles under
`~/.config/kiln` (or the platform's config directory). Old profiles, SSH aliases,
services, networking and runtime stores are not discovered, moved or deleted.
Do not point the new runtime at an old store or reuse old snapshots/templates.
No legacy boxd migration is provided; use a clean Kiln installation. Do not
install a second networked store alongside the old installation on the same
host: address pools, ports and reserved UID/GID ranges are still shared.

The bootstrap does not rename an older client or migrate its configuration.
Use a fresh client install and explicitly enroll it with a matching Kiln server. Earlier release notes and
benchmark results below describe pre-rename versions; commands use current names.

**Ubuntu host support:** starting with `v0.1.1`, both installer layers admit Ubuntu
24.04 and 26.04 x86-64 while retaining capability and fresh-install checks. The
provisioner passed read-only preflight on `ser7` with the release image and its
private IPv4 address; full provisioning/reboot remains unverified. The older
`v0.1.0` server package accepts only Ubuntu 24.04; changing its shell gate alone
does not add support for 26.04. The guest image remains Ubuntu 24.04 on either host.

## One-command installation

On a macOS or supported Linux laptop:

```sh
curl -fsSL https://raw.githubusercontent.com/Jabbslad/kiln-install/main/install.sh | sh
```

On a dedicated **Ubuntu 24.04 or 26.04 x86-64/KVM** server:

```sh
curl -fsSL https://raw.githubusercontent.com/Jabbslad/kiln-install/main/install.sh | sh -s -- server
```

**v0.3.2 adds public, unattended installation.** Packages use versioned public
release URLs and pinned checksums. No `gh`, Python, Rust, or JSON parser is
required on the laptop. Both bootstrap modes work without a controlling terminal.
Running `server` authorizes setup: use root or passwordless sudo. There are no
`SETUP`/`INSTALL` confirmations or password prompts.

**v0.3.3 adds automatic client and server updates.** Rerun the same command;
no upgrade flag is needed. Existing standard Kiln v0.3.0/v0.3.2 servers receive
compatible application binaries, while configuration, credentials, networking,
images, templates and VM disks are preserved. Identical installs are unchanged.
The update reuses the installed address, skips apt, backs up binaries, drains
accepted operations, restarts management services and checks HTTPS readiness.
API/SSH connections briefly disconnect; guest VMMs are not stopped. Failure
rolls back binaries; interrupted updates require recovery from retained backups.
No old boxd migration, downgrade or guest-image migration is included. See
[automatic server updates](../deploy/bootstrap/README.md#automatic-server-updates).
Real-server update/reboot validation remains outstanding; tests use disposable
files, real SQLite journals and simulated systemd operations.

The bootstrap detects the source IPv4 from `ip -4 route get 1.1.1.1` without
sending traffic, rejects non-private results, and fails rather than guessing
when it cannot detect one address. Supply `--address PRIVATE_IP` for a VPN or
another interface. Detection requires iproute2; choose a stable address because
it is used for the API binding and TLS certificate. Guest networking remains off
unless `--network` is supplied (detect uplink), or `--network-uplink INTERFACE`
is supplied (explicit uplink). See the [bootstrap guide](../deploy/bootstrap/README.md).

**v0.2.0 guest-access additions:** packages include interactive SSH,
SFTP, editor SSH configuration, and optional isolated guest IPv4 egress. The
public installer pins v0.4.0. These features require the new client, server,
and updated Ubuntu guest agent. Existing templates/boxes are not
upgraded automatically. SSH/SFTP require OpenSSH on Linux/macOS; Windows retains
management-only support. See the [client commands](remote-client.md#interactive-terminal-files-and-editors).

**v0.2.1 guest login fix:** the image starts `systemd-user-sessions` before the
warm preparation barrier and workload agent. Ready guests no longer retain
`/run/nologin` or print the stale "System is booting up" PAM warning. Rebuild
templates from the new image; installing a client or host binary does not change
existing guest disks. See the [existing-guest repair](remote-client.md#repair-login-readiness-in-an-existing-guest).
This release does not add network cards to old boxes or migrate networkless
stores. Internet access still requires an explicitly provisioned network-enabled
store and templates, with a reviewed migration for existing workloads.

**v0.2.2 networking launch fix:** the privileged helper uses `nsenter` to enter
only the VM network namespace, retaining the cgroup2 hierarchy needed by jailer.
This fixes `CgroupHierarchyMissing` on ordinary hosts. Networking installation
now explicitly requires util-linux. The guest image and wire protocol remain
compatible with v0.2.1; existing networkless stores still require migration.

The client installs to `~/.local/bin/kiln` without sudo and prints PATH setup
instructions if needed. To update, rerun the original command:

```sh
curl -fsSL https://raw.githubusercontent.com/Jabbslad/kiln-install/main/install.sh | sh
```

This verifies the downloaded checksum and executable version before atomic
replacement, retaining the old binary as `~/.local/bin/kiln.previous`. It leaves
profiles and credentials unchanged. Identical installs preserve the useful backup;
newer installed versions are never downgraded. See the
[bootstrap details](../deploy/bootstrap/README.md#automatic-client-updates).

The server bootstrap installs Ubuntu system packages (including Python) without
prompts using `sudo -n`, then invokes the provisioner. You do not install
dependencies or invoke Python yourself. Existing package configuration is retained.
The server still needs the resources and LAN/VPN connectivity described below;
containers and non-KVM hosts are not supported.

The script checks archive digests before extraction, downloads anonymously using
HTTPS-only redirects, and ignores user curl configuration. It removes temporary
files on normal exit and handled signals. Checksums assume trust in the bootstrap publisher, not
independent package signing. To inspect before execution, download the script
with `curl -fsSL URL -o install.sh`, read it, then run `sh install.sh [server]`.
Native Windows continues to use the public release zip described below.

**Fresh-host provisioning and reboot remain unverified.** The pilot uses the copy
disk backend, not the faster overlay benchmark configuration. After server setup,
follow [Connect the laptop](#connect-the-laptop); installation does not transfer
administrator credentials automatically.

## Manual client download (optional)

Download from [GitHub Releases](https://github.com/Jabbslad/kiln/releases),
or use curl as shown below. Draft releases are maintainer-only until published.

Choose one target:

| Laptop | Target |
| --- | --- |
| Apple Silicon Mac | `aarch64-apple-darwin` |
| Intel Mac | `x86_64-apple-darwin` |
| Linux x86-64, glibc 2.39+ | `x86_64-unknown-linux-gnu` |
| Windows x86-64 | `x86_64-pc-windows-msvc` (`.zip`, contains `kiln.exe`) |

For macOS/Linux, in a new download directory:

```sh
REPO=Jabbslad/kiln
VERSION=v0.4.0
TARGET=aarch64-apple-darwin
BASE="https://github.com/$REPO/releases/download/$VERSION"
curl -fL --proto '=https' --proto-redir '=https' -O "$BASE/kiln-$VERSION-$TARGET.tar.gz"
curl -fL --proto '=https' --proto-redir '=https' -O "$BASE/kiln-$VERSION-$TARGET.tar.gz.sha256"
shasum -a 256 -c "kiln-$VERSION-$TARGET.tar.gz.sha256"
tar -xzf "kiln-$VERSION-$TARGET.tar.gz"
mkdir -p "$HOME/.local/bin"
install -m 755 kiln "$HOME/.local/bin/kiln"
"$HOME/.local/bin/kiln" --version
```

Put `$HOME/.local/bin` on your shell's PATH if it is not already there. On Windows,
download the matching `.zip` and `.zip.sha256` from GitHub Releases, compare
`Get-FileHash -Algorithm SHA256` to the checksum, then `Expand-Archive` and place
`kiln.exe` in a directory on your user PATH. Verify downloads before execution;
do not disable system-wide OS security policies to run an unsigned pilot binary.

Checksums detect corruption, not a compromised publisher. The trust boundary is
the release publisher and the reviewed workflow; signing is not implemented yet.
The bootstrap embeds reviewed digests rather than trusting a downloaded sidecar alone.

## Server requirements and manual installation

Start with a **dedicated Ubuntu 24.04 or 26.04 x86-64 systemd host** with usable `/dev/kvm`,
cgroup v2, at least 6 GiB currently available RAM and 24 GiB free on `/var/lib`
after extracting the package. A 16 GiB+ host is recommended. Allow additional
temporary space for the downloaded archive and extracted image (about 3 GiB).
The shell bootstrap manages the system Python 3, OpenSSL, curl, CA certificates, tar and
Ubuntu account tools. For manual installation, supply these dependencies yourself;
the underlying Python provisioner only checks them.
It downloads checksum-pinned Firecracker/jailer 1.17.0 from GitHub. The package
already contains the kernel, Ubuntu filesystem and guest agent.

The server must be reachable over your existing LAN or VPN. The installer does
not configure a VPN or open the host API firewall. Guest networking is off by
default; new source packages support explicit `--network-uplink INTERFACE`
provisioning, including NAT/filtering and a boot unit (see
[networking](runtime-networking.md)). Pick a stable IPv4 address
assigned to that private interface. Public addresses and `0.0.0.0` are refused;
omitting the address when invoking `install.py` directly installs a **local-only**
endpoint at `127.0.0.1`. The shell bootstrap instead detects a private address.

Download as your normal user (or download on your laptop and securely copy both
files to the server). No GitHub credentials are required:

```sh
REPO=Jabbslad/kiln
VERSION=v0.4.0
PACKAGE="kiln-server-$VERSION-x86_64-unknown-linux-gnu.tar.gz"
BASE="https://github.com/$REPO/releases/download/$VERSION"
curl -fL --proto '=https' --proto-redir '=https' -O "$BASE/$PACKAGE"
curl -fL --proto '=https' --proto-redir '=https' -O "$BASE/$PACKAGE.sha256"
sha256sum -c "$PACKAGE.sha256"
mkdir kiln-server
tar -xzf "$PACKAGE" -C kiln-server
sudo -n python3 kiln-server/install.py --address 192.168.1.20 --apply
```

Replace `192.168.1.20` with the server's private/VPN address. `--apply` authorizes
installation without a prompt. Omit `--apply` for prerequisite checks only. The installer:

- Updates supported existing Kiln installations without reprovisioning. For fresh
  installs, refuses partial installs, unit overrides, conflicting
  account IDs, occupied ports and unsupported hosts before changing the host.
- Installs binaries, a restricted API account, eight locked VM identities, and
  cgroup preparation that runs again on boot.
- Generates a private CA, a correctly signed server certificate and a random
  administrator token locally. No credentials are included in a release archive.
- Builds a host-specific warm template with **4 GiB RAM / one vCPU**, publishes it
  as `ubuntu-4g`, and starts the host and HTTPS gateway as systemd services.
- Checks the authenticated HTTPS template catalog before reporting readiness.

This first installer uses the **copy disk backend**. It does not reformat disks,
enable fs-verity or create a device-mapper pool, and does not reproduce the
overlay benchmark's launch latency. Warm snapshots are built on this host, never
copied from CI. Normal per-box identities are retained; the shared-identity
experiment is not enabled.

The reserved UIDs 70000–70007 and GIDs 71000–71007 must not belong to another
runtime's numeric allocation. The installer checks accounts, subordinate ranges
and running processes, but cannot discover an arbitrary inactive runtime policy.
Use a dedicated host. Partial failures retain files/accounts/VM state for
diagnosis. Never delete state to force a rerun: inspect the host log and stop any
retained VM using the local `kiln-runtime` tool first. Uninstall is not automated.

## Connect the laptop

The installer creates `/etc/kiln/laptop.tar.gz` with the public CA certificate,
administrator token and `CONNECT.txt`. **Treat this bundle as a password.**
Transfer it over an existing trusted SSH/SFTP connection, using a private staging
file if your SSH account cannot read root files. Never attach it to an issue,
release, chat or repository. GitHub credentials are unrelated to this token.

Extract into a private, permanent directory on your laptop and run the one
`kiln profile add` command in `CONNECT.txt`, then:

```sh
kiln templates
kiln create --template ubuntu-4g --name first-box
kiln list
kiln exec BOX_ID -- /bin/sh -c 'printf hello'
```

Profiles reference the extracted token/CA files; do not delete them after setup.
Unix token permissions must be 0600 or 0400. On Windows, restrict the directory
and token ACL to your account. The service currently supports buffered exec,
while v0.1.1 does not include an interactive shell, SSH, file transfer, or guest
internet access. Those additions require the new builds described above.

The server leaf certificate expires after **one year**. Renew it before expiry,
using the retained private CA and the same IP SAN, and restart `kiln-api` after
replacing the certificate/key. There is no automatic certificate renewal yet.
See [operation recovery and service details](remote-client.md) for interrupted
requests, backups, token rotation, quotas and the operation-journal capacity.

## Maintainer workflow

Pushes and pull requests run Linux workspace checks, build the server/guest image,
and run native client TLS/CLI tests on Linux, macOS ARM/Intel and Windows. Archives
and individual SHA-256 files are available as run artifacts for seven
days. Use `gh run download RUN_ID --repo OWNER/NAME` for branch builds; do not
confuse those with a reviewed release. CI uses no credentials beyond the scoped
GitHub Actions token and never deploys a server. Public workflows/logs/artifacts
must not contain credentials or runtime data. Fork PRs use hosted runners and
read-only permissions; only the tag release job has contents-write permission.

For an approved release, update the workspace version, commit the reviewed source
and push a matching `vMAJOR.MINOR.PATCH` tag. Tag/version mismatches fail. When all
jobs pass, the workflow creates a **draft release**. Review/download the draft,
verify packages, and record real-host validation and any outstanding limitations
before publishing it publicly. Reruns do not overwrite
existing releases. Real KVM tests remain separate; ordinary CI tests do not prove
host isolation, reboot persistence or installer success on a real server.

Do not register a privileged production/KVM server as a runner for pull-request
code. Keep runtime directories, tokens and enrollment bundles out of Git. The
packager includes only named binaries, installer files and pristine image files.
Changing repository visibility later is a separate decision; this workflow never
makes that change.

The public installer is maintained in `deploy/bootstrap/`. Only `install.sh` and
its public `README.md` may be copied to `Jabbslad/kiln-install`. After an approved
release is published, update the script's pinned version and SHA-256
values from verified packages, run `scripts/test-bootstrap.py` and ShellCheck,
and verify a real anonymous download before publishing the public copy.
Never publish server state or credentials there.
The ordinary build workflow has no cross-repository publishing credentials.

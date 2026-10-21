# Public builds and unattended installation

GitHub Actions builds the client and server; neither destination needs Rust.
Source and published release downloads are public. No GitHub account, PAT, or
terminal is needed to install. Access to a running Kiln server still requires
browser login to an enrolled host or a direct administrator profile; public
downloads do not disable API authentication.

## v0.4.10 login resume and closed public door

`kiln login` saves the approved session before the server picker. If that
prompt gets no input, login exits and prints a `kiln login --server <id>`
command for each server. The next login uses the saved session, so resume
does not request a new device code.

The public 8443 door is removed. `Sockets=` names only
`kiln-door-proxy.socket`, the internal socket on port 8444. Both installers
disable `kiln-door-proxy-public.socket` on upgrade. After an upgrade, remove
any leftover nftables `tcp dport 8443 accept` rule.

Publishing this release does not deploy a server or restart a live service.
The public bootstrap pins the published v0.4.10 assets and runs
`install.sh`.

## v0.4.9 public door trust

`kiln login` stores public-root trust for `https://cli.dark-forge.dev`. A saved
profile keeps its private CA. An origin that still names `:8443` stays pinned
to that CA. Door commands for the portless origin use public roots.

The public socket stayed on `78.46.98.73:8443` for 0.4.6 clients.
`systemd-socket-proxyd` forwarded those connections to `127.0.0.1:8444`.
`docs/runbooks/2026-10-19-door-public-cert.md` records the Caddy reload and
the ACME checks.

v0.4.8 was tagged but never released. The Windows stamp failed and #31 fixed
it. v0.4.9 is the release that carries these changes. The public bootstrap
pins the published v0.4.9 assets and runs `install.sh`.

## v0.4.7 website and dashboard design

The static dashboard archive now includes a public landing page at `/` and the
existing console at `/boxes`. The imported root `design.md` records the shared
visual system and Kiln adaptations. The landing page uses original, locally
bundled artwork, real client installation instructions, and explicit links to the
read-only preview. It makes no host API requests. Console navigation, headings,
borders, and selection treatments follow the same system.

Browser authentication and live dashboard controls are still not integrated.
Existing preview gates remain; no VM operations are simulated. The static archive
is separate from server installation. Publishing it does not deploy either surface,
restart services, or change runtime state. The runtime behavior is unchanged from
v0.4.6. Browser validation covers phone/desktop layouts, accessibility, navigation,
clipboard success and denial, and the existing console interactions.

## v0.4.6 public door

`kiln-api door` binds one public address and presents a leaf from the existing
private CA. The name is `https://cli.dark-forge.dev:8443`. The door checks an
identity JWT, then dials `https://127.0.0.1:8443`. A request without that JWT
does not open the dial. The door does not accept the administrator token.

`deploy/enroll-dark-forge.sh` enrolls that origin when no enrollment files
exist. It continues only when `servers.origin` is exactly that URL and the
approving GitHub account is login `Jabbslad`, numeric id `3084`. An enrollment
that already exists is left in place.

A release build of `kiln-identity` omits `--github-authorize-url`,
`--github-token-url`, `--github-user-url`, and `--provider-ca-file`. Those
flags are the Cargo feature `lab-provider`, which is off by default.
`https://dark-forge.dev` refuses a substitute provider when that feature is on.

The dark-forge.dev upgrade to v0.4.5 is recorded in
`docs/runbooks/2026-10-19-dark-forge-v0.4.5-upgrade.md`. Before the bootstrap
installs `sqlite3`, it runs `apt-get update`.

A patch update from v0.4.5 keeps the guest image, the runtime format,
configuration, enrollment, and disks. Publishing this release does not deploy
a server or restart a live service. The public bootstrap pins the published
v0.4.7 assets and runs `install.sh`.

## v0.4.5 shell installer

The server archive contains `install.sh`. The script accepts Debian 13 x86-64,
Ubuntu 24.04, and Ubuntu 26.04. Before an update stops a service, it refuses a
downgrade and an installation older than 0.3.0. A minor or major bump requires
`min_upgrade_from` in `release.json`. This package sets that field to `0.3.0`.
A same-version package leaves the installation unchanged. A patch update from
v0.4.4 keeps the guest image, the runtime format, configuration, enrollment,
and disks.

GitHub Actions can install one published release on dark-forge.dev. A second
workflow only reads that host. The v0.4.4 deploy uploads this tree's
`install.sh`, because the published v0.4.4 archive contains `install.py` and
that copy rejects Debian 13. A newer stable tag deploys the `install.sh` inside
its archive.

A pull request runs the checks and does not build a release package. Branch
builds restore the Rust, npm, and Playwright caches. Tag builds skip those
caches. Each identity limits test uses its own clock. The dark-forge first
install is recorded in `docs/runbooks/2026-10-19-dark-forge-first-install.md`.
`.cursor/skills/verify-kiln/` drives a checkout and records the evidence.

Publishing this release does not deploy a server or restart a live service.
The public bootstrap pins the published v0.4.7 assets and runs `install.sh`.

## v0.4.4 dashboard frontend

The release includes `kiln-dashboard-v0.4.4.tar.gz` with the built React dashboard
and a SHA-256 checksum. It contains responsive inventory, detail views, templates,
operations, settings and a console guide, with locally bundled fonts. See the
[dashboard guide](../web/README.md) for static hosting and the proposed API contract.

**This is a frontend preview, not a connected management console.** Open
`/boxes?preview=1` for labelled, read-only fixtures. The default live view reports
a disconnected state because `/api/v1/dashboard` and browser authentication are
not integrated with the existing CLI API. VM mutations, terminal, SSH and token
controls remain disabled. No live VM operations are claimed by browser tests.

CI tests and builds the dashboard and includes it in the release separately from
the server installer. Publishing does not deploy it or change live services.
Server binary-only updates also accept v0.4.3; runtime and guest formats remain
unchanged, and no guest-image or state migration is performed.

## v0.4.3 recovery-test synchronization

The interrupted-jailer test waits for the child executable and working-directory
identities before recording its launcher. Linux `posix_spawn` can return while
procfs still exposes the parent's executable. The race was reproduced with a
low-priority child on one CPU; the corrected test passed 2,000 repetitions under
the same conditions. Production launch uses `pre_exec` and Rust's fork/exec
error-pipe synchronization, which waits beyond executable publication. Process
identity and PID-reuse protections are unchanged.

This release also includes the previously merged rate-limit persistence,
exclusive private-CA trust, duplicate-name rejection and guest-listener fixes.
Binary-only updates accept v0.4.2 and previously supported Kiln versions without
changing guest/runtime formats. Existing guest images are not replaced. No live
server deployment or restart was performed for this release.

## v0.4.2 CLI usability

Box commands accept names and unique ID prefixes as well as full IDs. Ambiguous
targets fail with a candidate list. `kiln create` selects the only available
template and generates a readable name; explicit names and templates still work.
Human-readable lists have headings and short IDs; JSON and exec byte/exit-code
contracts are unchanged. SSH/exec errors suggest starting stopped boxes or
resuming paused ones without doing so automatically.

These are client-side changes; existing v0.4.0/v0.4.1 servers and their boxes
need no restart, state migration or image rebuild. Existing names are preserved.
The server package also allows binary-only updates from v0.4.1 with unchanged
runtime, journal, authentication, guest protocol and unit contracts.
See [CLI usage and retry rules](remote-client.md#everyday-commands-v042-client).

## v0.4.1 device-page recovery

The identity service now returns to code entry when a browser remembers an
approved, denied, expired or consumed request. Pending requests still resume
after provider sign-in. Explicit invalid codes retain their error page, and
"Enter another code" opens an empty form even when another request is remembered.
No browser-cookie clearing, account changes or database migration is required.
Existing v0.4.0 clients and VM servers work with the corrected identity service;
they do not need an update for this fix. Binary-only server updates also accept
v0.4.0 and retain enrollment and guest state.

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
version you download. macOS binaries are not signed or notarized.

## v0.3.0 compatibility

Kiln v0.3.0 renames the client to `kiln`, the operator tool to `kiln-runtime`,
and the host/API services to `kiln-host` and `kiln-api`. Rust crates, release
archives, guest units, SSH upgrade headers and `KILN_*` environment variables
use the new identity. Source is `Jabbslad/kiln`. The public bootstrap
is `deploy/bootstrap/install.sh` in that repository.

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
curl -fsSL https://raw.githubusercontent.com/Jabbslad/kiln/main/deploy/bootstrap/install.sh | sh
```

The script sets its version from the latest stable release tag and checks the
SHA-256 sidecar published with that archive.

On a dedicated **Ubuntu 24.04, Ubuntu 26.04, or Debian 13 x86-64/KVM** server:

```sh
curl -fsSL https://raw.githubusercontent.com/Jabbslad/kiln/main/deploy/bootstrap/install.sh | sh -s -- server
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
public installer pins v0.4.10. These features require the new client, server,
and updated Ubuntu guest agent. Existing templates/boxes are not
upgraded automatically. SSH/SFTP require OpenSSH on Linux and macOS.
See the [client commands](remote-client.md#interactive-terminal-files-and-editors).

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
curl -fsSL https://raw.githubusercontent.com/Jabbslad/kiln/main/deploy/bootstrap/install.sh | sh
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

In a new download directory:

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

Put `$HOME/.local/bin` on your shell's PATH if it is not already there.
Verify downloads before execution; do not disable system-wide OS security
policies to run an unsigned pilot binary.

Checksums detect corruption, not a compromised publisher. The trust boundary is
the release publisher and the reviewed workflow; signing is not implemented yet.
The bootstrap embeds reviewed digests rather than trusting a downloaded sidecar alone.

## Server requirements and manual installation

Start with a **dedicated Ubuntu 24.04 or 26.04 x86-64 systemd host** with usable `/dev/kvm`,
cgroup v2, at least 6 GiB currently available RAM and 24 GiB free on `/var/lib`
after extracting the package. A 16 GiB+ host is recommended. Allow additional
temporary space for the downloaded archive and extracted image (about 3 GiB).
The shell bootstrap manages sqlite3, OpenSSL, curl, CA certificates, tar, and
Ubuntu account tools. For manual installation, supply these dependencies yourself.
`deploy/install.sh` only checks them. Installing and deploying Kiln is shell
scripts. The public bootstrap pins v0.4.10 and runs `install.sh`. It rejects an
archive whose installer is `install.py`. `cargo xtask release` packages releases in
CI and does not run on the server.
The installer downloads checksum-pinned Firecracker/jailer 1.17.0 from GitHub. The package
already contains the kernel, Ubuntu filesystem and guest agent.

The server must be reachable over your existing LAN or VPN. The installer does
not configure a VPN or open the host API firewall. Guest networking is off by
default; new source packages support explicit `--network-uplink INTERFACE`
provisioning, including NAT/filtering and a boot unit (see
[networking](runtime-networking.md)). Pick a stable IPv4 address
assigned to that private interface. Public addresses and `0.0.0.0` are refused;
omitting the address when invoking `install.sh` directly installs a **local-only**
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
sudo -n bash kiln-server/install.sh --address 192.168.1.20 --apply
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
Unix token permissions must be 0600 or 0400. The service currently supports buffered exec,
while v0.1.1 does not include an interactive shell, SSH, file transfer, or guest
internet access. Those additions require the new builds described above.

The server leaf certificate expires after **one year**. Renew it before expiry,
using the retained private CA and the same IP SAN, and restart `kiln-api` after
replacing the certificate/key. There is no automatic certificate renewal yet.
See [operation recovery and service details](remote-client.md) for interrupted
requests, backups, token rotation, quotas and the operation-journal capacity.

## Maintainer workflow

Pushes to main run the package jobs and keep the archives as run artifacts for
30 days. A pull request that changes code runs the checks and the client tests
and does not upload packages. A documentation pull request runs the Python
checks only. Use `gh run download RUN_ID --repo OWNER/NAME` for branch builds; do not
confuse those with a reviewed release. The build workflow uses no credentials
beyond the scoped GitHub Actions token. A releasable push to main creates the next patch tag, publishes the release, and deploys it. Public workflows/logs/artifacts
must not contain credentials or runtime data. Fork PRs use hosted runners and
read-only permissions. The release-plan job and the draft-release job have contents-write permission.
The attest job signs the package archives a tag publishes. A main push
signs the archives a later tag reuses. A tag whose package jobs rebuild
signs that rebuild. A manual run on main signs the archives it builds. A pull
request does not run the job. The job has `id-token` and attestations
write, and it does not have contents write.
A reused archive was signed on main, so verify with `gh attestation verify ARCHIVE --repo Jabbslad/kiln --cert-identity-regex '^https://github\.com/Jabbslad/kiln/\.github/workflows/build\.yml@refs/(heads/main|tags/v[0-9]+\.[0-9]+\.[0-9]+)$'`, which accepts `refs/heads/main` or `refs/tags/vX.Y.Z` and rejects any other branch.

A push to main is releasable when the diff is not documentation only. The release-plan job reads the highest existing `vMAJOR.MINOR.PATCH` tag, including a tag that has no GitHub release, and chooses the next patch. It creates that lightweight tag on the push commit before the job returns. The same run stamps the packages it just built onto that tag, checks every shipped client, and publishes the release. `install.sh` is rendered in that job from the archive checksums. The job publishes only after `sha256sum -c` and
`gh attestation verify` succeed for every archive. Attestations for this run are bound to `refs/heads/main`, so verification uses `--source-ref refs/heads/main`. The identity regex is
`^https://github\.com/Jabbslad/kiln/\.github/workflows/build\.yml@refs/(heads/main|tags/v[0-9]+\.[0-9]+\.[0-9]+)$`.
A documentation-only push does not create a tag. A tag push does not start this workflow. Branch and pull-request
builds keep the committed workspace version. A local stamp still rewrites
`Cargo.toml` and `Cargo.lock` when they differ. Do not commit that rewrite.
When the jobs pass, the workflow publishes the release, including that filled
`install.sh`. That same run then calls `.github/workflows/deploy.yml`.
Reruns do not overwrite
existing releases. Real KVM tests remain separate; ordinary CI tests do not prove
host isolation, reboot persistence or installer success on a real server.

Do not register a privileged production/KVM server as a runner for pull-request
code. Keep runtime directories, tokens and enrollment bundles out of Git. The
packager includes only named binaries, installer files and pristine image files.
Changing repository visibility later is a separate decision; this workflow never
makes that change.

The installer version is the release tag.

The copy of `deploy/bootstrap/install.sh` on main still has the publish
template in `version=` and the asset block. Running that copy reads the
latest stable release tag and the SHA-256 sidecar for each archive, then
downloads that tag and refuses the archive when the checksum differs.
The tag job renders `install.sh` from the archive checksums and writes that
tag into `version=`. The rendered copy installs that tag and checks the
checksums in its asset block. A `v*` tag that is not the `Cargo.toml`
version stamps that tag into the reused binaries, archive names, and
`release.json`, then re-attests those archives. A matching tag publishes
main's archives unchanged. The reused Linux client must print the tag.
The installer accepts the download when `kiln --version` prints `kiln` and
that version.
The tag job publishes only after `sha256sum -c` and
`gh attestation verify` succeed for every archive. The same job attaches
`install.sh` and `install.sh.sha256`. Publishing a release leaves main
unchanged. The tag does not have to match the workspace version in
`Cargo.toml`, and the tag job does not commit a version rewrite back to main.
The public command stays on the raw main URL.
`releases/latest/download/install.sh` returns 404 for v0.4.7, and the
published v0.4.14 `install.sh` reports 0.4.10 while it downloads 0.4.14.
Leave that URL until Jamie publishes a release whose `install.sh` reports
the tag it downloads.
Never publish server state or credentials.
The ordinary build workflow has no cross-repository publishing credentials.

## Deploy from GitHub Actions

GitHub-hosted runners install a published server release on `root@dark-forge.dev`.
Publishing a release runs the `deploy` job in `.github/workflows/build.yml`, which calls
`.github/workflows/deploy.yml`. The Actions tab on `main` can still run that
same workflow. `.github/workflows/diagnose.yml` is
manual and only reads the host. None of these workflows run for a pull request or
a fork.

The `dark-forge` environment is the approval gate. The API stays on the installer
default, a private address at port 8443 with the private CA. The deploy does not
open port 443 or install the dashboard. It installs the release's
`kiln-identity` archive, checks its checksum, and restarts `kiln-identity`
before the server installer restarts `kiln-app`. Guest NAT stays
off. When `address` is empty, the deploy detects one private source address.
The deploy stops if the host's default address is public. A manual run can
set the `address` input.

`v0.4.4` deploys with this tree's `install.sh`, because that release's own
archive contains `install.py`, which rejects Debian 13. A newer stable tag
deploys with the `install.sh` from its archive. Tags older than `v0.4.4`, and
pre-releases, are refused before SSH. The release checksum is verified before
either installer runs. Deploy does not use Python on the server. After doctor
and the health check pass, the deploy creates a box from the `ubuntu-4g`
template, runs a command in it, checks the output, and deletes the box. The job
fails if create, exec, the output check, or delete fails.

Run this once on the server as root. It prints the two environment secret values.

```sh
install -d -m 700 /root/.ssh
ssh-keygen -t ed25519 -f /root/.ssh/kiln-github-deploy -N "" -C kiln-github-deploy
cat /root/.ssh/kiln-github-deploy.pub >> /root/.ssh/authorized_keys
chmod 600 /root/.ssh/authorized_keys
printf 'Environment secret DARK_FORGE_SSH_KEY:\n'
cat /root/.ssh/kiln-github-deploy
printf 'Environment secret DARK_FORGE_KNOWN_HOSTS:\n'
printf 'dark-forge.dev %s\n' "$(cat /etc/ssh/ssh_host_ed25519_key.pub)"
```

Create an environment named `dark-forge`. Store `DARK_FORGE_SSH_KEY` and
`DARK_FORGE_KNOWN_HOSTS` there as environment secrets. Leave required reviewers
off. CI on the current head is the only merge gate, and a reviewer would block
the tag deploy. Limit deployment branches to `main` and deployment tags to `v*`.
Jobs that set `environment: dark-forge` receive these secrets. A workflow that
does not name the environment cannot read them. The only step that reads
`DARK_FORGE_SSH_KEY` and `DARK_FORGE_KNOWN_HOSTS` is `Install the release` in
`.github/workflows/deploy.yml`. The release workflow calls that workflow and
does not read the key itself.
After the secrets are saved, delete the private key from the server with
`shred -u /root/.ssh/kiln-github-deploy`.

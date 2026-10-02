# Private builds and installation

GitHub Actions builds the client and server; neither destination needs Rust.
The repository and its release downloads can remain private. GitHub authentication
is needed to download a package, **not** to run an installed client or server.

This is a trusted-workload, single-administrator pilot, not a hosted multi-tenant
sandbox. The automation is checked in; that alone does not mean a GitHub build,
release or real server installation has succeeded. Check the Actions run for the
version you download. macOS binaries are not signed/notarized, and Windows
binaries are not Authenticode-signed.

**Ubuntu host support:** starting with `v0.1.1`, both installer layers admit Ubuntu
24.04 and 26.04 x86-64 while retaining capability and fresh-install checks. The
provisioner passed read-only preflight on `ser7` with the release image and its
private IPv4 address; full provisioning/reboot remains unverified. The older
`v0.1.0` server package accepts only Ubuntu 24.04; changing its shell gate alone
does not add support for 26.04. The guest image remains Ubuntu 24.04 on either host.

## One-command installation

On a macOS or supported Linux laptop, run in a terminal:

```sh
curl -fsSL https://raw.githubusercontent.com/Jabbslad/boxd-install/main/install.sh | sh
```

On a dedicated **Ubuntu 24.04 or 26.04 x86-64/KVM** server:

```sh
curl -fsSL https://raw.githubusercontent.com/Jabbslad/boxd-install/main/install.sh | sh -s -- server
```

Only the bootstrap is public. It prompts for a GitHub token to download pinned
private release assets. Create a short-lived
[fine-grained token](https://github.com/settings/personal-access-tokens/new),
select owner `Jabbslad`, repository `boxd`, and repository permission
**Contents: Read-only**. Paste it into the hidden terminal prompt, not the command
line. You can revoke it after installation. No `gh`, Python, Rust, or JSON parser
is required on the laptop. GitHub authentication is unrelated to boxd enrollment.

**v0.2.0 guest-access additions:** packages include interactive SSH,
SFTP, editor SSH configuration, and optional isolated guest IPv4 egress. The
public installer pins v0.2.0. These features require the new client, server,
and updated Ubuntu guest agent. Existing templates/boxes are not
upgraded automatically. SSH/SFTP require OpenSSH on Linux/macOS; Windows retains
management-only support. See the [client commands](remote-client.md#interactive-terminal-files-and-editors).

The client installs to `~/.local/bin/boxctl` without sudo and prints PATH setup
instructions if needed. Existing installations are preserved, not upgraded.
The server bootstrap prompts for its private IPv4 address and permission to
install Ubuntu system packages (including Python) through sudo, then invokes the
existing installer. You do not install dependencies or invoke Python yourself.
The server still needs the resources and LAN/VPN connectivity described below;
containers and non-KVM hosts are not supported.

The script checks archive digests before extraction and sends your GitHub token
only to `api.github.com`. It removes temporary credentials on normal exit and
handled signals. Checksums assume trust in the public bootstrap publisher, not
independent package signing. To inspect before execution, download the script
with `curl -fsSL URL -o install.sh`, read it, then run `sh install.sh [server]`.
Native Windows continues to use the private release zip described below.

**Fresh-host provisioning and reboot remain unverified.** The pilot uses the copy
disk backend, not the faster overlay benchmark configuration. After server setup,
follow [Connect the laptop](#connect-the-laptop); installation does not transfer
administrator credentials automatically.

## Manual client download (optional)

Install [GitHub CLI](https://cli.github.com/) and run `gh auth login`. Use an
account with access to the private repository. Set `REPO` to its `OWNER/NAME` and
`VERSION` to a published release tag (for example `v0.1.1`). A maintainer can also
download a draft; other readers need the release published within the private
repository first. Publishing a release **does not** make the repository public.

Choose one target:

| Laptop | Target |
| --- | --- |
| Apple Silicon Mac | `aarch64-apple-darwin` |
| Intel Mac | `x86_64-apple-darwin` |
| Linux x86-64, glibc 2.39+ | `x86_64-unknown-linux-gnu` |
| Windows x86-64 | `x86_64-pc-windows-msvc` (`.zip`, contains `boxctl.exe`) |

For macOS/Linux, in a new download directory:

```sh
REPO=OWNER/NAME
VERSION=v0.1.1
TARGET=aarch64-apple-darwin
gh release download "$VERSION" --repo "$REPO" \
  --pattern "boxctl-$VERSION-$TARGET.tar.gz*"
shasum -a 256 -c "boxctl-$VERSION-$TARGET.tar.gz.sha256"
tar -xzf "boxctl-$VERSION-$TARGET.tar.gz"
mkdir -p "$HOME/.local/bin"
install -m 755 boxctl "$HOME/.local/bin/boxctl"
"$HOME/.local/bin/boxctl" --version
```

Put `$HOME/.local/bin` on your shell's PATH if it is not already there. On Windows,
download the matching `.zip` and `.zip.sha256` with `gh release download`, compare
`Get-FileHash -Algorithm SHA256` to the checksum, then `Expand-Archive` and place
`boxctl.exe` in a directory on your user PATH. Verify downloads before execution;
do not disable system-wide OS security policies to run an unsigned pilot binary.

Checksums detect corruption, not a compromised publisher. The trust boundary is
your authenticated GitHub repository and the reviewed workflow; signing is not
implemented yet. Do not paste a GitHub token into a URL or installer command.

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
omitting the address installs a **local-only** endpoint at `127.0.0.1`.

Download as your normal user (or download on your laptop and securely copy both
files to the server). Do not copy your GitHub credentials into the service:

```sh
REPO=OWNER/NAME
VERSION=v0.1.1
PACKAGE="boxd-server-$VERSION-x86_64-unknown-linux-gnu.tar.gz"
gh release download "$VERSION" --repo "$REPO" --pattern "$PACKAGE*"
sha256sum -c "$PACKAGE.sha256"
mkdir boxd-server
tar -xzf "$PACKAGE" -C boxd-server
sudo python3 boxd-server/install.py --address 192.168.1.20 --apply
```

Replace `192.168.1.20` with the server's private/VPN address. Review the displayed
plan and type `INSTALL`. Omit `--apply` for prerequisite checks only. The installer:

- Refuses existing installations, partial installs, unit overrides, conflicting
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
retained VM using the local `box` tool first. Upgrades/uninstall are not automated.

## Connect the laptop

The installer creates `/etc/boxd/laptop.tar.gz` with the public CA certificate,
administrator token and `CONNECT.txt`. **Treat this bundle as a password.**
Transfer it over an existing trusted SSH/SFTP connection, using a private staging
file if your SSH account cannot read root files. Never attach it to an issue,
release, chat or repository. GitHub credentials are unrelated to this token.

Extract into a private, permanent directory on your laptop and run the one
`boxctl profile add` command in `CONNECT.txt`, then:

```sh
boxctl templates
boxctl create --template ubuntu-4g --name first-box
boxctl list
boxctl exec BOX_ID -- /bin/sh -c 'printf hello'
```

Profiles reference the extracted token/CA files; do not delete them after setup.
Unix token permissions must be 0600 or 0400. On Windows, restrict the directory
and token ACL to your account. The service currently supports buffered exec,
while v0.1.1 does not include an interactive shell, SSH, file transfer, or guest
internet access. Those additions require the new builds described above.

The server leaf certificate expires after **one year**. Renew it before expiry,
using the retained private CA and the same IP SAN, and restart `boxd-api` after
replacing the certificate/key. There is no automatic certificate renewal yet.
See [operation recovery and service details](remote-client.md) for interrupted
requests, backups, token rotation, quotas and the operation-journal capacity.

## Maintainer workflow

Pushes and pull requests run Linux workspace checks, build the server/guest image,
and run native client TLS/CLI tests on Linux, macOS ARM/Intel and Windows. Archives
and individual SHA-256 files are available as private run artifacts for seven
days. Use `gh run download RUN_ID --repo OWNER/NAME` for branch builds; do not
confuse those with a reviewed release. CI uses no credentials beyond the scoped
GitHub Actions token and never deploys a server. Private-repository build minutes
and artifact storage count against the account's allowance and may incur costs.

For an approved release, update the workspace version, commit the reviewed source
and push a matching `vMAJOR.MINOR.PATCH` tag. Tag/version mismatches fail. When all
jobs pass, the workflow creates a **draft release**, only if the repository is
still private. Review/download the draft and complete real Ubuntu/KVM installation
validation before publishing it to repository readers. Reruns do not overwrite
existing releases. Real KVM tests remain separate; ordinary CI tests do not prove
host isolation, reboot persistence or installer success on a real server.

Do not register a privileged production/KVM server as a runner for pull-request
code. Keep runtime directories, tokens and enrollment bundles out of Git. The
packager includes only named binaries, installer files and pristine image files.
Changing repository visibility later is a separate decision; this workflow never
makes that change.

The public installer is maintained in `deploy/bootstrap/`. Only `install.sh` and
its public `README.md` may be copied to `Jabbslad/boxd-install`. After an approved
private release is published, update the script's pinned asset IDs and SHA-256
values from verified packages, run `scripts/test-bootstrap.py` and ShellCheck,
and verify a real authenticated download before publishing the public copy.
Never publish this private checkout, server state, or GitHub credentials there.
The ordinary build workflow has no cross-repository publishing credentials.

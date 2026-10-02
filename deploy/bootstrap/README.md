# boxd installer

Public bootstrap for **private** boxd pilot packages. This repository contains
only installation instructions and a shell script, not the platform source,
binaries, or credentials. You need access to `Jabbslad/boxd` to download packages.

## Laptop: macOS or Linux

Run in a terminal:

```sh
curl -fsSL https://raw.githubusercontent.com/Jabbslad/boxd-install/main/install.sh | sh
```

Supports Apple Silicon/Intel macOS and x86-64 Linux with glibc 2.39+. Installs
`~/.local/bin/boxctl` without sudo. Add that directory to PATH if prompted. No
GitHub CLI, Python, Rust, or JSON parser is required. Standard shell tools, curl,
tar and a SHA-256 utility must be present. Native Windows users should download
the `boxctl` zip from the private release instead; this is not a PowerShell installer.

The script asks for a GitHub token with read access to the private release.
Create a short-lived [fine-grained token](https://github.com/settings/personal-access-tokens/new):
choose owner **Jabbslad**, repository **boxd**, permission **Contents: Read-only**.
Paste it at the hidden prompt, never into the command itself. You can revoke it
after installation. Tokens are not saved in the installed application.

### Upgrade an existing laptop client

```sh
curl -fsSL https://raw.githubusercontent.com/Jabbslad/boxd-install/main/install.sh | sh -s -- client --upgrade
boxctl --version
```

The explicit upgrade verifies the package checksum and executable version before
atomically replacing `~/.local/bin/boxctl`. The previous binary is retained as
`~/.local/bin/boxctl.previous` (replaced on the next upgrade); profiles, tokens and
CA files are untouched. Failed downloads or validation leave both binaries alone.
To roll back, run `mv ~/.local/bin/boxctl.previous ~/.local/bin/boxctl`.

Symbolic links and non-regular destinations are refused. Concurrent installers
are blocked by `~/.local/bin/.boxctl-install.lock`; if an installer was forcibly
killed, confirm it is no longer running before removing that empty directory.
Plain installation still refuses to overwrite an existing client. This option
does not upgrade a server or its guest images.

## Server: dedicated Ubuntu 24.04 or 26.04 x86-64/KVM

```sh
curl -fsSL https://raw.githubusercontent.com/Jabbslad/boxd-install/main/install.sh | sh -s -- server
```

Use a systemd host with usable `/dev/kvm`, cgroup v2, 6 GiB currently available
RAM and 24 GiB free on `/var/lib`, plus about 3 GiB temporary extraction space.
16 GiB+ total RAM is recommended. Containers are not supported. The guest image
remains Ubuntu 24.04 regardless of the supported host Ubuntu version.

The script downloads and verifies the package, prompts for a stable private/VPN
IPv4 address assigned to the server, and requests permission to install required
Ubuntu packages through sudo. These include Python; you do not install or invoke
it yourself. The provisioner checks resources/conflicts and requests a separate
`INSTALL` confirmation before creating accounts, services, TLS credentials, and
a 4 GiB warm Ubuntu template. No VPN or API firewall opening is configured.

Guest networking is off by default. On a fresh host, explicitly opt in to
filtered IPv4 egress by setting the host's uplink interface (replace `enp1s0`):

```sh
curl -fsSL https://raw.githubusercontent.com/Jabbslad/boxd-install/main/install.sh | BOXD_NETWORK_UPLINK=enp1s0 sh -s -- server
```

This additionally installs iproute2/nftables and, after confirmation, enables
forwarding, a guest bridge, NAT/filter rules and a boot service. Host/LAN/metadata
and peer-guest access are blocked. Review coexistence with any host firewall
manager. This option cannot retrofit an existing immutable runtime store.

**This is a trusted-workload pilot, not a production multi-tenant sandbox.**
Fresh-host setup and reboot validation remain outstanding. The initial server
uses the copy disk backend, not the faster overlay benchmark configuration.
There is no automated server upgrade/uninstall. Existing server destinations are refused.
If provisioning fails, retain the state and inspect logs; do not delete VM state
or blindly rerun setup.

## Connect

After server setup, securely transfer `/etc/boxd/laptop.tar.gz` to the laptop
using existing SSH/SFTP. It contains an administrator token: treat it as a
password and never upload it to a repository, issue, or chat. Extract into a
permanent private directory and follow `CONNECT.txt`, then run:

```sh
boxctl templates
boxctl create --template ubuntu-4g --name first-box
boxctl list
boxctl exec BOX_ID -- /bin/sh -c 'printf hello'
boxctl ssh BOX_ID
boxctl cp ./local-file BOX_ID:/workspace/remote-file
boxctl ssh-config BOX_ID
```

Keep the extracted credential files; profiles reference them. Version 0.2.0 adds
interactive SSH, SFTP and editor SSH configuration over the same HTTPS endpoint;
no port 22 exposure is needed. Linux/macOS need OpenSSH (`ssh`, `scp`, `ssh-keygen`).
Windows supports management commands only. Existing servers/templates need a
controlled update; reinstalling the client alone does not update guest images.
Server certificates need manual renewal within one year.

Version 0.2.1 fixes login readiness in newly built guest images, removing the
stale "System is booting up" warning. Existing boxes need a guest-side repair
or migration; a client upgrade cannot change their disks. Networkless boxes
also cannot gain a virtual NIC in place. Internet access requires explicitly
provisioned network-enabled replacements, with existing data preserved during
a reviewed migration.

## Trust and maintenance

The bootstrap pins v0.2.1 asset IDs and SHA-256 digests, validates archive contents,
and authenticates only to GitHub's API. Redirected asset requests do not receive
the GitHub token. Temporary secrets are removed on normal exit and handled
signals. A checksum protects integrity under trust in this bootstrap publisher;
it is not independent signing. macOS/Windows binaries are not signed/notarized.

To review rather than pipe directly into a shell:

```sh
curl -fsSL https://raw.githubusercontent.com/Jabbslad/boxd-install/main/install.sh -o install.sh
less install.sh
sh install.sh
```

The authoritative files live under `deploy/bootstrap/` in the private source
repository. Maintainers publish only `install.sh` and this README after testing
and verifying the release assets. No private checkout history belongs here.

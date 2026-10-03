# Kiln installer

Public, non-interactive bootstrap for Kiln pilot packages. This repository contains
only installation instructions and a shell script, not the platform source,
binaries, or credentials. Source and releases are public in
[Jabbslad/kiln](https://github.com/Jabbslad/kiln). No GitHub account or PAT is needed.

**v0.3.0 requires matching Kiln client, server and freshly built guest templates.**
It is not an in-place upgrade from v0.2.x/boxd. No legacy migration is provided;
start with a clean Kiln installation. Do not reuse an old runtime store or
install beside it on the same server. Fresh installation uses new configuration
paths and requires explicit enrollment with a matching server.

## Laptop: macOS or Linux

Run locally or in automation (no terminal required):

```sh
curl -fsSL https://raw.githubusercontent.com/Jabbslad/kiln-install/main/install.sh | sh
```

Supports Apple Silicon/Intel macOS and x86-64 Linux with glibc 2.39+. Installs
`~/.local/bin/kiln` without sudo. Add that directory to PATH if it is not present. No
GitHub CLI, Python, Rust, or JSON parser is required. Standard shell tools, curl,
tar and a SHA-256 utility must be present. Native Windows users should download
the `kiln` zip from the public release instead; this is not a PowerShell installer.

The script downloads a versioned release over HTTPS and verifies its pinned
SHA-256 checksum. It never asks for credentials or reads answers from stdin.

### Browser login

The v0.4.0 pilot includes GitHub/Google login and `https://dark-forge.dev` as its
default identity service. After your administrator enrolls the server, run:

```sh
kiln login
```

Approve the device in the browser, then return to the terminal. No URL, token or
CA file is required on the laptop. Use the same provider and account that enrolled
the server. If you already have a direct administrator profile, preserve it and
use `kiln --profile personal login`. No enrolled server means login stops with
instructions to enroll; it does not create a hosted box or open a network tunnel.
LAN/VPN access to the private host is still required.

HTTPS, provider sign-in pages and native client tests pass. Completed real-account
consent and full central-authenticated VM acceptance remain outstanding; this
release provides the normal installer flow for pilot testing, not production certification.

### Automatic client updates

```sh
curl -fsSL https://raw.githubusercontent.com/Jabbslad/kiln-install/main/install.sh | sh
kiln --version
```

Rerunning the same command verifies the package checksum and executable version before
atomically replacing `~/.local/bin/kiln`. The previous binary is retained as
`~/.local/bin/kiln.previous` (replaced on the next upgrade); profiles, tokens and
CA files are untouched. Failed downloads or validation leave both binaries alone.
An identical install is a no-op and preserves the previous backup. Downgrades are
refused. No `--upgrade` flag is needed (the old client alias remains accepted).
To roll back, run `mv ~/.local/bin/kiln.previous ~/.local/bin/kiln`.

Symbolic links and non-regular destinations are refused. Concurrent installers
are blocked by `~/.local/bin/.kiln-install.lock`; if an installer was forcibly
killed, confirm it is no longer running before removing that empty directory.
The client command does not update a server or its guest images.

## Server: dedicated Ubuntu 24.04 or 26.04 x86-64/KVM

```sh
curl -fsSL https://raw.githubusercontent.com/Jabbslad/kiln-install/main/install.sh | sh -s -- server
```

Use a systemd host with usable `/dev/kvm`, cgroup v2, 6 GiB currently available
RAM and 24 GiB free on `/var/lib`, plus about 3 GiB temporary extraction space.
16 GiB+ total RAM is recommended. Containers are not supported. The guest image
remains Ubuntu 24.04 regardless of the supported host Ubuntu version.

**Running `server` authorizes installation or updating without further confirmation.** Use
a root shell or an account with passwordless sudo; the script uses `sudo -n` and
fails instead of asking for a password. It installs required Ubuntu packages
(including Python), then checks resources/conflicts before creating accounts,
services, TLS credentials, and a 4 GiB warm Ubuntu template. Package configuration
is non-interactive and retains existing configuration files. No VPN or API
firewall opening is configured.

On fresh installs, the server address is the source IPv4 selected by `ip -4 route get 1.1.1.1`
(a local route lookup, not a network request). It must be private/VPN IPv4;
public or ambiguous results fail with an error. Detection requires iproute2.
For a VPN, multi-interface host, or no default route, override it explicitly:

```sh
curl -fsSL https://raw.githubusercontent.com/Jabbslad/kiln-install/main/install.sh | sh -s -- server --address 100.64.5.6
```

Choose a stable address: it becomes the API binding and TLS certificate address.
`--address 127.0.0.1` is available for deliberate local-only installation.

Guest networking is off by default. On a fresh host, explicitly opt in to
filtered IPv4 egress with automatic uplink detection:

```sh
curl -fsSL https://raw.githubusercontent.com/Jabbslad/kiln-install/main/install.sh | sh -s -- server --network
```

Use `--network-uplink enp1s0` to opt in with an explicit interface instead.
The existing `KILN_NETWORK_UPLINK` environment override is also supported.
This additionally installs iproute2/nftables/util-linux and enables
forwarding, a guest bridge, NAT/filter rules and a boot service. Host/LAN/metadata
and peer-guest access are blocked. Review coexistence with any host firewall
manager. This option cannot retrofit an existing immutable runtime store.

**This is a trusted-workload pilot, not a production multi-tenant sandbox.**
Fresh-host setup and reboot validation remain outstanding. The initial server
uses the copy disk backend, not the faster overlay benchmark configuration.
Uninstall is not automated. Partial or unhealthy installations require recovery.
If provisioning fails, retain the state and inspect logs; do not delete VM state
or blindly rerun setup.

### Automatic server updates

Rerun the same `server` command. v0.4.0 updates healthy, standard Kiln v0.3.0,
v0.3.2 or v0.3.3 installations and leaves identical v0.4.0 installations alone. It reuses
the existing address and networking, skips apt, and preserves configuration,
credentials, units, Firecracker, guest images, templates and VM disks. Explicit
address/network options must agree with the installed configuration.

The updater retains binaries under `/opt/kiln/upgrades/`, stops the gateway,
waits up to 60 seconds for accepted operations, then restarts the management
services with the new binaries. **API/SSH sessions disconnect briefly.** Guest
VMMs are left running using the host unit's required `KillMode=process`. Do not
run local `kiln-runtime` commands or change units/configuration during an update.
If operations remain busy, the gateway is restored without stopping the host.

Failed replacement/startup/readiness rolls back the binaries and checks the old
service. A power loss, forced termination or failed rollback leaves
`/opt/kiln/pending-upgrade.json` with backup locations; the next run refuses to
proceed until recovery is reviewed. Do not delete that marker to force a retry.
Use `journalctl -u kiln-host -u kiln-api` and the recorded backup for diagnosis.

Updates are binary-only, not guest-image or state migrations. Compatibility must
be reviewed for each release; unsupported versions and custom service commands
are refused. File replacement/rollback and simulated service failures are tested;
real-server update and reboot validation remain outstanding.

## Connect

After installing/updating the server, run the one-time command printed by the
installer on that host:

```sh
sudo /usr/local/libexec/kiln-api enroll --url https://YOUR-SERVER-IP:8443
```

Sign in through the printed browser URL and approve the server endpoint and CA
fingerprint. Enrollment briefly restarts the API, not the VMs. If interrupted
after persistence, follow the reported `enroll --resume` instruction. The
installer never enrolls or replaces server ownership automatically.

On the laptop, run `kiln login` with the same provider/account, then:

```sh
kiln templates
kiln create --template ubuntu-4g --name first-box
kiln list
kiln exec BOX_ID -- /bin/sh -c 'printf hello'
kiln ssh BOX_ID
kiln cp ./local-file BOX_ID:/workspace/remote-file
kiln ssh-config BOX_ID
```

For independent administrator recovery, keep `/etc/kiln/laptop.tar.gz` private.
Its `CONNECT.txt` configures a direct profile without central login. It contains
an administrator token: never upload it to a repository, issue or chat. Keep
extracted credential files because direct profiles reference them. Version 0.2.0 adds
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

Version 0.2.2 fixes network-enabled VM launches on hosts where entering the
network namespace previously hid the cgroup hierarchy from Firecracker jailer.
It remains compatible with the 0.2.1 guest image.

## Trust and maintenance

The bootstrap pins v0.4.0 release URLs and SHA-256 digests, validates archive contents,
and downloads without authentication. Redirects are HTTPS-only and user curl
configuration is disabled. Temporary files are removed on normal exit and handled
signals. A checksum protects integrity under trust in this bootstrap publisher;
it is not independent signing. macOS/Windows binaries are not signed/notarized.

To review rather than pipe directly into a shell:

```sh
curl -fsSL https://raw.githubusercontent.com/Jabbslad/kiln-install/main/install.sh -o install.sh
less install.sh
sh install.sh
```

The authoritative files live under `deploy/bootstrap/` in the source
repository. Maintainers publish only `install.sh` and this README after testing
and verifying the release assets. Runtime state and credentials never belong here.

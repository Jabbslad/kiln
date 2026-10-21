# Installer

`deploy/bootstrap/install.sh` is the public one-command installer. `client` (the default) downloads a pinned `kiln` binary into `~/.local/bin`. `server` downloads the server package and runs `deploy/install.sh` under passwordless sudo. Installing and deploying Kiln is shell scripts only. The bootstrap pins the latest published release and runs `install.sh`. An archive whose installer is `install.py` is rejected.

Tagged deploy and auto-release also ship `kiln-identity-*.tar.gz` and `kiln-dashboard-*.tar.gz`. `deploy/github/install-identity.sh` runs before server install; `install-remote.sh` passes `--dashboard-archive` into `deploy/install.sh`, runs `--healthcheck`, `smoke.sh` (host metrics via `check-dashboard-metrics.sh`), then `check-dashboard-bundle.sh`. `deploy/github/deploy.sh` SSH-fetches the dashboard body and jq-verifies host metrics on the runner after install completes.

## Sub-features

- `sh install.sh` and `sh install.sh client` install or update the laptop binary (Linux x86-64, macOS Intel, or macOS Apple silicon only; Windows client builds were removed in #110).
- `sh install.sh server` detects a private IPv4 from `ip -4 route get 1.1.1.1` and installs.
- `sh install.sh server --address PRIVATE_IP` selects the address.
- `sh install.sh server --network` or `--network-uplink INTERFACE` opts into guest egress.
- `bash deploy/install.sh --address IP --apply [--dashboard-archive PATH]` is the provisioner the shell calls. Without `--apply` it only preflights. `--apply` requires root outside the test fixture.
- Checksums are pinned in the `assets` function of `install.sh`. The version variable is set in `main`.
- Identity upgrade on deploy: `deploy/github/install-identity.sh` replaces `/usr/local/libexec/kiln-identity` from the release tarball with rollback probes.

## How to get to it (user POV)

On a laptop:

```sh
curl -fsSL https://raw.githubusercontent.com/Jabbslad/kiln/main/deploy/bootstrap/install.sh | sh
kiln --version
```

On an Ubuntu 24.04, Ubuntu 26.04, or Debian 13 x86-64 host with KVM, cgroup v2, and passwordless sudo:

```sh
curl -fsSL https://raw.githubusercontent.com/Jabbslad/kiln/main/deploy/bootstrap/install.sh | sh -s -- server
```

That command changes the host: accounts, systemd units, `/var/lib/kiln`, a private CA, and an administrator token. Do not run it on a shared or production machine to "see what happens."

## Driving it with the bootstrap harness

The harness runs the real `install.sh` with stub `curl`, `sudo`, `uname`, `ip`, and `getconf`. It does not download a release and does not provision this host.

```sh
cargo xtask test-bootstrap
cargo xtask test-install
cargo xtask test-release
```

A pass is all three commands exiting 0. Save the combined stdout in `$VERIFY_EVIDENCE/installer-harness.txt`.

`test-bootstrap` checks the pinned version and the SHA-256 lines inside `install.sh`, refuses a non-private detected address, and checks that `curl` is invoked with `--proto '=https'`. `test-install` runs `deploy/install.sh` with `KILN_INSTALL_FIXTURE` so destinations stay in a temp directory, including `--dashboard-archive` extraction. `test-release` packages tiny stand-in binaries for client, server, identity, and dashboard kinds.

Identity-only upgrade logic:

```sh
bash scripts/test-identity-upgrade.sh
```

ShellCheck, as in CI:

```sh
shellcheck scripts/fetch-firecracker.sh images/build-ubuntu.sh deploy/install.sh deploy/bootstrap/install.sh deploy/github/*.sh
```

Auto-release and deploy wiring:

```sh
cargo xtask test-deploy-workflow
sh scripts/test-installer-release.sh
```

See `features/auto-release.md` for the main-push release and deploy chain.

## Gotchas

- A green harness is not an installed server. Fresh-host provisioning and reboot are still described as unverified in `docs/releases.md`.
- `deploy/bootstrap/install.sh` `main` pins the latest published release when the template pin matches. The harness reads that pin from the script and checks it against the GitHub releases API.
- Server mode requires `/dev/kvm`, Ubuntu 24.04, Ubuntu 26.04, or Debian 13, systemd, and cgroup controllers `cpu`, `memory`, and `pids`. The harness stubs those checks.
- CI runs the xtask harnesses, shell fixture tests, and ShellCheck in `.github/workflows/build.yml`. `deploy.yml` and `diagnose.yml` SSH to dark-forge and do not run on pull requests.
- Bootstrap server install does not lay down `kiln-identity` or the dashboard archive; those arrive on tagged deploy/auto-release (`deploy/github/install-remote.sh`).

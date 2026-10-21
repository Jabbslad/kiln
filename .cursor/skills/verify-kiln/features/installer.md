# Installer

`deploy/bootstrap/install.sh` is the public one-command installer. `client` (the default) downloads a pinned `kiln` binary into `~/.local/bin`. `server` downloads the server package and runs `deploy/install.sh` under passwordless sudo. Installing and deploying Kiln is shell scripts only. The bootstrap pins the latest published release and runs `install.sh`. An archive whose installer is `install.py` is rejected.

Tagged deploy and auto-release also ship `kiln-identity-*.tar.gz` and `kiln-dashboard-*.tar.gz`. `deploy/github/install-remote.sh` runs `deploy/github/install-identity.sh` before server install, passes `--dashboard-archive` into `deploy/install.sh`, runs `kiln-api self-check` immediately after `--healthcheck` (before guest smoke), then `deploy/github/smoke.sh`, then `deploy/github/check-dashboard-bundle.sh` against the live app HTML.

## Sub-features

- `sh install.sh` and `sh install.sh client` install or update the laptop binary (Linux x86-64, macOS Intel, or macOS Apple silicon only; Windows client builds were removed in #110).
- `sh install.sh server` detects a private IPv4 from `ip -4 route get 1.1.1.1` and installs.
- `sh install.sh server --address PRIVATE_IP` selects the address.
- `sh install.sh server --network` or `--network-uplink INTERFACE` opts into guest egress.
- `bash deploy/install.sh --address IP --apply [--dashboard-archive PATH]` is the provisioner the shell calls. Without `--apply` it only preflights. `--apply` requires root outside the test fixture.
- Checksums are pinned in the `assets` function of `install.sh`. The version variable is set in `main`. Current server packages ship `operations-journal.max-version` beside `install.sh`; bootstrap accepts older archives that omit it and requires the file when present.
- Binary updates call `wait_for_operations` on `/var/lib/kiln/journal/operations.sqlite` before stopping `kiln-host`. The max supported schema version is pinned in `deploy/operations-journal.max-version` (also shipped beside `install.sh` and included by `crates/kiln-server/src/journal.rs`). The installer accepts versions `1` through that max; anything else fails with the sqlite command, the found version, and the accepted range. Any failed update step prints its name, command, exit code, and the last twenty output lines.
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

Pull-request tag-path rehearsal (`scripts/tag-path-dry-run.sh`) builds a server package from the PR, then runs `KILN_SERVER_UPGRADE_PREV_TAR` / `KILN_SERVER_UPGRADE_TO_TAR` through the same script against a v2 operation journal fixture.

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
- CI runs the xtask harnesses, shell fixture tests, and ShellCheck in `.github/workflows/build.yml`. Pull-request tag-path dry-run also rehearses a server upgrade over a fixture journal plus reuse-fetched release tarballs.
- `scripts/test-operations-journal-version-sync.sh` fails when `deploy/operations-journal.max-version` diverges from `crates/kiln-server/src/journal.rs`.
- Failed server update steps print step name, command, exit code, and trailing output (`install_step_fail` / `report_install_step` in `deploy/install.sh`).
- Bootstrap server install does not lay down `kiln-identity` or the dashboard archive; those arrive on tagged deploy/auto-release (`deploy/github/install-remote.sh`).

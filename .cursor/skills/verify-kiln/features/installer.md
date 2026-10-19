# Installer

`deploy/bootstrap/install.sh` is the public one-command installer. `client` (the default) downloads a pinned `kiln` binary into `~/.local/bin`. `server` downloads the server package and runs `deploy/install.sh` under passwordless sudo. The script in this repo is the source that is copied to `Jabbslad/kiln-install` as `install.sh` plus its public README. Nothing else from the repo belongs there. Installing and deploying Kiln is shell scripts. The bootstrap pins v0.4.6 and runs `install.sh`. An archive whose installer is `install.py` is rejected.

## Sub-features

- `sh install.sh` and `sh install.sh client` install or update the laptop binary.
- `sh install.sh server` detects a private IPv4 from `ip -4 route get 1.1.1.1` and installs.
- `sh install.sh server --address PRIVATE_IP` selects the address.
- `sh install.sh server --network` or `--network-uplink INTERFACE` opts into guest egress.
- `bash deploy/install.sh --address IP --apply` is the provisioner the shell calls. Without `--apply` it only preflights. `--apply` requires root outside the test fixture.
- Checksums are pinned in the `assets` function of `install.sh`. The version variable is set in `main`.

## How to get to it (user POV)

On a laptop:

```sh
curl -fsSL https://raw.githubusercontent.com/Jabbslad/kiln-install/main/install.sh | sh
kiln --version
```

On an Ubuntu 24.04, Ubuntu 26.04, or Debian 13 x86-64 host with KVM, cgroup v2, and passwordless sudo:

```sh
curl -fsSL https://raw.githubusercontent.com/Jabbslad/kiln-install/main/install.sh | sh -s -- server
```

That command changes the host: accounts, systemd units, `/var/lib/kiln`, a private CA, and an administrator token. Do not run it on a shared or production machine to "see what happens."

## Driving it with the bootstrap harness

The harness runs the real `install.sh` with stub `curl`, `sudo`, `uname`, `ip`, and `getconf`. It does not download a release and does not provision this host.

```sh
python3 scripts/test-bootstrap.py
python3 scripts/test-install.py
python3 scripts/test-release.py
```

A pass is all three unittest modules exiting 0. Save the combined stdout in `$VERIFY_EVIDENCE/installer-harness.txt`.

`test-bootstrap.py` checks the pinned version and the SHA-256 lines inside `install.sh`, refuses a non-private detected address, and checks that `curl` is invoked with `--proto '=https'`. `test-install.py` runs `deploy/install.sh` with `KILN_INSTALL_FIXTURE` so destinations stay in a temp directory. `test-release.py` packages tiny stand-in binaries.

ShellCheck, as in CI:

```sh
shellcheck scripts/fetch-firecracker.sh images/build-ubuntu.sh deploy/install.sh deploy/bootstrap/install.sh deploy/github/*.sh
```

## Gotchas

- A green harness is not an installed server. Fresh-host provisioning and reboot are still described as unverified in `docs/releases.md`.
- `deploy/bootstrap/install.sh` `main` pins `version=0.4.6`. The harness tests the pin that is in the script.
- Server mode requires `/dev/kvm` to exist, Ubuntu 24.04, Ubuntu 26.04, or Debian 13, systemd, and cgroup controllers `cpu`, `memory`, and `pids`. The authoring VM's `/dev/kvm` was not usable by uid 1000 until an ACL was added. The harness stubs those checks.
- CI runs the Python modules and ShellCheck in `.github/workflows/build.yml`. `deploy.yml` and `diagnose.yml` SSH to dark-forge and do not run on pull requests. The build workflow has no dependency advisory scan (`cargo audit`, `cargo-deny`, Dependabot, or OSV).
- The authoring proof ran the three Python modules from the repo root. All three exited 0 (32 bootstrap tests, 31 install tests, 6 release tests). `shellcheck` was not installed on that machine, so the ShellCheck line stayed unverified. The harness still does not install a server.

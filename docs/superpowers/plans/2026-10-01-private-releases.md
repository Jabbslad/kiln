# Private releases and guided installation

> Execute inline with the executing-plans skill. Preserve unrelated dashboard work.

**Goal:** Download clients and a ready-to-install server package from a private
GitHub repository without building Rust on either destination.

**Architecture:** GitHub-hosted native builds produce explicit allowlisted
archives and SHA-256 files. A reviewed version tag creates a draft release only
after all builds/tests pass. Download through authenticated `gh`, not a public
curl installer. A first-install Python program provisions one Ubuntu 24.04 x86-64
KVM host from the server archive. It never contacts GitHub with credentials.

**Constraints:** Keep Amp origin. Do not push, create releases, install services
or expose this runner during local development. Confirm the GitHub destination
before publishing. Do not distribute tokens, host snapshots, state, benchmark
artifacts, or the unrelated web work. Build production binaries without fault
injection. Use native macOS ARM/Intel, Linux x86-64 and Windows x86-64 clients.
Only Ubuntu 24.04 x86-64 is an installer target initially. Existing installations
and upgrades fail closed. Private Actions minutes/storage may incur charges.

## 1. Native build and packaging

- [x] Add `scripts/release.py` and `scripts/test-release.py`: client tar/zip,
  server tar with prebuilt warm Ubuntu image, explicit file lists and per-archive
  checksums. Test paths, executable modes, missing inputs, no-overwrite and that
  private files cannot enter packages. Start with failing tests.
- [x] Add `.github/workflows/build.yml`: PR/push tests and release builds,
  native client TLS tests, pinned Actions, least-privilege token, no self-hosted
  runners. Tag releases wait for all jobs, reject version mismatches and public
  repositories, and create drafts. No automatic deployment or public attestation.
- [x] Reuse `images/build-ubuntu.sh` and `scripts/fetch-firecracker.sh` without
  changing their pinned dependencies. Image build runs only in disposable CI.

## 2. Guided first installation

- [x] Add `deploy/install.py` plus `scripts/test-install.py`: validate private
  bind IPv4 (also used as the certificate IP SAN), supported OS, KVM, cgroups, tools, free space, empty
  destinations and unused UID/GID/subordinate ranges before mutation. A default
  check-only run prints the plan; `--apply` additionally requires root and a
  confirmation. Never execute package files while checking prerequisites.
- [x] Implement provisioning of dedicated locked accounts, fixed trusted paths, cgroup boot
  service, a locally generated CA/leaf certificate and token, copy-backend warm
  4 GiB template, and existing host/API units with private bind override.
  Generate a private laptop enrollment bundle, excluding the CA private key.
  Verify authenticated HTTPS catalog before reporting success. On failure stop
  enabled services but retain state for diagnosis; do not delete VMs or retry
  partially completed setup automatically.
- [x] Test validators at boundaries, generated policy/units, crypto with real
  OpenSSL and failure sequencing using temporary directories and fake commands.
  No live account/cgroup/service mutations during tests.

## 3. Delivery documentation and verification

- [x] Document authenticated release downloads, checksum validation, one-command
  server setup and laptop enrollment. Distinguish checked-in automation from
  actual published releases and tested installer behavior from a real install.
- [x] Run Python tests, Bash lint, workflow validation, workspace fmt/Clippy/tests
  and a real local client archive/extract/run check. Inspect archive contents.
- [x] Review explicit source publish list for credentials/generated artifacts.
  Request approval for the concrete private GitHub destination if still needed.

Not included: automatic upgrades, binary signing/notarization, public downloads,
hosted service deployment, networking inside guests, fs-verity filesystem
provisioning or snapshot overlay pool creation. The installer uses the existing
copy backend and does not claim the overlay benchmark's launch latency.

## Local verification and delivery state

82 ordinary Rust tests and 12 Python tests pass. Formatting, Clippy, Ruff,
ShellCheck and Actionlint pass. Real Linux release archives were produced,
checksummed, extracted and their binaries executed. Generated systemd units and
overrides pass `systemd-analyze verify` in a disposable filesystem tree; this does
not prove boot-time behavior. OpenSSL tests verify the generated CA/leaf and the
private enrollment bundle. No live accounts/services/cgroups were provisioned.

The installer correctly refuses this Ubuntu 26.04 runner; a complete fresh-host
Ubuntu 24.04/KVM install and reboot test remain outstanding. Native macOS/Windows
execution and the hosted workflow need the first GitHub run. After local review,
the owner approved creating private `Jabbslad/kiln`, committing/pushing this work
and following the first build. The repository was created and its private
visibility verified. Publish to GitHub `main` through a separate `github` remote,
preserving Amp `origin` and leaving unrelated dashboard work out. This approval
does not deploy the server or publish a versioned release.

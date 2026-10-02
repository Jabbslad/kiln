# Guest Access Implementation Plan

Execute inline with the executing-plans skill; bounded, disjoint implementation
units may run concurrently. Preserve unrelated dashboard files.

**Goal:** Interactive SSH, SFTP/editor connections and isolated guest internet.

**Architecture:** Existing HTTPS authentication carries an upgraded byte stream
to a fixed guest vsock service. OpenSSH owns terminal and file-transfer semantics.
Opt-in runtime networking owns per-box NICs and egress isolation independently.

**Tech stack:** Rust/Tokio, Axum/reqwest HTTP upgrades, OpenSSH, Firecracker,
Linux network namespaces/TAP and nftables.

## Constraints

- Keep verified TLS, strict host keys, workspace ownership and initialization gates.
- No secret or terminal payload journaling; no arbitrary host paths/ports from clients.
- Preserve existing boxes and networkless templates. No live server deployment.
- Root/network tests must not change this runner's shared routes or firewall.

## Implementation units

- [x] Guest SSH: extend `box-protocol`, `box-guest` and Ubuntu image configuration.
  Fixed vsock port, framed key admission, fresh host keys and bounded OpenSSH children.
  Tests reject malformed keys and pre-initialization access; real SSH/SFTP test
  verifies binary content, exit status and host key behavior.
- [x] Runtime egress: extend `box-runtime` device provisioning and restoration;
  add a reviewed operator network helper/policy. Test address allocation, hostile
  destinations, cleanup and networkless compatibility. Privileged tests use a
  disposable enclosing namespace, never shared host networking.
- [x] Runtime SSH: expose fixed-purpose host-key and stream methods, bound setup
  deadlines, enforce running state and release locks after connecting.
- [x] Host and gateway: add authenticated host-key and HTTP upgrade endpoints,
  independent session quotas, workspace checks and no arbitrary destination input.
  Test missing/invalid credentials, unmanaged IDs, connection cleanup and streams.
- [x] Laptop: add `ssh`, `cp`, `ssh-config` and internal stdio transport commands.
  Store generated keys privately, use verified host keys and argument arrays,
  preserve SSH/scp exit status and keep diagnostics off tunnel stdout.
- [x] Integrate: run workspace tests, format/Clippy, image/installer tests and
  disposable integration tests; document installation and compatibility limits.

Use failing focused tests before each behavioral implementation and rerun them
after changes. Final checks: `cargo test --workspace --all-features --locked`,
`cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`,
`cargo fmt --all -- --check`, and affected Python/shell checks. The owner
subsequently authorized commit, push and release after each iteration; follow
`AGENTS.md` for delivery, without deploying to the running installation.

## Local validation, 2026-10-02

- 102 ordinary Rust tests pass (12 privileged checks ignored by default).
  Clippy with warnings denied, formatting and ShellCheck pass.
- All 33 bootstrap/installer/package Python tests pass; fresh Ubuntu image builds.
- Real jailed KVM run passes HTTPS SSH, exit status 37, strict host-key rejection,
  a byte-exact 200123-byte SFTP roundtrip, PTY resize/Ctrl-C, editor loopback
  forwarding and non-loopback rejection. Host keys persist across stop/start
  and differ across clones. Simulated public HTTP egress works after restart
  and from a second network slot; host-process crash/no-replay still passes.
- Disposable packet tests pass initialization gating, public IPv4 replies,
  host/LAN/metadata/peer denial, unsolicited ingress, spoofing and cleanup.
- No live installation was replaced; no shared routes/firewall were changed.
  At initial local completion, the public bootstrap still pinned v0.1.1;
  release delivery is recorded below. Deployment and migration remain separate.
- Remaining validation limits: native macOS interactive sessions, actual VS Code
  UI integration, fresh-host provisioning/reboot and firewall-manager coexistence.
  Windows SSH/SFTP commands are explicitly unsupported; management still works.

## Release delivery

The owner authorized commit, push and release after every iteration. Version
`v0.2.0` is published as a private pre-release. The tag workflow passed all four
native client jobs (Linux, Apple Silicon, Intel Mac and Windows), server checks,
production builds, the warm Ubuntu image build, packaging and checksum validation.

All five downloaded packages match their SHA-256 files. The updated bootstrap
passed 17 tests and ShellCheck, plus real authenticated Linux-client installation
and server-download/archive validation; server setup was cancelled before any
system changes. Only the bootstrap script and its public README were published
to `Jabbslad/boxd-install`, with verified v0.2.0 asset IDs and digests. No live
server or existing VM was upgraded.

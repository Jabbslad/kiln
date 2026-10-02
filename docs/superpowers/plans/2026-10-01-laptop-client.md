# Laptop client and single-host service implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Manage real persistent microVMs from a laptop through an authenticated HTTPS API, without installing Linux/KVM dependencies on the laptop.

**Architecture:** `boxctl` → unprivileged HTTPS gateway → restricted Unix socket → host service → existing runtime. A private SQLite journal tracks at-most-once request dispatch and bounded results; the runtime owns VM records and capacity. Create uses an operator-controlled template catalog and a preallocated box ID.

**Tech Stack:** Rust 1.95.0 / edition 2024, Tokio, Serde, Axum, rustls, reqwest, rusqlite/SQLite.

## Global constraints

- Trusted development workloads only; one administrator/workspace.
- Keep Linux runtime dependencies out of the laptop crate.
- Mandatory TLS verification, no credential-bearing redirects or automatic mutation retries.
- Never replay interrupted exec; preserve unknown outcomes and request IDs.
- No client-selected host paths; do not expose raw runtime records/errors.
- Runtime quota: 8 boxes / 16384 MiB; service in-flight work is bounded separately.
- No persistent service installation, public port, host networking changes, commit or push as part of implementation.
- Leave `web/` and the separate dashboard plan untouched.

## Task 1: portable API contract and deterministic runtime allocation

Files: new `crates/box-api/{Cargo.toml,src/lib.rs}`; update workspace manifest and `crates/box-runtime/src/runtime/checkpoint.rs`.

Interface: `Action`, `Submit { id, action }`, `Operation { id, box_id, state, result, error }`, `BoxView`, `TemplateView`, `Outcome`. IDs are lowercase 32-character hexadecimal strings; `Action::validate()` rejects paths, malformed IDs, empty commands and out-of-range deadlines. Runtime adds `clone_template_with_id(snapshot_id, name, box_id)` with an exclusive, non-reusable directory reservation.

- [x] Add contract tests that reject `../outside`, unknown JSON fields and deadlines 0/3600001 while accepting 1/3600000.
- [x] Add runtime tests proving invalid/reused box IDs fail before reading a template and do not alter existing files.
- [x] Run `cargo test -p box-api -p box-runtime --lib`; observe missing behavior, implement and rerun.

## Task 2: durable host operations and Unix HTTP service

Files: new `crates/box-server/{Cargo.toml,src/lib.rs,src/journal.rs,src/host.rs,src/bin/boxd-host.rs,tests/host.rs}`.

Interface: authenticated gateway forwards `GET /v1/templates`, `GET /v1/boxes`, `GET /v1/boxes/{id}`, `GET /v1/operations/{id}`, `POST /v1/operations`. SQLite acceptance stores a SHA-256 fingerprint and assigned box ID before starting a detached, bounded operation. Identical retries return the existing operation, including after delete; conflicting payloads return 409. Restart marks unfinished requests unknown; runtime inspect performs VM reconciliation. Only boxes associated with service creates are exposed.

- [x] Test same-key retry, changed-body conflict, restart unknown, managed-ID ownership and no secret argv/env in SQLite.
- [x] Run `cargo test -p box-server`; implement the journal and real-runtime adapter, then rerun.
- [x] Add HTTP tests against temporary state: catalog paths never appear in public JSON, invalid/unowned IDs fail, body size is bounded, concurrent same-ID submissions dispatch once.
- [x] Add the host executable with private-state/exclusive-process locking, explicit development opt-in and restricted socket creation; test startup refusal for unsafe paths/policies.

## Task 3: TLS gateway and laptop binary

Files: new `crates/box-server/src/{gateway.rs,bin/boxd-api.rs}`, `crates/box-client/{Cargo.toml,src/lib.rs,src/main.rs,tests/cli.rs}`.

Interface: `boxctl profile add NAME --url https://HOST:8443 --token-file PATH [--ca-file PATH]`; `templates`, `list`, `inspect ID`, `create --template ALIAS --name NAME`, lifecycle commands, `exec ID -- ARGV`, `operation ID`. Mutations accept `--request-id` and `--no-wait`; ordinary mutation calls poll the durable operation and exec propagates stdout/stderr/status.

- [x] Write TLS integration tests using temporary self-signed certificates: reject missing/wrong tokens and untrusted certificates; accept explicit trust; refuse cleartext URLs and redirects.
- [x] Run failing tests; implement gateway with constant-time token hash comparison and bounded local proxying, then client profiles, commands and result rendering.
- [x] Exercise the actual client binary over TLS: byte-exact output, exit 37, timeout 124 and JSON output. Separately exercise all three release binaries against real KVM. Request IDs are printed before submission and included in disconnection diagnostics.
- [x] Attempt a non-Linux client target and report the limitation: aarch64-apple-darwin checking stops in ring's C compilation because this Linux runner lacks Apple SDK headers. No native macOS/Windows validation is claimed.

## Task 4: operator delivery and end-to-end validation

Files: new `docs/remote-client.md`, `deploy/boxd-{api,host}.service`, `deploy/host.example.json`; update `README.md`. New opt-in `crates/box-server/tests/lifecycle.rs`.

- [x] Document building three binaries, server template preparation, private tokens and certificates, systemd configuration, profiles and recovery. Commands are operator instructions, not deployment performed by this task.
- [x] Run disposable local HTTPS → Unix → KVM lifecycle: development fixture and isolated Ubuntu/warm/overlay both pass, including same-request retry, disconnect, file preservation, host-service crash/unknown/no-replay and delete tombstones.
- [x] Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`, and `cargo test --workspace --all-features --locked`: 82 ordinary tests pass; 12 environment-dependent cases remain ignored in the ordinary run, with the new KVM test executed separately in both modes.
- [x] Review diff, verify unrelated work unchanged, record actual checks and remaining limitations. Changes remain local, uncommitted and undeployed.

## Delivery evidence and limitations

Local evidence is under `.amp/in/artifacts/remote-client-*`. Release smoke testing
initially failed because the disposable OpenSSL fixture used a CA certificate as
the server leaf. TLS correctly rejected it (`CaUsedAsEndEntity`); correcting that
fixture to `CA:FALSE` made the release CLI → gateway → host → KVM lifecycle pass.
Certificate validation was not weakened.

The example systemd units are not installed. `systemd-analyze verify` reported
only their missing `/usr/local/libexec` executables; deployment behavior remains
unverified. No service account, firewall rule or persistent listener was added.
Linux release binaries were built; signed distribution and native laptop-OS
validation remain separate delivery steps. The API has one administrator, no guest
network/SSH/PTY, and a 10,000-operation journal cap pending retention work.

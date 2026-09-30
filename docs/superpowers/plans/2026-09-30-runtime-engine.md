# Single-host Runtime Engine Implementation Plan

> **For agentic workers:** User authorized implementation. Continue in the checkout containing the work. The original detailed checklist below remains the target, not a claim that every hardening item shipped. Consult the delivery status here and [operator guide](../../runtime.md) before continuing.

**Goal:** Run real Linux microVMs from a CLI, preserve and restore their state, clone controlled templates, and measure request-to-command launch latency.

**Architecture:** A Rust runtime owns private per-instance state, Firecracker processes, disk files, and full snapshots. A small Rust guest agent provides readiness and bounded command execution over vsock, sharing protocol types without depending on the host runtime. Start with an explicit trusted development profile and validate the jailed profile separately; do not introduce a public API or PostgreSQL in this milestone.

**Tech Stack:** Rust 1.95.0 (stable), edition 2024, Cargo workspace, Tokio, Serde, Clap, thiserror, maintained Linux/vsock crates, Linux x86_64, KVM, Firecracker/jailer 1.17.0, cgroup v2, raw ext4 guest disks, JSON manifests, Cargo unit and real-KVM integration tests.

## Delivery status

- [x] Three-crate Rust workspace, pinned stable toolchain/lockfile, preflight, image verification, Firecracker Unix API transport, and PID-safe process ownership.
- [x] Rootless trusted fixture build and bounded guest protocol/exec over real vsock.
- [x] Persistent CLI lifecycle, per-box serialization, quota reservations, forced stop, and overlapping-start protection.
- [x] Coordinated full memory/disk checkpoints, same-box restore, reference-protected snapshot deletion, pre-initialization templates, and independent clone identities/disks.
- [x] Release benchmarks: 30 sequential samples per mode and 16 concurrency-four samples per mode, all successful; first-command timing includes checksum/disk work.
- [x] Real-KVM process-crash tests around spawn/PID recording and checkpoint pause/publication, plus killed-process memory restoration and corruption rejection.
- [ ] Complete the broader storage/ENOSPC/power-loss/restore/cancellation failure matrix; add operation-wide cancellation, bounded blocking work, orphan generation cleanup, and disk/log budgets. Current implementation does not claim these are complete.
- [ ] Task 8: implement jailed/cgroup-enforced profile and Ubuntu image; obtain explicit approval before privileged host setup and validation.

**Version correction:** Real testing reproduced Firecracker 1.16.0's vsock pause/resume bug. The implementation pins the fixed 1.17.0 release using a checksum-verified local download. System-installed 1.16.0 binaries were not replaced. Historical 1.16 API examples below remain design context; tested operator commands use 1.17.0.

**Implementation shape:** Small owners remain single Rust source files rather than the initially proposed directory trees. CLI timing uses `--timeout-ms`, output is JSON-only, and actual integration scenarios are consolidated in `tests/lifecycle.rs`. See that file for the executed assertions rather than inferring completion from the original checklist.

## Global constraints

- No existing host configuration, firewall, service, logical volume, or filesystem is changed by this design approval. Privileged installation and storage provisioning require explicit approval before execution.
- Use full snapshots and Firecracker's file-backed, demand-paged memory restoration. Do not implement a userfaultfd pager or differential memory snapshot chain in milestone one.
- Keep Firecracker API sockets private. The platform, not users, supplies VMM paths, kernel command lines, resource limits, and jailer arguments.
- No sub-10ms end-to-end SLA is assumed. Establish a reproducible baseline before choosing a latency target.
- Milestone one needs vsock only. Guest networking, SSH, preview routes, a public API, billing, and a UI are excluded so the runtime can be tested without changing host routing or exposing services.

Specification: [platform design](../specs/2026-09-30-boxd-platform-design.md).

## File ownership

| Path | Responsibility |
| --- | --- |
| `crates/box-runtime/src/bin/box.rs` | CLI entry point |
| `crates/box-runtime/src/cli/` | CLI parsing, human/JSON output, exit codes |
| `crates/box-runtime/src/host.rs` | Read-only preflight and compatibility fingerprint |
| `crates/box-runtime/src/image.rs` | Image manifest loading and validation |
| `crates/box-runtime/src/firecracker/` | Typed Unix-socket API client and VMM launch/identity |
| `crates/box-runtime/src/guest.rs` | Host-side guest client |
| `crates/box-runtime/src/storage/` | Independent disk copying and durable snapshot publication |
| `crates/box-runtime/src/runtime/` | Inventory, lifecycle serialization, recovery, templates |
| `crates/box-runtime/src/benchmark.rs` | Sample aggregation and benchmark output |
| `crates/box-protocol/src/lib.rs` | Shared versioned guest message types |
| `crates/box-guest/src/` | Guest service and bounded command execution |
| `images/` | Pinned input manifests, fixture/Ubuntu build scripts |
| `crates/box-runtime/tests/` | CLI and opt-in real-KVM integration checks |
| `docs/runtime.md` | Operator commands, failure recovery, limitations |

Use three crates: `box-runtime` (library and `box` binary), `box-protocol` (shared serializable types), and `box-guest` (guest binary). Add each crate only when its task needs it. Keep unit tests in `#[cfg(test)]` modules and end-to-end checks in the runtime crate's `tests/` directory. Set `publish = false`, workspace resolver 3, edition 2024, and `rust-version = "1.95"`; commit Cargo.lock and select Rust 1.95.0 in `rust-toolchain.toml`. Do not change the runner's default toolchain.

Use Tokio deadlines and `tokio_util::sync::CancellationToken` for cancellation. Dropping a future does not roll back an operation, stop a child process, or cancel an already-running `spawn_blocking` task. Own and join cleanup work explicitly, check cancellation between copy chunks, and recover durable intent after abrupt process death. Prefer safe Linux wrappers such as rustix; isolate any required unsafe syscall wrapper and document its safety contract.

Shared host-library interfaces return `Result<T, Error>` with module-specific typed errors. Guest wire failures use serializable stable codes, not host error internals. Lifecycle entry points accept an operation control value containing a deadline and cancellation token. Keep host-only process/storage/HTTP dependencies out of `box-protocol` and `box-guest`.

Define `#[ignore = "requires real KVM and prepared images"]` on VM integration tests, and run them explicitly with `--ignored --test-threads=1`. Once explicitly requested, missing prerequisites fail the test rather than returning an apparent pass. Normal `cargo test --workspace --locked` must not launch VMs. Concurrency tests exercise simultaneous operations inside one test, independent of the harness's test-thread limit.

## CLI contract

All runtime commands accept `--state-dir PATH`. New state directories are private to the current operator. Generated resource IDs, not user-controlled relative paths, identify artifacts. JSON output is versioned (`schema_version: 1`); diagnostics go to stderr. Exit 0 means success, 1 means operation/transport failure, and 2 means invalid invocation. Guest commands return their exit code where representable, and JSON distinguishes that result from a transport error.

```text
box doctor --json
box create --profile development --allow-unsafe-development --image MANIFEST --name NAME --json
box list --json
box inspect ID --json
box exec ID --timeout 10s --json -- /bin/sh -c 'printf hello'
box pause ID
box resume ID
box stop ID
box start ID
box delete ID
box checkpoint save ID --json
box checkpoint restore ID CHECKPOINT --acknowledge-external-state-replay
box template build --image MANIFEST --profile development --allow-unsafe-development --json
box clone TEMPLATE --name NAME --profile development --allow-unsafe-development --json
box benchmark --image MANIFEST --template TEMPLATE --samples 30 --concurrency 1 --json
```

`create`, `clone`, `checkpoint save`, and `template build` return an `id`. `exec` returns `stdout`, `stderr`, `exit_code`, and `truncated` when transport succeeds. A terminal error includes a stable code and contextual message, never secrets. A stopped box retains the profile and image necessary for `start`.

## Task 1: Preflight and immutable input validation

**Files:** Create `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `crates/box-runtime/Cargo.toml`, `crates/box-runtime/src/lib.rs`, `src/bin/box.rs`, `src/cli/mod.rs`, `src/host.rs`, `src/image.rs`, `tests/cli.rs` within that crate, `.gitignore`, and `docs/runtime.md`.

**Consumes:** OS observations only; no privileged mutations.

**Produces:** `host::check` returning a typed `Report` and `image::load(path: &Path) -> Result<Manifest, Error>`; the report records KVM access, architecture, kernel, cgroup mode, binary versions/hashes, and storage capabilities. A manifest has `schema_version`, `architecture`, `kernel_path`, `kernel_sha256`, `rootfs_path`, `rootfs_sha256`, and `agent_protocol_version`.

- [ ] Write table tests for absent KVM, permission denied, mismatched VMM/jailer versions, unsupported architecture, and valid observations. Use injected filesystem/command observations so tests do not depend on the test host. Test a corrupted rootfs with an otherwise correct manifest: it must fail checksum validation before launch.
- [ ] Run `cargo test -p box-runtime --locked`; confirm failures come from missing behavior, not malformed tests. Generate/update Cargo.lock when dependencies are intentionally added, then use `--locked` for verification.
- [ ] Implement read-only checks and strict manifest decoding. Resolve image paths relative to the manifest directory. Restrict supported format/version combinations explicitly. Keep development and isolated readiness separate in the report.
- [ ] Add the `doctor` command and run the following smoke check. `doctor` must not claim the isolated profile is ready merely because `/dev/kvm` is writable.

```bash
cargo run --locked -p box-runtime --bin box -- doctor --json
cargo test --locked -p box-runtime
```

- [ ] Record actual host limitations in `docs/runtime.md`; commit as `feat: add runtime preflight and image validation`.

## Task 2: Firecracker transport and owned-process management

**Files:** Create `crates/box-runtime/src/firecracker/mod.rs`, `client.rs`, and `process.rs` in that module, with unit tests alongside each implementation; extend `src/lib.rs` and the crate manifest.

**Consumes:** Validated host report and image paths.

**Produces:** `Client::new(socket: PathBuf) -> Client`; async typed methods for machine configuration, boot source, root drive, vsock, instance start, pause/resume, full snapshot creation/loading, and instance status. Requests use operation deadlines and cancellation. A launched process handle records PID, Linux start ticks, executable identity, and API socket.

- [ ] Write a fake HTTP server on a real temporary Unix socket. Assert method, path, and JSON fields for pause, resume, snapshot save, and file-backed snapshot restore. Return HTTP 400 with a Firecracker fault body and prove it remains a failure. Hold a request open and assert context timeout cancels it.
- [ ] Run `cargo test --locked -p box-runtime firecracker:: -- --nocapture` and confirm the red phase.
- [ ] Implement the Unix HTTP client with bounded response reads and explicit non-2xx handling. Test the wire protocol below; do not use defaults that enable differential snapshots.

```json
{"snapshot_type":"Full","snapshot_path":"state.snap","mem_file_path":"memory.snap"}
```

```json
{"snapshot_path":"state.snap","mem_backend":{"backend_path":"memory.snap","backend_type":"File"},"track_dirty_pages":false,"resume_vm":false}
```

- [ ] Implement process launch without binding VMM lifetime to the short-lived CLI context. Wait for real API readiness with a deadline. Isolate each working directory so snapshot-recorded relative paths remain valid for restore. Keep stdout/stderr logs bounded or rotated.
- [ ] Test PID reuse by supplying a mismatched process start identity; adoption/termination must refuse it. Exercise launch failure after process creation and verify only the owned process and partial socket are cleaned up.
- [ ] Run `cargo test --locked -p box-runtime firecracker::` and `cargo clippy --locked -p box-runtime --all-targets -- -D warnings`; commit as `feat: manage Firecracker APIs and owned processes`.

## Task 3: Trusted image fixture and guest command protocol

**Files:** Create `crates/box-protocol/Cargo.toml`, `crates/box-protocol/src/lib.rs`, `crates/box-guest/Cargo.toml`, `crates/box-guest/src/main.rs`, `server.rs` in that crate, `crates/box-runtime/src/guest.rs`, `images/build-fixture.sh`, `images/README.md`, and `crates/box-runtime/tests/guest.rs`. Update workspace membership, module declarations, and Cargo.lock.

**Consumes:** Firecracker launch/client and image manifest format.

**Produces:** A versioned guest protocol with `hello`, `initialize`, and `exec`; async host client methods `hello`, `initialize`, and `exec` with deadlines and cancellation. `ExecRequest` carries argv, environment, working directory, and timeout. `ExecResult` carries stdout, stderr, exit code, and truncation.

- [ ] Write guest-server tests over local transport for asymmetric stdout/stderr, exit 7, an argument containing spaces, timeout, output flooding, and disconnect. A test command that increments a counter must run once, even when the client loses its response; the client must not auto-retry.
- [ ] Run `cargo test --locked -p box-protocol -p box-guest` and confirm missing behavior fails.
- [ ] Implement framed, size-limited JSON messages with bounded concurrent requests and output. Start commands with argv, not an interpolated shell string. On timeout, terminate the owned command process group and reap it; join output-draining tasks. Agent listeners survive vsock transport reset. Cancellation during an exec request must still complete process cleanup.
- [ ] Use a maintained Tokio-compatible vsock crate and pin its resolved version. Verify the Firecracker host Unix-socket `CONNECT` handshake and guest listener behavior against 1.16.0 documentation before coding transport assumptions.
- [ ] Prepare the Rust 1.95.0 `x86_64-unknown-linux-musl` target and required linker tooling. Build with `cargo build --locked --release -p box-guest --target x86_64-unknown-linux-musl`. Inspect the ELF to verify it has no dynamic interpreter; a host glibc-linked binary is not an acceptable substitute for the minimal guest fixture.
- [ ] Build a fixture with a pinned kernel and small raw filesystem containing the guest binary and shell. The script writes only a caller-selected output directory, verifies input hashes, and uses offline filesystem population without mounting host disks. Keep downloaded images and build outputs out of Git. Record real source URLs and checksums in the manifest; do not insert invented hashes.
- [ ] Cold boot the fixture with no network device and send the following command over vsock. The test must observe exact separate outputs and exit code, not merely a successful connection.

```sh
/bin/sh -c 'printf fixture-out; printf fixture-err >&2; exit 7'
```

- [ ] Run `cargo test --locked -p box-protocol -p box-guest` and `cargo test --locked -p box-runtime --test guest -- --ignored --test-threads=1 --nocapture`. If the real test cannot run, record the blocker rather than counting a skip as VM verification. Commit as `feat: execute bounded commands inside a real guest`.

## Task 4: Durable box lifecycle and CLI

**Files:** Create `crates/box-runtime/src/runtime/mod.rs`, `inventory.rs`, and `reconcile.rs` in that module; create `src/storage/mod.rs` and `copy.rs`; extend `src/cli/` and `src/lib.rs`; create `crates/box-runtime/tests/lifecycle.rs`. Keep module unit tests beside the implementations.

**Consumes:** Image manifests, guest client, Firecracker client/process handle.

**Produces:** `Runtime::open(state_dir: PathBuf) -> Result<Runtime, Error>` and lifecycle methods `create`, `list`, `inspect`, `exec`, `pause`, `resume`, `stop`, `start`, and `delete`. Asynchronous operations take explicit operation control for deadline/cancellation. `CreateOptions` contains image, name, profile, explicit development opt-in, vCPU count, and RAM MiB. Default fixture size is 1 vCPU and 256 MiB; configured host limits cap allocations and concurrent operations.

- [ ] Write transition tests for create failure, two concurrent starts, exec while paused, delete during checkpoint preparation, and stale PID reuse. Test interrupted records both before and after VMM launch. Invalid transitions must fail without advancing recorded state.
- [ ] Run `cargo test --locked -p box-runtime` and verify failures.
- [ ] Test and implement `storage::copy_disk` accepting source/destination `&Path` and operation control, returning `Result<CopyMethod, Error>`, with an independent exclusive-create destination, cancellation-aware byte copying, and file synchronization. Initially return `CopyMethod::Copy`; Task 5 adds `Reflink`. Change one byte in the copy and assert the source remains unchanged. Reject pre-existing and symlink destinations.
- [ ] Implement private atomic inventory records, cross-process locks, per-box operation intent, and resource reservations. Persist artifact/operation identity before spawn. Reconciliation scans deterministic operation directories and validates actual process identity to recover the crash window between spawning and recording a PID.
- [ ] Implement development-only profile opt-in and no-NIC launch. Refuse an isolated launch that cannot use jailer. Enforce RAM/CPU, copy-space, and operation limits even in development. Make stop preserve disk; distinguish cooperative guest shutdown from a forced VMM stop in output.
- [ ] Wire the CLI contract. Test stdout JSON independently of diagnostic stderr and reject path traversal, symlink escapes, malformed IDs, and unrecognized versions before touching instance data.
- [ ] Run a real lifecycle test: create, write `persistent-one`, stop/start, read exactly `persistent-one`, pause, verify execution is refused, resume, execute successfully, delete. Start a second CLI process after creation to prove VM lifetime is independent of the creator.
- [ ] Run `cargo test --workspace --locked` and `cargo test --locked -p box-runtime --test lifecycle -- --ignored --test-threads=1 --nocapture`; commit as `feat: add durable single-host box lifecycle`.

## Task 5: Consistent checkpoints and same-box restore

**Files:** Extend `crates/box-runtime/src/storage/copy.rs`; create `src/storage/publish.rs`, `src/runtime/checkpoint.rs`, and `tests/checkpoint.rs` in that crate; extend CLI and module declarations, with unit tests beside each implementation.

**Consumes:** Serialized box lifecycle and image/host fingerprints.

**Produces:** Runtime methods `checkpoint` and `restore`, accepting validated box/checkpoint IDs and operation control, returning `Result<CheckpointRecord, Error>` and `Result<(), Error>` respectively. Records contain immutable artifact names/checksums, compatibility metadata, disk-copy method, source ID, and completion state.

- [ ] Write fault-injection tests for disk-copy error, full disk, memory-artifact sync failure, manifest publication failure, source-resume failure, and interrupted restore. No failure may expose an incomplete checkpoint as ready. A source paused before capture must stay paused afterward.
- [ ] Run `cargo test --locked -p box-runtime --lib` and confirm failures. Include cancellation while paused: cleanup must explicitly await the source-resume attempt, because Rust destructors cannot await it.
- [ ] Implement explicit reflink attempt and safe byte-copy fallback. Fall back only for unsupported reflink/cross-device errors, not permission, corruption, or capacity errors. Create destination files exclusively, bound copy work by context and space limits, synchronize artifacts, and clean owned partial output.
- [ ] Implement capture ordering from the specification. For 1.16.0, explicitly sync disk backing files after snapshot creation while paused. Publish the complete manifest last; sync the containing directory. Resume the source according to its prior state and report a resume error distinctly.
- [ ] Implement restore with compatibility checks before stopping the source. Stage a private writable disk generation, verify old VMM termination before replacement, and retain the original generation until readiness. Require the CLI acknowledgement for replay of workload state. Keep mapped snapshot memory immutable and referenced.
- [ ] Real test: keep a process with a unique in-memory marker alive, checkpoint it, change its disk marker and process counter, restore, and assert both memory and disk return to their captured values. Verify the marker is not regenerated as it would be on cold boot. Test that a corrupted manifest is rejected before disturbing a running box.
- [ ] Run `cargo test --workspace --locked` and `cargo test --locked -p box-runtime --test checkpoint -- --ignored --test-threads=1 --nocapture`; commit as `feat: capture and restore coordinated VM checkpoints`.

## Task 6: Controlled template initialization and independent clones

**Files:** Create `crates/box-runtime/src/runtime/template.rs` with unit tests and `crates/box-runtime/tests/clone.rs`; extend guest initialization, CLI, and module declarations.

**Consumes:** Checkpoint/storage primitives and guest initialization barrier.

**Produces:** Runtime methods `build_template` and `clone_template`, accepting options and operation control, returning `Result<TemplateRecord, Error>` and `Result<BoxRecord, Error>`. Use `clone_template` to distinguish the VM operation from Rust's `Clone` trait. Only builder-produced records are cloneable. Template records link immutable artifacts and versioned initialization requirements.

- [ ] Test refusal to clone an ordinary workload checkpoint. Test two different host-supplied identities and initialization nonces after restoring the same template. Before initialization, exec must fail; protocol version mismatch must never open the barrier.
- [ ] Run `cargo test --workspace --locked` and confirm failures.
- [ ] Implement a builder that boots only a trusted fixture into the pre-workload barrier, captures it, and stops the builder VM. Templates contain no user secrets or running customer commands. On clone, use a private disk and private relative-path vsock endpoint; inject fresh identity and entropy through initialization. Start customer programs only afterward.
- [ ] Track durable references so removing a box cannot remove a memory file backing another live clone. Add deletion tests with two clones and an interrupted create. Never share a writable disk inode.
- [ ] Real test: create clones A and B, write `alpha-17` into A and `beta-93` into B at the same guest path, read both, delete A, then read `beta-93` from B. Assert different guest identities and different initialization nonces. Compare artifact metadata to prove both started from the template rather than silently cold-booting.
- [ ] Run `cargo test --workspace --locked` and `cargo test --locked -p box-runtime --test clone -- --ignored --test-threads=1 --nocapture`; commit as `feat: launch independent boxes from controlled templates`.

## Task 7: Launch measurements and fault-recovery evidence

**Files:** Create `crates/box-runtime/src/benchmark.rs` with unit tests, `src/cli/benchmark.rs`, and `tests/recovery.rs` in that crate; extend module declarations and `docs/runtime.md`.

**Consumes:** Real create/clone/exec APIs, lifecycle timing events, compatibility metadata.

**Produces:** Versioned JSON benchmark results with every sample, successes/failures, phase timings, sample count, concurrency, copy method, versions, and cache condition. Percentiles use nearest-rank over successful observations and always accompany the failure count.

- [ ] Write percentile tests with durations 1, 2, 3, 4, and 100 ms: p50 must be 3 ms and p95 100 ms. Include a failed sixth operation, assert the failure count is 1, and avoid inventing a latency for it. Zero successes must produce no latency percentile, not zero latency.
- [ ] Run `cargo test --locked -p box-runtime benchmark:: -- --nocapture` and confirm failures.
- [ ] Implement monotonic measurement using `std::time::Instant` around the entire create/clone-through-successful-exec sequence. Include disk preparation, validation, process startup, and initialization in end-to-end time. Cache verified immutable manifests only under a trusted ownership model; do not omit necessary integrity work from reported timings. Build the CLI with `cargo build --locked --release -p box-runtime --bin box` and use release host/guest binaries for published latency results.
- [ ] Run small bounded samples before larger benchmarks. Use sequential and concurrency-4 batches within configured RAM/disk limits; clean only benchmark-owned boxes. Report cold boot, cached restore, and uncontrolled-cache restore separately. Never write `/proc/sys/vm/drop_caches` on this runner.
- [ ] Add recovery integration tests that terminate the manager at persisted failpoints before/after spawn and before/after snapshot publication. A new manager must adopt the exact owned process or report recoverable failure, without duplicate VMMs or disk writers.
- [ ] Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`, and `cargo test --release --locked -p box-runtime --tests -- --ignored --test-threads=1 --nocapture`. Normal Rust tests do not replace concurrency testing: include overlapping lifecycle operations and cancellation at durable failpoints. Save benchmark artifacts under `.amp/in/artifacts/` only after adding `/.amp/in/` to the repository-local Git exclude file. Publish measured results in the completion response; do not commit machine-specific benchmark outputs by default.
- [ ] Commit as `test: benchmark real launch latency and lifecycle recovery`.

## Task 8: Isolated profile and Ubuntu development image

**Files:** Create `crates/box-runtime/src/firecracker/jailer.rs` with unit tests and `images/build-ubuntu.sh`; extend image manifests, module declarations, integration tests, and operator documentation.

**Consumes:** Proven development lifecycle, prepared templates, image tooling.

**Produces:** Explicit isolated-profile operator invocation, a versioned Ubuntu image build, and documented host setup. The long-running privileged host service belongs to milestone two, not this CLI milestone.

- [ ] Test generated jailer arguments for cgroup v2, UID/GID, resource limits, and private artifact paths. Assert no shell invocation or caller-supplied arbitrary host path can escape validated configuration. Simulate missing privilege and verify it fails without launching an unjailed VM.
- [ ] Run `cargo test --locked -p box-runtime firecracker::jailer:: -- --nocapture` and confirm failures.
- [ ] Implement jailed launch using the matching installed binaries and explicit cgroup v2. Adapt artifact paths and vsock ownership to the jail rather than weakening directory permissions. Keep the management socket private and guest NICs absent in this milestone.
- [ ] Prepare an Ubuntu image build with pinned verified upstream inputs, systemd guest-agent service, per-instance identity provisioning, and the required kernel configuration. Keep any root-required image preparation in an explicit documented script, not an automatic build/test side effect.
- [ ] Run argument/configuration unit tests without root. Describe the exact host installation and privileged validation commands for approval; do not execute them until authorized. This is a real remaining validation gate if permission is not granted.
- [ ] After approval, run the same real lifecycle tests in isolated mode, inspect the VMM's UID, cgroup membership/limits, jail, and effective seccomp state, and verify the guest cannot access host management sockets. Test `systemctl` and Docker inside the Ubuntu guest if Docker support is advertised.
- [ ] Commit as `feat: add explicit jailed runtime and Ubuntu image support`. Report development versus isolated results separately and leave any unverified claims out of documentation.

## Completion and continuation

Milestone one is complete only when actual cold boot, exec, disk persistence, memory restore, clone independence, and manager recovery checks pass, with performance results and environment constraints recorded. If privileged isolated validation remains unapproved, deliver the verified development runtime and state that the isolated profile is unverified; do not call it production-ready.

Use local commits at the testable boundaries above. Do not push, deploy, provision storage, or install a host service without authorization. Continue implementation here by default rather than handing this coherent runtime implementation to another thread.

Milestone two receives a separate plan after the runtime interfaces and measured constraints are known. It includes PostgreSQL operations/idempotency, authenticated host service, networking, CLI/SDK API integration, and the dashboard; none is represented by a mock in milestone one.

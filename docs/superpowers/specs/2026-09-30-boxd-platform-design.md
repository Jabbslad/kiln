# Self-hosted box platform

Date: 2026-09-30
Status: Architecture and Rust selection approved in conversation; written specification pending review.

## Outcome

Build a working, self-hosted platform for persistent Linux computers, then extend it to a hosted multi-tenant service. Real VM operations and measured time to useful work take priority over the dashboard. The reference product is boxd.sh, not just its marketing website.

The first milestone is a single-host runtime with a CLI and real KVM integration tests. The second milestone adds the service API, PostgreSQL-backed control plane, network access, and dashboard. The third adds fleet scheduling and advanced lifecycle operations. Each milestone has its own implementation plan; this specification fixes their boundaries rather than claiming they can all ship together.

## Established environment

The repository was empty at design time. The development runner is Linux x86_64 with accessible `/dev/kvm`, cgroup v2, stable Rust/Cargo 1.95.0, and matching Firecracker/jailer 1.16.0 binaries. Its default Rust toolchain is nightly, so the project must select its pinned stable toolchain explicitly. Its workspace filesystem is ext4, not a reflink-capable filesystem. These observations are development facts, not portable installation assumptions.

No existing host configuration, firewall, service, logical volume, or filesystem is changed by this design approval. Privileged installation and storage provisioning require explicit approval before execution.

## Runtime decisions

- Use Firecracker/KVM, not containers as the security boundary for customer code.
- Implement host orchestration, CLI, guest agent, and the later service backend in Rust. Use Rust 1.95.0, edition 2024, with a pinned stable toolchain and committed Cargo.lock. Rust gives explicit resource ownership and allocation control without a garbage collector; this is not an unmeasured claim of faster VM launches.
- Use Tokio for asynchronous process/socket orchestration, Serde for versioned wire formats, and maintained Linux/vsock crates rather than inventing ABI wrappers. Keep host-only dependencies out of the guest agent. Use typed errors, bounded tasks, explicit cancellation, and safe resource wrappers; Rust memory safety does not make external side effects or crash recovery transactional.
- Start on Linux x86_64 with cgroup v2 and an exact, matching Firecracker/jailer version pair. Validate 1.16.0 first because it is installed; record the actual binary hashes in compatibility metadata. Do not silently upgrade system binaries.
- Use full snapshots and Firecracker's file-backed, demand-paged memory restoration. Do not implement a userfaultfd pager or differential memory snapshot chain in milestone one.
- Build a minimal trusted guest fixture first, then a versioned Ubuntu 24.04 development image. Pin kernel, root filesystem inputs, and guest-agent version in an image manifest. The guest kernel must support KVM, virtio block/vsock, and VMGenID; x86 VMGenID requires Linux 5.18 or later. Docker support is a separate image acceptance test, not inferred from the Ubuntu label.
- Keep Firecracker API sockets private. The platform, not users, supplies VMM paths, kernel command lines, resource limits, and jailer arguments.

## Three lifecycle operations with different contracts

### Pause and resume

Pause freezes vCPUs while retaining the VMM process, RAM, and disks. Resume continues that same instance. Pausing is not hibernation and does not release resident-memory costs.

### Checkpoint and restore

A checkpoint contains one coordinated full memory image, VMM state, disk image, and compatibility manifest. A restore replaces the execution state of the same box; it is not an unrestricted clone operation.

The engine serializes checkpoint creation with other mutations on that box. It pauses the VM, creates its full snapshot, flushes backing disk files, copies the disks while the source remains paused, flushes all snapshot artifacts, and atomically publishes the checkpoint manifest. It resumes a previously running source after successful capture or recoverable capture failure. A failed resume is surfaced as a failure with the actual paused state, not reported as success.

With Firecracker 1.16.0, the operator must explicitly handle backing-file durability. An uncommitted directory is never a restore source. Publishing requires file and parent-directory synchronization, not just a rename. A failed or interrupted restore cannot leave two writers to the same disk. Retain the previous disk generation until the replacement is ready or the failure is recorded for recovery.

Captured memory includes application buffers; a coordinated memory-and-disk restore differs from a disk-only crash recovery. It does not undo external side effects such as payments, remote database writes, or sent messages. Replaying state can also reuse application tokens and private randomness. The CLI documents these limits and requires explicit acknowledgement for workload rollback. Existing TCP/vsock sessions are not guaranteed to survive; clients reconnect.

### Prepared-template clones

A template is captured after guest boot at an explicit agent barrier, before user workloads, user secrets, external sessions, and per-instance identity are initialized. Only the controlled template-builder path can publish a cloneable template. An arbitrary checkpoint cannot be relabelled as a safe template.

Each clone receives a private writable disk, a unique box ID, and a fresh host/guest session. The initialization barrier provisions instance identity, refreshes entropy, fixes wall-clock time where required, and starts customer workloads as fresh processes. Guest readiness is acknowledged only after initialization. VMGenID reseeds the kernel but does not repair arbitrary cloned userspace state.

Arbitrary live application forks remain experimental future work. They are not offered under the prepared-template safety guarantee.

## Storage and speed

Start with immutable template/checkpoint files and independent writable raw disk files. On supported filesystems, use explicit reflink copying; otherwise use a bounded, cancellation-aware sparse/full copy. Record the actual copy method and time. Never hard-link writable clone disks, and never describe byte-copy performance as copy-on-write performance.

Ext4 supports a correct baseline but will make large disk copies expensive. Evaluate a dedicated Btrfs storage pool before optimizing forks at scale. Btrfs is a deployment option requiring separate provisioning approval, not an implicit change to the development host. No thin-pool or custom remote block service is required for milestone one.

Snapshot memory backing files stay immutable and alive for every restored VM that maps them. Garbage collection follows durable references from instances and operations. It cannot unlink the last usable template artifacts merely because a template was removed from the user-facing list.

Initially, snapshots are local and host-specific. Compatibility checks cover architecture, CPU fingerprint/template, host kernel, VMM binary/version, guest kernel/image, and agent protocol. Conservative refusal is preferable to silently attempting an incompatible restore. Cross-host portability is not promised.

Fast launch comes from avoiding repeated boot and setup, caching immutable template pages locally, and avoiding disk copies where storage supports it. Later scheduling places workloads near compatible cached templates. A warm pool of physical hosts is separate from a pool of already allocated user boxes; capacity and cache costs remain visible.

## Milestone one: executable runtime, not a hosted service

Deliver a Cargo workspace with a runtime library, `box` CLI, `box-guest` agent, shared protocol types, image build instructions, and reproducible integration/benchmark commands. Build the guest agent for a compatible guest target; the minimal fixture uses a static musl build. Benchmark release binaries, not debug builds.

Commands cover host preflight, cold create, list/inspect, exec, pause/resume, stop/start, delete, checkpoint/restore, controlled template creation, and template clone. Stop preserves disk and loses RAM; checkpoint/restore preserves captured RAM; delete removes a box's writable state after stopping its process. Readiness means an actual successful guest-agent exchange, not an open Firecracker API socket.

Guest command execution uses structured argv, environment, working directory, a deadline, and bounded output. It reports stdout, stderr, exit code, truncation, and transport failure separately. A lost connection does not automatically retry a potentially side-effecting command. A vsock timeout is not proof that the command never ran.

Store private host inventory and per-operation intent in a locked local state directory. Use atomic, synchronized records and per-box mutation exclusion. This inventory is local runtime bookkeeping, not a second service-level database. Milestone two makes PostgreSQL authoritative for desired state while retaining host records for observed processes and recovery.

On restart, identify owned processes by more than PID: verify process start identity and expected executable/socket before attachment or termination. Inspect the actual VMM state and resolve interrupted operations. Do not create replacement VMs merely because the managing CLI exited. Do not delete unrecognized processes or paths.

Two explicit execution profiles:

1. **Development:** private, unjailed VMMs, no guest NIC, trusted fixtures only, and a prominent development-only warning. Explicit opt-in is required; no silent fallback from jailer failure.
2. **Isolated:** an explicitly privileged operator invocation launches jailer with cgroup v2, seccomp, controlled ownership and resource limits. Host setup and privileged validation run only after approval. Milestone two moves this responsibility into the host service. No claim of multi-tenant production readiness follows from a passing development test.

Milestone one needs vsock only. Guest networking, SSH, preview routes, a public API, billing, and a UI are excluded so the runtime can be tested without changing host routing or exposing services.

## Milestone two: single-host product

Use a Rust API and host service, PostgreSQL, a React/TypeScript dashboard, and a TypeScript SDK. Axum and SQLx are the default service-layer choices, to be validated when that milestone is planned. Start with one administrator, workspace-owned resources, and scoped API tokens. The API records intent; a reconciler performs operations and records observed results. Durable idempotency keys prevent duplicate boxes on retried creates. Resource reservation prevents concurrent requests from oversubscribing configured capacity.

The host service's privileged surface accepts validated resource operations, never arbitrary host commands or paths. It is reachable through a permission-restricted Unix socket on a single-host installation. A later remote-host transport must authenticate both ends and scope host authority.

Add per-box network isolation and host-enforced routing/egress policy. Block access to host management endpoints and other boxes by default. Internet egress is a separately configured capability. Enforce workspace ownership in every lookup and operation, including exec, snapshots, previews, and metrics. Treat resource IDs as identifiers, not authorization.

Provide CLI and browser terminals, SSH access, and authenticated HTTP previews. Terminate TLS at the gateway, keep sessions scoped to their box/workspace, support WebSockets, and make public previews an explicit opt-in. Never route a guessed hostname directly to an arbitrary private address.

The dashboard exposes boxes, templates, operation progress/errors, console/exec access, resource use, and measured launch timings. It shows only backend state and supports empty, loading, failed, paused, and restored states. No simulated lifecycle operations or invented usage figures.

Store credentials encrypted at rest with a key outside the database. Supply secrets after template restore; do not bake user secrets into reusable templates. Apply CPU, RAM, disk, output, and operation-count limits. Record lifecycle and credential-administration audit events without logging secret values.

## Milestone three: fleet and lifecycle expansion

Add automatic hibernation with explicit idle policy, gateway-triggered wake, bounded request waiting, and protection against accidental suspension of active work. Hibernation releases VMM memory after durable capture; disk/snapshot storage still costs capacity. Do not advertise zero idle cost.

Add multiple hosts, compatible-template placement, host fencing, authenticated host transport, encrypted S3-compatible snapshot backup, reference-aware garbage collection, organisation roles, usage metering, and optional billing integration. Host failure cannot safely preserve uncaptured memory; recovery promises depend on durable snapshots and fencing.

## Verification and performance acceptance

- Unit tests cover API encoding, state transitions, interrupted publication, path containment, disk independence, quotas, cancellation, and process identity checks.
- Real KVM integration tests cold-boot a fixture, execute commands, preserve files across stop/start, freeze/resume a process, restore memory and disk from a checkpoint, and create two independent template clones.
- Use asymmetric clone writes and a process counter to detect shared disks and accidental cold boots masquerading as restores.
- Kill/restart the manager during lifecycle transitions and verify adoption or explicit recoverable failure without duplicate processes or writers.
- Test the isolated profile separately before claiming that untrusted code is supported. Missing KVM or privileged setup is reported as a skipped/unavailable integration environment, not a passing VM test.
- Measure CLI/API request through successful guest command completion using monotonic time. Also record storage preparation, VMM launch, restore, agent reconnect, and initialization durations.
- Report p50/p95, failures, sample count, concurrency, host/image versions, disk-copy method, and cache condition. Separate cold boots, cached template restores, and cold-cache restores. Do not drop failed samples from the reported success rate.
- Never evict the host-wide page cache on the shared runner for a benchmark. A genuinely cold-cache benchmark requires a dedicated approved environment; otherwise label cache state uncontrolled.
- No sub-10ms end-to-end SLA is assumed. Establish a reproducible baseline before choosing a latency target.

## Implementation order

1. Host preflight and reproducible input manifests.
2. Typed Firecracker client and process lifecycle.
3. Guest protocol and cold-boot/exec integration.
4. Durable instance operations and restart reconciliation.
5. Full checkpoints and same-box restore.
6. Controlled template clones and storage-copy measurement.
7. Real lifecycle benchmarks and isolated-profile validation.
8. Plan the service control plane and UI against the verified runtime contracts.

## Sources

- [Reference platform](https://docs.boxd.sh/)
- [Firecracker 1.16.0 snapshot contract](https://github.com/firecracker-microvm/firecracker/blob/v1.16.0/docs/snapshotting/snapshot-support.md)
- [Firecracker 1.16.0 jailer](https://github.com/firecracker-microvm/firecracker/blob/v1.16.0/docs/jailer.md)
- [Clone randomness considerations](https://github.com/firecracker-microvm/firecracker/blob/v1.16.0/docs/snapshotting/random-for-clones.md)
- [Cloud Hypervisor alternative](https://github.com/cloud-hypervisor/cloud-hypervisor/blob/main/docs/snapshot_restore.md)

Firecracker main-branch documentation includes behavior not present in 1.16.0. The pinned release contract takes precedence for implementation and tests.

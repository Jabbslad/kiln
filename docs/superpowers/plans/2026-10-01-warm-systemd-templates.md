# Warm systemd templates implementation plan

**Goal:** Reduce template-to-first-command latency while preserving full Ubuntu/systemd and unique clone identity. User approved implementation and investigation of disposable boxd.sh instances.

**Architecture:** Add an explicit `systemd_warm` image mode, keeping existing image modes intact. Boot the trusted image's `basic.target` once, quiesce its services and activation sources, and expose an uninitialized bootstrap listener only after admission checks. Clone initialization reseeds the kernel, changes identity, reexecutes PID1, starts fresh services, verifies identity, and hands off to a new workload-agent invocation. No arbitrary live-workload forks.

**Tech stack:** Existing Rust/Tokio/vsock protocol, Firecracker 1.17.0, pinned Ubuntu systemd 255.4, fs-verity/dm-snapshot storage. No new host service, guest NIC, dependency, or infrastructure change.

## Evidence and constraints

- Two fresh boxd.sh instances and a fork retained the same machine ID, PID1-reported machine ID, boot ID, D-Bus bus ID and Docker/journal invocation IDs. Hostnames changed; the guest kernel log reported CRNG reseeding on virtual-machine fork. These are guest observations, not proof of backend internals or a security assessment. Raw evidence is under `.amp/in/artifacts/warm-vendor-*`. All three disposable instances were deleted; existing `full-isle` remains hibernated and unchanged.
- Upstream systemd v255 reexec resets PID1's TLS machine-ID cache, but serializes unit invocation IDs, environment, descriptors and jobs. Other live processes retain caches. A process-only admission check cannot rule out retained FD stores or activation races.
- Only the pinned trusted image policy is supported. Reject unknown processes, services, pending jobs, activators, credentials and retained FD stores. Do not claim to sanitize arbitrary images or previously executed workloads. Completed boot-unit invocation IDs and the prepared kernel boot ID remain preparation history; fresh daemon/agent invocations and machine identities must differ.
- Guest exec and ready responses remain blocked through the entire reset. Reexec alone is insufficient: the workload agent must start as a new systemd invocation. Preserve old snapshot compatibility and same-box checkpoint semantics.
- Keep current worktree and unrelated dashboard files untouched. Existing disk changes remain local and uncommitted. Do not push or change shared infrastructure. Use bounded disposable validation and retain failures as evidence.

## 1. Establish the pinned image's quiescence policy

- [x] Inspect active units, jobs, activation sources, manager environment and retained FD stores on a disposable local Ubuntu box. Identify the fixed service/unit allowlist and stopped activation units to restore later.
- [x] Implement `crates/kiln-guest/src/warm.rs`: bounded systemctl operations; parse and validate unit/property inventories; stop activators before daemons; drain jobs and confirm only PID1/bootstrap remain; check FD stores and credential properties. Expose the listener only after this invariant holds. Store activation names in boot-local state, not arbitrary shell commands.
- [x] Add failing tests first for unexpected service/process, malformed or missing inventory fields, nonempty FD stores/credentials, pending jobs and activators. Verify stability after delaying at the KVM preparation barrier; separately exercise captured templates in the lifecycle suite.

## 2. Add the reset and fresh-agent handoff

- [x] Reuse the existing bootstrap refusal of Exec and suppression of ready Hello. Refactor its serve path only enough to accept a warm initializer; keep the legacy PID1 path unchanged.
- [x] Warm initialization: validate and inject entropy using checked RNG ioctls; update identity; daemon-reexec PID1; restart intended services/activators; validate PID1 and D-Bus machine ID; publish a boot-local completion marker. Close bootstrap listener before acknowledging. A separate bootstrap service starts the ordinary workload agent and exits; no self-stop deadlock or reuse of the initialized bootstrap as a workload agent.
- [x] Bind the workload handoff marker to current machine identity. Test mismatched/missing/invalid marker and failed reset. Host clones must reject unexpectedly initialized templates instead of skipping initialization.
- [x] Test raw protocol requests before, during and after handoff; every failure before verified completion remains not-ready. Distinguish D-Bus GetId (bus instance) from Peer.GetMachineId (machine identity).

## 3. Integrate explicit image/runtime policy

- [x] Add/test `BootMode::SystemdWarm` in `crates/kiln-runtime/src/image.rs`; version it through the existing strict manifest enum. Extend only the relevant host readiness branches in `runtime.rs` and template capture as needed.
- [x] Extend `images/build-ubuntu.sh` with an explicit warm mode; default remains legacy systemd. Generate a separate console-logging bootstrap unit ordered after basic startup and a fresh marker-gated workload service. Preserve image checksum/ownership/no-overwrite behavior. Do not silently remove systemd functionality or permanently disable activation sources to pass admission.
- [x] Build a new pinned Ubuntu image without modifying existing images. Run ordinary tests and the full development/isolated lifecycle suite for old and warm images.

## 4. Verify safety and measure the actual benefit

- [x] Verify fresh machine/PID1/D-Bus identity, distinct bus and restarted-service/agent invocation IDs, correct journal identity, Python/systemd-run, agent restart, cold stop/start, overlay checkpoint restore, and legacy image compatibility.
- [x] Exercise initialization/handoff faults and negative template admission, including an exited service with a retained FD store; confirm no workload executes before verified readiness.
- [x] Compare legacy and warm templates on the same host, image inputs, sealed-overlay filesystem and resource limits, 30 sequential and 16 concurrency-four samples. Include every failure, phase and first-command boundary; do not equate vendor boot fields with end-to-end latency. If the safe reset erases the speed benefit, report it rather than weakening identity checks.
- [x] Update README/operator guidance with measured results, opt-in policy and limitations. Verify no disposable VMs/devices/mounts remain; preserve evidence and remove scratch prototypes. Report actual Git delivery state.

## Implementation findings

- `bpfilter_umh` is a userspace helper embedded in the pinned kernel, not a PF_KTHREAD task. Upstream Linux v6.1 implements no filtering operations in it; normal netfilter remains independent. Warm boot skips its initcall instead of whitelisting another live userspace process.
- `StandardOutput=console` is not a valid systemd output specifier. The bootstrap uses `tty` with `/dev/console`, avoiding journal descriptors. A full journald stop drops its restart-preserved FD store; the subsequent audit verifies zero rather than clearing unexpected retained state.
- The Ubuntu D-Bus policy needs an explicit root-only permission for the bus driver's read-only `Peer.GetMachineId` query.
- Admission must not run on a user's later cold restart. Provisioned disks take the legacy pre-systemd initialization path. A real regression test first reproduced rejection of a custom credential-using service, then verified its normal startup after this change.
- Initial three-sample pilots were slower (warm template p50 924 ms versus legacy 738 ms). Combining activation-policy reload with PID1 reexec and flushing boot writeback reduced a subsequent warm pilot to 505 ms. Instrumentation observed Dirty 440 KiB → 0; it was removed afterward. These small exploratory runs are not the final benchmark and do not isolate either optimization's individual contribution.

## Final evidence and delivery

- Final matched images: `ubuntu-systemd-v5` and `ubuntu-warm-v8`, identical pinned input and builder/guest/init hashes. Only template creation requests warm preparation; all ordinary cold boots bypass it. Guest warm preparation checks its kernel flag and pristine identity before any mutating systemctl operation, preventing accidental host execution.
- 65 ordinary tests passed; Clippy all targets/features with warnings denied passed. Each final lifecycle suite passed 7 checks: warm development, legacy development, warm isolated-overlay and legacy isolated-overlay. Five final real-KVM/raw-vsock admission/handoff cases passed, including retained FD store, credential, reset failure and marker-write failure. Evidence: `.amp/in/artifacts/warm-final2-*.log`, `ubuntu-{warm-v8,systemd-v5}-overlay-lifecycle.log`, and `warm-wire-final2/results.json`.
- Final template p50/p95: sequential **721/756 → 512/536 ms**, concurrent **909/969 → 630/677 ms**. Sequential p50 improved **29.0%**; successful-sample concurrent p50 improved **30.7%**. Cold sequential p50 **2,551 → 2,560 ms**, concurrent **3,148 → 3,144 ms**. Median sequential guest initialization **559 → 345 ms**.
- **183/184 complete benchmark attempts succeeded; all 92 warm attempts passed.** One legacy concurrent template first exec returned `Resource temporarily unavailable (os error 11)` after a successful launch. Its exact syscall/cause is undiagnosed; the legacy concurrent percentiles use 15 successful samples. The CLI exited 1, and the failed sample is retained, not replaced or retried. No claim of improved reliability follows from one run. All counts, percentiles, phase accounting and matched inputs were independently validated in `warm-final-validation.txt`.
- All disposable template/box state was removed through the CLI. No Firecracker processes, test child cgroups or task-owned device mappings remained; the loop-backed pool was unmounted. Intermediate images and debug scratch were removed; final images, binaries and evidence remain. The unrelated dashboard work and existing vendor guest were untouched.
- Implementation and documentation are local, **uncommitted and unpushed**. These timings are not a matched performance comparison with boxd.sh. The transient legacy first-exec failure remains a follow-up reliability investigation, not a hidden passing result.

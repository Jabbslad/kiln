# Single-host development runtime

## Safety and prerequisites

This is a trusted-workload prototype. Firecracker runs as the invoking user with its normal seccomp policy, **without jailer, per-VM host cgroups, or guest networking**. Guest vCPU/RAM configuration and local inventory quotas are not complete host resource isolation. Do not execute customer or adversarial code, expose the state directory, run as root, or advertise this as production-ready.

Linux x86_64, accessible KVM, cgroup v2, and matching Firecracker/jailer **1.17.0** binaries on `PATH` are required. `box doctor` is read-only and reports `isolated_ready: false`. An isolated launch is rejected; there is no silent unjailed fallback.

The runner originally had 1.16.0. Crash testing reproduced its permanent vsock failure after bare pause/resume. [Firecracker 1.17.0 fixes this](https://github.com/firecracker-microvm/firecracker/releases/tag/v1.17.0) in PR #6100. `scripts/fetch-firecracker.sh` pins and verifies the official x86_64 release archive SHA-256 and installs into an explicitly selected local directory. No system binaries, services, users, cgroups, firewall rules, or filesystems are provisioned automatically.

The fixture uses a checksum-pinned Linux 6.1.155 kernel from Firecracker's v1.15 CI artifacts, a statically linked Rust musl guest agent, and locally supplied static BusyBox. Its `inputs.json` records source URL and component hashes. `mkfs.ext4 -d` populates a 128 MiB root filesystem without host mounts or root. This is a test image, not a supported general-purpose distribution. Build instructions are in the [README](../README.md).

## CLI contracts

For brevity, commands below use `box`; the built executable is `target/release/box`. All commands accept `--state-dir PATH`; the parent directory must already exist. The default is `.boxd`. Existing state directories must be owned by the operator and have no group/other permissions. State and guest secrets are stored **unencrypted**.

Output is always JSON (`--json` is accepted for scripting compatibility). Diagnostics go to stderr. Exit codes: 0 success, 1 runtime/transport failure, 2 argument-parser failure. `exec` returns the guest exit code, 124 on command timeout, or 125 for a signal termination without a numeric exit code. Its JSON distinguishes command failure from transport failure. CLI output decodes stdout/stderr as UTF-8 with replacement; this is not a binary file-transfer protocol.

```sh
box create --image IMAGE_MANIFEST --name NAME --allow-unsafe-development
box list
box inspect BOX_ID
box exec BOX_ID --timeout-ms 10000 --cwd /workspace --env KEY=value \
  -- /bin/sh -c 'printf "%s" "$KEY"'
box pause BOX_ID
box resume BOX_ID
box stop BOX_ID
box start BOX_ID
box stop BOX_ID --force
box delete BOX_ID
```

- VM lifetime is independent of the CLI process that launches it.
- A successful launch includes a real guest handshake and initialization, not just a successful VMM spawn.
- `stop` asks the guest to sync and then terminates the VMM. A paused guest is briefly resumed to sync. This is not an OS/application shutdown transaction. `start` cold-boots the retained disk and reinitializes the agent; it does **not** restore memory.
- `stop --force` pins and validates process identity, then kills without consulting the guest/API. It loses guest RAM and unflushed writes. The response records this warning. `delete` likewise force-stops the owned VM and removes its disk generations.
- An exec request holds the box lock, so overlapping operations on that box fail busy. Concurrent launches of different boxes reserve quota durably before doing launch I/O. Allocation waits are bounded; other conflicting lifecycle operations fail rather than race disk writers.
- Limits: 8 allocated boxes / 8192 MiB total guest RAM per state directory, 128–2048 MiB and 1–4 vCPUs per box, 16 snapshot directories, 64 KiB output per stream, 1 MiB protocol frames, bounded guest connections/executions, CLI exec timeout 1–3,600,000 ms. Host overhead is additional. These are development limits, not tenant quotas.
- Copy operations check free space and a 120-second deadline; requests have transport timeouts. No general operation-wide cancellation token or strict I/O deadline exists yet. OS filesystem calls can block. Logs/files have a 3 GiB per-file VMM limit to accommodate full memory snapshots; there is no total disk quota or log rotation yet.

Disk copies try reflink first. The byte-copy fallback uses `SEEK_DATA`/`SEEK_HOLE` to skip filesystem holes, scans allocated ranges for zero chunks, preserves logical length, and syncs the new file and parent directory. Filesystems without hole-seek support retain full-file zero scanning. No host filesystem provisioning is performed.

The wire protocol is one length-framed request per vsock connection. Arguments are passed as argv, not interpolated into a shell. Use a shell explicitly if desired. The agent starts each command in a process group, caps output, and cancels that group on timeout or host disconnect. Descendants that deliberately escape the process group are outside this development mechanism; guest cgroups are not implemented. Commands are never automatically retried after an unknown transport outcome.

## Checkpoints restore both memory and disk

```sh
box checkpoint save BOX_ID
box checkpoint list
box checkpoint restore BOX_ID CHECKPOINT_ID --acknowledge-external-state-replay
box checkpoint delete CHECKPOINT_ID
```

Capture holds the box lock, records intent, pauses execution, creates a full Firecracker snapshot, explicitly syncs the disk backing file, copies it while paused, syncs and hashes all artifacts, and publishes the manifest last. A previously running source resumes; a previously paused source stays paused. Snapshot files are private and read-only. Firecracker maps memory from the retained snapshot file; it must remain immutable for the restored VM's lifetime.

A same-box checkpoint preserves process memory and guest filesystem state at the pause boundary, including dirty guest page-cache state in RAM. It does not roll back databases, remote requests, clocks outside the VM, or other external systems. Existing vsock sessions are reset. Restore therefore requires an explicit external-state replay acknowledgement.

Restore verifies checksums and an exact host/VMM compatibility fingerprint before stopping the current VM. It stages a new private writable disk generation, confirms old-process termination, launches from snapshot, and removes the old generation only after guest readiness. A template or another box's checkpoint cannot be used for same-box restore.

Snapshot deletion checks durable box references under the allocation lock. It refuses to delete a snapshot referenced by a box, including a stopped box. Delete the referencing box first. Deleting a box never automatically deletes its backing snapshot. This deliberately conservative policy keeps mapped memory files alive without a background garbage collector.

## Prepared templates are not arbitrary live-workload forks

```sh
box template build --image IMAGE_MANIFEST --allow-unsafe-development
box template list
box clone TEMPLATE_ID --name alpha --allow-unsafe-development
box clone TEMPLATE_ID --name beta --allow-unsafe-development
```

The builder boots a trusted image to the guest agent's pre-initialization barrier, snapshots it, and deletes its temporary VM. Only those template records are cloneable. Every clone receives an independent writable disk, distinct hostname/machine ID, and fresh host entropy before execution is allowed. The kernel boot ID is intentionally inherited from the prepared boot and is **not** a clone identity.

Clone validates snapshot metadata and reserves quota plus a durable snapshot reference under the allocation lock, then releases that lock before scanning all artifact checksums. The per-box lock remains held through verification and launch; snapshot deletion refuses the durable reference. Corruption still prevents VM startup. A verification failure leaves a failed reservation that counts against quota and pins the snapshot until `box delete BOX_ID`; `box list` reconciles it to stopped. Same-box checkpoint restore still verifies before stopping the source VM.

The image author must keep workload daemons, credentials, and userspace random state out of the prepared image. The minimal fixture satisfies that contract; arbitrary Linux images may not. VMGenID and entropy injection do not magically rewrite application-level credentials or cached random values.

## Crash recovery and its limits

`box inspect` and `box list` reconcile persisted records with actual processes. Process identity includes PID, Linux start ticks, executable, and working directory; pidfds pin termination targets. A reused PID is refused, never blindly killed. A process spawned before its PID was recorded is found by its unique run directory and executable. Multiple matching VMMs are a hard error.

- Interrupted launch: adopt an initialized running VM, or terminate only the identified incomplete VMM and leave its disk stopped for explicit `start`/restore. A pre-copy failure may require deleting the failed box and recreating it.
- Interrupted capture of a running source: reconciliation resumes the paused source. An incomplete snapshot has no published manifest and is not listed as usable. Its directory can remain after abrupt death and counts against the snapshot limit. Once no capture is active, `checkpoint delete ID` also removes an unpublished directory whose ID you obtained from the private `snapshots/` directory.
- Restore failure: `restore.previous.json` and the original disk generation remain until a successful restore. Recovery does not automatically replay a restore request or switch back to that old generation. Inspect, then explicitly retry a verified checkpoint restore. Retained generations from interrupted attempts can remain until box deletion; do not manually unlink files while a VMM could use them.
- Unresponsive API/guest: use `stop --force`, then inspect. Never delete a state directory containing a live VM.

Recovery runs on explicit commands, not an always-on supervisor. Abrupt process death releases kernel locks, but dropping an async Rust future is not transactional rollback. The fault suite verifies selected process-crash boundaries, not power-loss behavior or every storage/ENOSPC/cancellation permutation. Broader fault coverage and cleanup are still required before production use.

## Reproduce launch measurements

```sh
cargo build --locked --release -p box-runtime --bin box
box benchmark --image IMAGE_MANIFEST --template TEMPLATE_ID \
  --samples 30 --concurrency 1 --allow-unsafe-development
box benchmark --image IMAGE_MANIFEST --template TEMPLATE_ID \
  --samples 16 --concurrency 4 --allow-unsafe-development
```

The JSON includes each sample, failures, nearest-rank p50/p95/p99, launch-to-ready and first-exec timing, actual copy method, versions, host fingerprint, concurrency, and cache assumptions. It returns a failure status if any measured operation fails. Measurement starts at the runtime call, includes validation/allocation waits/disk preparation/VMM/guest initialization, and ends after `/bin/printf boxd-ready` returns the expected bytes and exit code. It excludes benchmark program startup and cleanup. Only boxes with this benchmark invocation's unique name prefix are removed.

Each sample includes non-overlapping wall-clock `phases_ms`: image or snapshot verification (including validation/preflight), allocation wait, durable reservation, disk copy, boot preflight, VMM spawn/API readiness, VMM configuration/snapshot load, guest readiness, initialization, and launch commit. Timings are task-local, so concurrent requests do not share counters. Failed attempts include completed/failed phase timings but no successful `total_ms`; phases that were never reached are absent. Phase sums exclude small bookkeeping gaps and the separately reported first exec.

"Cold boot" means a fresh guest kernel boot, **not** an evicted host filesystem cache. No host cache eviction is performed. Firecracker uses file-backed demand paging, but the runtime currently reads every artifact for integrity verification before restore; those reads are included in the measurements and populate the cache.

Matched before/after measurements on the development runner, release host/guest, Firecracker 1.17.0, Ryzen 7 7840HS, host Linux 7.0.0-34-generic, ext4 without reflink, 256 MiB / 1 vCPU / 128 MiB disk. Both runs use the same image and prepared snapshot; the before run includes timing instrumentation but neither optimization. Each row has the indicated sample count per implementation:

| Mode | Samples | Concurrency | Before p50 / p95 | After p50 / p95 |
| --- | ---: | ---: | ---: | ---: |
| Cold boot | 30 | 1 | 844 / 888 ms | 807 / 851 ms |
| Template restore | 30 | 1 | 281 / 288 ms | 245 / 257 ms |
| Cold boot | 16 | 4 | 836 / 881 ms | 805 / 852 ms |
| Template restore | 16 | 4 | 537 / 973 ms | 277 / 325 ms |

All 184 launches succeeded. Sequential template disk-copy median fell from 41 to 13 ms; concurrency-four allocation-wait median fell from 340 to 24 ms. Full snapshot verification remains about 196 ms sequential and dominates the remaining template latency. Guest kernel readiness still takes about 602 ms on cold boots. These are small local samples with uncontrolled cache/load, not an SLA or a matched comparison with boxd.sh.

Raw results are retained locally as `.amp/in/artifacts/perf-{before,after}-{sequential,concurrent}.json`, not committed. No sub-10ms end-to-end claim is made. A disposable-file `FS_IOC_ENABLE_VERITY` probe returned `EOPNOTSUPP` on this host; kernel feature support alone does not mean the filesystem has verity enabled. No filesystem features were changed. A future host service should evaluate verified immutable artifact storage (for example, fs-verity where available) and reflink-capable disks before adding a custom memory pager. Metadata-only checksum caches are not implemented.

## Verification and remaining milestones

The README lists exact commands. The normal suite checks protocol limits, guest initialization/execution/cancellation/output, API faults/timeouts, image integrity, file ownership/copy/locking, process identity, CLI rejection, and percentile calculation. Opt-in real-KVM tests verify:

1. Cold create, exec, exit codes, stop/start disk persistence, immediate post-resume execution, paused capture semantics, memory/disk restore (including reviving a killed process), and corruption rejection before source termination.
2. Independent clone identities/disks/lifetimes and prevention of backing-memory deletion.
3. CLI crashes before spawn, after spawn, after PID persistence, after pause, and after manifest publication, with safe process reconciliation and no partial snapshot publication.
4. Simultaneous starts and forced termination with an inaccessible API socket.
5. Per-request, non-overlapping phase timings during concurrent benchmarks and cleanup of measured boxes.
6. A clone blocked in verification releases the allocation lock while its quota/reference remain durable, prevents snapshot deletion, and rejects corrupt contents without spawning a VMM.

Not implemented: jailed launch/cgroup enforcement, Ubuntu/systemd/Docker image, public API, PostgreSQL operations/idempotency, network/SSH/preview access, web UI, SDK, fleet scheduling, or hosted tenancy. Privileged host setup and validation require separate approval. These are real remaining implementation steps, not mocked features in this runtime.

# Single-host runtime

Commands and configuration names use Kiln v0.3.0. Historical measurements below
predate the rename; their retained evidence and installations have not been moved
or regenerated. Rebuild guest images/templates before using the renamed runtime.

## Safety and prerequisites

This is a trusted-workload prototype. The default development profile runs Firecracker as the invoking user with its normal seccomp policy, **without jailer, per-VM host cgroups, or guest networking**. Do not run that profile as root. Guest vCPU/RAM configuration and local inventory quotas are not complete host resource isolation. The experimental isolated profile below passes the privileged lifecycle suite on the development runner with the trusted fixture. Neither profile is advertised as production-ready or approved for customer/adversarial workloads.

Linux x86_64, accessible KVM, cgroup v2, and matching Firecracker/jailer **1.17.0** binaries on `PATH` are required. `kiln-runtime doctor` is read-only and reports `isolated_ready: false` unless an explicit, valid `--isolation-config` passes privileged preflight. That flag reports configuration readiness, not a completed VM isolation test. There is no silent unjailed fallback.

The runner originally had 1.16.0. Crash testing reproduced its permanent vsock failure after bare pause/resume. [Firecracker 1.17.0 fixes this](https://github.com/firecracker-microvm/firecracker/releases/tag/v1.17.0) in PR #6100. `scripts/fetch-firecracker.sh` pins and verifies the official x86_64 release archive SHA-256 and installs into an explicitly selected local directory. No system binaries, services, users, cgroups, firewall rules, or filesystems are provisioned automatically.

The fixture uses a checksum-pinned Linux 6.1.155 kernel from Firecracker's v1.15 CI artifacts, a statically linked Rust musl guest agent, and locally supplied static BusyBox. Its `inputs.json` records source URL and component hashes. `mkfs.ext4 -d` populates a 128 MiB root filesystem without host mounts or root. This is a test image, not a supported general-purpose distribution. Build instructions are in the [README](../README.md).

## Ubuntu image contract

`bash images/build-ubuntu.sh OUTPUT_DIRECTORY [ROOTFS_TARBALL]` builds an experimental Ubuntu Minimal 24.04 amd64 guest. The optional local tarball must match the same pinned SHA-256 as the downloaded input. The builder pins [release-20260905](https://cloud-images.ubuntu.com/minimal/releases/noble/release-20260905/) (`094dc0afc6ded1c3e5ce71f7d0b48d5db922155097bc8fb1ec19db2ebdd17ece`) and the fixture's Linux 6.1.155 kernel. It builds static `kiln-init`/`kiln-guest` binaries, preserves upstream numeric ownership with fakeroot, and uses `mkfs.ext4 -d` to create a 2 GiB disk without root, host mounts, chroot, or package scripts. `inputs.json` records upstream URLs/checksums and builder/guest binary hashes. Inputs are repeatable; filesystem timestamps/UUIDs mean this is not a bit-for-bit reproducible image claim. Existing output images are never overwritten.

Included tools are the upstream systemd, Bash, Python 3, curl, and apt packages. There is no guest NIC, package-download connectivity, SSH server access, Docker, or compiler toolchain. The custom `kiln.target` starts basic systemd services and the agent; cloud-init is disabled and network/SSH units are masked. No host service is installed. Security updates require explicitly updating/revalidating the pinned input and rebuilding; an automatic image update/release process remains future work.

Ubuntu manifests default to `boot_mode: "systemd"`; missing `boot_mode` retains the fixture's `/sbin/init` behavior. The opt-in `systemd_warm` mode is described below. Only fixed boot modes are accepted, not arbitrary kernel arguments. In the default mode Ubuntu boots `/sbin/kiln-init` as PID 1, mounts guest pseudo-filesystems, and waits at a host-vsock initialization barrier. It never executes workloads. Templates capture this state **before systemd starts**, avoiding systemd/D-Bus caching a shared template machine ID. On initialization the guest mixes 32 bytes of host entropy into the kernel pool with `RNDADDENTROPY`, explicitly reseeds the CRNG, sets the hostname (including `/etc/hostname`), writes `/etc/machine-id`, and creates a boot-local handoff marker. The bootstrap closes its listener before acknowledging, then execs systemd. The host waits up to 30 seconds for the initialized systemd-managed agent; initialization is never replayed to compensate for a lost response.

The service requires the valid handoff marker and resumes initialized after a service restart. `/run` is recreated on cold boot, so a normal stop/start provisions again using the existing box ID; same-box checkpoint restore resumes captured service state. The prepared kernel boot ID is deliberately shared by clones and is not the per-instance identity. SSH keys, machine ID, cloud-init state, persistent journals, and the stored random seed are cleared in the image. Workloads and credentials added afterward must not be promoted into shared templates.

On `ser7`, all six development-mode lifecycle tests passed against this Ubuntu image, including Python output, systemd-run service execution, systemd's D-Bus machine ID matching each independent clone, agent restart without reinitialization, disk persistence, memory restore, and crash recovery. The freshly rebuilt BusyBox fixture also passes the shared suite. Image checks confirmed preserved root/shadow ownership, empty initial machine ID, required binaries/service configuration, refusal to overwrite an image, and rejection of a wrong input checksum. On 2026-10-01 the privileged Ubuntu suite also passed all six tests in 54.51 seconds, followed by 92 successful isolated benchmark launches. The script exited 0 without remaining child cgroups, and a fresh process check found no Firecracker processes. The separate installation/results remain available for inspection. Earlier fixture timings must not be reported as Ubuntu results.

Run the shared suite with `KILN_TEST_IMAGE` set to the Ubuntu manifest; all other development/isolated test flags below are unchanged. For measurement, build a template from that same image and use `kiln-runtime benchmark` with 30 samples at concurrency 1 and 16 at concurrency 4. Benchmark JSON now records `profile` alongside cache assumptions, versions, per-sample phases/failures, and first-command latency. The 2 GiB disk's integrity scan, private copies, and post-resume systemd startup remain inside the measured path; this bootstrap template is not a snapshot of an already running systemd system.

## Opt-in warm systemd templates

Build a **new** image directory; the default builder mode and existing templates are unchanged:

```sh
bash images/build-ubuntu.sh images/output/ubuntu-warm '' systemd_warm
# Or supply the checksum-pinned root tarball instead of ''.
```

Use that manifest with the existing `template build` and `clone` commands and your chosen host profile. Warm mode preserves Ubuntu/systemd; it is not a stripped-down replacement init. It supports only the builder's trusted, pinned systemd **255.4-1ubuntu8.17** image policy, not arbitrary customized images or live-workload forks. Network/SSH/Docker and hostile multi-tenant isolation remain outside the current image/runtime contract.

Preparation boots through `basic.target` using a public placeholder machine ID, then gates and stops sockets, timers, paths and automounts before stopping D-Bus, journald and udev. It rejects unexpected running/exited services, retained service descriptors, configured credentials, nonempty credential stores, pending jobs, unexpected manager environment and remaining userspace processes other than PID1/bootstrap. Completed audited boot services may remain active/exited. The bootstrap checks quiescence twice and flushes the root filesystem before exposing its uninitialized vsock listener. This is an admission guard for trusted images, **not a sanitizer that discovers every possible secret in arbitrary files or configuration**.

Every clone rechecks quiescence, injects/reseeds host entropy, sets hostname and machine ID, removes the temporary masks without redundant reloads, and reexecutes PID1 to reset its cached identity. It then starts fresh daemons and restores the original activation sources, verifies both PID1 and the bus daemon's `Peer.GetMachineId`, and writes the handoff marker. A root-only D-Bus policy permits that read-only identity query. Only a **new invocation** of the workload agent may advertise readiness or execute commands. The marker must match the current machine ID; any reset/handoff failure leaves execution blocked, including direct vsock requests. A host launch rejects an unexpectedly preinitialized template rather than skipping initialization.

The prepared kernel boot ID, completed boot-unit invocation IDs and preparation journal history remain shared history. Machine IDs, running-daemon/agent invocation IDs, bus IDs and new journal metadata are fresh. Only template creation performs warm preparation. Ordinary cold creation and cold restarts use the pre-systemd initialization path without paying for preparation or applying template admission to user-added services and credentials. Same-box checkpoint restore resumes captured state without reinitializing it.

The pinned kernel embeds an experimental userspace `bpfilter_umh` helper. Warm mode skips its initial launch with `initcall_blacklist=load_umh`, rather than allowing an extra userspace process into the snapshot. The [upstream helper implements no filtering operations](https://github.com/torvalds/linux/blob/v6.1/net/bpfilter/main.c#L15-L29); normal netfilter is independent. Revalidate this assumption and the full admission policy when changing the kernel or systemd package.

The shared lifecycle suite checks independent clone identity, fresh daemon/bus invocation IDs, journal identity, Python/systemd-run, agent restart, disk and memory persistence, and cold restart of a custom credential-using service. Additional raw-vsock tests inject a retained FD into an exited service, a systemd credential, an identity-verification failure, and a marker-write failure:

```sh
python3 scripts/test-warm-guest.py images/output/ubuntu-warm/image.json \
  .tools/firecracker-1.17.0/firecracker .amp/in/artifacts/warm-wire
```

These tests use disposable **unjailed trusted** guests, no guest networking, and no sudo. They delay at the preparation barrier and race requests against handoff. They do not constitute a hostile-image security review.

### Experimental retained-PID1 policy

`systemd_warm_shared` is a separate, explicit image mode for the pinned trusted,
networkless worker image. Existing images, templates and defaults do not change:

```sh
bash images/build-ubuntu.sh images/output/ubuntu-warm-shared '' systemd_warm_shared
```

All boxes using this mode deliberately share the public machine ID
`11111111111111111111111111111111`, including ordinary cold creates and restarts.
Host box IDs and guest hostnames remain unique. Only template construction enables
warm preparation and PID1 retention. After restore, PID1 reloads unit definitions
instead of reexecuting; it retains its preparation-time process/library state.
D-Bus, journald and udev still start fresh after host entropy reseeding and hostname
assignment. Quiescence admission, private disks, identity verification and the
fresh workload-agent handoff are unchanged. A request for a different machine ID
fails before any reset mutation. Same-box checkpoints retain their existing replay
semantics; this is not a live-workload fork facility.

This trades machine-identity uniqueness for a potential launch improvement. Shared
IDs can duplicate machine-derived DHCP identities and randomized timer offsets;
they also affect systemd's host credential-key identity. Do not enable this mode
for arbitrary services, networked general-purpose machines or unreviewed secrets.
The standard warm mode retains fresh machine identities. The inspected systemd
v255 random helpers normally call kernel RNG interfaces, but this is not an audit
of every library or retained PID1 field, nor a multi-tenant security guarantee.

Both warm policies emit one `kiln_warm_reset {JSON}` line in the host generation's
`console.log`. It includes `retain_pid1`, `success`, `total_ms`, and nonoverlapping
`phases_ms`: `audit`, `provision`, `unmask`, `manager_refresh`, `start_services`,
`verify_identity`. A failed step is timed too; later phases are absent. These are
guest-local reset timings, not the host's full launch time: handoff marker writing,
fresh agent startup, host transport and log emission are outside those phases.
No entropy, credentials or user-supplied values are logged. The raw-vsock suite
accepts either mode and checks the chosen manager operation; shared mode also
tests mismatched-identity rejection with execution remaining blocked.

### Retained-PID1 measurements

On 2026-10-01, compared freshly built `systemd_warm` and `systemd_warm_shared`
images on the same runner. Both use identical pinned Ubuntu/kernel/builder/guest
binary inputs, 4096 MiB / one vCPU, a 2 GiB disk, the isolated profile, fs-verity
and authenticated dm-snapshot writes. The release CLI has no fault injection.
Policy order alternates per batch; each concurrent batch contains four clones of
the same policy. OS page cache and background load were uncontrolled.

| CLI boundary | Unique identity p50 / p95 | Retained PID1 p50 / p95 |
| --- | --- | --- |
| Sequential create (30 each) | 517 / 538 ms | **475 / 493 ms** |
| Sequential through first command | 536 / 556 ms | **493 / 511 ms** |
| Concurrency 4 create (16 each) | 618 / 673 ms | **581 / 645 ms** |
| Concurrency 4 through first command | 636 / 690 ms | **598 / 663 ms** |

All **92 attempts succeeded**, with no retries or replaced samples. Create p50
improved **8.1% sequentially / 6.0% concurrently**; first-command p50 improved
**8.0% / 6.0%**. Timers include CLI/sudo/env startup; template construction, log
collection and resource deletion are outside the measured interval. The command
is a fixed printf readiness probe with a five-second guest deadline. With 16 samples,
nearest-rank p95 is the maximum. These are modest single-host improvements, not
proof of better hosted performance. The older boxd.sh sequential create median
was 408 ms; the new 475 ms local result does not close that observed gap, and
the hosted service was not remeasured in this experiment.

| Sequential guest reset step, median | Unique identity | Retained PID1 |
| --- | --- | --- |
| Admission recheck | 92.8 ms | 90.4 ms |
| Entropy/hostname/machine-ID provisioning | 0.9 ms | 0.9 ms |
| Unmask definitions | 12.5 ms | 12.1 ms |
| Manager reexec / reload | 145.4 ms | 106.2 ms |
| Start daemons and activators | 80.1 ms | 76.6 ms |
| Verify cached identities | 5.9 ms | 5.8 ms |
| Entire measured reset | 337.9 ms | 291.9 ms |

The manager still needs a reload to observe unmasked units. Retention saves about
39 ms in that operation, not its entire 145 ms. Entropy/identity provisioning is
not the bottleneck. Admission queries and manager reload are the next targets for
profiling/batching without deleting checks. Keep shared identity experimental;
this result does not justify changing the default or retaining every daemon.
Medians of separate steps need not sum to the median total.

Evidence: `.amp/in/artifacts/retained-pid1-comparison.json`, its reproducible
`retained-pid1-benchmark.py`, per-clone console logs, and
`retained-pid1-validation.txt`. Independent checks verified all sample counts,
unique names/IDs, resources, input hashes, nearest-rank percentiles, timing sums
and cleanup. The raw-vsock cases passed for both policies (11 total). An initial
shared-mode lifecycle benchmark check failed; the old test helper printed only
stderr and lost its JSON failure report. The helper now includes stdout and exit
status. Three diagnostic reruns passed without a runtime fix; the original cause
remains unknown and is **not claimed fixed**. Its failed log is retained separately
from the 92 successful comparison samples.

### Matched warm-template measurements

On 2026-10-01, compared fresh legacy and warm images with identical pinned Ubuntu archive, kernel, builder and guest/init binary hashes. Both used the same release CLI without fault injection, Firecracker/jailer 1.17.0, Ryzen 7 7840HS host, 256 MiB / 1 vCPU, isolated profile and loop-backed ext4 with fs-verity and authenticated dm-snapshot disks. Only the image boot mode and its required configuration differed. These times run from the runtime request through the first successful guest command, including integrity checks and identity initialization.

| Launch path | Legacy p50 / p95 | Warm p50 / p95 | Successful attempts, legacy / warm |
| --- | --- | --- | --- |
| Sequential template | 721 / 756 ms | **512 / 536 ms** | 30/30 / 30/30 |
| Concurrency 4 template | 909 / 969 ms | **630 / 677 ms** | **15/16** / 16/16 |
| Sequential cold boot | 2,551 / 2,953 ms | 2,560 / 2,589 ms | 30/30 / 30/30 |
| Concurrency 4 cold boot | 3,148 / 3,200 ms | 3,144 / 3,164 ms | 16/16 / 16/16 |

All **92 warm attempts succeeded**; the complete comparison has **183 successes and one failure**. Legacy concurrent template sample 2 completed launch but its first exec returned `Resource temporarily unavailable (os error 11)` after 0.029 ms. The report retains that failure and the CLI exited 1. No request was retried or sample replaced. The exact failing syscall was not captured, so its cause remains undiagnosed; the concurrent legacy percentiles describe only its 15 successful samples, not all attempts.

Sequential template p50 fell **29.0%**. Successful-sample concurrent p50 fell **30.7%**, subject to the failure caveat above. Median guest-initialization phases fell from **559 → 345 ms** sequentially and **658 → 365 ms** concurrently. Integrity verification, disk setup and VMM start stayed broadly similar. Cold boots deliberately use the normal pre-systemd path; their median latency was essentially unchanged. Fresh-service startup and PID1 reset still dominate warm initialization.

Cohorts ran legacy sequential, warm sequential, warm concurrent, then legacy concurrent; each measured cold boots before restores. OS page cache was uncontrolled, with no eviction. Template construction and benchmark cleanup are outside the clock. Nearest-rank percentiles, counts, unique indices, phase non-overlap, launch/exec accounting, resource limits and input hashes were independently checked. Small samples, shared-host noise and fixed ordering limit generalization. This is **not a matched boxd.sh comparison or a hosted SLA**.

Raw evidence remains in `.amp/in/artifacts/warm-final-{before,after}-{sequential,concurrent}.json`, template manifests and `warm-final-validation.txt`. Earlier exploratory pilots remain separate, including the initial slower warm implementation; none were pooled into this comparison. Guest inspection of disposable boxd.sh instances found retained machine/boot/bus and daemon identities across fresh instances and a fork, with changed hostnames and a kernel reseed message. This is consistent with prebooted-state reuse, not proof of its backend implementation or a security assessment. All three investigation guests were deleted; the existing user guest was left untouched.

### Fresh boxd.sh comparison at 4 GiB / one vCPU

On 2026-10-01, repeated the hosted comparison with **4096 MiB / one vCPU on both sides**, 30 sequential and 16 concurrency-four launches each. Both restore prepared snapshots and run a fixed printf readiness probe with a five-second guest-command deadline. The timer begins before spawning the create/clone CLI and ends after the first successful exec CLI returns. Local timings include `sudo`/`env` startup. Template creation, resource probes and deletion are outside the timer. No failed request is retried. The earlier 256 MiB and runtime-only timings are not pooled into this comparison.

| CLI launch through first successful command | Our local isolated runtime | boxd.sh hosted CLI |
| --- | --- | --- |
| Sequential p50 / p95 (30 each) | **523 / 643 ms** | 803 / 975 ms |
| Concurrency 4 p50 / p95 (16 each) | **640 / 695 ms** | 1,002 / 1,224 ms |
| Successful attempts | 46/46 | 46/46 |

Observed local end-to-end medians were **34.9% lower sequentially / 36.1% lower at concurrency four**. However, **boxd.sh's create command returned faster**: sequential create median **408 ms hosted versus 505 ms local**, and concurrent **589 versus 623 ms**. The separate first-exec medians were **388 versus 18 ms** sequentially and **398 versus 17 ms** concurrently. The local total advantage is substantially explained by avoiding the remote exec/service round trip; this does **not** establish a faster local VM-restore engine. Phase medians are not additive percentiles.

The harness checked every live clone's resource metadata before deletion and probed a guest in each cohort. Linux reported one CPU and roughly 4 GiB on both sides (local MemTotal 4,034,740 KiB; hosted 4,034,284 KiB). Four local clones fit the 16 GiB aggregate guest quota; a separate KVM regression confirms a fifth is rejected and a full 4 GiB snapshot is captured. Host memory was checked before launch (about 25 GiB available).

**Not all limits are matched:** hosted root disk is 100 GiB, local is 2 GiB; host hardware, kernel, installed services, network transport and identity-reset policies differ. Provider host CPU/I/O/cgroup limits are not exposed. Account/state-directory maximums differ but do not bind within these four-machine batches; no provider account limits were changed. Local jailer, CPU/memory limits, fs-verity checks and authenticated overlays remain enabled. Hosted CLI version was 0.2.20; local Firecracker/jailer was 1.17.0. Neither CPU throughput nor memory-intensive workload performance was measured.

Order: hosted sequential, local sequential, local concurrent, hosted concurrent. OS page cache and background load were uncontrolled; with 16 samples p95 is the maximum. All **92 attempts succeeded**. Independent checks verified sample counts, unique IDs, resources, nearest-rank percentiles and create-plus-exec accounting. Raw samples, binary hashes, guest probes and the exact harness are retained under `.amp/in/artifacts/matched4g-*` and `.amp/in/artifacts/compare-4g.py`. The disposable hosted source, clones and snapshot were deleted; existing `full-isle` stayed hibernated and untouched.

## CLI contracts

For brevity, commands below use `kiln-runtime`; the built executable is `target/release/kiln-runtime`. All commands accept `--state-dir PATH`; the parent directory must already exist. The default is `.kiln`. Existing state directories must be owned by the operator and have no group/other permissions. State and guest secrets are stored **unencrypted**.

Output is always JSON (`--json` is accepted for scripting compatibility). Diagnostics go to stderr. Exit codes: 0 success, 1 runtime/transport failure, 2 argument-parser failure. `exec` returns the guest exit code, 124 on command timeout, or 125 for a signal termination without a numeric exit code. Its JSON distinguishes command failure from transport failure. CLI output decodes stdout/stderr as UTF-8 with replacement; this is not a binary file-transfer protocol.

```sh
kiln-runtime create --image IMAGE_MANIFEST --name NAME --allow-unsafe-development
kiln-runtime list
kiln-runtime inspect BOX_ID
kiln-runtime exec BOX_ID --timeout-ms 10000 --cwd /workspace --env KEY=value \
  -- /bin/sh -c 'printf "%s" "$KEY"'
kiln-runtime pause BOX_ID
kiln-runtime resume BOX_ID
kiln-runtime stop BOX_ID
kiln-runtime start BOX_ID
kiln-runtime stop BOX_ID --force
kiln-runtime delete BOX_ID
```

- VM lifetime is independent of the CLI process that launches it.
- A successful launch includes a real guest handshake and initialization, not just a successful VMM spawn.
- `stop` asks the guest to sync and then terminates the VMM. A paused guest is briefly resumed to sync. This is not an OS/application shutdown transaction. `start` cold-boots the retained disk and reinitializes the agent; it does **not** restore memory.
- `stop --force` pins and validates process identity, then kills without consulting the guest/API. It loses guest RAM and unflushed writes. The response records this warning. `delete` likewise force-stops the owned VM and removes its disk generations.
- An exec request holds the box lock, so overlapping operations on that box fail busy. Concurrent launches of different boxes reserve quota durably before doing launch I/O. Allocation waits are bounded; other conflicting lifecycle operations fail rather than race disk writers.
- Limits: 8 allocated boxes / 16384 MiB total guest RAM per state directory, 128–4096 MiB and 1–4 vCPUs per box, 16 snapshot directories, 64 KiB output per stream, 1 MiB protocol frames, bounded guest connections/executions, CLI exec timeout 1–3,600,000 ms. Host overhead is additional. These are development limits, not tenant quotas.
- Copy operations check free space and a 120-second deadline; requests have transport timeouts. No general operation-wide cancellation token or strict I/O deadline exists yet. OS filesystem calls can block. Logs/files have a 5 GiB per-file VMM limit to accommodate full 4 GiB memory snapshots; there is no total disk quota or log rotation yet.

Unsealed disk copies try reflink first. Their byte-copy fallback uses `SEEK_DATA`/`SEEK_HOLE` to skip filesystem holes, scans allocated ranges for zero chunks, preserves logical length, and syncs the new file and parent directory. Filesystems without hole-seek support retain full-file zero scanning. Sealed snapshot disks instead require authenticated reads of every logical byte, as described below. The runtime does not provision host filesystems.

The wire protocol is one length-framed request per vsock connection. Arguments are passed as argv, not interpolated into a shell. Use a shell explicitly if desired. The agent starts each command in a process group, caps output, and cancels that group on timeout or host disconnect. Descendants that deliberately escape the process group are outside this development mechanism; guest cgroups are not implemented. Commands are never automatically retried after an unknown transport outcome.

## Checkpoints restore both memory and disk

```sh
kiln-runtime checkpoint save BOX_ID
kiln-runtime checkpoint list
kiln-runtime checkpoint restore BOX_ID CHECKPOINT_ID --acknowledge-external-state-replay
kiln-runtime checkpoint delete CHECKPOINT_ID
```

Capture holds the box lock, records intent, pauses execution, creates a full Firecracker snapshot, explicitly syncs the disk backing file, copies it while paused, syncs and hashes all artifacts, and publishes the manifest last. A previously running source resumes; a previously paused source stays paused. Snapshot files are private and read-only. Firecracker maps memory from the retained snapshot file; it must remain immutable for the restored VM's lifetime.

A same-box checkpoint preserves process memory and guest filesystem state at the pause boundary, including dirty guest page-cache state in RAM. It does not roll back databases, remote requests, clocks outside the VM, or other external systems. Existing vsock sessions are reset. Restore therefore requires an explicit external-state replay acknowledgement.

Restore verifies checksums and an exact host/VMM compatibility fingerprint before stopping the current VM. It stages a new private writable disk generation, confirms old-process termination, launches from snapshot, and removes the old generation only after guest readiness. A template or another box's checkpoint cannot be used for same-box restore.

Snapshot deletion checks durable box references under the allocation lock. It refuses to delete a snapshot referenced by a box, including a stopped box. Delete the referencing box first. Deleting a box never automatically deletes its backing snapshot. This deliberately conservative policy keeps mapped memory files alive without a background garbage collector.

## Prepared templates are not arbitrary live-workload forks

```sh
kiln-runtime template build --image IMAGE_MANIFEST --allow-unsafe-development
kiln-runtime template list
kiln-runtime clone TEMPLATE_ID --name alpha --allow-unsafe-development
kiln-runtime clone TEMPLATE_ID --name beta --allow-unsafe-development
```

Template build accepts `--memory-mib` and `--vcpus` (defaults: 256 MiB and one vCPU). Use `--memory-mib 4096 --vcpus 1` for the 4 GiB configuration; clones inherit the template's resources. Four such allocated boxes fill the 16 GiB guest-memory quota. This is not a reservation of host RAM: check available memory and allow for VMM/page-cache overhead before increasing guest sizes.

The builder boots a trusted image to the guest agent's pre-initialization barrier, snapshots it, and deletes its temporary VM. Only those template records are cloneable. Every clone receives an independent writable disk, distinct hostname/machine ID, and fresh host entropy before execution is allowed. The kernel boot ID is intentionally inherited from the prepared boot and is **not** a clone identity.

Clone validates snapshot metadata and reserves quota plus a durable snapshot reference under the allocation lock, then releases that lock before verifying artifact integrity. The per-box lock remains held through verification and launch; snapshot deletion refuses the durable reference. A verification failure leaves a failed reservation that counts against quota and pins the snapshot until `kiln-runtime delete BOX_ID`; `kiln-runtime list` reconciles it to stopped. Same-box checkpoint restore checks artifact identity before stopping the source VM. With fs-verity, damaged data blocks can instead fail when read, as described below.

The image author must keep workload daemons, credentials, and userspace random state out of the prepared image. The minimal fixture satisfies that contract; arbitrary Linux images may not. VMGenID and entropy injection do not magically rewrite application-level credentials or cached random values.

### Kernel-enforced snapshot integrity

New snapshots attempt to enable [Linux fs-verity](https://docs.kernel.org/filesystems/fsverity.html) on each immutable artifact before publication. On supporting filesystems, the manifest records a `seals` entry containing the kernel digest and the full-file SHA-256 read from the same descriptor **after** sealing. The original `hashes` remain for compatibility and verification of independent jail copies. No inode/timestamp-only checksum cache is used.

For a sealed artifact, clone/restore requires the expected kernel digest and agreement between the seal's SHA-256 and the manifest's original hash. This check does not scan the artifact contents. The kernel prevents writes and verifies blocks when read or mmap-faulted. A missing seal, substituted file, digest mismatch, permission error, or I/O error fails closed; it does not trigger a successful unsealed fallback. Verity checks content, not path names: the existing private, operator-owned manifest and directory hierarchy remain the trust boundary. This is not protection against an operator who can rewrite that manifest or a compromised host kernel.

Only `ENOTTY`/`EOPNOTSUPP` during initial sealing selects the legacy path; old snapshots with no `seals` also retain full SHA-256 scans on each launch. Permission, capacity, and busy-writer errors fail capture. Filesystems must support the 4 KiB SHA-256 verity configuration. Enabling verity costs a full read/tree build during capture; the runtime additionally computes the compatibility SHA-256 once before publication. Snapshot deletion and durable reference rules are unchanged. Writable guest disks are independent copies and are never sealed.

**Authenticated writable disk copies:** A writable reflink does not inherit fs-verity. Merely checking the source digest before `FICLONE` would leave the destination's blocks unauthenticated; trusting hole metadata could also skip verification of logical zeros. The default copy backend therefore verifies the sealed source descriptor and reads every logical byte through that same descriptor, without reflinks or hole seeking. Already authenticated zero buffers may be omitted from writes to keep the result sparse. This authenticates initial materialization, not future mutable-disk writes. The optional snapshot backend below avoids full materialization while retaining authenticated base reads.

**Shared immutable jail inputs:** Newly captured isolated sealed state/memory files are root-owned 0444, within private snapshot directories. Staging hardlinks only files with the expected seal, root owner, and exact mode; it rechecks the linked inode before exposing it to Firecracker and never chowns it. Firecracker 1.17.0 reads state and maps memory privately (`MAP_PRIVATE`), so guest writes do not change the shared backing inode. Writable disks are never hardlinked. Legacy 0400 or unsealed inputs and cross-filesystem staging retain independent copies with full destination checksums. Rebuild old templates to gain shared staging; do not weaken their permissions manually. Capture still uses separate output names, and snapshot deletion still refuses durable references.

Unlike a full pre-launch scan, fs-verity can detect latent corruption only when the affected block is consumed. A bad read fails with EIO, and a bad mmap page can terminate the VMM with SIGBUS. Do not interpret a successful digest check as an exhaustive disk-health check. Copies/backups do not automatically preserve verity metadata; a restored ordinary file with a claimed seal is rejected. Rebuild the snapshot on the destination rather than silently stripping its metadata.

Cold-image checks and guest/systemd initialization are unchanged. Benchmark JSON records `template_verification` as `sha256` or `fs_verity` for each template artifact. Verification and authenticated disk reads remain within launch timings. The original benchmark tables used full scans; the matched fs-verity comparison below measures digest verification before the subsequent shared-staging and disk-authentication changes.

**Initial digest-path validation:** 52 ordinary tests and all six Ubuntu development KVM lifecycle tests passed. The explicit real-fs-verity test correctly refused to pass on the runner's original unsupported filesystem. On 2026-10-01, operator execution of `.amp/in/verity-validation/run.sh` passed the real sealing/replacement test, all six isolated Ubuntu lifecycle tests (60.02 seconds), and all 184 matched benchmark launches. This used only new dedicated pre-rename verity-test installation paths, a 16 GiB sparse loop image formatted as ext4 with verity, UID 74000–74007 / GID 75000–75007, and a separate cgroup. Root-filesystem features and existing installations were not modified. The script exited 0, reported no remaining child cgroups, and unmounted the test filesystem; fresh checks found no Firecracker processes and no mount at the test mountpoint. The installation, loop image, and results remain for inspection. Do not rerun the one-shot setup over that installation.

To exercise the storage contract on an **already provisioned** supported filesystem:

```sh
KILN_TEST_VERITY_DIR=/path/to/private/verity-test-directory \
  cargo test --release --locked -p kiln-runtime --lib \
  storage::verity::tests::real_seal_enforces_immutability_and_rejects_replacements \
  -- --ignored --exact --nocapture
```

For isolated lifecycle validation, use the isolated test procedure below with `KILN_TEST_STATE_PARENT` on that filesystem and `KILN_TEST_REQUIRE_VERITY=1`. The flag makes a missing seal fail the test rather than silently validating fallback. The storage test separately verifies a known kernel digest, write rejection, independent writable copies, and rejection of both unsealed and differently sealed replacements.

**Shared-input/disk validation:** The follow-up passes 55 ordinary tests, all-feature Clippy, and all six Ubuntu development and isolated KVM scenarios (43.63 and 56.93 seconds respectively, including the cleanup unit test in each run). The isolated clone test proves shared sealed state/memory inode identity, root ownership, write rejection, independent guest identities/disks, and survival of one clone after deleting another. Three real-filesystem tests also pass on disposable Btrfs, including an explicit successful raw reflink followed by an authenticated copy that must instead read the entire logical file, including holes. Boundary tests for the optimized zero scan place a nonzero byte at every position around 32-byte boundaries and partial tails. These checks do not simulate latent block-device corruption; they verify sealing, replacement rejection, read coverage, and materialization semantics.

The privileged run uses separate dedicated pre-rename sealed-link installation paths, UID 76000–76007 / GID 77000–77007, a 512 MiB Btrfs unit-test image and a 16 GiB ext4-verity lifecycle/benchmark image. It does not change host-root filesystem features or previous installations. Build the fault-injection test binaries without root, then install and run the reviewed unit-test binary as root with `KILN_TEST_VERITY_DIR` set to a private Btrfs test directory and `--ignored --test-threads=1 --nocapture`; all three ignored storage/isolation tests must pass. Do not treat an unsupported-filesystem fallback as a successful test.

### Optional authenticated disk overlays

For a **new isolated state store**, add `"disk_backend": "snapshot"` to the root-owned isolation policy. The default remains `"copy"` (also used when the field is absent). The policy is pinned when the store is created; do not change it underneath existing boxes. Development mode, cold creates, unsealed snapshots, and old binaries are not silently converted to overlays. A snapshot-policy clone/restore requires a sealed base or fails closed.

Prerequisites: Linux loop devices with atomic `LOOP_CONFIGURE`, the device-mapper persistent snapshot target, trusted `/usr/sbin/dmsetup`, an fs-verity-enabled filesystem, and a state mount that permits block device nodes (not `nodev`). Existing isolated UID/GID/cgroup requirements still apply. This is an experimental root-operated backend, not a privileged interface to expose directly to tenants. The runtime does not provision filesystems or change host device permissions.

Each clone gets a private persistent COW file over the immutable sealed disk. Buffered read-only loop I/O reads the base through fs-verity; direct I/O and partition scanning are disabled. A second AUTOCLEAR loop backs the writable COW file. Firecracker sees only the resulting jail-local 0600 block node, owned by that box's assigned UID/GID. It cannot traverse to the base, COW file, or other layers. State and memory retain their existing sealed private-mapping behavior.

The COW file is sparse, with logical capacity sufficient for every 4 KiB disk chunk plus metadata; supported base disks are sector-aligned and at most 2 GiB. A free-space check is not a reservation. Operators must budget actual pool capacity across boxes and snapshots. ENOSPC, invalid/overflowed COW state, or backing I/O failure can lose mutable data; this is not a disk quota, replicated store, or authenticated mutable-write log. A detected unhealthy mapper prevents readiness/inspection from reporting a healthy VM. Force-stop/delete remain available, but cleanup refuses a mapping whose ownership cannot be proved.

Layers live outside jail generations. Stop/start reuses the same layer; mapping recreation checks the sealed base, both backing-file identities and sizes, exact UUID/table, and loop identities/flags. Device-mapper helpers inherit open loop descriptors and a per-layer lock, preventing a manager crash from releasing devices or permitting deletion while its helper still runs. Cleanup validates ownership before removal and never forces or defers device removal. Busy or foreign mappings preserve state for diagnosis; never remove backing files manually while a mapper may reference them.

Checkpoint capture pauses the VM and flattens its composed block disk to an ordinary sealed snapshot. Restore prepares a new layer before stopping the current VM; successful restore deletes old layers only after readiness. Published layer manifests independently pin their base snapshots, including layers left by interrupted restores. `checkpoint delete` refuses those references even when they differ from the current memory snapshot. Box deletion detaches all owned layers before deleting files. Mapping recreation has been tested by explicit detach/remap, not by power-cycling this host.

The existing `disk_copy` benchmark phase measures COW-file publication for this backend; device allocation and validation are included in `vmm_start` alongside jail staging. Successful overlay launches report `disk_copy: "Snapshot"`. Cold creates still report the ordinary copy method. End-to-end timings include both phases and the first successful command.

## Crash recovery and its limits

`kiln-runtime inspect` and `kiln-runtime list` reconcile persisted records with actual processes. Process identity includes PID, Linux start ticks, executable, and working directory; pidfds pin termination targets. A reused PID is refused, never blindly killed. A process spawned before its PID was recorded is found by its unique run directory and executable. Multiple matching VMMs are a hard error.

- Interrupted launch: adopt an initialized running VM, or terminate only the identified incomplete VMM and leave its disk stopped for explicit `start`/restore. A pre-copy failure may require deleting the failed box and recreating it.
- Interrupted capture of a running source: reconciliation resumes the paused source. An incomplete snapshot has no published manifest and is not listed as usable. Its directory can remain after abrupt death and counts against the snapshot limit. Once no capture is active, `checkpoint delete ID` also removes an unpublished directory whose ID you obtained from the private `snapshots/` directory.
- Restore failure: `restore.previous.json` and the original disk generation remain until a successful restore. Recovery does not automatically replay a restore request or switch back to that old generation. Inspect, then explicitly retry a verified checkpoint restore. Retained generations from interrupted attempts can remain until box deletion; do not manually unlink files while a VMM could use them.
- Unresponsive API/guest: use `stop --force`, then inspect. Never delete a state directory containing a live VM.

Recovery runs on explicit commands, not an always-on supervisor. Abrupt process death releases kernel locks, but dropping an async Rust future is not transactional rollback. The fault suite verifies selected process-crash boundaries, not power-loss behavior or every storage/ENOSPC/cancellation permutation. Broader fault coverage and cleanup are still required before production use.

## Reproduce launch measurements

```sh
cargo build --locked --release -p kiln-runtime --bin kiln-runtime
kiln-runtime benchmark --image IMAGE_MANIFEST --template TEMPLATE_ID \
  --samples 30 --concurrency 1 --allow-unsafe-development
kiln-runtime benchmark --image IMAGE_MANIFEST --template TEMPLATE_ID \
  --samples 16 --concurrency 4 --allow-unsafe-development
```

The JSON includes each sample, failures, nearest-rank p50/p95/p99, launch-to-ready and first-exec timing, actual copy method, actual guest RAM/vCPU allocation, versions, host fingerprint, concurrency, and cache assumptions. Cold launches use the supplied template's RAM/vCPU allocation, just like restores; build a 4 GiB template to measure both paths at 4 GiB. It returns a failure status if any measured operation fails. Measurement starts at the runtime call, includes validation/allocation waits/disk preparation/VMM/guest initialization, and ends after `/bin/printf kiln-ready` returns the expected bytes and exit code. It excludes benchmark program startup and cleanup. Only boxes with this benchmark invocation's unique name prefix are removed.

Each sample includes non-overlapping wall-clock `phases_ms`: image or snapshot verification (including validation/preflight), allocation wait, durable reservation, disk copy, boot preflight, VMM spawn/API readiness, VMM configuration/snapshot load, guest readiness, initialization, and launch commit. Timings are task-local, so concurrent requests do not share counters. Failed attempts include completed/failed phase timings but no successful `total_ms`; phases that were never reached are absent. Phase sums exclude small bookkeeping gaps and the separately reported first exec.

"Cold boot" means a fresh guest kernel boot, **not** an evicted host filesystem cache. No host cache eviction is performed. Firecracker uses file-backed demand paging. For legacy/unsealed snapshots the runtime reads every artifact for integrity verification before restore; those reads are included in the measurements and populate the cache. Sealed snapshots check kernel digests instead; consult `template_verification` rather than assuming either path.

Matched before/after measurements on the development runner, release host/guest, Firecracker 1.17.0, Ryzen 7 7840HS, host Linux 7.0.0-34-generic, ext4 without reflink, 256 MiB / 1 vCPU / 128 MiB disk. Both runs use the same image and prepared snapshot; the before run includes timing instrumentation but neither optimization. Each row has the indicated sample count per implementation:

| Mode | Samples | Concurrency | Before p50 / p95 | After p50 / p95 |
| --- | ---: | ---: | ---: | ---: |
| Cold boot | 30 | 1 | 844 / 888 ms | 807 / 851 ms |
| Template restore | 30 | 1 | 281 / 288 ms | 245 / 257 ms |
| Cold boot | 16 | 4 | 836 / 881 ms | 805 / 852 ms |
| Template restore | 16 | 4 | 537 / 973 ms | 277 / 325 ms |

All 184 launches succeeded. Sequential template disk-copy median fell from 41 to 13 ms; concurrency-four allocation-wait median fell from 340 to 24 ms. Full snapshot verification remains about 196 ms sequential and dominates the remaining template latency. Guest kernel readiness still takes about 602 ms on cold boots. These are small local samples with uncontrolled cache/load, not an SLA or a matched comparison with boxd.sh.

Raw results are retained locally as `.amp/in/artifacts/perf-{before,after}-{sequential,concurrent}.json`, not committed. No sub-10ms end-to-end claim is made. A disposable-file `FS_IOC_ENABLE_VERITY` probe returned `EOPNOTSUPP` on this host; kernel feature support alone does not mean the filesystem has verity enabled. No filesystem features were changed. The optional fs-verity path described above is subsequent work, not part of these measurements. Reflink-capable storage still requires evaluation before adding a custom memory pager. Metadata-only checksum caches are not implemented.

### Ubuntu isolated launch measurements

Measured 2026-10-01 on `ser7`: Ubuntu Minimal 24.04 release-20260905, Linux guest 6.1.155, Firecracker/jailer 1.17.0, release host/guest binaries, Ryzen 7 7840HS, 256 MiB / 1 vCPU, 2 GiB root disk, host ext4 sparse-copy fallback (`Copy`, not reflink). Every operation uses the isolated profile, private jail files, full artifact checksums, and first-command completion. OS page cache is uncontrolled; no cache eviction was performed. These are local samples, not an SLA or a comparison against boxd.sh.

| Mode | Samples | Concurrency | p50 | p95 | p99 | Failures |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Cold boot | 30 | 1 | 2,800 ms | 2,836 ms | 2,841 ms | 0 |
| Template restore | 30 | 1 | 2,037 ms | 2,054 ms | 2,064 ms | 0 |
| Cold boot | 16 | 4 | 3,374 ms | 3,452 ms | 3,452 ms | 0 |
| Template restore | 16 | 4 | 2,553 ms | 3,900 ms | 3,900 ms | 0 |

All 92 sample records, success counts, and nearest-rank percentiles were independently checked against the raw JSON in `.amp/in/artifacts/ubuntu-isolated-validation.log`. Each sample's phases sum to within 0.18 ms of its launch time, with first-exec latency accounted for separately. The original reports are also retained in the pre-rename Ubuntu validation installation.

Sequential template phase medians: snapshot verification 1,081 ms; guest initialization/systemd startup 523 ms; private disk copy 208 ms; jail input staging/VMM startup 198 ms; VMM configuration/snapshot load 5 ms; guest readiness 2 ms; first exec 4 ms. Phase medians describe different samples and should not be summed into a percentile. Snapshot verification still scans the entire disk/state/memory, and the template intentionally predates systemd startup. These costs, rather than the snapshot-load API alone, dominate end-to-end readiness.

At concurrency four, two slow restore samples spent 1,603–1,621 ms in allocation wait, and another spent 1,590 ms in VMM configuration/snapshot load. The cause of that stall is not established by these phase timings. Restore improved median latency but had a worse p95 than cold boot; with 16 samples, nearest-rank p95 and p99 both select the maximum. Do not infer a stable tail-latency SLA or attribute the difference versus BusyBox solely to jailer: image size, guest initialization, and isolation changed together.

Known reporting caveat: nested `versions.isolated_ready` is `false` because the benchmark embeds `host::check()`, a generic probe with no isolation policy. It is not the launch profile. The reports explicitly record `profile: "isolated"`; the preceding policy-aware doctor check passed, lifecycle tests inspected actual jail/cgroup state, and each benchmark launch enforced the runtime isolation checks. The raw metadata has not been rewritten.

### Matched fs-verity launch measurements

Measured 2026-10-01 on the same runner and Ubuntu image, 256 MiB / 1 vCPU / 2 GiB disk, release binaries, isolated profile. Both implementations use the **same sealed template** and the **same new loop-backed ext4 filesystem**, still using sparse `Copy`, not reflink. The prior binary ignores the additive seal metadata and scans full SHA-256 hashes. The new binary records `fs_verity` for all three template artifacts and checks kernel digests. This comparison isolates the runtime verification change rather than comparing the old host filesystem against the new loop filesystem.

| Mode | Samples per implementation | Concurrency | Before p50 / p95 | After p50 / p95 |
| --- | ---: | ---: | ---: | ---: |
| Cold boot | 30 | 1 | 2,931 / 2,998 ms | 2,917 / 2,956 ms |
| Template restore | 30 | 1 | 2,145 / 2,181 ms | 1,045 / 1,060 ms |
| Cold boot | 16 | 4 | 3,506 / 3,576 ms | 3,532 / 4,094 ms |
| Template restore | 16 | 4 | 2,724 / 2,760 ms | 1,594 / 1,619 ms |

All 184 launches succeeded. Sample counts, unique indices, nearest-rank percentiles, shared image/template/host, verification methods, and launch-plus-exec accounting were independently checked from `.amp/in/artifacts/verity-isolated-validation.log`. Phase sums differ from launch times by at most 0.126 ms. Root-owned original reports remain in the pre-rename verity validation installation, outside the unmounted pool.

Sequential template p50 fell 51.3% (2.05× faster); concurrency-four p50 fell 41.5% (1.71× faster). Sequential snapshot-verification phase median fell from 1,083.25 to 6.71 ms, including metadata/preflight overhead, not just the kernel ioctl. Remaining sequential phase medians are guest initialization/systemd 519 ms, private disk copy 258 ms, jail input staging/VMM startup 214 ms, and first exec 4 ms. At concurrency four disk-copy median is 597 ms and guest initialization is 635 ms. Phase medians are not additive percentiles.

Cold boot is not optimized by this change: its sequential median stayed near 2.9 seconds and concurrent p95 worsened in this sample. No causal explanation for that tail difference is established. Sample order was before-sequential, after-sequential, after-concurrent, before-concurrent; OS cache/load were uncontrolled, with no cache eviction. With 16 observations, p95 and p99 both select the maximum. These results are not a cold-cache SLA, a boxd.sh comparison, or evidence that writable-disk copying and systemd startup have been removed. The nested readiness-metadata caveat above also applies to these reports.

### Matched sealed-sharing and disk-copy measurements

Measured 2026-10-01 on the same runner/image configuration, isolated profile, release binaries, 256 MiB / 1 vCPU / 2 GiB disk. The preserved fs-verity binary and final implementation use the same newly captured sealed template on the same disposable ext4-verity pool. Both report `fs_verity` for each artifact and `Copy` for writable disks. The new implementation shares eligible sealed jail inputs, authenticates every logical snapshot-disk byte, and uses 32-byte zero comparisons after reading. It does not implement writable reflinks from sealed snapshots.

| Mode | Samples per implementation | Concurrency | Before p50 / p95 | Final p50 / p95 |
| --- | ---: | ---: | ---: | ---: |
| Cold boot | 30 | 1 | 2,954 / 2,989 ms | 2,632 / 2,675 ms |
| Template restore | 30 | 1 | 1,070 / 1,116 ms | 966 / 1,016 ms |
| Cold boot | 16 | 4 | 3,540 / 3,577 ms | 3,257 / 3,316 ms |
| Template restore | 16 | 4 | 1,612 / 1,873 ms | 1,513 / 1,548 ms |

All 184 final-comparison launches succeeded. Sequential template median improved 9.8%; concurrency-four median improved 6.2%. Sequential template staging/VMM-start median fell from 223 to 33 ms; authenticated disk-copy median rose from 278 to 368 ms. Guest initialization/systemd remains about 518 ms. At concurrency four, disk copy is 762 ms and guest initialization 611 ms. The shared zero-scan optimization also reduced cold-boot disk-copy median from 659 to 351 ms, with cold end-to-end p50 improving 10.9%. Phase medians describe different samples and are not additive percentiles.

The first implementation was slower: before optimizing the zero scan, its matched sequential template p50 rose from 1,043 to 1,252 ms, despite faster jail staging. Those 184 initial samples are retained separately; they are not pooled with the final results. The optimized scan still reads and checks every byte, including partial tails; it changes CPU scanning cost, not the authentication boundary.

Raw reports are `.amp/in/artifacts/sealed-links-final-{before,after}-{sequential,concurrent}.json`; initial reports omit `final-`. Original root-owned reports remain in the pre-rename sealed-link validation installation. All 368 initial/final samples were independently checked for counts, unique indices, failures, copy/verification methods, matching image/template/host within each comparison, nearest-rank percentiles, and launch-plus-exec accounting. Final phase sums differ from launch time by at most 0.085 ms. The privileged scripts exited 0; both test filesystems are unmounted, with no child cgroups or Firecracker processes remaining. Installations and loop images are retained for inspection.

Order in each comparison was before-sequential, after-sequential, after-concurrent, before-concurrent. Cache and background load were uncontrolled, with no cache eviction. These are small local samples, not a stable tail-latency SLA or a matched boxd.sh comparison. The nested readiness-metadata caveat above still applies. Removing the remaining full disk materialization requires an authenticated base/overlay architecture; avoiding systemd startup requires a separate safe guest-initialization design.

### Matched authenticated-overlay measurements

Measured 2026-10-01 on `ser7`, same Ubuntu Minimal 24.04 image, isolated release binaries, 256 MiB / 1 vCPU / 2 GiB disk. The preserved optimized-copy binary and overlay implementation use one sealed template on the same new ext4-verity pool. Both authenticate immutable artifacts. The benchmark-only policy changes between cohorts only after all boxes have been deleted; this is not a supported migration of a live store.

| Mode | Samples per implementation | Concurrency | Copy p50 / p95 | Overlay p50 / p95 |
| --- | ---: | ---: | ---: | ---: |
| Cold boot | 30 | 1 | 2,597 / 2,651 ms | 2,596 / 2,637 ms |
| Template restore | 30 | 1 | 945 / 970 ms | **714 / 740 ms** |
| Cold boot | 16 | 4 | 3,262 / 3,312 ms | 3,308 / 3,516 ms |
| Template restore | 16 | 4 | 1,508 / 1,531 ms | **894 / 1,024 ms** |

All 184 launches succeeded. Template median improved **24.5% sequentially and 40.7% at concurrency four**. Sequential disk preparation fell from 343 to 27 ms; staging/VMM start rose from 34 to 82 ms because it now includes mapper allocation and validation. Guest initialization rose from 521 to 556 ms in this sample and dominates the remaining path. Concurrency-four disk preparation fell from 765 to 50 ms, with staging at 112 ms and guest initialization at 651 ms. These are separate phase medians, not additive percentiles. Cold creates still copy their disks and are not optimized here; their concurrent tail worsened in this run.

Order: copy sequential, overlay sequential, overlay concurrent, copy concurrent. Cache and background load were uncontrolled; no eviction was performed. The 16-sample p95/p99 select the maximum. Results do not establish a tail-latency SLA or a matched advantage over boxd.sh. Its earlier hosted CLI measurements use different resources, infrastructure, and timer boundaries. Raw reports are `.amp/in/artifacts/overlays-{before,after}-{sequential,concurrent}.json`, with root-owned originals in the pre-rename overlay validation installation. Counts, unique indices, failures, shared host/image/template, actual backend, nearest-rank percentiles, phase non-overlap, and launch-plus-exec accounting were independently checked.

The isolated Ubuntu suite passed all seven checks on both backends (67.75 seconds overlay, 63.71 seconds copy), including cleanup, block-node type/ownership, private writes, layer persistence across jail generations, independent disk-base references, coordinated capture/restore, and crashes after layer publication/mapping. Real storage tests exercise authenticated base reads, asymmetric private writes, detach/remap persistence, flattening, substituted COW-file rejection, bad seals, and refusal to remove a foreign mapping. They do not simulate host power loss, latent physical corruption, or exhaust the whole pool. A test-fixture teardown encountered transient `EBUSY` after creating its synthetic foreign device; fixture removal uses bounded `dmsetup --retry`, never forced or deferred removal. Runtime ownership checks and fail-busy cleanup remain unchanged.

Guest startup remains unmodified. A separate post-benchmark `systemd-analyze` trace reported 521 ms of userspace boot, with the agent ordered after `basic.target`. The chain includes tmpfiles, sysusers, and journal-catalog updates; udev, linker-cache, console, snapd, and maintenance units also start. Moving the shared template past initialized systemd would violate the current identity/entropy contract. A faster guest profile needs an explicit compatibility choice rather than silently dropping systemd services or cloning their cached identity.

Final validation also passed 60 ordinary tests in three consecutive workspace runs, all-feature Clippy, three consecutive real overlay tests, and all seven Ubuntu development checks (47.98 seconds). The existing lock-lifetime test now allows a bounded wait for parallel fork/exec helpers to close temporarily inherited CLOEXEC descriptors; the exclusive-lock assertion remains unchanged. The published local CLI build excludes fault injection. The disposable pool is unmounted, with no owned mapper/loop devices, VMMs or child cgroups left. Root-owned installation files and the loop image remain for inspection; existing installations and the host root filesystem were not modified.

## Verification and remaining milestones

The README lists exact commands. The normal suite checks protocol limits, guest initialization/execution/cancellation/output, API faults/timeouts, image integrity, file ownership/copy/locking, process identity, CLI rejection, and percentile calculation. Opt-in real-KVM tests verify:

1. Cold create, exec, exit codes, stop/start disk persistence, immediate post-resume execution, paused capture semantics, memory/disk restore (including reviving a killed process), and corruption rejection before source termination.
2. Independent clone identities/disks/lifetimes and prevention of backing-memory deletion.
3. CLI crashes before spawn, after spawn, after PID persistence, after pause, and after manifest publication, with safe process reconciliation and no partial snapshot publication.
4. Simultaneous starts and forced termination with an inaccessible API socket.
5. Per-request, non-overlapping phase timings during concurrent benchmarks and cleanup of measured boxes.
6. A clone blocked in verification releases the allocation lock while its quota/reference remain durable, prevents snapshot deletion, and rejects corrupt contents without spawning a VMM.

All six scenarios also pass with jailed launch and cgroup enforcement for both BusyBox and Ubuntu on the development runner. Still not implemented: Docker support, public API, PostgreSQL operations/idempotency, network/SSH/preview access, web UI, SDK, fleet scheduling, or hosted tenancy. Privileged host setup and validation require separate approval.

## Experimental isolated profile

The operator runs the CLI as root with root-owned, canonical configuration, binaries, images, and state paths. Every ancestor must be root-owned and not group/other writable; a state directory under a user's home or `/tmp` is rejected. Use a dedicated empty state store. Its `isolation.json` pins the policy; subsequent lifecycle commands load it automatically. Existing development boxes/templates cannot be converted or restored into the isolated profile.

The policy reserves eight UID/GID pairs, one per allocated box (including stopped boxes). **Reserve these IDs exclusively on the host**, outside login/service/subuid/subgid allocations, and do not use the same range in another live state store. This is an operator prerequisite, not an identity allocator for a shared fleet. The privileged CLI is a trusted operator tool, not a restricted sudo interface for tenants.

Every launch uses foreground jailer, a fresh jail generation, cgroup v2, normal Firecracker seccomp, empty supplementary groups, and no guest NIC. Start copies ordinary retained disks to a new generation, or exposes the retained private overlay there. Restore/clone share kernel-sealed root-owned state/memory inodes where eligible, otherwise staging independent checksum-verified copies. No writable disk inode is hardlinked between boxes; guest memory writes use private mappings. Capture uses separate temporary output names, never overwriting a restored VM's mapped input. Disk materialization and fallback staging increase latency and disk use; development benchmark numbers do not describe this profile.

Limits are CPU quota = configured vCPUs, `memory.max` = (2 × guest MiB + 128) MiB, swap = 0, `pids.max` = 64, open files = 256, plus the 5 GiB per-file limit. The memory overhead allowance needs validation with larger/dirty workloads; OOM must fail the launch rather than bypass limits. Before reporting readiness, and when inspecting a running/paused VM, the runtime checks the jail root, executable, cgroup membership/limits, every thread's UID/GID, empty groups, zero effective capabilities, `NoNewPrivs=1`, and `Seccomp=2`.

Stop confirms process termination and removes the empty generation cgroup. Recovery recognizes only the owned jailer → jailed Firecracker exec transition. A manager crash before API creation stops the incomplete launcher. Force-stop does not require working VMM/guest APIs or a successful launch preflight. Interrupted filesystem cleanup and orphan generations still have the limitations described above; retain state until all VM processes have been stopped. There is no total disk quota, always-on reconciliation, networking, or hosted multi-tenant security claim.

### Proposed disposable host setup — execute only after explicit approval

These commands install separate test binaries/images/configuration and create a test cgroup; they do not replace system Firecracker, install a service, or modify networking/filesystem features. They assume the fixture has already been built and the chosen numeric IDs are reserved. Refuse existing test locations rather than overwriting another installation. The root cgroup's `cpu`, `memory`, and `pids` controllers must already be enabled; arrange that with the host administrator if missing. The CLI does not enable them automatically.

```sh
# Read-only prerequisites. Review these and UID/GID reservations first.
cat /sys/fs/cgroup/cgroup.subtree_control
getent passwd | awk -F: '$3 >= 70000 && $3 <= 70007'
getent group | awk -F: '$3 >= 71000 && $3 <= 71007'
cat /etc/subuid /etc/subgid

# Run from the reviewed checkout. Build as the ordinary user, never with sudo.
cargo build --release --locked -p kiln-runtime --bin kiln-runtime
test ! -e /opt/kiln-test && test ! -e /var/lib/kiln-test && \
  test ! -e /etc/kiln-test.json && test ! -e /sys/fs/cgroup/kiln-test || exit 1
sudo install -d -o root -g root -m 0755 /opt/kiln-test
sudo install -d -o root -g root -m 0700 /var/lib/kiln-test \
  /var/lib/kiln-test/image /var/lib/kiln-test/states
sudo install -o root -g root -m 0755 target/release/kiln-runtime \
  .tools/firecracker-1.17.0/firecracker .tools/firecracker-1.17.0/jailer /opt/kiln-test/
sudo install -o root -g root -m 0444 images/output/fixture-kiln/image.json \
  images/output/fixture-kiln/vmlinux images/output/fixture-kiln/rootfs.ext4 /var/lib/kiln-test/image/
sudo mkdir /sys/fs/cgroup/kiln-test
printf '+cpu +memory +pids\n' | sudo tee /sys/fs/cgroup/kiln-test/cgroup.subtree_control >/dev/null
sudo install -o root -g root -m 0600 /dev/null /etc/kiln-test.json
sudo tee /etc/kiln-test.json >/dev/null <<'JSON'
{
  "firecracker": "/opt/kiln-test/firecracker",
  "jailer": "/opt/kiln-test/jailer",
  "cgroup_parent": "kiln-test",
  "uid_base": 70000,
  "gid_base": 71000
}
JSON
sudo env PATH=/opt/kiln-test:/usr/sbin:/usr/bin:/sbin:/bin \
  /opt/kiln-test/kiln-runtime doctor --isolation-config /etc/kiln-test.json
```

No host accounts are created by these commands. Account/service/subordinate-ID reservations must be managed separately. Test setup is ephemeral across reboot for cgroups; production service provisioning remains future work.

### Validate the same real lifecycle suite under jailer

After approval/setup, build the fault-injection test harness without privileges, copy the reviewed outputs into the trusted installation, and execute only those binaries as root. Tests create disposable state stores under the supplied root-owned parent. **Run serially and do not run other stores using this UID range concurrently.** Missing prerequisites fail, rather than skip, this opt-in run.

```sh
test_bin=$(cargo test --release --locked -p kiln-runtime --features fault-injection \
  --test lifecycle --no-run --message-format=json | python3 -c '
import json, sys
for line in sys.stdin:
    item = json.loads(line)
    if item.get("reason") == "compiler-artifact" and item.get("target", {}).get("name") == "lifecycle" and item.get("executable"):
        print(item["executable"])
')
test -x "$test_bin" || exit 1
sudo install -o root -g root -m 0755 "$test_bin" /opt/kiln-test/lifecycle-tests
sudo install -o root -g root -m 0755 target/release/kiln-runtime /opt/kiln-test/kiln-runtime-test
sudo env PATH=/opt/kiln-test:/usr/sbin:/usr/bin:/sbin:/bin \
  KILN_TEST_RUNTIME=/opt/kiln-test/kiln-runtime-test \
  KILN_TEST_IMAGE=/var/lib/kiln-test/image/image.json \
  KILN_TEST_ISOLATION_CONFIG=/etc/kiln-test.json \
  KILN_TEST_STATE_PARENT=/var/lib/kiln-test/states \
  /opt/kiln-test/lifecycle-tests --ignored --nocapture --test-threads=1
# Restore the local CLI build without crash failpoints afterward.
cargo build --release --locked -p kiln-runtime --bin kiln-runtime
```

The suite exercises persistence, killed-process memory restoration, independent clones, concurrent starts, force-stop, and manager crashes. Isolated runs additionally inspect actual process identity, jail root, seccomp/capabilities, cgroup limits, private backing inodes, distinct host UIDs/GIDs, and a guest with only loopback and no management socket.

On 2026-10-01, the privileged retry on `ser7` passed all six tests in 18.10 seconds with Firecracker/jailer 1.17.0 and the trusted fixture. The first run had failed on namespace-relative procfs path checks and left six test VMMs/cgroups after failed cleanup. The retry verified and stopped those processes before testing the correction: followed device/inode ownership checks retain PID/start-time protection, and the harness preserves unfinished state if cleanup fails. The privileged script reported no remaining child cgroups; a subsequent unprivileged process check found no processes in the reserved test UID/GID range. The test installation remains available for inspection. This is single-host lifecycle evidence, not isolated launch latency characterization or adversarial security validation. Do not rerun the one-shot setup over an existing installation.

For manual use after validation, initialize a separate empty state store with `--state-dir /var/lib/kiln-test/manual --isolation-config /etc/kiln-test.json create --profile isolated --image /var/lib/kiln-test/image/image.json`. Later commands only need that `--state-dir`; create/template build/clone/benchmark also require `--profile isolated`. Do not run the manual store concurrently with the test suite. To remove test setup, first delete every test box and verify the test cgroup contains no children or processes; only then remove the dedicated test paths. Never recursively remove live state.

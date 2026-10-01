# Single-host runtime

## Safety and prerequisites

This is a trusted-workload prototype. The default development profile runs Firecracker as the invoking user with its normal seccomp policy, **without jailer, per-VM host cgroups, or guest networking**. Do not run that profile as root. Guest vCPU/RAM configuration and local inventory quotas are not complete host resource isolation. The experimental isolated profile below passes the privileged lifecycle suite on the development runner with the trusted fixture. Neither profile is advertised as production-ready or approved for customer/adversarial workloads.

Linux x86_64, accessible KVM, cgroup v2, and matching Firecracker/jailer **1.17.0** binaries on `PATH` are required. `box doctor` is read-only and reports `isolated_ready: false` unless an explicit, valid `--isolation-config` passes privileged preflight. That flag reports configuration readiness, not a completed VM isolation test. There is no silent unjailed fallback.

The runner originally had 1.16.0. Crash testing reproduced its permanent vsock failure after bare pause/resume. [Firecracker 1.17.0 fixes this](https://github.com/firecracker-microvm/firecracker/releases/tag/v1.17.0) in PR #6100. `scripts/fetch-firecracker.sh` pins and verifies the official x86_64 release archive SHA-256 and installs into an explicitly selected local directory. No system binaries, services, users, cgroups, firewall rules, or filesystems are provisioned automatically.

The fixture uses a checksum-pinned Linux 6.1.155 kernel from Firecracker's v1.15 CI artifacts, a statically linked Rust musl guest agent, and locally supplied static BusyBox. Its `inputs.json` records source URL and component hashes. `mkfs.ext4 -d` populates a 128 MiB root filesystem without host mounts or root. This is a test image, not a supported general-purpose distribution. Build instructions are in the [README](../README.md).

## Ubuntu image contract

`bash images/build-ubuntu.sh OUTPUT_DIRECTORY [ROOTFS_TARBALL]` builds an experimental Ubuntu Minimal 24.04 amd64 guest. The optional local tarball must match the same pinned SHA-256 as the downloaded input. The builder pins [release-20260905](https://cloud-images.ubuntu.com/minimal/releases/noble/release-20260905/) (`094dc0afc6ded1c3e5ce71f7d0b48d5db922155097bc8fb1ec19db2ebdd17ece`) and the fixture's Linux 6.1.155 kernel. It builds static `box-init`/`box-guest` binaries, preserves upstream numeric ownership with fakeroot, and uses `mkfs.ext4 -d` to create a 2 GiB disk without root, host mounts, chroot, or package scripts. `inputs.json` records upstream URLs/checksums and builder/guest binary hashes. Inputs are repeatable; filesystem timestamps/UUIDs mean this is not a bit-for-bit reproducible image claim. Existing output images are never overwritten.

Included tools are the upstream systemd, Bash, Python 3, curl, and apt packages. There is no guest NIC, package-download connectivity, SSH server access, Docker, or compiler toolchain. The custom `boxd.target` starts basic systemd services and the agent; cloud-init is disabled and network/SSH units are masked. No host service is installed. Security updates require explicitly updating/revalidating the pinned input and rebuilding; an automatic image update/release process remains future work.

Ubuntu manifests select `boot_mode: "systemd"`; missing `boot_mode` retains the fixture's `/sbin/init` behavior. Only these fixed boot modes are accepted, not arbitrary kernel arguments. Ubuntu boots `/sbin/box-init` as PID 1, mounts guest pseudo-filesystems, and waits at a host-vsock initialization barrier. It never executes workloads. Templates capture this state **before systemd starts**, avoiding systemd/D-Bus caching a shared template machine ID. On initialization the guest mixes 32 bytes of host entropy into the kernel pool with `RNDADDENTROPY`, explicitly reseeds the CRNG, sets the hostname (including `/etc/hostname`), writes `/etc/machine-id`, and creates a boot-local handoff marker. The bootstrap closes its listener before acknowledging, then execs systemd. The host waits up to 30 seconds for the initialized systemd-managed agent; initialization is never replayed to compensate for a lost response.

The service requires the valid handoff marker and resumes initialized after a service restart. `/run` is recreated on cold boot, so a normal stop/start provisions again using the existing box ID; same-box checkpoint restore resumes captured service state. The prepared kernel boot ID is deliberately shared by clones and is not the per-instance identity. SSH keys, machine ID, cloud-init state, persistent journals, and the stored random seed are cleared in the image. Workloads and credentials added afterward must not be promoted into shared templates.

On `ser7`, all six development-mode lifecycle tests passed against this Ubuntu image, including Python output, systemd-run service execution, systemd's D-Bus machine ID matching each independent clone, agent restart without reinitialization, disk persistence, memory restore, and crash recovery. The freshly rebuilt BusyBox fixture also passes the shared suite. Image checks confirmed preserved root/shadow ownership, empty initial machine ID, required binaries/service configuration, refusal to overwrite an image, and rejection of a wrong input checksum. On 2026-10-01 the privileged Ubuntu suite also passed all six tests in 54.51 seconds, followed by 92 successful isolated benchmark launches. The script exited 0 without remaining child cgroups, and a fresh process check found no Firecracker processes. The separate installation/results remain available for inspection. Earlier fixture timings must not be reported as Ubuntu results.

Run the shared suite with `BOXD_TEST_IMAGE` set to the Ubuntu manifest; all other development/isolated test flags below are unchanged. For measurement, build a template from that same image and use `box benchmark` with 30 samples at concurrency 1 and 16 at concurrency 4. Benchmark JSON now records `profile` alongside cache assumptions, versions, per-sample phases/failures, and first-command latency. The 2 GiB disk's integrity scan, private copies, and post-resume systemd startup remain inside the measured path; this bootstrap template is not a snapshot of an already running systemd system.

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

### Ubuntu isolated launch measurements

Measured 2026-10-01 on `ser7`: Ubuntu Minimal 24.04 release-20260905, Linux guest 6.1.155, Firecracker/jailer 1.17.0, release host/guest binaries, Ryzen 7 7840HS, 256 MiB / 1 vCPU, 2 GiB root disk, host ext4 sparse-copy fallback (`Copy`, not reflink). Every operation uses the isolated profile, private jail files, full artifact checksums, and first-command completion. OS page cache is uncontrolled; no cache eviction was performed. These are local samples, not an SLA or a comparison against boxd.sh.

| Mode | Samples | Concurrency | p50 | p95 | p99 | Failures |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Cold boot | 30 | 1 | 2,800 ms | 2,836 ms | 2,841 ms | 0 |
| Template restore | 30 | 1 | 2,037 ms | 2,054 ms | 2,064 ms | 0 |
| Cold boot | 16 | 4 | 3,374 ms | 3,452 ms | 3,452 ms | 0 |
| Template restore | 16 | 4 | 2,553 ms | 3,900 ms | 3,900 ms | 0 |

All 92 sample records, success counts, and nearest-rank percentiles were independently checked against the raw JSON in `.amp/in/artifacts/ubuntu-isolated-validation.log`. Each sample's phases sum to within 0.18 ms of its launch time, with first-exec latency accounted for separately. The original reports are also retained under `/var/lib/boxd-ubuntu-test/results/`.

Sequential template phase medians: snapshot verification 1,081 ms; guest initialization/systemd startup 523 ms; private disk copy 208 ms; jail input staging/VMM startup 198 ms; VMM configuration/snapshot load 5 ms; guest readiness 2 ms; first exec 4 ms. Phase medians describe different samples and should not be summed into a percentile. Snapshot verification still scans the entire disk/state/memory, and the template intentionally predates systemd startup. These costs, rather than the snapshot-load API alone, dominate end-to-end readiness.

At concurrency four, two slow restore samples spent 1,603–1,621 ms in allocation wait, and another spent 1,590 ms in VMM configuration/snapshot load. The cause of that stall is not established by these phase timings. Restore improved median latency but had a worse p95 than cold boot; with 16 samples, nearest-rank p95 and p99 both select the maximum. Do not infer a stable tail-latency SLA or attribute the difference versus BusyBox solely to jailer: image size, guest initialization, and isolation changed together.

Known reporting caveat: nested `versions.isolated_ready` is `false` because the benchmark embeds `host::check()`, a generic probe with no isolation policy. It is not the launch profile. The reports explicitly record `profile: "isolated"`; the preceding policy-aware doctor check passed, lifecycle tests inspected actual jail/cgroup state, and each benchmark launch enforced the runtime isolation checks. The raw metadata has not been rewritten.

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

Every launch uses foreground jailer, a fresh jail generation, cgroup v2, normal Firecracker seccomp, empty supplementary groups, and no guest NIC. Start copies the retained disk to a new generation; restore/clone copy state and memory into root-owned read-only jail files. No writable disk or memory backing inode is hardlinked between boxes. Capture uses separate temporary output names, never overwriting a restored VM's mapped input. Extra copies and hashing may increase latency and disk use; development benchmark numbers do not describe this profile.

Limits are CPU quota = configured vCPUs, `memory.max` = (2 × guest MiB + 128) MiB, swap = 0, `pids.max` = 64, open files = 256, plus the existing 3 GiB per-file limit. The memory overhead allowance needs validation with larger/dirty workloads; OOM must fail the launch rather than bypass limits. Before reporting readiness, and when inspecting a running/paused VM, the runtime checks the jail root, executable, cgroup membership/limits, every thread's UID/GID, empty groups, zero effective capabilities, `NoNewPrivs=1`, and `Seccomp=2`.

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
cargo build --release --locked -p box-runtime --bin box
test ! -e /opt/boxd-test && test ! -e /var/lib/boxd-test && \
  test ! -e /etc/boxd-test.json && test ! -e /sys/fs/cgroup/boxd-test || exit 1
sudo install -d -o root -g root -m 0755 /opt/boxd-test
sudo install -d -o root -g root -m 0700 /var/lib/boxd-test \
  /var/lib/boxd-test/image /var/lib/boxd-test/states
sudo install -o root -g root -m 0755 target/release/box \
  .tools/firecracker-1.17.0/firecracker .tools/firecracker-1.17.0/jailer /opt/boxd-test/
sudo install -o root -g root -m 0444 images/output/fixture-v2/image.json \
  images/output/fixture-v2/vmlinux images/output/fixture-v2/rootfs.ext4 /var/lib/boxd-test/image/
sudo mkdir /sys/fs/cgroup/boxd-test
printf '+cpu +memory +pids\n' | sudo tee /sys/fs/cgroup/boxd-test/cgroup.subtree_control >/dev/null
sudo install -o root -g root -m 0600 /dev/null /etc/boxd-test.json
sudo tee /etc/boxd-test.json >/dev/null <<'JSON'
{
  "firecracker": "/opt/boxd-test/firecracker",
  "jailer": "/opt/boxd-test/jailer",
  "cgroup_parent": "boxd-test",
  "uid_base": 70000,
  "gid_base": 71000
}
JSON
sudo env PATH=/opt/boxd-test:/usr/sbin:/usr/bin:/sbin:/bin \
  /opt/boxd-test/box doctor --isolation-config /etc/boxd-test.json
```

No host accounts are created by these commands. Account/service/subordinate-ID reservations must be managed separately. Test setup is ephemeral across reboot for cgroups; production service provisioning remains future work.

### Validate the same real lifecycle suite under jailer

After approval/setup, build the fault-injection test harness without privileges, copy the reviewed outputs into the trusted installation, and execute only those binaries as root. Tests create disposable state stores under the supplied root-owned parent. **Run serially and do not run other stores using this UID range concurrently.** Missing prerequisites fail, rather than skip, this opt-in run.

```sh
test_bin=$(cargo test --release --locked -p box-runtime --features fault-injection \
  --test lifecycle --no-run --message-format=json | python3 -c '
import json, sys
for line in sys.stdin:
    item = json.loads(line)
    if item.get("reason") == "compiler-artifact" and item.get("target", {}).get("name") == "lifecycle" and item.get("executable"):
        print(item["executable"])
')
test -x "$test_bin" || exit 1
sudo install -o root -g root -m 0755 "$test_bin" /opt/boxd-test/lifecycle-tests
sudo install -o root -g root -m 0755 target/release/box /opt/boxd-test/box-test
sudo env PATH=/opt/boxd-test:/usr/sbin:/usr/bin:/sbin:/bin \
  BOXD_TEST_BOX=/opt/boxd-test/box-test \
  BOXD_TEST_IMAGE=/var/lib/boxd-test/image/image.json \
  BOXD_TEST_ISOLATION_CONFIG=/etc/boxd-test.json \
  BOXD_TEST_STATE_PARENT=/var/lib/boxd-test/states \
  /opt/boxd-test/lifecycle-tests --ignored --nocapture --test-threads=1
# Restore the local CLI build without crash failpoints afterward.
cargo build --release --locked -p box-runtime --bin box
```

The suite exercises persistence, killed-process memory restoration, independent clones, concurrent starts, force-stop, and manager crashes. Isolated runs additionally inspect actual process identity, jail root, seccomp/capabilities, cgroup limits, private backing inodes, distinct host UIDs/GIDs, and a guest with only loopback and no management socket.

On 2026-10-01, the privileged retry on `ser7` passed all six tests in 18.10 seconds with Firecracker/jailer 1.17.0 and the trusted fixture. The first run had failed on namespace-relative procfs path checks and left six test VMMs/cgroups after failed cleanup. The retry verified and stopped those processes before testing the correction: followed device/inode ownership checks retain PID/start-time protection, and the harness preserves unfinished state if cleanup fails. The privileged script reported no remaining child cgroups; a subsequent unprivileged process check found no processes in the reserved test UID/GID range. The test installation remains available for inspection. This is single-host lifecycle evidence, not isolated launch latency characterization or adversarial security validation. Do not rerun the one-shot setup over an existing installation.

For manual use after validation, initialize a separate empty state store with `--state-dir /var/lib/boxd-test/manual --isolation-config /etc/boxd-test.json create --profile isolated --image /var/lib/boxd-test/image/image.json`. Later commands only need that `--state-dir`; create/template build/clone/benchmark also require `--profile isolated`. Do not run the manual store concurrently with the test suite. To remove test setup, first delete every test box and verify the test cgroup contains no children or processes; only then remove the dedicated test paths. Never recursively remove live state.

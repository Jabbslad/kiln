# boxd

A self-hosted-first platform for persistent Linux microVMs, implemented in Rust.

**Current delivery: working single-host runtime plus a laptop client and authenticated HTTPS service for trusted development workloads. Not a hosted service or a safe sandbox for untrusted code.** The runtime boots real Firecracker/KVM guests, executes commands over vsock, preserves writable disks, restores coordinated memory/disk checkpoints, and launches independent boxes from prepared templates. No marketing-site mockups or simulated VM operations.

An experimental isolated profile now integrates jailer, per-box host identities, cgroup limits, and jail-local snapshot handling. Unit checks, development-mode regression tests, and all six privileged isolated lifecycle tests pass with both BusyBox and Ubuntu on the development runner. See the [setup and validation procedure](docs/runtime.md#experimental-isolated-profile). This single-host validation is not a production security claim.

## Use a laptop client

Install the macOS/Linux client from a terminal:

```sh
curl -fsSL https://raw.githubusercontent.com/Jabbslad/boxd-install/main/install.sh | sh
```

The script prompts for a GitHub token with read access to the private packages;
no GitHub CLI, Python or Rust installation is needed on the laptop. See the
[installation guide](docs/releases.md) for token permissions, the one-command
Ubuntu server setup, platform requirements and optional manual/Windows downloads.
To build from source instead, use
`cargo install --locked --path crates/box-client`.
It manages templates, create/list/inspect, exec, pause/resume, stop/start and delete
over verified HTTPS. Only the server needs Linux/KVM. The single-host service uses
embedded SQLite, a separate unprivileged TLS gateway and a restricted Unix socket
to the existing runtime. Durable request IDs survive disconnection and prevent
automatic replay of interrupted commands.

See the [server setup and laptop guide](docs/remote-client.md). Installing the
client does not provision server services. Server setup requires confirmation
and binds only to the chosen private address. This pilot has one administrator.
The working tree additionally implements `boxctl ssh`, SFTP via `boxctl cp`,
editor configuration via `boxctl ssh-config`, and opt-in isolated IPv4 egress.
These require new client/server/Ubuntu guest builds and are **not in the pinned
v0.1.1 download**. SSH uses authenticated HTTPS/vsock, without a public SSH port;
internet access requires separate [network provisioning](docs/runtime-networking.md).

## Try the runtime

Requires Linux x86_64, read/write access to `/dev/kvm`, cgroup v2, Rust/rustup, curl, Python 3, binutils, e2fsprogs, and a static x86_64 BusyBox at `/usr/bin/busybox` (or pass another path to the fixture builder). Do not run the CLI as root.

```sh
# Installs only into this checkout. Does not replace system binaries.
bash scripts/fetch-firecracker.sh .tools/firecracker-1.17.0
export PATH="$PWD/.tools/firecracker-1.17.0:$PATH"
rustup target add --toolchain 1.95.0 x86_64-unknown-linux-musl
cargo build --locked --release -p box-runtime --bin box
bash images/build-fixture.sh images/output/fixture

target/release/box doctor
target/release/box create --image images/output/fixture/image.json \
  --name first-box --allow-unsafe-development
# Use the returned ID:
target/release/box exec BOX_ID -- /bin/sh -c 'printf hello'
target/release/box delete BOX_ID
```

The fixture builder refuses to overwrite existing images; use a new output directory when rebuilding. State defaults to a private `.boxd/` directory. Commands accept `--state-dir PATH` and emit versioned JSON. See the [operator guide](docs/runtime.md) for templates, checkpoints, benchmarks, recovery, and limitations.

## Ubuntu development guest

`images/build-ubuntu.sh` builds a pinned Ubuntu Minimal 24.04 amd64 image with systemd, Bash, Python 3, curl, and apt. It needs `fakeroot` and `xz` in addition to the build tools above, but no root, host mounts, or chroot. Both builders refuse to overwrite an existing image.

```sh
bash images/build-ubuntu.sh images/output/ubuntu
target/release/box create --image images/output/ubuntu/image.json \
  --name ubuntu --allow-unsafe-development
target/release/box exec BOX_ID -- /usr/bin/python3 -c 'print(17 + 93)'
```

Templates pause at a Rust bootstrap before systemd starts. Each clone receives fresh identity and a kernel RNG reseed before systemd or workload services run. The host waits for the systemd-managed agent before returning a usable box. All six development and privileged isolated KVM lifecycle tests pass with Ubuntu, including systemd identity and agent-restart checks. See the [Ubuntu image contract](docs/runtime.md#ubuntu-image-contract). Fresh Ubuntu images support SSH over vsock; networking remains opt-in. Docker and a general-purpose compiler toolchain are not included.

## Measured launch performance

Release host/guest builds, Ryzen 7 7840HS, ext4 sparse-copy fallback, 256 MiB / 1 vCPU fixture, Firecracker 1.17.0. Includes integrity checks and first successful guest command; OS page cache was uncontrolled.

| Launch path | Sequential p50 / p95 (30 samples) | Concurrency 4 p50 / p95 (16 samples) |
| --- | --- | --- |
| Cold boot | 807 / 851 ms | 805 / 852 ms |
| Prepared-template restore | 245 / 257 ms | 277 / 325 ms |

All 92 measured launches succeeded. A matched pre-optimization run measured template launches at 281 ms sequential p50 and 537 / 973 ms concurrent p50 / p95. Skipping disk holes and releasing the allocation lock before full checksum verification reduced these costs without disabling checksums or durability syncs. The [operator guide](docs/runtime.md#reproduce-launch-measurements) records the comparison and phase timings.

These are local measurements, not a hosted SLA or a matched comparison with boxd.sh. Full snapshot scans dominated these original template measurements. Subsequent immutable-storage and authenticated-overlay optimizations are measured separately below.

Ubuntu Minimal 24.04 **isolated-profile** measurements on the same runner, release builds, 256 MiB / 1 vCPU, 2 GiB disk, include private jail copies, integrity checks, systemd startup, and the first successful command:

| Launch path | Sequential p50 / p95 (30 samples) | Concurrency 4 p50 / p95 (16 samples) |
| --- | --- | --- |
| Cold boot | 2,800 / 2,836 ms | 3,374 / 3,452 ms |
| Prepared-template restore | 2,037 / 2,054 ms | 2,553 / 3,900 ms |

All 92 Ubuntu launches succeeded. Sequential restore spends about 1,081 ms verifying artifacts and 523 ms initializing the guest/systemd. Concurrent restore has a worse p95 than cold boot in this small sample; the slow samples include allocation waits and a snapshot-load stall. The larger image, different init system, and isolation profile make this **not** a controlled comparison against the BusyBox results above. See [phase analysis and caveats](docs/runtime.md#ubuntu-isolated-launch-measurements).

The subsequent [kernel-enforced snapshot integrity](docs/runtime.md#kernel-enforced-snapshot-integrity) optimization uses kernel digest checks instead of repeated full scans on fs-verity-capable storage. Unsupported filesystems and old snapshots keep full SHA-256 verification. The real sealing test and all six isolated Ubuntu lifecycle tests pass. A matched comparison on the same sealed template and loop-backed ext4 filesystem measured:

| Template restore | Before p50 / p95 | With fs-verity p50 / p95 |
| --- | --- | --- |
| Sequential (30 samples each) | 2,145 / 2,181 ms | **1,045 / 1,060 ms** |
| Concurrency 4 (16 samples each) | 2,724 / 2,760 ms | **1,594 / 1,619 ms** |

All 184 cold/template launches across both implementations succeeded. Sequential template median latency fell **51%**; the snapshot-verification phase fell from 1,083 to 6.7 ms. Systemd startup and private disk/jail copies remain. Cold boot was not optimized, and its concurrent p95 worsened in this small sample. See the [full matched results and cache/order caveats](docs/runtime.md#matched-fs-verity-launch-measurements). These figures are separate from the original measurements above, not a comparison across filesystems or against boxd.sh.

The next follow-up shares sealed immutable jail inputs, authenticates all snapshot-disk reads, and speeds up zero-buffer scanning. A fresh matched comparison against the fs-verity baseline measured:

| Launch path | Before p50 / p95 | Latest p50 / p95 |
| --- | --- | --- |
| Sequential template (30 each) | 1,070 / 1,116 ms | **966 / 1,016 ms** |
| Concurrency 4 template (16 each) | 1,612 / 1,873 ms | **1,513 / 1,548 ms** |
| Sequential cold boot (30 each) | 2,954 / 2,989 ms | **2,632 / 2,675 ms** |

All 184 final-comparison launches succeeded. Template median latency improved another **9.8% sequentially / 6.2% at concurrency four**. Writable disk materialization and systemd startup remain the dominant costs; unsafe writable reflinks from sealed snapshots are deliberately excluded. All 55 ordinary tests, three real-filesystem tests, and Ubuntu development/isolated lifecycle suites pass. See [measurements, the initial regression, and limitations](docs/runtime.md#matched-sealed-sharing-and-disk-copy-measurements). These remain uncontrolled-cache local results, not a boxd.sh comparison.

The latest opt-in [authenticated disk-overlay backend](docs/runtime.md#optional-authenticated-disk-overlays) removes full disk materialization from sealed isolated template launches. It shares a read-only fs-verity base through Linux dm-snapshot with private persistent writes; the existing copy backend remains the default.

| Template launch | Copy p50 / p95 | Overlay p50 / p95 |
| --- | --- | --- |
| Sequential (30 each) | 945 / 970 ms | **714 / 740 ms** |
| Concurrency 4 (16 each) | 1,508 / 1,531 ms | **894 / 1,024 ms** |

All 184 matched cold/template launches succeeded. Template median improved **24.5% / 40.7%**. Both isolated backends pass seven lifecycle checks, including interrupted launches, private disks, checkpoint restore, and restart persistence. Cold creation is unchanged; systemd/guest initialization still takes about 556 ms sequentially. See [full results and limitations](docs/runtime.md#matched-authenticated-overlay-measurements). This does not establish that we are faster than boxd.sh, whose earlier hosted measurements use different resources and timing boundaries.

Opt-in [warm systemd templates](docs/runtime.md#opt-in-warm-systemd-templates) now reuse completed systemd boot work, then reseed entropy, reset identity and start fresh workload services before accepting commands. Ubuntu/systemd remains available; the default image mode is unchanged.

| Template-to-first-command | Legacy p50 / p95 | Warm p50 / p95 |
| --- | --- | --- |
| Sequential (30 each) | 721 / 756 ms | **512 / 536 ms** |
| Concurrency 4 (16 attempts each) | 909 / 969 ms† | **630 / 677 ms** |

Sequential template p50 improved **29%** on matched isolated-profile images with authenticated overlays. All **92 warm cold/template attempts succeeded**. †One legacy concurrent first exec failed with `Resource temporarily unavailable`; its percentiles include only 15 successful samples, and the failure is retained. Cold-boot p50 stayed essentially unchanged at 2.55–2.56 seconds sequentially. See [complete measurements, phase timings and caveats](docs/runtime.md#matched-warm-template-measurements). The warm policy admits only the pinned trusted image, not arbitrary live workloads or hostile tenant images. These are local comparisons against our previous boot path, **not evidence that we beat boxd.sh**.

A fresh [4 GiB / one-vCPU CLI comparison with boxd.sh](docs/runtime.md#fresh-boxdsh-comparison-at-4-gib--one-vcpu) measured launch through first command at **523 / 643 ms p50/p95 locally versus 803 / 975 ms hosted** (30 sequential samples), and **640 / 695 versus 1,002 / 1,224 ms** at concurrency four (16 each). All 92 attempts succeeded. Both clocks include CLI startup, and RAM/CPU allocations were verified. **boxd.sh's create command alone was faster** (408 versus 505 ms sequential median); our lower total largely reflects cheaper local exec (18 versus 388 ms). Disk sizes, hosts and service functionality still differ, so this is not evidence of a faster VM-restore engine. Template build now accepts `--memory-mib 4096 --vcpus 1`; the local aggregate guest quota is 16 GiB.

The opt-in [`systemd_warm_shared` experiment](docs/runtime.md#experimental-retained-pid1-policy)
retains PID1 and deliberately shares the machine ID while preserving entropy
reseeding, private disks, fresh daemons and the workload-readiness barrier.
A matched 4 GiB / one-vCPU run reduced sequential create p50 **517 → 475 ms**
and launch-through-first-command **536 → 493 ms**; concurrent totals fell
**636 → 598 ms**. All 92 comparison attempts succeeded. This is an **8% / 6%**
improvement, not elimination of the earlier hosted create gap. Existing modes and
defaults keep unique identities. See [reset breakdowns, compatibility restrictions
and the separately retained transient test failure](docs/runtime.md#retained-pid1-measurements).

## Development

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --locked
PATH="$PWD/.tools/firecracker-1.17.0:$PATH" \
  BOXD_TEST_IMAGE="$PWD/images/output/fixture/image.json" \
  cargo test --release --locked -p box-runtime --features fault-injection \
  --test lifecycle -- --ignored --test-threads=1 --nocapture
```

The opt-in integration suite launches disposable VMs and deliberately crashes CLI processes. Normal tests never launch a VM. Build production/development CLI binaries **without** the `fault-injection` feature.

## Path to the platform

The [architecture](docs/superpowers/specs/2026-09-30-boxd-platform-design.md) and [runtime plan](docs/superpowers/plans/2026-09-30-runtime-engine.md) describe the wider product. The [laptop-first service slice](docs/remote-client.md) uses embedded SQLite rather than requiring PostgreSQL. Still outstanding: signed client releases, an image security-update/release process, operation-history retention, scoped authentication, preview routing, dashboard, SDK, and fleet scheduling. Both guest images remain intended for trusted development workloads, not a hosted multi-tenant environment.

# boxd

A self-hosted-first platform for persistent Linux microVMs, implemented in Rust.

**Current delivery: working single-host development runtime, not a hosted service or a safe sandbox for untrusted code.** The CLI boots real Firecracker/KVM guests, executes commands over vsock, preserves writable disks, restores coordinated memory/disk checkpoints, and launches independent boxes from prepared templates. No marketing-site mockups or simulated VM operations.

An experimental isolated profile now integrates jailer, per-box host identities, cgroup limits, and jail-local snapshot handling. Unit checks, development-mode regression tests, and all six privileged isolated lifecycle tests pass with both BusyBox and Ubuntu on the development runner. See the [setup and validation procedure](docs/runtime.md#experimental-isolated-profile). This single-host validation is not a production security claim.

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

Templates pause at a Rust bootstrap before systemd starts. Each clone receives fresh identity and a kernel RNG reseed before systemd or workload services run. The host waits for the systemd-managed agent before returning a usable box. All six development and privileged isolated KVM lifecycle tests pass with Ubuntu, including systemd identity and agent-restart checks. See the [Ubuntu image contract](docs/runtime.md#ubuntu-image-contract). No network access, Docker, SSH login, or general-purpose compiler toolchain is included.

## Measured launch performance

Release host/guest builds, Ryzen 7 7840HS, ext4 sparse-copy fallback, 256 MiB / 1 vCPU fixture, Firecracker 1.17.0. Includes integrity checks and first successful guest command; OS page cache was uncontrolled.

| Launch path | Sequential p50 / p95 (30 samples) | Concurrency 4 p50 / p95 (16 samples) |
| --- | --- | --- |
| Cold boot | 807 / 851 ms | 805 / 852 ms |
| Prepared-template restore | 245 / 257 ms | 277 / 325 ms |

All 92 measured launches succeeded. A matched pre-optimization run measured template launches at 281 ms sequential p50 and 537 / 973 ms concurrent p50 / p95. Skipping disk holes and releasing the allocation lock before full checksum verification reduced these costs without disabling checksums or durability syncs. The [operator guide](docs/runtime.md#reproduce-launch-measurements) records the comparison and phase timings.

These are local measurements, not a hosted SLA or a matched comparison with boxd.sh. Full snapshot scans still dominate template latency. Verified immutable artifact storage and reflink-capable disks remain work to evaluate, not performance already achieved.

Ubuntu Minimal 24.04 **isolated-profile** measurements on the same runner, release builds, 256 MiB / 1 vCPU, 2 GiB disk, include private jail copies, integrity checks, systemd startup, and the first successful command:

| Launch path | Sequential p50 / p95 (30 samples) | Concurrency 4 p50 / p95 (16 samples) |
| --- | --- | --- |
| Cold boot | 2,800 / 2,836 ms | 3,374 / 3,452 ms |
| Prepared-template restore | 2,037 / 2,054 ms | 2,553 / 3,900 ms |

All 92 Ubuntu launches succeeded. Sequential restore spends about 1,081 ms verifying artifacts and 523 ms initializing the guest/systemd. Concurrent restore has a worse p95 than cold boot in this small sample; the slow samples include allocation waits and a snapshot-load stall. The larger image, different init system, and isolation profile make this **not** a controlled comparison against the BusyBox results above. See [phase analysis and caveats](docs/runtime.md#ubuntu-isolated-launch-measurements).

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

The [architecture](docs/superpowers/specs/2026-09-30-boxd-platform-design.md) and [runtime plan](docs/superpowers/plans/2026-09-30-runtime-engine.md) describe the wider product. Still outstanding: an image security-update/release process, launch-latency optimization, broader failure/cancellation coverage, authenticated host service, PostgreSQL control plane, networking/SSH/preview routing, dashboard, SDK, and fleet scheduling. Both guest images remain intended for trusted development workloads, not a hosted multi-tenant environment.

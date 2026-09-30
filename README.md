# boxd

A self-hosted-first platform for persistent Linux microVMs, implemented in Rust.

**Current delivery: working single-host development runtime, not a hosted service or a safe sandbox for untrusted code.** The CLI boots real Firecracker/KVM guests, executes commands over vsock, preserves writable disks, restores coordinated memory/disk checkpoints, and launches independent boxes from prepared templates. No marketing-site mockups or simulated VM operations.

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

## Measured baseline

Release host/guest builds, Ryzen 7 7840HS, ext4 byte-copy fallback, 256 MiB / 1 vCPU fixture, Firecracker 1.17.0. Includes integrity checks and first successful guest command; OS page cache was uncontrolled.

| Launch path | Sequential p50 / p95 (30 samples) | Concurrency 4 p50 / p95 (16 samples) |
| --- | --- | --- |
| Cold boot | 844 / 895 ms | 829 / 868 ms |
| Prepared-template restore | 279 / 287 ms | 504 / 944 ms |

All 92 measured launches succeeded. These are local measurements, not a hosted SLA. Every restore currently scans full snapshot checksums and takes an allocation lock during validation. A long-running host service with verified immutable artifact caching and reflink-capable storage is the next optimization to evaluate, not a performance claim already achieved.

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

The [architecture](docs/superpowers/specs/2026-09-30-boxd-platform-design.md) and [runtime plan](docs/superpowers/plans/2026-09-30-runtime-engine.md) describe the wider product. Still outstanding: a jailed/cgroup-enforced profile, a maintained Ubuntu development image, broader failure/cancellation coverage, authenticated host service, PostgreSQL control plane, networking/SSH/preview routing, dashboard, SDK, and fleet scheduling. The current guest is a minimal trusted test fixture—not Ubuntu, Docker, or a multi-tenant environment.

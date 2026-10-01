# Authenticated disk overlays implementation plan

**Goal:** Remove full disk materialization from isolated template clone and checkpoint restore, preserving private writes, durable recovery, and authenticated base reads.

**Architecture:** Opt-in `disk_backend: "snapshot"` in the pinned isolated policy. Attach a verified fs-verity regular base through a read-only buffered loop device and a private persistent COW file through a writable loop device. Linux dm-snapshot presents the writable disk to Firecracker. Keep copy-backed compatibility as the default. Guest startup optimization follows disk validation; do not snapshot initialized services without a separate identity/entropy design.

**Constraints:** No custom block server or Firecracker fork. No writable reflinks from fs-verity sources. No changes to pre-existing installations, root-filesystem features, or concurrent dashboard work. Never remove a mapping by name alone: verify UUID, table, loop backing inode/device, offsets, flags, and reference labels. Use autoclear loops held open through mapping creation, eliminating the attach-before-persist leak window. Record backing-file identity before mapping creation. Never detach a foreign device or use forced/deferred dm removal.

## Storage implementation

- [x] Add `storage/overlay.rs` and its loop-device support. Persist layer metadata and a sparse COW file in a private per-box layer directory before kernel allocation. Use random layer IDs in dm names/UUIDs and loop reference labels. Validate regular single-link COW files, sealed bases, disk sizes and bounded COW capacity. Reject invalid/overflowed dm status instead of reporting healthy storage.
- [x] Write tests first for layer metadata, device/table identity validation, and invalid configuration. Use a privileged disposable fs-verity test for asymmetric writes in two overlays, unchanged base, remapping after device removal, flattening, corruption/replacement refusal, and an unrelated mapping that cleanup must preserve.
- [x] Create jail-local block nodes only for a validated mapping, mode 0600 and the assigned jail UID/GID. Add a bounded block-device-to-regular-file flatten operation for checkpoint capture; retain authenticated regular-file copying elsewhere.

## Runtime integration

- [x] Add defaulted `disk_layer` to box records and defaulted policy backend selection. New cold boxes continue using ordinary private files; sealed clone/restore can allocate layers. Store layers outside jail generations, so stop/start retains the same disk without copying.
- [x] Centralize disk preparation, exposure, flattening, and deletion in `runtime/disk.rs`. Restore creates the replacement layer before stopping the source. Layer manifests conservatively pin their base snapshots, including interrupted restores and orphan layers, until explicit box deletion. Preserve memory-snapshot references separately.
- [x] Implement recreation from durable files after host restart; validate through explicit detach/remap (no host reboot was performed). Adopt existing mappings only after exact ownership checks. Keep stopped-box layers until restart/delete. Delete mappings before files, only after confirming the owned VMM stopped. Retain state if cleanup fails. Test crashes after layer publication and mapping creation. Helpers inherit pinned devices and layer locks so their lifetime can safely exceed the manager's.

## Verification and measurements

- [x] Run formatting, all-feature Clippy, 60 ordinary tests (three final consecutive runs), real overlay storage tests (three final consecutive runs), and seven Ubuntu lifecycle checks each in development, isolated copy, and isolated snapshot modes. Test start persistence, coordinated checkpoint restore, clone independence, bad seals, COW replacement/status failure, and cleanup. Full-pool ENOSPC and power loss are not simulated. Build ordinary CLI without fault injection.
- [x] Compare the preserved copy binary and final overlay implementation with the same sealed Ubuntu template, image, pool, 256 MiB/1 vCPU and first-command boundary: 30 sequential and 16 concurrency-four samples per mode. All 184 launches succeeded. Template p50/p95: 945/970→714/740 ms sequential, 1508/1531→894/1024 ms concurrent. Guest startup is unchanged.
- [x] Update operator prerequisites, opt-in policy, integrity/overflow/recovery limitations and measured results. Retain evidence under `.amp/in/artifacts/`; remove scratch prototypes. Test pool is unmounted; no owned mappings, loops, VMMs, or child cgroups remain. Changes are local, uncommitted and unpushed.

## Remaining guest-startup decision

Guest initialization/systemd now consumes about 556 ms sequentially. A post-benchmark trace confirms the agent waits for `basic.target`, including tmpfiles/sysusers/catalog work, with udev, linker-cache, console and cloud-image units also starting. Do not move a shared template past initialized systemd without addressing cached identity and userspace randomness. An opt-in command-oriented Ubuntu profile can avoid systemd entirely; the current systemd-compatible profile must remain available. This is a compatibility decision still to resolve with the user, not an implemented optimization or a sub-10ms claim.

## Alternatives and evidence

Direct writable reflinks bypass fs-verity authentication and remain excluded. A userspace block server adds a daemon/protocol and failure boundary; dm-snapshot already supplies persistent copy-on-write and block flush handling. Buffered loop I/O uses the backing file's read iterator, so fs-verity authenticates pages before admitting them to page cache. Firecracker 1.17.0 supports block-special disks and reopens the saved `disk.ext4` path during restore. Match the original capacity/sector geometry; expose only the private top-level mapper device, never the base/COW devices, inside the jail.

References: Linux `drivers/block/loop.c`, `fs/verity/verify.c`, `Documentation/admin-guide/device-mapper/snapshot.rst`; Firecracker v1.17.0 `src/vmm/src/devices/virtio/block/virtio/{device,persist}.rs` and its test jailer block-node helper. The library investigation confirmed read, flush, and restore contracts before implementation.

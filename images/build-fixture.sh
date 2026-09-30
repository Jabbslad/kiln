#!/usr/bin/env bash
# Build a trusted, networkless integration fixture without root or host mounts.
set -euo pipefail
repo=$(realpath "$(dirname "$0")/..")
output=${1:?usage: build-fixture.sh OUTPUT_DIRECTORY [STATIC_BUSYBOX]}
busybox=${2:-/usr/bin/busybox}
mkdir -p "$output"
output=$(realpath "$output")
test ! -e "$output/image.json" && test ! -e "$output/rootfs.ext4" || { echo 'Refusing to overwrite an image' >&2; exit 1; }
file "$busybox" | grep -Eq 'static(ally|-pie) linked' || { echo 'A static x86_64 BusyBox is required' >&2; exit 1; }
kernel_url=https://s3.amazonaws.com/spec.ccfc.min/firecracker-ci/v1.15/x86_64/vmlinux-6.1.155
kernel_sha=e20e46d0c36c55c0d1014eb20576171b3f3d922260d9f792017aeff53af3d4f2
if [ ! -f "$output/vmlinux" ]; then
    curl --fail --location --retry 3 "$kernel_url" -o "$output/vmlinux"
fi
printf '%s  %s\n' "$kernel_sha" "$output/vmlinux" | sha256sum -c -
cargo build --manifest-path "$repo/Cargo.toml" --locked --release -p box-guest --target x86_64-unknown-linux-musl
agent="$repo/target/x86_64-unknown-linux-musl/release/box-guest"
if readelf -l "$agent" | grep -q INTERP; then echo 'Guest has a dynamic interpreter' >&2; exit 1; fi
root=$(mktemp -d "$output/.root.XXXXXX")
trap 'rm -rf -- "$root"' EXIT
mkdir -p "$root"/{bin,sbin,etc/init.d,dev,proc,sys,tmp,run,root,workspace}
chmod 1777 "$root/tmp"
cp "$busybox" "$root/bin/busybox"
for applet in sh mount umount mkdir sleep cat printf echo sync reboot poweroff hostname setsid head wc touch rm ls dd; do
    ln -s busybox "$root/bin/$applet"
done
ln -s ../bin/busybox "$root/sbin/init"
cp "$agent" "$root/sbin/box-guest"
touch "$root/etc/machine-id"
printf 'root:x:0:0:root:/root:/bin/sh\n' > "$root/etc/passwd"
printf 'root:x:0:\n' > "$root/etc/group"
cat > "$root/etc/inittab" <<'EOF'
::sysinit:/etc/init.d/rcS
::respawn:/sbin/box-guest
::ctrlaltdel:/bin/reboot
::shutdown:/bin/sync
EOF
cat > "$root/etc/init.d/rcS" <<'EOF'
#!/bin/sh
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev
mkdir -p /dev/pts
mount -t devpts devpts /dev/pts
# Materialize the kernel's lazily generated boot ID before the template barrier.
# Tests can then distinguish restoring this boot from silently booting anew.
cat /proc/sys/kernel/random/boot_id > /run/prepared-boot-id
EOF
chmod 755 "$root/etc/init.d/rcS"
truncate -s 128M "$output/rootfs.ext4"
/usr/sbin/mkfs.ext4 -q -F -d "$root" "$output/rootfs.ext4"
python3 - "$output" "$busybox" "$agent" "$kernel_url" <<'PY'
import hashlib, json, pathlib, sys
output = pathlib.Path(sys.argv[1])
def digest(path):
    with open(path, 'rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()
manifest = dict(schema_version=1, architecture='x86_64', kernel_path='vmlinux',
                kernel_sha256=digest(output / 'vmlinux'), rootfs_path='rootfs.ext4',
                rootfs_sha256=digest(output / 'rootfs.ext4'), agent_protocol_version=1)
(output / 'image.json').write_text(json.dumps(manifest, indent=2) + '\n')
(output / 'inputs.json').write_text(json.dumps(dict(kernel_url=sys.argv[4],
    busybox_sha256=digest(sys.argv[2]), agent_sha256=digest(sys.argv[3])), indent=2) + '\n')
print(output / 'image.json')
PY

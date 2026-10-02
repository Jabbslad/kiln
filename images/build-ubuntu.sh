#!/usr/bin/env bash
# Build a pinned, networkless Ubuntu guest without root, mounts, or chroot.
set -euo pipefail
repo=$(realpath "$(dirname "$0")/..")
output=${1:?usage: build-ubuntu.sh OUTPUT_DIRECTORY [ROOTFS_TARBALL] [systemd|systemd_warm|systemd_warm_shared]}
archive=${2:-}
mode=${3:-systemd}
case "$mode" in systemd|systemd_warm|systemd_warm_shared) ;; *) echo 'Unsupported boot mode' >&2; exit 1 ;; esac
if [[ ${BOXD_FAKEROOT:-} != 1 ]]; then
    test "$EUID" -ne 0 || { echo 'Run as an ordinary user, not root.' >&2; exit 1; }
    for tool in fakeroot curl python3 tar readelf mkfs.ext4; do command -v "$tool" >/dev/null; done
    mkdir -p "$output"
    output=$(realpath "$output")
    for file in image.json rootfs.ext4; do
        if [[ -e "$output/$file" || -L "$output/$file" ]]; then
            echo 'Refusing to overwrite an image' >&2
            exit 1
        fi
    done
    cargo build --manifest-path "$repo/Cargo.toml" --locked --release -p box-guest --bins --target x86_64-unknown-linux-musl
    if test -n "$archive"; then archive=$(realpath "$archive"); fi
    exec fakeroot -- env BOXD_FAKEROOT=1 bash "$0" "$output" "$archive" "$mode"
fi
root_url=https://cloud-images.ubuntu.com/minimal/releases/noble/release-20260905/ubuntu-24.04-minimal-cloudimg-amd64-root.tar.xz
root_sha=094dc0afc6ded1c3e5ce71f7d0b48d5db922155097bc8fb1ec19db2ebdd17ece
kernel_url=https://s3.amazonaws.com/spec.ccfc.min/firecracker-ci/v1.15/x86_64/vmlinux-6.1.155
kernel_sha=e20e46d0c36c55c0d1014eb20576171b3f3d922260d9f792017aeff53af3d4f2
if test -z "$archive"; then
    archive="$output/ubuntu-root.tar.xz"
    curl --fail --location --retry 3 "$root_url" -o "$archive"
fi
printf '%s  %s\n' "$root_sha" "$archive" | sha256sum -c -
if test ! -f "$output/vmlinux"; then curl --fail --location --retry 3 "$kernel_url" -o "$output/vmlinux"; fi
printf '%s  %s\n' "$kernel_sha" "$output/vmlinux" | sha256sum -c -
root=$(mktemp -d "$output/.root.XXXXXX")
trap 'rm -rf -- "$root"' EXIT
tar --extract --xz --numeric-owner --same-owner --preserve-permissions --file "$archive" --directory "$root"
for file in usr/sbin/sshd usr/bin/ssh-keygen etc/pam.d/sshd; do
    test -e "$root/$file" || { echo "Ubuntu rootfs lacks OpenSSH prerequisite: /$file" >&2; exit 1; }
done
mkdir -p "$root"/{workspace,run,proc,sys,dev,etc/systemd/system}
for binary in box-guest box-init; do
    source="$repo/target/x86_64-unknown-linux-musl/release/$binary"
    if readelf -l "$source" | grep -q INTERP; then echo 'Guest binaries must be static' >&2; exit 1; fi
    install -o 0 -g 0 -m 0755 "$source" "$root/usr/sbin/$binary"
done
# Templates contain no preassigned host identity, entropy seed, or SSH keys.
rm -f "$root/var/lib/dbus/machine-id" "$root/var/lib/systemd/random-seed" "$root"/etc/ssh/ssh_host_*
rm -rf "$root/var/lib/cloud" "$root/var/log/journal"
truncate -s 0 "$root/etc/machine-id"
printf 'localhost\n' > "$root/etc/hostname"
printf '127.0.0.1 localhost\n::1 localhost\n' > "$root/etc/hosts"
printf '/dev/vda / ext4 defaults 0 0\n' > "$root/etc/fstab"
touch "$root/etc/cloud/cloud-init.disabled"
ln -s /etc/machine-id "$root/var/lib/dbus/machine-id"
# OpenSSH is launched only in inetd mode by box-guest over vsock. Its network
# service remains masked below; no cloud-init or login console is enabled.
cat > "$root/etc/systemd/system/boxd.target" <<'EOF'
[Unit]
Description=boxd development guest
Requires=basic.target box-guest.service
After=basic.target box-guest.service
AllowIsolate=yes
EOF
cat > "$root/etc/systemd/system/box-guest.service" <<'EOF'
[Unit]
Description=boxd host-vsock guest agent
Requires=basic.target systemd-user-sessions.service
After=basic.target systemd-user-sessions.service
ConditionPathExists=/run/boxd-initialized

[Service]
Type=simple
ExecStart=/sbin/box-guest --systemd-service
RuntimeDirectory=sshd
RuntimeDirectoryMode=0755
Restart=on-failure
RestartSec=100ms
UMask=0077
StandardOutput=journal+console
StandardError=journal+console
EOF
if [[ "$mode" == systemd_warm || "$mode" == systemd_warm_shared ]]; then
    # Public preparation identity; only the explicit shared mode retains it for
    # workloads. Avoid systemd's transient read-only machine-id mount at boot.
    printf '11111111111111111111111111111111\n' > "$root/etc/machine-id"
    # Permit only root's read-only verification of the bus daemon's cached ID.
    mkdir -p "$root/etc/dbus-1/system.d"
    cat > "$root/etc/dbus-1/system.d/boxd-identity.conf" <<'EOF'
<busconfig>
  <policy user="root">
    <allow send_destination="org.freedesktop.DBus"
           send_interface="org.freedesktop.DBus.Peer" send_member="GetMachineId"/>
  </policy>
</busconfig>
EOF
    cat > "$root/etc/systemd/system/boxd.target" <<'EOF'
[Unit]
Description=boxd warm development guest
Requires=basic.target box-bootstrap.service box-guest.service
After=basic.target box-bootstrap.service box-guest.service
AllowIsolate=yes
EOF
    cat > "$root/etc/systemd/system/box-bootstrap.service" <<'EOF'
[Unit]
Description=boxd warm template barrier
Requires=systemd-user-sessions.service
After=basic.target systemd-user-sessions.service
ConditionKernelCommandLine=boxd.warm=1
ConditionPathExists=!/run/boxd-initialized

[Service]
Type=simple
ExecStart=/sbin/box-guest --warm-bootstrap
Restart=no
UMask=0077
StandardOutput=tty
StandardError=tty
TTYPath=/dev/console
EOF
fi
ln -sfn boxd.target "$root/etc/systemd/system/default.target"
# Prevent sockets/generators from activating network services or stale seeds.
for unit in ssh.service ssh.socket systemd-networkd.service systemd-networkd.socket systemd-networkd-wait-online.service systemd-resolved.service systemd-timesyncd.service systemd-random-seed.service; do
    ln -sfn /dev/null "$root/etc/systemd/system/$unit"
done
chown -R 0:0 "$root/etc/systemd/system" "$root/workspace"
truncate -s 2G "$output/rootfs.ext4"
/usr/sbin/mkfs.ext4 -q -F -d "$root" "$output/rootfs.ext4"
python3 - "$output" "$repo" "$root_url" "$root_sha" "$kernel_url" "$mode" <<'PY'
import hashlib, json, pathlib, sys
output, repo = map(pathlib.Path, sys.argv[1:3])
def digest(path):
    with open(path, 'rb') as file:
        return hashlib.file_digest(file, 'sha256').hexdigest()
manifest = dict(schema_version=1, architecture='x86_64', kernel_path='vmlinux',
    kernel_sha256=digest(output / 'vmlinux'), rootfs_path='rootfs.ext4',
    rootfs_sha256=digest(output / 'rootfs.ext4'), agent_protocol_version=1, boot_mode=sys.argv[6])
(output / 'inputs.json').write_text(json.dumps(dict(rootfs_url=sys.argv[3], rootfs_sha256=sys.argv[4],
    kernel_url=sys.argv[5], builder_sha256=digest(repo / 'images/build-ubuntu.sh'),
    agent_sha256=digest(repo / 'target/x86_64-unknown-linux-musl/release/box-guest'),
    init_sha256=digest(repo / 'target/x86_64-unknown-linux-musl/release/box-init')), indent=2) + '\n')
(output / 'image.json').write_text(json.dumps(manifest, indent=2) + '\n')
print(output / 'image.json')
PY

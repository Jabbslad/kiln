#!/bin/sh
# Real isolated VM test inside disposable mount/network/PID namespaces.
# Uses one new host cgroup subtree, always killed/removed by the cleanup trap.
set -eu
export PATH=/usr/sbin:/usr/bin:/sbin:/bin
[ "$#" = 2 ] || { echo 'usage: sudo sh test-network-kvm.sh IMAGE_DIR LIFECYCLE_TEST_BINARY' >&2; exit 1; }
repo=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
if [ "${BOXD_NETWORK_KVM_CHILD:-}" != 1 ]; then
    [ "$(id -u)" = 0 ] || exit 1
    exec unshare --mount --net --pid --fork --mount-proc env BOXD_NETWORK_KVM_CHILD=1 sh "$0" "$@"
fi
mount --make-rprivate /
mount -t tmpfs -o mode=0755 tmpfs /run
mkdir /run/host-cgroups
mount --bind /sys/fs/cgroup /run/host-cgroups
mount -t sysfs sysfs /sys
# Keep only the normal mount: an alias outside /sys masks launchers which
# accidentally hide /sys/fs/cgroup when entering a network namespace.
mount --move /run/host-cgroups /sys/fs/cgroup
rmdir /run/host-cgroups
base=/run/boxd-test
mkdir -m 0700 "$base"
mkdir "$base/bin" "$base/image" "$base/state"
cp "$1"/image.json "$1"/inputs.json "$1"/vmlinux "$1"/rootfs.ext4 "$base/image/"
for binary in boxctl boxd-host; do install -m 0755 "$repo/target/debug/$binary" "$base/bin/$binary"; done
for binary in firecracker jailer; do install -m 0755 "$repo/.tools/firecracker-1.17.0/$binary" "$base/bin/$binary"; done
install -m 0755 "$repo/deploy/boxd-network" "$base/bin/boxd-network"
install -m 0755 "$2" "$base/bin/lifecycle"
cg="boxd-access-$(cat /proc/sys/kernel/random/uuid)"
mkdir "/sys/fs/cgroup/$cg"
cleanup() {
    printf '1\n' > "/sys/fs/cgroup/$cg/cgroup.kill"
    for _ in 1 2 3 4 5; do
        for dir in "/sys/fs/cgroup/$cg"/*; do rmdir "$dir" 2>/dev/null || :; done
        if rmdir "/sys/fs/cgroup/$cg" 2>/dev/null; then return; fi
        sleep 1
    done
    echo "Test cgroup cleanup failed: $cg" >&2
}
trap cleanup EXIT
printf '+cpu +memory +pids\n' > "/sys/fs/cgroup/$cg/cgroup.subtree_control"
for id in 80000 80001 80002 80003 80004 80005 80006 80007; do
    if getent passwd "$id" >/dev/null; then echo 'test UID already allocated' >&2; exit 1; fi
done
cat > "$base/isolation.json" <<EOF
{"firecracker":"$base/bin/firecracker","jailer":"$base/bin/jailer","cgroup_parent":"$cg","uid_base":80000,"gid_base":81000,"network":{"helper":"$base/bin/boxd-network","namespace_scope":"boxd","resolver":"1.1.1.1"}}
EOF
ip link set lo up
ip netns add wan
ip link add wan0 type veth peer name wan1 netns wan
ip address add 8.8.8.1/24 dev wan0
ip link set wan0 up
ip -n wan address add 8.8.8.8/24 dev wan1
ip -n wan link set wan1 up
ip -n wan link set lo up
ip -n wan route add default via 8.8.8.1
"$base/bin/boxd-network" provision wan0
mkdir "$base/www"
printf 'boxd-egress\n' > "$base/www/probe"
ip netns exec wan python3 -m http.server 8080 --bind 8.8.8.8 --directory "$base/www" > "$base/http.log" 2>&1 &
export PATH="$base/bin:$PATH"
export BOXD_TEST_IMAGE="$base/image/image.json"
export BOXD_TEST_STATE_PARENT="$base/state"
export BOXD_TEST_ISOLATION_CONFIG="$base/isolation.json"
export BOXD_TEST_HOST="$base/bin/boxd-host"
export BOXD_TEST_CLIENT="$base/bin/boxctl"
export BOXD_TEST_NETWORK=1
if ! "$base/bin/lifecycle" --ignored --nocapture --test-threads=1; then
    find "$base/state" -type f \( -name host.log -o -name console.log -o -name serial.log \) -exec tail -60 {} \;
    exit 1
fi
echo 'PASS: disposable isolated KVM guest access and egress'

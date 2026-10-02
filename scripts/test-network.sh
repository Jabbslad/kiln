#!/bin/sh
# Packet-level policy test. Everything, including /run and sysctls, is private.
set -eu
export PATH=/usr/sbin:/usr/bin:/sbin:/bin
repo=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
if [ "${BOXD_NETWORK_TEST_CHILD:-}" != 1 ]; then
    [ "$(id -u)" = 0 ] || { echo 'Run this disposable namespace test with sudo.' >&2; exit 1; }
    exec unshare --mount --net --pid --fork --mount-proc env BOXD_NETWORK_TEST_CHILD=1 sh "$0"
fi
mount --make-rprivate /
mount -t tmpfs tmpfs /run
mount -t sysfs sysfs /sys
helper="$repo/deploy/boxd-network"
ip link set lo up
ip netns add wan
ip link add wan0 type veth peer name wan1 netns wan
ip address add 8.8.8.1/24 dev wan0
ip link set wan0 up
ip -n wan address add 8.8.8.8/24 dev wan1
ip -n wan address add 10.20.30.40/32 dev wan1
ip -n wan address add 169.254.169.254/32 dev wan1
ip -n wan link set wan1 up
ip -n wan link set lo up
ip -n wan route add default via 8.8.8.1
ip route add 10.20.30.40/32 via 8.8.8.8
ip route add 169.254.169.254/32 via 8.8.8.8
sh "$helper" provision wan0
for slot in 0 1; do
    owner=$(printf '%032d' "$((slot + 1))")
    sh "$helper" prepare "$owner" "bd-boxd-$slot" "100.96.$slot.2" "100.96.$slot.1" 1.1.1.1 80000
    ip -n "bd-boxd-$slot" tuntap show tap0 | grep -q 'user 80000'
    ip netns add "guest$slot"
    ip -n "bd-boxd-$slot" link add test0 type veth peer name eth0 netns "guest$slot"
    ip -n "bd-boxd-$slot" link set test0 master br0
    ip -n "bd-boxd-$slot" link set test0 up
    ip -n "guest$slot" link set lo up
    ip -n "guest$slot" address add "100.96.$slot.2/24" dev eth0
    ip -n "guest$slot" link set eth0 up
    ip -n "guest$slot" route add default via "100.96.$slot.1"
done
blocked() {
    if ip netns exec "$1" ping -n -c 1 -W 1 "$2" >/dev/null 2>&1; then
        echo "FAIL: $1 reached forbidden $2" >&2; exit 1
    fi
}
blocked guest0 8.8.8.8
sh "$helper" activate 00000000000000000000000000000001 bd-boxd-0 100.96.0.2 100.96.0.1 1.1.1.1 80000
ip netns exec guest0 ping -n -c 1 -W 2 8.8.8.8 >/dev/null
for address in 8.8.8.1 10.20.30.40 169.254.169.254 100.96.1.2 100.104.0.1 100.96.0.1; do
    blocked guest0 "$address"
done
blocked wan 100.104.0.2
ip -n guest0 address add 8.8.8.9/32 dev eth0
if ip netns exec guest0 ping -n -I 8.8.8.9 -c 1 -W 1 8.8.8.8 >/dev/null 2>&1; then
    echo 'FAIL: spoofed guest source escaped' >&2; exit 1
fi
for slot in 0 1; do
    owner=$(printf '%032d' "$((slot + 1))")
    sh "$helper" cleanup "$owner" "bd-boxd-$slot" "100.96.$slot.2" "100.96.$slot.1" 1.1.1.1 80000
    test ! -e "/run/netns/bd-boxd-$slot"
done
echo 'PASS: initialization barrier, public IPv4 return traffic, host/LAN/metadata/peer isolation, unsolicited ingress, spoofing and cleanup'

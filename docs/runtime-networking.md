# Opt-in runtime networking

Networking is available only for isolated state stores whose immutable
`isolation.json` includes:

```json
"network": {
  "helper": "/opt/boxd/bin/boxd-network",
  "namespace_scope": "boxd",
  "resolver": "1.1.1.1"
}
```

The only supported `namespace_scope` is `boxd`. The fixed host transit pool
supports exactly one network-enabled state store per host; a second store is
rejected rather than risking duplicate transit addresses. Omitting `network`
preserves networkless behavior (including existing immutable stores). NIC
topology is recorded in each box and snapshot; it is never inferred later.

The installer must copy `deploy/boxd-network` as a root-owned, non-writable
executable at the configured path and install `iproute2` and `nftables`. This
is supported by the fresh-install provisioner's explicit
`--network-uplink INTERFACE` option. After reviewing the plan, `INSTALL` authorizes
creation of the bridge/firewall and a `boxd-network.service` boot prerequisite.
Omitting the option makes no networking changes. This is not an upgrade path
for an existing immutable state store.

The shell bootstrap accepts `BOXD_NETWORK_UPLINK=INTERFACE` and installs the
additional prerequisites only after `SETUP` confirmation. It refuses old
packages without the helper. **The published bootstrap still pins v0.1.1**;
using this feature requires a new reviewed release and updated bootstrap pins.

An operator must explicitly enable host forwarding/NAT once, choosing the
public uplink:

```sh
sudo /opt/boxd/bin/boxd-network provision eth0
```

This is intentionally not run by the runtime. It creates `boxd0`, enables IPv4
forwarding, and installs the `boxd_global` nftables table. Review integration
with the host's firewall manager and persistence mechanism before production
use. Per-box lifecycle calls create only owned namespaces/TAPs and refuse to
adopt or remove resources whose ownership marker differs.

IPv4 egress is enabled only after guest initialization, then statefully NATed
in the per-VM namespace and again at the host uplink. Source spoofing, unsolicited return traffic,
IPv6, host-local destinations, guest-to-guest traffic, and private, link-local,
CGNAT, documentation, benchmark, multicast, and reserved ranges are blocked.
The configured public resolver is written to the guest's plain
`/etc/resolv.conf` after initialization; systemd-resolved is not required.

Privileged verification must run inside a disposable enclosing network
namespace. Ordinary Rust tests validate allocation, policy ranges, legacy
serialization, and snapshot metadata without changing host networking.

`sudo sh scripts/test-network.sh` exercises packet-level allow/deny behavior
inside disposable mount/network/PID namespaces. `scripts/test-network-kvm.sh`
takes a freshly built Ubuntu image directory and the compiled server lifecycle
test executable; it also launches real jailed VMs, uses a simulated public HTTP
endpoint, and cleans its uniquely named cgroup. Neither script modifies the
runner's shared routes/firewall. Fresh-host installation, reboot persistence,
and coexistence with other firewall managers still need separate validation.

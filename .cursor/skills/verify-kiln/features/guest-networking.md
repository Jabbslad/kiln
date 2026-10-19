# Guest networking

Guest IPv4 egress is off unless the state store was created with an isolation policy that names `kiln-network`. The development profile has no NIC. The laptop installer turns networking on only with `server --network` or `server --network-uplink INTERFACE`.

## Sub-features

- Isolation field `network.helper`, `namespace_scope` (`kiln` only), and `resolver` (`docs/runtime-networking.md`).
- `kiln-network provision UPLINK` creates `kiln0` and the host nftables table. The runtime does not call `provision`.
- Per-box `prepare`, `activate`, `cleanup`, and `run` (`deploy/kiln-network`).
- Eight slots, addresses `100.96.SLOT.2`, namespace `kn-kiln-SLOT` (`crates/kiln-runtime/src/network.rs`).
- One networked state store per host. A second store is rejected.

## How to get to it (user POV)

On a dedicated Ubuntu host, as root, after the server install with an uplink:

```sh
sudo /opt/kiln/bin/kiln-network provision eth0
```

New boxes from a network-enabled template receive a NIC. The guest's `/etc/resolv.conf` is written after initialization. `sudo sh scripts/test-network.sh` checks allow and deny rules inside disposable namespaces and does not boot a VM. `scripts/test-network-kvm.sh` boots jailed guests.

## Driving it with tmux

Do not run `provision` on a shared machine. The in-repo packet test is:

```sh
sudo sh scripts/test-network.sh
```

A KVM drive uses a freshly built Ubuntu image, an isolation config whose `network.helper` is `deploy/kiln-network`, and `kiln-runtime` as the operator. Success is a guest `exec` that reaches the simulated public HTTP endpoint used by `scripts/test-network-kvm.sh`, and a second guest that cannot reach a blocked range. Capture the script's stdout in `$VERIFY_EVIDENCE`.

## Gotchas

- `inspect` adopts an interrupted launch only when `record.network.is_none()` and hello reports `initialized: true` (`crates/kiln-runtime/src/runtime.rs:481-496`). A networked box is stopped instead. `docs/runtime.md` line 307 still says to adopt an initialized running VM, with no network exception.
- Hello is the readiness signal (`crates/kiln-runtime/src/runtime.rs:865-890`). `kiln-network activate` runs later (`crates/kiln-runtime/src/runtime.rs:1015-1028`), after guest `/sbin/ip` setup. A hello can succeed before the host `kiln_active` forward rule exists.
- `cleanup` exits 0 when the ownership marker is missing (`deploy/kiln-network:53-54`), even if `/run/netns/kn-kiln-SLOT` is still there. The next `prepare` refuses that namespace (`deploy/kiln-network:80`). Slot choice looks only at box records (`crates/kiln-runtime/src/runtime.rs:181-186`), so the leftover namespace occupies a slot the record list thinks is free.
- The Ubuntu image build does not check `/sbin/ip`. The lifecycle tests never start a NIC. See `templates-and-checkpoints.md`.
- This journey was not driven here. It needs root, a provisioned uplink, and KVM. CI does not run it (`.github/workflows/build.yml`).

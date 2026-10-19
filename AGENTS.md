# Delivery preference

After each completed iteration, verify the changes, commit, push, and publish a
release through the existing GitHub release workflow. This is the
owner's standing authorization; do not ask again for these delivery steps.
Keep the public installer pins in sync with verified release assets, publishing
only the designated bootstrap files to the installer repository.

The owner authorized public source and releases for Jabbslad/kiln. Audit for
credentials and private runtime data before publication. Release publication does not authorize
deployment, restarting the live server, migrating existing state, destructive
changes, or publishing credentials or runtime state. Preserve unrelated work.

## Cloud Agent

The environment bootstrap installs Rust 1.95 (musl target included), Node 22,
shellcheck, a static BusyBox, and Firecracker 1.17.0 under
`.tools/firecracker-1.17.0`. Login shells set `RUSTUP_HOME=/usr/local/rustup`
and `CARGO_HOME=/usr/local/cargo`. On boot, the dashboard dev server listens on
port 4173, and `/dev/kvm` is made writable when the device exists.

`cargo test --workspace --locked` does not launch a VM. `kiln-runtime doctor`
can report `development_ready` on a Cloud Agent VM because `/dev/kvm` opens,
but starting a guest hits a kernel fault in `kvm_arch_vcpu_create` on the
current 6.12 host. Do not run `kiln-runtime create`, guest image launches, or
the ignored KVM lifecycle tests in this environment. Use a Linux host where
creating a KVM vCPU succeeds for those checks.

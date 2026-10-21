# Boxes

A box is a persistent microVM. On the host, `kiln-runtime` creates it from an image. On a laptop, `kiln` creates it from a published template over HTTPS. Both can list, inspect, pause, resume, stop, start, and delete.

## Sub-features

- `kiln-runtime create|list|inspect|pause|resume|stop|start|delete` (`crates/kiln-runtime/src/bin/kiln-runtime.rs`).
- The same verbs on `kiln`, plus name and unique id-prefix targets (`crates/kiln-client/src/main.rs`).
- Development profile: unjailed Firecracker, no guest NIC, `--allow-unsafe-development` required.
- Isolated profile: jailer, per-box uid/gid, cgroup v2. Needs an empty state dir and `--isolation-config`.
- Client create picks the only published template and generates a name when those flags are omitted.

## How to get to it (user POV)

Host, after Doctor reports `development_ready`:

```sh
target/release/kiln-runtime --state-dir "$KILN_STATE" create \
  --image images/output/fixture/image.json \
  --name first-box --allow-unsafe-development
```

The command prints JSON, including `id` and `state`. Human-readable `kiln list` prints a table, or `No boxes yet. Create one with kiln create.`

Laptop path: add a profile (`features/private-ca.md` or `features/login.md`), then `kiln templates` and `kiln create --template ubuntu-4g --name first-box`. An empty catalog fails with `no templates available`.

## Driving it with tmux

```sh
tmux new-session -d -s kiln-box -c "$PWD" -- "${SHELL:-bash}" -l
tmux send-keys -t kiln-box 'export PATH="$PWD/.tools/firecracker-1.17.0:$PATH"' C-m
tmux send-keys -t kiln-box 'export KILN_STATE="'"$KILN_STATE"'"' C-m
tmux send-keys -t kiln-box 'target/release/kiln-runtime --state-dir "$KILN_STATE" doctor | tee "$VERIFY_EVIDENCE/doctor.json"' C-m
tmux send-keys -t kiln-box 'target/release/kiln-runtime --state-dir "$KILN_STATE" create --image images/output/fixture/image.json --name first-box --allow-unsafe-development | tee "$VERIFY_EVIDENCE/create.json"' C-m
```

Read `id` from `create.json`. Then:

```sh
target/release/kiln-runtime --state-dir "$KILN_STATE" exec "$BOX_ID" -- /bin/sh -c 'printf hello'
target/release/kiln-runtime --state-dir "$KILN_STATE" list
target/release/kiln-runtime --state-dir "$KILN_STATE" delete "$BOX_ID"
target/release/kiln-runtime --state-dir "$KILN_STATE" list
```

A pass is create JSON with `"state": "running"`, exec stdout `hello` and exit 0, the id present in the first list, delete JSON `{"schema_version":1,"deleted":"..."}`, and a second list whose `boxes` array is empty. `kill -0` on `process.pid` fails after delete.

Pause, resume, stop, and start are the same binary with that id. They were not required to call the create/exec/delete journey passed.

## Gotchas

- Read-write `/dev/kvm` is required (`crates/kiln-runtime/src/host.rs` `check`). On the authoring VM the node existed as root-owned mode 660 and `open` returned `EACCES` until `setfacl -m u:ubuntu:rw /dev/kvm`. Nested `KVM_CREATE_VM` then succeeded. Development commands still run as the unprivileged user.
- That was not enough to finish a guest. After the ACL, `kiln-runtime doctor` exited 0 with `development_ready: true` and Firecracker/jailer 1.17.0. `create --allow-unsafe-development` exited 1. stderr was `Firecracker API timed out; operation outcome unknown`. The guest log received `InstanceStart` and recorded no status line. The API client uses a 30 second timeout (`crates/kiln-runtime/src/runtime.rs:327`). The box record was left `failed`. Exec, pause, stop, and a running delete were not run. Do not treat doctor as a booted box.
- State directory mode must be `0700` and must not be a symlink. Memory is 128–4096 MiB, vCPUs 1–4, at most 8 boxes and 16384 MiB in one store (`crates/kiln-runtime/src/runtime.rs`).
- `--profile isolated` without a matching store returns `refusing unjailed fallback`. Root refuses `allow_unsafe_development`.
- Client create/exec go through the host journal. At 10,000 rows, `kiln-host` prunes old terminal operations before accepting more; create rows for live boxes are kept (`crates/kiln-server/src/journal.rs`, `docs/remote-client.md`). Direct `kiln-runtime` does not use that journal.
- After a host crash, `inspect` adopts a running guest only when `record.network.is_none()` (`crates/kiln-runtime/src/runtime.rs:481`). Networked boxes are covered in `guest-networking.md`.
- CI does not boot guests. Ignored tests start at `crates/kiln-runtime/tests/lifecycle.rs:226`. `.github/workflows/build.yml` runs `cargo test --workspace --all-features --locked` with no `--ignored` and no KVM job.
- The interrupted-jailer unit test waits until `/bin/sleep` is visible in procfs before it records the pid (`crates/kiln-runtime/src/runtime.rs` around the `sleep fixture did not finish exec` loop). That flake is closed. It is not a reason to skip this journey.

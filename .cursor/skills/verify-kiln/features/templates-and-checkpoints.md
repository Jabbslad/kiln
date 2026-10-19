# Templates and checkpoints

A template is a paused, prepared snapshot. `clone` starts a new box from it. A checkpoint is a memory and disk snapshot of one box. Restore rewinds that box. It does not clone it.

## Sub-features

- `kiln-runtime template build --image IMAGE --memory-mib N --vcpus N --allow-unsafe-development`
- `kiln-runtime template list`
- `kiln-runtime clone TEMPLATE_ID --name NAME --allow-unsafe-development`
- `kiln-runtime checkpoint save|list|delete`
- `kiln-runtime checkpoint restore BOX CHECKPOINT --acknowledge-external-state-replay`
- Laptop: `kiln templates` and `kiln create --template NAME`. The client does not build templates. The server catalog is the host config's published names.

## How to get to it (user POV)

After Doctor is green and an image exists:

```sh
target/release/kiln-runtime --state-dir "$KILN_STATE" template build \
  --image images/output/fixture/image.json \
  --allow-unsafe-development
```

The JSON `id` is the template id. `template list` shows it with `"template": true`. Clone:

```sh
target/release/kiln-runtime --state-dir "$KILN_STATE" clone "$TEMPLATE_ID" \
  --name from-template --allow-unsafe-development
```

Checkpoint, on a running box:

```sh
target/release/kiln-runtime --state-dir "$KILN_STATE" checkpoint save "$BOX_ID"
target/release/kiln-runtime --state-dir "$KILN_STATE" checkpoint restore "$BOX_ID" "$CHECKPOINT_ID" \
  --acknowledge-external-state-replay
```

Omitting the acknowledgement flag fails with `restore rewinds memory and disk, not external systems`.

Ubuntu templates are `images/build-ubuntu.sh`. The default mode pauses before systemd. `systemd_warm` and `systemd_warm_shared` are explicit third arguments.

## Driving it with tmux

Use a private `KILN_STATE`. Capture each JSON document under `$VERIFY_EVIDENCE`.

A pass is: template build JSON, `template list` containing that id, clone JSON with `"state": "running"` and a new id, exec `printf hello` on the clone, checkpoint save JSON, a guest write to a file on the disk, restore with the acknowledgement flag, and a following exec that shows the file is back to its pre-checkpoint contents. Delete the clone and any cold box before Cleanup.

The dashboard restore dialog is a different surface. Its button stays disabled (`features/dashboard.md`).

## Gotchas

- Restore without `--acknowledge-external-state-replay` is a hard error. The flag is the user's statement that external side effects are not undone.
- `images/build-ubuntu.sh` refuses to overwrite an image and refuses to run as root unless `KILN_FAKEROOT=1`, which the script sets itself via `fakeroot`.
- The Ubuntu builder checks OpenSSH paths (`images/build-ubuntu.sh` around the `sshd` test). It never checks that `/sbin/ip` exists. Boot runs `/sbin/ip` once per NIC (`crates/kiln-runtime/src/runtime.rs:970`). The lifecycle suite has no network branch (`crates/kiln-runtime/tests/lifecycle.rs` is entirely `#[ignore]` and does not mention a NIC).
- Checkpoint and template commands need KVM. They were not driven in the authoring proof. CI does not run them (`.github/workflows/build.yml` has no `--ignored` and no KVM job).
- `docs/runtime.md` "Crash recovery and its limits" says an interrupted launch adopts an initialized running VM. Adoption also requires `record.network.is_none()` (`crates/kiln-runtime/src/runtime.rs:481`). A networked guest is not adopted. Details are in `guest-networking.md`.

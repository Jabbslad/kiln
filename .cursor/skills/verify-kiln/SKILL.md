---
name: verify-kiln
description: Drive Kiln (self-hosted Linux microVMs) the way a user does and keep evidence. Covers kiln-runtime, the kiln laptop client, browser login, the installer, and the read-only web dashboard. Use when checking a Kiln change against a real checkout.
---

# Verify Kiln

Kiln has several user surfaces. Pick the one in the feature file and launch only that surface. Read the feature file before driving it. A missing precondition is a failed journey: record the reason in that file's Gotchas and leave the steps as they are.

Surfaces:

- `kiln-runtime` on a Linux host with KVM. This is the local box, exec, template, and checkpoint tool.
- `kiln` over HTTPS to `kiln-host` / `kiln-api`. This is the laptop client, including SSH and a private CA.
- `kiln login` against the identity service. The client default issuer is `https://dark-forge.dev`.
- The public door in `features/public-door.md`. A laptop dials `cli.dark-forge.dev:8443` after identity approval. The door checks the token before it dials loopback `kiln-api`.
- `deploy/bootstrap/install.sh` for client and server install.
- The dashboard in `web/`. It is a static React app. Preview mode does not call a VM API.

There is no in-tree server that implements `GET /api/v1/dashboard`. Workspace version is `0.4.7` (`Cargo.toml`).

Build once. Give each drive its own tmux session. Do not run `kiln-runtime` as root.

## Launch

From the repository root.

### Dashboard

No KVM and no Rust build. Node 22.12+ and npm.

```sh
cd web
npm ci
npx playwright install chromium
tmux new-session -d -s kiln-dash -c "$PWD" -- npm run dev -- --host 127.0.0.1 --port 4173 --strictPort
```

Record the pane pid (`tmux list-panes -t kiln-dash -F '#{pane_pid}'`). Ready means `http://127.0.0.1:4173/boxes?preview=1` renders the heading `Boxes` and the label `Design preview`. A listening port is not enough; run Doctor.

### Host runtime

Needs Linux x86_64, read-write `/dev/kvm`, cgroup v2, Rust 1.95.0, curl, Python 3, `file`, `readelf`, `mkfs.ext4`, and a static x86_64 BusyBox. Debian's `busybox-static` package provides `/usr/bin/busybox`.

```sh
bash scripts/fetch-firecracker.sh .tools/firecracker-1.17.0
export PATH="$PWD/.tools/firecracker-1.17.0:$PATH"
rustup target add --toolchain 1.95.0 x86_64-unknown-linux-musl
cargo build --locked --release -p kiln-runtime --bin kiln-runtime
bash images/build-fixture.sh images/output/fixture /usr/bin/busybox
export KILN_STATE="${KILN_STATE:-$PWD/.kiln-verify-state}"
rm -rf "$KILN_STATE"
mkdir -m 0700 "$KILN_STATE"
```

The fixture builder refuses to overwrite `image.json` or `rootfs.ext4`. Use a new output directory to rebuild.

Pass `--target x86_64-unknown-linux-musl` only to `kiln-guest`, which is what `images/build-fixture.sh` does. One `cargo build --target x86_64-unknown-linux-musl` also builds `kiln-runtime` for musl. libc 0.2.189 defines musl `Ioctl` as `c_int` and gnu `Ioctl` as `c_ulong`. The request constants in `crates/kiln-runtime/src/storage.rs` and `storage/verity.rs` are `c_ulong`, so the musl host build fails and the default gnu host build does not.

Ready means Doctor exits 0 and prints `"development_ready": true`. `kiln-runtime` itself exits after each command. Firecracker keeps running. Record `process.pid` from the create JSON so Cleanup can signal that pid if `delete` does not.

### Laptop API, login, installer

Those launches are in `features/private-ca.md`, `features/login.md`, and `features/installer.md`. Do not start them for a dashboard or runtime drive.

## Doctor

Dashboard, after Launch. This opens the preview and checks the rendered page. It does not click through the inventory.

```sh
cd web
node --input-type=module <<'EOF'
import { chromium } from '@playwright/test'
const browser = await chromium.launch()
const page = await browser.newPage()
const api = []
page.on('request', (request) => {
  if (request.url().includes('/api/')) api.push(request.url())
})
await page.goto('http://127.0.0.1:4173/boxes?preview=1')
const title = await page.title()
const heading = await page.getByRole('heading', { name: 'Boxes' }).isVisible()
const preview = await page.getByText('Design preview', { exact: true }).isVisible()
console.log(JSON.stringify({ title, heading, preview, api }))
await browser.close()
if (title !== 'Boxes — kiln' || !heading || !preview || api.length !== 0) process.exit(1)
EOF
```

Worth driving when that process exits 0. Preview must make no `/api/` request. If the heading is missing, stop.

Runtime:

```sh
target/release/kiln-runtime --state-dir "$KILN_STATE" doctor
```

Worth a guest drive when the exit status is 0 and `development_ready` is true. `isolated_ready` stays false unless `--isolation-config` points at a policy that passes preflight. That flag does not boot a VM. If `development_ready` is false, read `problems` and do not create a box. `/dev/kvm` can exist as mode 660 and still fail this check for a user outside the device group.

## Drive

One tmux session per drive. The feature file has the commands and the selectors.

```sh
tmux new-session -d -s kiln-drive -c "$PWD" -- "${SHELL:-bash}" -l
```

Send one command, wait until it returns, then `tmux capture-pane -p -t kiln-drive`. Use ids printed by the tool. `kiln-runtime` takes a full 32-character hex id. `kiln` also accepts a box name or a unique id prefix of at least 8 hex characters (`crates/kiln-client/src/main.rs`).

## Evidence

```sh
export VERIFY_EVIDENCE="${VERIFY_EVIDENCE:-/tmp/verify-kiln-evidence}"
mkdir -p "$VERIFY_EVIDENCE"
```

Cleanup must not delete this directory. For each command, save stdout, stderr, and the exit code. Save `tmux capture-pane -p` as well. For the dashboard, set `KILN_SCREENSHOT_DIR` to a directory inside `$VERIFY_EVIDENCE` before `npm run test:browser`. Screenshots are the rendered preview, not a running VM.

A box drive is done when create JSON has `"state": "running"`, exec stdout is the bytes you asked the guest to print, list contains that id, delete reports it, and a later list has no boxes. Then `kill -0` on the recorded pid must fail.

Workspace baseline, saved next to the drive. These commands do not boot a guest. `cargo test` does not pass the `#[ignore]` KVM tests.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
```

Do not copy tokens, CA private keys, enrollment files, or `laptop.tar.gz` into the evidence directory.

## Cleanup

Stop only processes you started, by pid or tmux session.

Dashboard:

```sh
pid=$(tmux list-panes -t kiln-dash -F '#{pane_pid}')
kill "$pid"
tmux kill-session -t kiln-dash
```

Runtime: `kiln-runtime --state-dir "$KILN_STATE" delete "$BOX_ID"`. If `process.pid` from create is still alive, `kill` that pid. Then `rm -rf "$KILN_STATE"`. Leave `$VERIFY_EVIDENCE` in place and `test -s` a file you wrote before cleanup.

## Helpers

This skill ships no extra script. Repo scripts:

| Script | Invocation |
| --- | --- |
| `scripts/fetch-firecracker.sh` | `bash scripts/fetch-firecracker.sh .tools/firecracker-1.17.0` |
| `images/build-fixture.sh` | `bash images/build-fixture.sh images/output/fixture /usr/bin/busybox` |
| `images/build-ubuntu.sh` | `bash images/build-ubuntu.sh images/output/ubuntu` |
| `scripts/test-bootstrap.py` | `python3 scripts/test-bootstrap.py` |
| `scripts/test-install.py` | `python3 scripts/test-install.py` |
| `scripts/test-release.py` | `python3 scripts/test-release.py` |
| `scripts/test-network.sh` | `sudo sh scripts/test-network.sh` |
| `scripts/test-network-kvm.sh` | see the script header; needs a built Ubuntu image |
| `scripts/test-ssh-pty.py` | `python3 scripts/test-ssh-pty.py SSH_CONFIG HOST` on an already running guest |
| `scripts/test-warm-guest.py` | `python3 scripts/test-warm-guest.py IMAGE_JSON FIRECRACKER OUTPUT_DIR` |
| `scripts/test-central-login.py` | after a real enrolled host and a disposable running box; it does not log in |

## Maintenance

Keep the map honest with `/maintain-verification-skill`.

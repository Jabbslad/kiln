# Exec

Exec runs one argv inside a running box and returns the guest's stdout, stderr, and exit code. `kiln-runtime` prints a JSON object and uses the guest exit code as its own status (124 on timeout, 125 when the code is missing). `kiln exec` writes guest stdout and stderr through, and prints the operation JSON only with `--json`.

## Sub-features

- `kiln-runtime exec ID --timeout-ms N --cwd DIR --env KEY=VALUE -- ARGV...`
- `kiln exec BOX --timeout-ms N --cwd DIR --env KEY=VALUE -- ARGV...`
- Timeout bounds: runtime rejects values outside 1..3600000 ms. The client default wait is 300 seconds, separate from the guest timeout.
- `--request-id` on the client reuses one operation. A changed body returns 409.
- `--no-wait` returns the accepted operation. `kiln operation ID --wait` polls it.

## How to get to it (user POV)

The box is already running (`boxes.md`). On the host:

```sh
target/release/kiln-runtime --state-dir "$KILN_STATE" exec "$BOX_ID" -- /bin/sh -c 'printf hello'
```

On the laptop, the same argv after `kiln list` shows the box running. Stopped and paused boxes are not started for you. The client suggests `kiln start` or `kiln resume` (`crates/kiln-client/src/main.rs` `box_state_hint`).

## Driving it with tmux

In the box session, after create:

```sh
target/release/kiln-runtime --state-dir "$KILN_STATE" exec "$BOX_ID" -- /bin/sh -c 'printf hello' \
  > "$VERIFY_EVIDENCE/exec-stdout.txt"
echo $? | tee "$VERIFY_EVIDENCE/exec-exit.txt"
```

Runtime JSON is on stdout, so the shell redirect captures the JSON, not a raw `hello`. The field `stdout` must be `hello`, `exit_code` 0, `timed_out` false. The process exit status is 0.

A non-zero guest status is the client contract in `cargo xtask central-login`: the guest writes four stdout bytes, `guest-error\n` on stderr, and exits 23. That command needs a real enrolled host. Do not point it at a fixture profile.

## Gotchas

- Exec on a box that is not `running` returns before guest I/O (`crates/kiln-runtime/src/runtime.rs:1037-1041`, `cannot execute while ...`). Through `kiln-host`, that error is stored as `Unknown`, because every exec failure uses `OperationState::Unknown` (`crates/kiln-server/src/host.rs:360-364`). The saved message is the generic "did not return a confirmed result" line, not the runtime's state string.
- A client retry without the same `--request-id` can allocate another operation. The journal stops accepting at 10,000 rows (`crates/kiln-server/src/journal.rs:73-80`).
- `kiln-runtime` JSON uses lossy UTF-8 for stdout. Embedded NULs are a client concern; the runtime field is a JSON string.
- This journey needs a running guest. See `boxes.md` for the KVM permission check. CI does not run it (`.github/workflows/build.yml` has no KVM job).

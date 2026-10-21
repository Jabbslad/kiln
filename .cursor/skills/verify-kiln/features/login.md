# Login

`kiln login` is the device-code flow. The client prints a code and a URL, opens a browser, and polls until the identity service returns tokens. The default issuer is `https://dark-forge.dev` (`crates/kiln-api/src/auth.rs`, `DEFAULT_IDENTITY_ORIGIN`). Override it with `--issuer` or `KILN_IDENTITY_URL`.

## Sub-features

- `kiln login` and `kiln login --server SERVER_ID`.
- `kiln --profile NAME login` to keep an existing direct administrator profile.
- `kiln auth status`, `kiln auth devices list`, `kiln auth devices revoke ID`.
- `kiln logout` revokes the device remotely, then deletes the local credential.
- `kiln auth keys create|list|revoke` for a server-scoped automation key.
- Host enrollment: `kiln-api enroll` prints a one-time code. The laptop discovers the host after activation.

## How to get to it (user POV)

```sh
kiln login
```

The terminal prints `Open URL and enter code CODE`. The browser page is `/device` on the issuer. Sign in with GitHub or Google, then approve. With one enrolled server, login saves a profile and exits 0. With none, it exits with `no active servers are available` and tells you to run `kiln-api enroll`.

`kiln-identity` itself listens on loopback HTTP (`crates/kiln-identity/src/main.rs`, default `127.0.0.1:8091`). The CLI client is HTTPS-only and uses the public root store. A loopback process is not a login target.

## Driving it with tmux

Use a disposable config directory so the home profile is untouched.

```sh
export KILN_CONFIG="$VERIFY_EVIDENCE/../kiln-login-config/profiles.json"
mkdir -m 0700 "$(dirname "$KILN_CONFIG")"
tmux new-session -d -s kiln-login -c "$PWD" -- "${SHELL:-bash}" -l
tmux send-keys -t kiln-login 'kiln --config "$KILN_CONFIG" --issuer https://dark-forge.dev login' C-m
```

`kiln --json login` exits before a device code with `browser login requires an interactive terminal`. One server is saved. Several servers save the approved session, then prompt on a terminal. With stdin closed, or a terminal that sends no selection, login exits non-zero and prints `kiln login --server` with each server id. A later `kiln login --server` does not request a new device code. Capture the pane. A pass is exit 0, `kiln --config "$KILN_CONFIG" auth status` JSON containing `device_id` and `account_id`, and a profile file that has `issuer` and `credential_file` and no `token_file`.

Do not run this against `https://dark-forge.dev` from an unattended agent. Approval needs a real GitHub or Google account, and a successful login writes a credential.

## Gotchas

- Completing approval needs the real issuer and a provider account. The authoring proof did not log in.
- `kiln-identity` will not bind a non-loopback address. Its `public_origin` must be an HTTPS origin with an empty path. `https://dark-forge.dev` refuses substitute GitHub URLs. `--github-authorize-url`, `--github-token-url`, `--github-user-url`, and `--provider-ca-file` exist only on a build with the non-default `lab-provider` feature. A release binary does not accept them.
- First-use interactive VM commands can start login. `--json`, SSH proxy, and a missing explicit profile do not open a browser.
- `cargo xtask central-login` checks an already approved profile. It is not a substitute for this journey.
- Device requests last ten minutes. The client poll interval starts at the value the server returns (5 seconds in the identity store).

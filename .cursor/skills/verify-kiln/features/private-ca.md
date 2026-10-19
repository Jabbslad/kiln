# Private CA

A direct administrator profile trusts one server certificate. `kiln profile add` stores the URL, a 64-hex token file, and an optional `--ca-file`. With a CA, the client disables the public root store (`crates/kiln-client/src/lib.rs`). Identity login does not take a CA file. That client uses public roots only.

## Sub-features

- `kiln profile add NAME --url https://HOST:8443 --token-file PATH --ca-file PATH`
- `kiln profile list`
- Authenticated `kiln templates` and `kiln list` against that profile.
- A wrong CA, a public-root-only client, or a bad token fails before any box command.
- Server side: `kiln-api` presents the cert. `kiln-host` serves the runtime on a Unix socket. A missing enrollment file is allowed (`Enrollment::load` returns none on not-found).

## How to get to it (user POV)

The server installer writes `/etc/kiln/laptop.tar.gz` with the CA, the administrator token, and `CONNECT.txt`. Treat that archive as a password. On the laptop:

```sh
kiln profile add default --url https://SERVER:8443 --token-file ./admin.token --ca-file ./ca.crt
kiln templates
kiln list
```

Token file mode is `0600` or `0400`. The URL is an HTTPS origin with no path, query, user, or fragment.

A local empty catalog, without KVM, is the same client path. Build `kiln`, `kiln-host`, and `kiln-api`. Use a private directory mode `0700` for the socket parent (`crates/kiln-server/src/host.rs` `bind_socket`). `kiln-host` with `allow_unsafe_development: true` and `"templates": {}` starts without a guest image. `kiln-api` must not run as root. Point `--enrollment-file` at a path that does not exist, not at a real `/etc/kiln/identity.json`.

The certificate SAN must match the URL host. The in-tree client tests use `localhost`.

## Driving it with tmux

Start `kiln-host` and `kiln-api` in two tmux sessions and record both pane pids. Ready is HTTP 200 and a JSON array:

```sh
curl --fail --silent --show-error --cacert "$CA" \
  -H "Authorization: Bearer $(cat "$TOKEN")" \
  https://localhost:8443/v1/templates
```

Then:

```sh
kiln --config "$KILN_CONFIG" profile add default \
  --url https://localhost:8443 --token-file "$TOKEN" --ca-file "$CA"
kiln --config "$KILN_CONFIG" --json templates
kiln --config "$KILN_CONFIG" list
```

A pass is `added profile default`, templates JSON `[]`, and `No boxes yet. Create one with kiln create.` Repeat `templates` with a different CA file and expect a TLS error. `curl` without `--cacert` must fail. Delete the lab directory in Cleanup after killing the two pane pids. Do not leave the token in `$VERIFY_EVIDENCE`; record only status codes and the redacted command output.

## Gotchas

- An empty catalog cannot `kiln create`. That failure is the product rule, not a broken profile.
- `kiln-api` reads the token with `kiln_api::read_token`: 64 hex characters, regular file, mode with no group or other bits.
- Two host processes cannot share one runtime directory (`.service.lock`). Two development stores can run side by side. Two networked stores cannot (`features/guest-networking.md`).
- This journey was not driven in the authoring proof. The commands above are the binary flags and the client tests in `crates/kiln-client/src/lib.rs` (`private_ca_reaches_its_own_server_and_not_a_different_ca`). CI does run the client TLS tests on the `client` job (`.github/workflows/build.yml`).
- Create and exec through this API still hit the 10,000-row journal (`crates/kiln-server/src/journal.rs:73-80`) and the exec `Unknown` state (`features/exec.md`).

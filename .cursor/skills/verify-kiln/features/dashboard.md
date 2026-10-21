# Dashboard

The console in `web/` is a React app served as static files. Release `v0.4.6` publishes `kiln-dashboard-v0.4.6.tar.gz`. The archive is not installed by `deploy/install.sh`. Preview mode (`?preview=1`) renders `web/src/fixtures.ts` and does not call the network. Without that flag the app requests `GET /api/v1/dashboard` (`web/src/data.ts`).

`kiln-host` builds the document and serves that GET on its Unix socket (`crates/kiln-server/src/host.rs`). Loopback `kiln-api` proxies it (`crates/kiln-server/src/gateway.rs`). The installed unit listens on `127.0.0.1:8443` (`deploy/kiln-api.service`). `kiln-door` (`kiln-api door`, `deploy/kiln-door.service`) listens on `:8443`, checks an identity JWT, and dials that loopback API (`crates/kiln-server/src/door.rs`).

## Who can read it

A read-scope owner JWT opens the public door. The audience is `kiln:server:` plus the enrolled server id, and the subject is the enrolled owner. The door forwards `GET /api/v1/dashboard` to loopback `kiln-api` with the same `Authorization` header. The certificate on the door is signed by the server CA.

The administrator token is a 64-character hex secret read from the token file. Loopback `kiln-api` accepts it for this GET. The door rejects it.

A missing credential or a bogus bearer is HTTP 401, code `unauthorized`, on the door and on the gateway. The door does not dial loopback `kiln-api`. The gateway does not dial the host socket. A query string is HTTP 400, code `invalid_request`, and the host socket is not dialed.

## Document

`schema_version` is `1`. `observed_at` is UTC `YYYY-MM-DDTHH:MM:SSZ`. The gateway takes that clock after authentication and writes it onto the document. `Content-Type` contains `application/json`. `Cache-Control` is `no-store`.

`workspace` is the enrollment server id on an enrolled server. An unenrolled server publishes `local`. `host.name` is that same string.

`host.state` is `online` when the host socket answers. When the dial fails, the gateway still returns HTTP 200, `host.state` is `offline`, and `boxes`, `templates`, `checkpoints`, and `operations` are empty. The socket path is absent from the body.

`checkpoints` is an empty array. Metric fields are null. A box is listed only when the journal has its create row. Box `state` is `running`, `paused`, `stopped`, `failed`, `creating`, or `unknown`.

Operations are the newest journal rows. `kiln-host` reads 100 rows ordered by `created_at` descending, then `id` (`crates/kiln-server/src/journal.rs`, `recent`). A row whose stored JSON does not parse is skipped. That skip happens after the 100-row read, so a corrupt row still uses a slot. An operation whose state is unknown is omitted. `kind` is `create` for a create row and `unknown` otherwise. `state` is `running`, `completed`, or `failed`. `detail` is the operation error text. Exec stdout and stderr are not copied into the document.

Error text that contains `/` or `\` is dropped (`crates/kiln-server/src/dashboard.rs`, `public_error`). On a box, `last_error` is null. On an operation, `detail` is empty.

A host body larger than 1 MiB (`kiln_api::MAX_RESPONSE_BYTES`) makes `stamp` return HTTP 502 with code `host_unavailable`. The test is `oversized_host_body_is_an_error` in `crates/kiln-server/src/gateway.rs`. On the route, `proxy` returns that same error while it is reading the stream, and the handler publishes the offline document for every `host_unavailable` error, including that size cap.

`web/src/data.ts` exports `dashboardSchema`. The contract test `dashboard_schema_accepts_enrolled_and_unenrolled_servers` in `crates/kiln-server/tests/dashboard_api.rs` parses three gateway bodies with it: enrolled and online, unenrolled and online, unenrolled and offline.

## Sub-features

- Inventory at `/boxes`: search, state filters, name sort, row links.
- Box detail at `/boxes/:id` with terminal, exec, checkpoint, and preview tabs.
- Templates at `/templates`, including the create-box drawer.
- Operations at `/operations`.
- Settings at `/settings` and the guide at `/docs`.
- Explicit preview versus a disconnected live view.
- Create, restore, resume, and exec stay disabled in preview (`web/src/dialogs.tsx`, `web/src/BoxDetail.tsx`).
- A terminal session on a running box, beside copyable `kiln ssh` commands. See below.

## Terminal session

The terminal is a session. It stays out of `kiln_api::Action`, `OPERATION_ACTION_TYPES` in `web/src/operationKinds.ts`, the `satisfies Record<OperationActionType, …>` tables and `setDashboardObserver` in `web/src/data.ts`, and `trackerCases` in `web/src/data.test.ts`. The contract test `dashboard_operation_action_types_match_api_action_tags` in `crates/kiln-api/src/lib.rs` compares those tags with the Rust enum.

`kiln-app` serves `GET /v1/boxes/{id}/terminal` (`crates/kiln-server/src/app_listener.rs`, `crates/kiln-server/src/terminal.rs`). The browser opens a same-origin WebSocket and sends the `__Host-kiln_app` and `__Host-kiln_csrf` cookies. `Origin` must be `https://app.dark-forge.dev`. The session must be operate scope. The server checks ownership by reading the box SSH host key, writes one audit line when the socket upgrades (`audit terminal outcome=opened box_id=…`), and bridges an OpenSSH PTY over the existing SSH upgrade. Idle limit is 15 minutes. Maximum duration is 2 hours. Binary frames are PTY bytes. A text frame `{"type":"resize","cols":N,"rows":M}` resizes the PTY. The server ends the socket with `{"type":"closed",…}`.

Connect in `TerminalPane` (`web/src/BoxDetail.tsx`) is enabled only when the console is live, the host is `online`, and the box is `running`. Preview, a stopped box, and an offline host leave the button disabled and the heading at `Session not connected`. The heading says `Connected` only while the socket is open. xterm.js loads when Connect is used. The Access panel on the same tab copies `kiln ssh {id}` and `kiln ssh-config {id}`. HTTP preview stays labelled **Not available yet**.

The in-repo proof does not boot Firecracker. `openssh-server` must provide `/usr/sbin/sshd`, and `/run/sshd` must exist. CI installs that before server-check.

```sh
cargo test -p kiln-server --test app_listener --locked -- --nocapture \
  terminal_rejects_unauthenticated_read_scope_bad_origin_and_other_owner
cargo test -p kiln-server --test app_listener --locked -- --nocapture \
  operate_session_gets_a_pty_echo_from_a_running_box
cargo test -p kiln-api --lib --locked -- --nocapture \
  dashboard_operation_action_types_match_api_action_tags
```

A rejection pass is HTTP 401 `unauthorized` with no cookie, HTTP 403 `insufficient_scope` for a read session, HTTP 403 `invalid_origin` for another origin, and HTTP 401 `unauthorized` for another owner. Those four do not dial the host. An operate session for an unknown box is HTTP 404 `not_found` and does not audit `opened`. A shell pass prints `kiln-term-ok` and audits `outcome=opened` with that box id.

```sh
cd web
npm test -- src/App.test.tsx src/BoxDetail.test.tsx
```

`enables Connect terminal only for a running box on a live console` covers the button. `shows copyable kiln ssh commands with the full box id and no SSH gateway placeholder` covers the Access panel. Playwright on `/boxes?preview=1` still expects `Session not connected` and a disabled Connect button. That run does not open a shell.

A browser shell on a deployed console needs `kiln-app`, an operate-scoped `__Host-kiln_app` session, `Origin: https://app.dark-forge.dev`, and a running box. The Vite dev server does not proxy `/v1/boxes/{id}/terminal`. Caddy and the app socket proxy already forward the upgrade, so this journey does not change install or deploy.

## How to get to it (user POV)

From a checkout, with Node 22:

```sh
cd web
npm ci
npm run dev
```

Open `http://127.0.0.1:5173/boxes?preview=1`. The dev server binds to loopback. The Playwright config uses port 4173:

```sh
npm run dev -- --host 127.0.0.1 --port 4173 --strictPort
```

A published archive is extracted into a static root served at `/`, with SPA fallback to `index.html`. Open `/boxes?preview=1` on that host. Serving the archive was not part of the authoring proof.

The header shows `Design preview` in preview mode. The footer shows `READ-ONLY DESIGN PREVIEW`.

## Driving the live document

`$ADMIN_TOKEN`, `$OWNER_JWT`, `$BOGUS`, and `$CA` are placeholders. Put the credential in the environment for the command. Do not write a token, key, or secret into this file, the shell transcript you save, or `$VERIFY_EVIDENCE`.

Loopback administrator token. The lab shape is `features/private-ca.md`: `kiln-host` and `kiln-api`, private CA, token file mode `0600` or `0400`, and an `--enrollment-file` that is absent for an unenrolled server. The URL host must match the certificate.

```sh
curl --silent --show-error --cacert "$CA" \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -D - \
  https://127.0.0.1:8443/api/v1/dashboard
```

A pass is HTTP 200, `schema_version` 1, `cache-control: no-store`, and JSON. `workspace` is `local` when the process has no enrollment, and the server id when it does. `host.state` is `online` while `kiln-host` is listening and `offline` when the socket is missing. Stop `kiln-host` and repeat the curl to see the offline document.

These two are HTTP 401 and do not dial the host:

```sh
curl --silent --show-error --cacert "$CA" \
  -o /dev/null -w '%{http_code}\n' \
  https://127.0.0.1:8443/api/v1/dashboard
curl --silent --show-error --cacert "$CA" \
  -o /dev/null -w '%{http_code}\n' \
  -H "Authorization: Bearer $BOGUS" \
  https://127.0.0.1:8443/api/v1/dashboard
```

Public door, read-scope owner JWT. Use the server CA. Do not send the administrator token to this URL.

```sh
curl --silent --show-error --cacert "$CA" \
  -H "Authorization: Bearer $OWNER_JWT" \
  -D - \
  https://cli.dark-forge.dev/api/v1/dashboard
```

A pass is HTTP 200 and `workspace` equal to that server's id. The same URL with no header, with `Authorization: Bearer $BOGUS`, or with `Authorization: Bearer $ADMIN_TOKEN`, is HTTP 401.

The in-repo proof does not need a guest:

```sh
cargo test -p kiln-server --test dashboard_api --locked -- --nocapture \
  dashboard_schema_accepts_enrolled_and_unenrolled_servers
cargo test -p kiln-server --test public_door --locked -- --nocapture \
  dashboard_read_rejects_untrusted_bearers_and_forwards_the_owner_jwt
cargo test -p kiln-server --lib --locked -- --nocapture \
  oversized_host_body_is_an_error
```

The schema test runs `npm ci` in `web/` when `web/node_modules/zod` is missing, then runs the gateway JSON through `dashboardSchema`. `read_scope_and_admin_token_receive_schema_version_one` in the same file covers the administrator token, the read-scope JWT, and HTTP 401 for a missing or bogus bearer. The door test forwards an owner JWT, and it checks that a missing bearer, a bogus bearer, and the administrator token stay at HTTP 401 without a dial.

## Driving it with Playwright

Install the browser once: `npx playwright install chromium`. Set `KILN_SCREENSHOT_DIR` to an existing directory to keep screenshots. The suite starts or reuses the dev server (`web/playwright.config.ts`).

```sh
cd web
KILN_SCREENSHOT_DIR="$VERIFY_EVIDENCE/dashboard" npm run test:browser
```

The inventory test (`web/tests/dashboard.spec.ts`) opens `/boxes?preview=1`, expects the title `Boxes — kiln`, types `worker` into the search box, and expects the link `worker-test`. The Running filter then shows `No matching boxes`. `Clear filters` brings the full table back. Sorting by Name puts `worker-test` first. Opening `api-dev` shows `Session not connected` and a disabled `Connect terminal` button. The exec tab accepts `["/bin/echo", "hello"]` as valid JSON and does not send it. The create drawer rejects `My Box!`, refuses the existing name `api-dev`, and leaves `Create box` disabled. Restore stays disabled after the acknowledgement checkbox. Search hides `api-dev`; clear the filter before opening that row.

Also open the Vite server without `?preview=1`. That process does not proxy `kiln-api`, so the page is the disconnected screen:

```sh
cd web
node --input-type=module <<'EOF'
import { chromium } from '@playwright/test'
const browser = await chromium.launch()
const page = await browser.newPage()
await page.goto('http://127.0.0.1:4173/boxes')
await page.getByRole('heading', { name: 'Connect your control plane' }).waitFor()
const message = await page.locator('.connection-error').innerText()
console.log(JSON.stringify({ url: page.url(), message }))
if (process.env.KILN_SCREENSHOT_DIR) {
  await page.screenshot({ path: process.env.KILN_SCREENSHOT_DIR + '/live-disconnected.png', fullPage: true })
}
await browser.close()
EOF
```

A pass is the preview heading plus zero `/api/` requests, a disabled terminal button, and the live route showing `Connect your control plane` with the API error text. Fixture box names do not appear on the live route.

## Gotchas

- Preview data is labelled example data. It is not the local runtime inventory.
- `Create box` and `Restore checkpoint` stay disabled in preview. `Connect terminal` stays disabled in preview, on a stopped box, and while the host is offline. A live console enables it when the host is online and the box is running. A green Playwright run does not create a box and does not open a shell. The PTY echo test is the working-shell proof.
- The terminal is a session. Leave it out of `OPERATION_ACTION_TYPES` and the operation tracker. Identity still issues the dashboard cookie at read scope, so a signed-in console cannot open the shell until that session can operate.
- CI runs this surface. `.github/workflows/build.yml` job `dashboard` runs `npm test`, Prettier, `npm run build`, and `npm run test:browser`. It does not boot Firecracker.
- `README.md` still lists dashboard among outstanding platform work, and its intro still says the dashboard API is not integrated. `web/README.md` still says the CLI API does not implement this endpoint. Those sentences are behind `kiln-api`.
- The authoring proof used the Vite dev server and Playwright. It did not extract the release tarball or attach the static app to a host. The live document drive above is curl against `kiln-api`, or the contract test.
- Record status codes and field names. Leave `Authorization` values out of the evidence directory.

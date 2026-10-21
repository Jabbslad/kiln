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

`checkpoints` is an empty array. `host.metrics.cpu_percent` and `host.metrics.memory_bytes` come from `/proc/stat` deltas and `/proc/meminfo` while `kiln-host` is online. A running or paused isolated box reads its jailer cgroup `cpu.stat` (`usage_usec` deltas) and `memory.current`; other boxes keep null metrics. `disk_bytes` in metrics stays null. The dashboard marks `metrics` as `available` in `web/src/capabilities.ts`; stopped or inactive boxes show **Stopped** or **Not available yet** instead of em dashes. A box is listed only when the journal has its create row. Box `state` is `running`, `paused`, `stopped`, `failed`, `creating`, or `unknown`.

Operations are the newest journal rows. `kiln-host` reads 100 rows ordered by `created_at` descending, then `id` (`crates/kiln-server/src/journal.rs`, `recent`). A row whose stored JSON does not parse is skipped. That skip happens after the 100-row read, so a corrupt row still uses a slot. An operation whose state is unknown is omitted. `kind` is `create` for a create row and `unknown` otherwise. `state` is `running`, `completed`, or `failed`. `detail` is the operation error text. Exec stdout and stderr are not copied into the document.

Error text that contains `/` or `\` is dropped (`crates/kiln-server/src/dashboard.rs`, `public_error`). On a box, `last_error` is null. On an operation, `detail` is empty.

A host body larger than 1 MiB (`kiln_api::MAX_RESPONSE_BYTES`) makes `stamp` return HTTP 502 with code `host_unavailable`. The test is `oversized_host_body_is_an_error` in `crates/kiln-server/src/gateway.rs`. On the route, `proxy` returns that same error while it is reading the stream, and the handler publishes the offline document for every `host_unavailable` error, including that size cap.

`web/src/data.ts` exports `dashboardSchema`. The contract test `dashboard_schema_accepts_enrolled_and_unenrolled_servers` in `crates/kiln-server/tests/dashboard_api.rs` parses three gateway bodies with it: enrolled and online, unenrolled and online, unenrolled and offline.

## Sub-features

- Inventory at `/boxes`: search, state filters, name sort, row links.
- Box detail at `/boxes/:id` with workspace, exec, checkpoint, and preview tabs.
- Capability guard in `web/src/capabilities.ts`: metrics, terminal, SSH, HTTP preview, public URL, and launch timing each declare `available` or `not-yet`. Panels read that map and render the shared **Not available yet** state instead of blank placeholders or fake broken controls. Vitest in `web/src/capabilities.test.tsx` fails if an `available` capability shows `—`, `-`, `N/A`, or empty values, or if a `not-yet` capability renders anything else. Metrics is `available` and reads live host and box telemetry from the dashboard document.
- Templates at `/templates`, including the create-box drawer.
- Operations at `/operations`.
- Settings at `/settings` and the guide at `/docs`.
- Explicit preview versus a disconnected live view.
- Create, restore, resume, exec, and terminal controls that stay disabled (`web/src/dialogs.tsx`, `web/src/BoxDetail.tsx`).

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
- `Create box`, `Restore checkpoint`, and `Connect terminal` are hard-disabled. A green browser run does not create a box.
- CI runs this surface. `.github/workflows/build.yml` job `dashboard` runs `npm test`, Prettier, `npm run build`, and `npm run test:browser`. It does not boot Firecracker.
- `README.md` still lists dashboard among outstanding platform work, and its intro still says the dashboard API is not integrated. `web/README.md` still says the CLI API does not implement this endpoint. Those sentences are behind `kiln-api`.
- The authoring proof used the Vite dev server and Playwright. It did not extract the release tarball or attach the static app to a host. The live document drive above is curl against `kiln-api`, or the contract test.
- Record status codes and field names. Leave `Authorization` values out of the evidence directory.

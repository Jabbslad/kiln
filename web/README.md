# Kiln website and dashboard

React/TypeScript public landing page and inventory-first console, styled from the
root [design.md](https://github.com/Jabbslad/kiln/blob/main/design.md). That reference is preserved with a Kiln adaptation
section documenting the imagery, font substitution, and accessibility choices.
Geist and Geist Mono are bundled locally; there are no third-party font requests.
The original generated landscape is a local 70 KB WebP, not a live installation photograph.

## Run

Node 22.12+ (or a newer supported Node release) and npm are required.

```sh
npm ci
npm run dev
```

Open `/` for the public landing page (no API requests), `/boxes` for the live connection state, or `/boxes?preview=1` to explicitly explore read-only example data. The dev server binds to loopback by default. To view it through a trusted runner connection, use `npm run dev -- --host 0.0.0.0 --port 4173`. This is a development server, not a production deployment.

## Delivery boundary

Implemented: responsive inventory, search and sorting, state filters, box detail tabs, template selection and create form validation, create, pause, resume, stop (optional force), start, and delete when the console is live and the host is online, same-box checkpoint/restore confirmation, lifecycle explanations, operation details, loading/empty/error states, settings, and the console guide.

**Create, pause, resume, stop, start, and delete are the mutations this console sends.** A live authenticated console (`GET /api/v1/dashboard` succeeded and the host is `online`, not preview) can submit them. Stop includes an optional force flag. Delete asks for the box name and submits only when the typed name matches. The buttons stay disabled in preview, while disconnected, and while the host is offline. Exec, restore, terminal sessions, SSH, secrets, and HTTP preview routing stay unavailable. Their controls never report success or mutate fixture state. A created box is added to the inventory only after a later dashboard response contains it. A lifecycle action changes the displayed box state only after a later dashboard response reports that state. A deleted box leaves the inventory only after a later dashboard response omits it.

Preview mode uses `src/fixtures.ts`, is labelled across all screens, makes no API requests, and is never automatically entered on a failed connection. Example resource sizes and host status are not claims about the local runtime.

## Published dashboard

GitHub releases include `kiln-dashboard-v0.4.10.tar.gz` and its SHA-256 checksum.
The archive contains the static build: production HTML, JavaScript, CSS, imagery, and bundled fonts.
Node is only needed to build from source, not to serve the archive. It is separate from the server
installer and is not deployed or connected to a host automatically.

Extract it into a static web root served at `/`, with SPA fallback to
`index.html`. `/` is the public landing page. Open `/boxes?preview=1` for the labelled design preview. On `/boxes` without
preview mode, a request that gets no session sees the disconnected screen.
`GET /api/v1/dashboard` is implemented on the CLI API. This archive does not
add a browser session.
Do not embed administrator tokens or bypass authentication to connect it.

## Read-only API contract

The dashboard issues **one** same-origin `GET /api/v1/dashboard` on connection or explicit refresh. Requests include `Accept: application/json`, same-origin credentials, and `cache: no-store`. The client aborts after 10 seconds and never retries automatically. Serve JSON with `Content-Type: application/json`. The response's required schema is exported as `dashboardSchema` from `src/data.ts`; the TypeScript `Dashboard` type is inferred from it. `src/fixtures.ts` is a complete, schema-validated example response, but not a server.

Top-level fields:

| Field            | Contract                                                                                |
| ---------------- | --------------------------------------------------------------------------------------- |
| `schema_version` | Exactly `1`                                                                             |
| `observed_at`    | ISO-8601 UTC snapshot timestamp; displayed explicitly                                   |
| `workspace`      | Authenticated workspace name                                                            |
| `host`           | Name, observed `online`/`offline`/`unknown` state, metrics                              |
| `boxes`          | Workspace-owned IDs, names, observed states, allocation, nullable metrics/timing/errors |
| `templates`      | Prepared templates only, allocation and host compatibility                              |
| `checkpoints`    | ID, owning `box_id`, compatibility, capture time and nullable size                      |
| `operations`     | Request kind, observed status, box identity, timestamp, detail and progress steps       |

All metric values are nullable, not optional. `null` renders as unknown; `0` renders as measured zero. Byte values use IEC units. CPU is percent of the assigned CPU capacity (0–100). Timing values are nonnegative milliseconds, with `total_ms` representing request through first successful guest command. State strings are explicit; unknown state must be sent as `unknown`, never omitted. Unsupported schemas, failed HTTP requests, malformed JSON and HTML SPA fallbacks produce a disconnected screen; stale inventory is cleared on refresh.

The server, **not** frontend filters, must enforce workspace ownership. Do not include secrets, paths to private runtime state, or unauthorised resources in this response. The `box_id` checkpoint filter is an additional UI safeguard, not authorization.

For deployment, serve `dist/` behind the same authenticated origin as the API. Route `/api/` to the service before the SPA fallback; route other application paths to `index.html` so deep links work. Apply authorization to terminal, preview, snapshot, metrics and exec endpoints independently. The frontend is not an authentication barrier.

Create, pause, resume, stop, start, and delete are wired through the same helper. `kiln-app` already forwards `POST /v1/operations` and `GET /v1/operations/{id}` to the host journal. The host journal supplies the request-id idempotency and the single box reservation. Cookie writes reuse the logout origin and `__Host-kiln_csrf` check. Each operation attempt appends an audit line. The browser posts one operation, polls it, and renders the box only when a following `GET /api/v1/dashboard` contains the observed result. Stop sends `force: true` only when that option is selected. Delete posts `{ type: "delete", id }` after the typed name matches, then removes the box only when a following dashboard omits that id. A timeout or a failed operation does not invent a box. An operation outcome of `unknown` is shown as unknown and does not change the displayed inventory.

That forwarder sends the current `__Host-kiln_app` session as it is. Identity still issues that session with read scope, and the gateway accepts writes only with operate scope, so a signed-in dashboard cannot run these actions until identity grants an operate-scoped dashboard session or a separate step-up. This console does not change that.

Exec and same-box restore are not forwarded by the console. Same-box restore still requires explicit rollback acknowledgement and must not become an unrestricted clone action. Never optimistically mark a box running or remove it before the dashboard omits it. A timeout is an unknown outcome, not proof that a command did not run.

## Verify

```sh
npm test
npm run build
npx playwright install chromium
npm run test:browser
npx prettier --check .
```

For an already-installed Chromium binary, set `PLAYWRIGHT_CHROMIUM_EXECUTABLE` to its absolute path. Set `KILN_SCREENSHOT_DIR` to an existing artifact directory to capture the exercised desktop/mobile states. Browser tests use controlled API responses or explicit preview mode and do not launch VMs. They test actual navigation, validation, disabled mutation gates, dialog focus/Escape, deep links, mobile overflow, and automated WCAG checks. Screenshots are design evidence, not runtime-operation verification.

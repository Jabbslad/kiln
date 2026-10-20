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

Implemented: responsive inventory, search and sorting, state filters, box detail tabs, template selection and create form validation, same-box checkpoint/restore confirmation, lifecycle explanations, operation details, loading/empty/error states, settings, and the console guide.

**This is a frontend, not the authenticated control plane.** The existing Rust API is not integrated with this frontend. Create, lifecycle mutations, exec, terminal sessions, SSH, secrets, tokens, and HTTP preview routing are deliberately unavailable. Their controls never report success or mutate fixture state. The forms are implementation-ready interaction surfaces, not simulated VM operations. Do not remove these gates until the server enforces the contracts below.

Preview mode uses `src/fixtures.ts`, is labelled across all screens, makes no API requests, and is never automatically entered on a failed connection. Example resource sizes and host status are not claims about the local runtime.

## Published dashboard

GitHub releases include `kiln-dashboard-v0.4.9.tar.gz` and its SHA-256 checksum.
The archive contains both surfaces: production HTML, JavaScript, CSS, imagery, bundled fonts, this
guide, dependency licenses and release metadata; Node is only needed to build from source, not to
serve the archive. It is separate from the server installer and is not deployed
or connected to a host automatically.

Extract it into a static web root served at `/`, with SPA fallback to
`index.html`. `/` is the public landing page. Open `/boxes?preview=1` for the labelled design preview. On `/boxes` without
preview mode, the missing dashboard API produces a disconnected screen; the
existing CLI API does not implement this aggregate endpoint or browser sessions.
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

Before adding mutations, implement durable idempotency and resource reservation server-side, CSRF protection for cookie-authenticated writes, lifecycle/audit records, and observed-result reconciliation. Never optimistically mark a box running. A timeout is an unknown outcome, not proof that a command did not run. Same-box restore requires explicit rollback acknowledgement and must not become an unrestricted clone action.

## Verify

```sh
npm test
npm run build
npx playwright install chromium
npm run test:browser
npx prettier --check .
```

For an already-installed Chromium binary, set `PLAYWRIGHT_CHROMIUM_EXECUTABLE` to its absolute path. Set `KILN_SCREENSHOT_DIR` to an existing artifact directory to capture the exercised desktop/mobile states. Browser tests use controlled API responses or explicit preview mode and do not launch VMs. They test actual navigation, validation, disabled mutation gates, dialog focus/Escape, deep links, mobile overflow, and automated WCAG checks. Screenshots are design evidence, not runtime-operation verification.

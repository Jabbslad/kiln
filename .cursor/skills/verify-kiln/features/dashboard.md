# Dashboard

The console in `web/` is a React app served as static files. Release `v0.4.17` publishes `kiln-dashboard-v0.4.17.tar.gz`. Server `deploy/install.sh` accepts `--dashboard-archive` and installs the static tree under `/usr/local/share/kiln/dashboard` (`deploy/kiln-app.service` serves it through `kiln-api app`). Preview mode (`?preview=1`) renders `web/src/fixtures.ts` and does not call the network. Without that flag the app requests `GET /api/v1/dashboard` and, when signed in, posts operations to `/v1/operations` on the same origin (`web/src/data.ts`).

`kiln-host` builds the document and serves inventory on its Unix socket (`crates/kiln-server/src/host.rs`). Loopback `kiln-api` proxies read routes (`crates/kiln-server/src/gateway.rs`). The installed gateway listens on `127.0.0.1:8443` (`deploy/kiln-api.service`). `kiln-door` checks an identity JWT and forwards read traffic. `kiln-api app` on `127.0.0.1:8093` (`deploy/kiln-app.service`) serves the dashboard archive, browser login, CSRF-protected mutations, and proxies dashboard GET plus operate routes to loopback `kiln-api` with the owner’s operate-scoped JWT (`crates/kiln-server/src/app_listener.rs`).

## Who can read it

A read-scope owner JWT opens the public door. The audience is `kiln:server:` plus the enrolled server id. The door forwards `GET /api/v1/dashboard` to loopback `kiln-api` with the same `Authorization` header.

The administrator token is a 64-character hex secret read from the token file. Loopback `kiln-api` accepts it for this GET. The door and the app listener reject it for browser traffic.

A missing credential or a bogus bearer is HTTP 401, code `unauthorized`. A query string on the dashboard route is HTTP 400, code `invalid_request`.

## Who can mutate from the browser

Mutations require a signed-in operate session: cookies `__Host-kiln_app` and `__Host-kiln_csrf`, plus header `x-kiln-csrf` on `POST /v1/operations`. The app listener mints that session after OAuth against the enrolled identity origin. `POST /logout` with form field `csrf` clears it (`web/src/App.tsx` **Sign out**).

Live mutations (not preview, host `online`): create box, pause, resume, stop (optional force), start, delete, checkpoint, same-box restore, and one-shot exec (`web/src/dialogs.tsx`, `web/src/BoxDetail.tsx`). Preview, a disconnected live view, or host offline/unknown keeps controls disabled. The in-browser terminal tab stays disabled; do not document terminal transport here.

**Shared operation tracker (`trackOperation`, #104/#105).** All console mutations funnel through `trackOperation` in `web/src/data.ts` (formerly `submitOperation`). It POSTs `/v1/operations` with a random 32-hex `id`, sends `x-kiln-csrf` from `__Host-kiln_csrf`, polls `/v1/operations/{id}` with exponential backoff until the receipt leaves `running`, then either returns exec stdout/stderr on the receipt or re-fetches `GET /api/v1/dashboard` until `dashboardReflectsOperation` sees the expected box, checkpoint, or deletion. Inventory never updates optimistically. `web/src/data.test.ts` covers create, lifecycle, checkpoint, restore, delete, exec, CSRF or session rejection, and pending-dashboard messaging. `web/src/operationKinds.ts` lists the nine action tags kept in sync with `kiln_api::Action` by `dashboard_operation_action_types_match_api_action_tags`.

**Access panel (#107, corrected in #109).** On the Terminal tab, `Access` wraps SSH, HTTP preview, and public URL in `CapabilitySurface` (`ssh`, `httpPreview`, `publicUrl` all `not-yet`). It does not show `kiln ssh` or `kiln ssh-config` copy hints. `web/src/BoxDetail.test.tsx` checks `data-capability-state="not-yet"` on SSH and no `kiln ssh` text.

**Why laptops still cannot `kiln ssh`.** SSH upgrade routes live on loopback `kiln-api` (`/v1/boxes/{id}/ssh`, `/v1/boxes/{id}/ssh-key` in `crates/kiln-server/src/gateway.rs`). The public door (`kiln-door`) forwards read and operate JSON routes only; it does not proxy SSH. A laptop profile aimed at `https://cli.dark-forge.dev` therefore cannot complete `kiln ssh` today even when boxes run on the host. Guest SSH remains a separate journey in `features/guest-access.md` for profiles that reach a gateway exposing those routes. Do not expand door SSH in this file; that belongs in its own PR.

## Document

`schema_version` is `1`. Top-level `version` is the stamped server release (for example `0.4.17`); the gateway adds it when stamping the dashboard document. The web header shows `v` plus that value, or the build-stamped dashboard version when the field is absent. `observed_at` is UTC `YYYY-MM-DDTHH:MM:SSZ`. The gateway stamps `observed_at` and `workspace` after authentication. `Content-Type` contains `application/json`. `Cache-Control` is `no-store`.

`workspace` is the enrollment server id on an enrolled server, else `local`. `host.name` matches.

`host.state` is `online` when the host socket answers. When the dial fails, the gateway still returns HTTP 200 with `host.state` `offline` and empty `boxes`, `templates`, `checkpoints`, and `operations`.

When online, `checkpoints` lists snapshot metadata from the runtime store (`crates/kiln-server/src/host.rs`). `host.metrics.cpu_percent` and `host.metrics.memory_bytes` come from `/proc/stat` deltas and `/proc/meminfo` while `kiln-host` is online. `host.metrics.disk_bytes` and `host.metrics.disk_total_bytes` come from `statvfs` on the runtime store directory (the filesystem that holds box storage). `kiln-host` primes a baseline `/proc/stat` sample at startup (`metrics::Sampler::prime`) and retries briefly on each host read until jiffies move, so the first dashboard GET after a restart still returns a numeric `cpu_percent`. A running or paused isolated box reads its jailer cgroup `cpu.stat` (`usage_usec` deltas) and `memory.current`; stopped or inactive boxes keep null CPU and memory metrics. Every box with a disk image reports `metrics.disk_bytes` as the allocated on-host size of that image (`st_blocks` × 512); the console pairs it with `resources.disk_bytes` for a **host-observed** used-of-total label (image blocks, not guest filesystem use). The dashboard marks `metrics` as `available` in `web/src/capabilities.ts`; stopped or inactive boxes show **Stopped** or **Not available yet** for CPU and memory instead of em dashes. A box is listed only when the journal has its create row. Box `state` includes `running`, `paused`, `stopped`, `failed`, `creating`, `restoring`, and `unknown` (`web/src/data.ts` `boxStates`; host mapping in `crates/kiln-server/src/dashboard.rs`).

Operations are recent journal rows (100 newest by `created_at`, then `id`). Corrupt JSON rows are skipped after the read. Rows whose operation state is unknown are omitted. Published `kind` is `create` for a create row and `unknown` for other journal rows today (`crates/kiln-server/src/dashboard.rs`). The web schema accepts any string for `kind`; `web/src/operationKinds.ts` lists the nine `kiln_api::Action` tags the console submits. Contract test `dashboard_operation_action_types_match_api_action_tags` in `crates/kiln-api/src/lib.rs` keeps that list aligned with the API. Operation `state` in the document is `running`, `completed`, or `failed`; the web schema also allows `queued` and step arrays for forward compatibility.

Error text containing `/` or `\` is dropped (`public_error`). Exec stdout and stderr are not copied into the dashboard document.

A host body larger than 1 MiB makes the gateway return HTTP 502 with code `host_unavailable` (`oversized_host_body_is_an_error` in `crates/kiln-server/src/gateway.rs`).

`web/src/data.ts` exports `dashboardSchema`. Integration test `dashboard_schema_accepts_enrolled_and_unenrolled_servers` in `crates/kiln-server/tests/dashboard_api.rs` parses enrolled online, unenrolled online, and offline gateway bodies through that schema (via `npm ci` in `web/` when needed).

## Sub-features

- Inventory at `/boxes`: search, state filters, name sort, row links.
- Box detail at `/boxes/:id` with Terminal, Exec, Previews, Checkpoints, Metrics, and Activity tabs.
- Capability map in `web/src/capabilities.ts` (#113): `metrics`, `terminal`, `ssh`, `httpPreview`, `publicUrl`, and `launchTiming` are each `available` or `not-yet`. `CapabilitySurface` in `web/src/ui.tsx` renders **Not available yet** for `not-yet` entries (Access, terminal body, previews, launch timing). `web/src/capabilities.test.tsx` fails if an `available` capability shows placeholder dashes or empty values, or if a `not-yet` capability shows fake connected UI. `metrics` is `available` and reads live host and box telemetry from the dashboard document.
- Templates at `/templates`, including the create-box drawer.
- Operations at `/operations`.
- Settings at `/settings` and the guide at `/docs`.
- Explicit preview versus live authenticated console.
- Sign out for the operate session.
- Release version label in the console header (`web/src/kiln-version.ts`, `web/src/App.test.tsx`).
- Access panel (SSH, HTTP preview, public access labelled unavailable).
- Shared `trackOperation` polling and dashboard refresh after mutations.

## How to get to it (user POV)

From a checkout, with Node 22:

```sh
cd web
npm ci
npm run dev
```

Open `http://127.0.0.1:5173/boxes?preview=1`. Playwright uses port 4173:

```sh
npm run dev -- --host 127.0.0.1 --port 4173 --strictPort
```

On an enrolled server, open the dashboard origin served by `kiln-app` (via Caddy on the public name after enrollment). Sign in through the identity provider, then use `/boxes` without `preview=1`.

## Driving the live document

`$ADMIN_TOKEN`, `$OWNER_JWT`, `$BOGUS`, and `$CA` are placeholders. Do not write secrets into this file or `$VERIFY_EVIDENCE`.

Loopback administrator token (lab stack in `features/private-ca.md`):

```sh
curl --silent --show-error --cacert "$CA" \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -D - \
  https://127.0.0.1:8443/api/v1/dashboard
```

Public door, read-scope owner JWT:

```sh
curl --silent --show-error --cacert "$CA" \
  -H "Authorization: Bearer $OWNER_JWT" \
  -D - \
  https://cli.dark-forge.dev/api/v1/dashboard
```

In-repo proof without a guest:

```sh
cargo test -p kiln-server --test dashboard_api --locked -- --nocapture \
  dashboard_schema_accepts_enrolled_and_unenrolled_servers
cargo test -p kiln-server --test public_door --locked -- --nocapture \
  dashboard_read_rejects_untrusted_bearers_and_forwards_the_owner_jwt
cargo test -p kiln-api --lib --locked -- --nocapture \
  dashboard_operation_action_types_match_api_action_tags
cargo test -p kiln-server --lib --locked -- --nocapture \
  oversized_host_body_is_an_error
cargo test -p kiln-server --test app_listener --locked -- --nocapture \
  a_signed_in_operate_session_reaches_the_host
```

Deploy and smoke validate live host metrics with `kiln-api self-check`, which deserializes the same `dashboard::Document` the server publishes and checks `host.metrics.cpu_percent`, `host.metrics.memory_bytes`, `host.metrics.disk_bytes`, and `host.metrics.disk_total_bytes` (running-box CPU may be null on the first read after restart; box disk is not gated). No shell JSON parsing on the host.

```sh
cargo test -p kiln-server --test dashboard_self_check --locked
sh scripts/test-check-dashboard-bundle.sh
```

## Driving it with Playwright

Install the browser once: `npx playwright install chromium`. Set `KILN_SCREENSHOT_DIR` before `npm run test:browser`.

```sh
cd web
KILN_SCREENSHOT_DIR="$VERIFY_EVIDENCE/dashboard" npm run test:browser
```

Preview tests keep mutations disabled and make no `/api/` requests. Live-route tests mock `GET /api/v1/dashboard` for loading, empty, and error states. Without preview on the dev server (no proxy), `/boxes` shows **Connect your control plane**. Unit tests include `web/src/capabilities.test.tsx`, `web/src/BoxDetail.test.tsx`, and `web/src/App.test.tsx` for capability surfaces.

```sh
cd web
npm test -- capabilities.test.tsx BoxDetail.test.tsx
```

## Gotchas

- Preview data is labelled example data; it is not the local runtime inventory.
- A green preview browser run does not prove live mutations; drive an operate session or the app_listener test for that.
- CI job `dashboard` runs `npm test`, Prettier, `npm run build`, and `npm run test:browser` without Firecracker.
- Record status codes and field names. Leave `Authorization` and session cookies out of the evidence directory.

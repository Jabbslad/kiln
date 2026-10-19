# Dashboard

The console in `web/` is a React app served as static files. Release `v0.4.6` publishes `kiln-dashboard-v0.4.6.tar.gz`. The archive is not installed by `deploy/install.sh`. Preview mode (`?preview=1`) renders `web/src/fixtures.ts` and does not call the network. Without that flag the app requests `GET /api/v1/dashboard` (`web/src/data.ts`). No Rust service implements that route.

## Sub-features

- Inventory at `/boxes`: search, state filters, name sort, row links.
- Box detail at `/boxes/:id` with workspace, exec, checkpoint, and preview tabs.
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

## Driving it with Playwright

Install the browser once: `npx playwright install chromium`. Set `KILN_SCREENSHOT_DIR` to an existing directory to keep screenshots. The suite starts or reuses the dev server (`web/playwright.config.ts`).

```sh
cd web
KILN_SCREENSHOT_DIR="$VERIFY_EVIDENCE/dashboard" npm run test:browser
```

The inventory test (`web/tests/dashboard.spec.ts`) opens `/boxes?preview=1`, expects the title `Boxes — kiln`, types `worker` into the search box, and expects the link `worker-test`. The Running filter then shows `No matching boxes`. `Clear filters` brings the full table back. Sorting by Name puts `worker-test` first. Opening `api-dev` shows `Session not connected` and a disabled `Connect terminal` button. The exec tab accepts `["/bin/echo", "hello"]` as valid JSON and does not send it. The create drawer rejects `My Box!`, refuses the existing name `api-dev`, and leaves `Create box` disabled. Restore stays disabled after the acknowledgement checkbox. Search hides `api-dev`; clear the filter before opening that row.

Also open the unmocked live route. Vite has no dashboard API, so this is the screen a user gets:

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
- `README.md:198` still lists "dashboard" among outstanding platform work. The intro at `README.md:15` already points at `web/README.md`.
- The authoring proof used the Vite dev server and Playwright. It did not extract the release tarball or attach the app to a host.

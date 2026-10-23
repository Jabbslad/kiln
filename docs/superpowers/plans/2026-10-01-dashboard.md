# Dashboard implementation plan

**Goal:** Implement the approved inventory-first, dark kiln dashboard without representing fixture data or unavailable services as real VM functionality.

**Architecture:** An independently built React/TypeScript application in `web/`. A validated, read-only dashboard API boundary supplies observed state. The existing Rust runtime is not exposed through a development HTTP shell bridge. Backend mutations and interactive sessions remain gated pending the authenticated control plane.

**Design:** Geist and Geist Mono, surfaces `#0e0e0e`/`#111111`, borders `#202020`/`#2a2a2a`, text `#f4f4f4`/`#949494`, status green `#78e772`, square controls and white primary actions. Approved references remain in the pre-rename web mockups directory under `.amp/in/artifacts/`.

## Constraints

- Preserve concurrent Rust, image and runtime documentation changes.
- No fabricated usage, inferred zero metrics, optimistic lifecycle transitions, or automatically retried commands.
- Preserve distinctions between pause, stop, same-box restore and prepared-template clone.
- Default to live mode with an explicit disconnected state when the API is absent. Never silently substitute examples.
- Do not implement authentication, networking, PostgreSQL or a host service as part of this frontend task.

## Tasks

- [x] Add the isolated Vite package and write failing API/state tests (`web/src/data.test.ts`). Validate versioned responses, absent telemetry, API failures and immutable preview fixtures.
- [x] Implement `web/src/data.ts` and `web/src/fixtures.ts`; run `npm test` from `web/`.
- [x] Write failing interaction tests (`web/src/App.test.tsx`) for search/filter, preview navigation, form validation, same-box checkpoint selection and restore acknowledgement.
- [x] Implement the application shell, inventory, box detail tabs, templates/create drawer, operations, settings and connection/recovery states. Use accessible native dialogs, semantic controls, visible focus and responsive CSS.
- [x] Add API integration documentation and development commands in `web/README.md` rather than modifying concurrent runtime documentation.
- [x] Run type checking, production build, unit/component tests, browser interaction tests and accessibility checks. Inspect desktop, mobile and non-default-state screenshots. Leave the preview server available on the runner.

## API boundary

`GET /api/v1/dashboard` returns an explicitly versioned snapshot of the workspace, host, boxes, prepared templates, same-box checkpoints and operations. Its exact schema lives in `web/src/data.ts`. Missing metrics are nullable; HTTP errors, malformed JSON and invalid schemas produce a visible connection error. This is a proposed frontend contract, not an existing Rust API. A future authenticated same-origin gateway must provide it. The client does not store credentials or issue VM mutations.

## Verification

Use asymmetric resource fixtures and mixed lifecycle states. Test that failed requests cannot turn into an empty successful inventory, zero differs from missing telemetry, unrelated checkpoints cannot be selected, preview cannot call the network, and acknowledging rollback does not enable a mutation without the backend. Browser checks cover search, filtering, navigation, tabs, modal focus/Escape, narrow screens and refresh/deep links. Screenshots verify the approved visual direction, not VM behavior.

Verified locally for v0.4.4 shipment: 14 unit/component tests, 10 Chromium browser tests (including automated accessibility checks), strict TypeScript compilation, production build, and Prettier. Browser-discovered horizontal overflow, dialog focus-loop and mobile drawer overlap regressions were reproduced and corrected. Release screenshots are in `.amp/in/artifacts/v044-dashboard/`. Backend integration remains outstanding; the frontend issues no VM mutations. The release workflow now builds and packages the static dashboard with dependency licenses, independently of the server installer. No runtime behavior was changed by this work.

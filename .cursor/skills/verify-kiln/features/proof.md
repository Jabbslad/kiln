# PR proof capture

Agents and contributors attach rerunnable visual proof when a pull request changes what the dashboard or CLI shows. The workflow is scripted; do not rely on one-off manual browser sessions.

## Dashboard (built static preview)

From the repository root, with Node 22+ and Rust (for the kiln-app binary build):

```sh
sh scripts/proof.sh dashboard --phase before   # optional baseline before edits
# … make the change …
sh scripts/proof.sh dashboard --phase after
```

Artifacts land in `artifacts/proof/` (override with `KILN_PROOF_DIR`). Each run builds `web/` (`npm run build`), builds `kiln-api` (the kiln-app listener binary), serves the production bundle with Vite preview on `127.0.0.1:4174`, and runs `web/tests/proof.spec.ts`. PNGs are named `{phase}-dashboard-boxes.png`.

For live-route unreachable vs recovered screenshots on the built bundle, use `sh scripts/capture-dashboard-unreachable-proof.sh` (same Playwright proof config, before/after PNGs under `artifacts/dashboard-unreachable-proof/` by default).

CI smoke (no kiln-api build): `KILN_PROOF_SKIP_KILN_APP=1 sh scripts/test-proof.sh`.

## CLI output

```sh
sh scripts/proof.sh cli --phase before
sh scripts/proof.sh cli --phase after
```

Default capture writes `artifacts/proof/cli/{phase}-version.txt` and `{phase}-help.txt` from a release `kiln` build. Pass `--` plus client args to record a specific command:

```sh
sh scripts/proof.sh cli --phase after -- boxes --help
```

## Pull request body

When guarded paths change (`web/src`, `web/public`, `crates/kiln-client/src`, `crates/kiln-runtime/src/bin`), the PR body needs a **Proof** section with a markdown image or video link, or an exact line `proof: n/a` for non-visual edits. `scripts/check-pr-proof.sh` enforces this on pull requests.

## Gotchas

- Preview proof uses `?preview=1` fixture data; it does not replace live host or operate-session evidence for mutations.
- Set `KILN_PROOF_SKIP_KILN_APP=1` only for the CI smoke or when Rust is unavailable; full local proof should build kiln-api.
- Playwright installs Chromium on first run (`npx playwright install chromium`).

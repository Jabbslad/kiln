# Auto-release (main to dark-forge)

A green push to `main` can publish the next patch release and deploy it to dark-forge in the same `.github/workflows/build.yml` run. Pull requests rehearse pieces of this path but do not publish or deploy.

## Sub-features

- `required` aggregate: fmt, clippy, tests, packaging, dashboard, and docs guards must succeed.
- `release-plan`: `scripts/plan-release.sh` with `PLAN_RELEASE_APPLY=1` decides `release=true|false` and the next `vX.Y.Z` tag from the push diff.
- `version-guard`: when `release=true`, stamps artifacts to the planned tag and runs `scripts/check-stamped-clients.sh`.
- `attest`: build provenance on `dist/*.tar.gz` and `dist/*.zip` for main pushes.
- `draft-release`: verifies checksums and attestations (`scripts/verify-release-attestations.sh` retries Sigstore verifier init flakes), then `gh release create` for the planned tag.
- `deploy`: reusable `deploy.yml` with `tag: ${{ needs.release-plan.outputs.tag }}`. One multiplexed SSH connection per job (`deploy/github/ssh-session.sh`, `ControlPath` under `RUNNER_TEMP`); master open retries pre-auth failures only. Staging-dir cleanup and every `ssh`/`scp` run while the master is open; deploy logs `ssh session opened` / `ssh session closed`.
- Post-checks on the host (`deploy/github/install-remote.sh`): `install.sh --healthcheck`, `kiln-api self-check` (typed dashboard metrics right after restart), `smoke.sh` (with `--require-launch-timing` for the smoke box), `check-dashboard-bundle.sh` against live `kiln-app` HTML, and a kiln-identity stamp guard.

## How to get to it (operator POV)

Merge to `main` and wait for the build workflow on that commit. Docs-only pushes set `release=false`; no tag is created. A releasable code push creates the tag, publishes assets, and starts deploy with the dark-forge environment.

Manual recovery uses workflow dispatch on `Deploy` with a published tag, or `build.yml` `dry_run` to validate packages without publishing.

## Driving it in a checkout

Before push, enable the repository hook once per clone (`bash scripts/install-hooks.sh`).
The pre-push hook runs the fast local gate (fmt check, `cargo fmt`/`clippy`/workspace tests, feature-map guard):

```sh
bash scripts/check.sh
```

Opt-in: `--docs-check` runs the full docs-check script bundle; `--release` runs `scripts/tag-path-dry-run.sh` (needs `GH_TOKEN`).

Rehearse without publishing:

```sh
sh scripts/test-plan-release.sh
sh scripts/test-tag-path-dry-run.sh
sh scripts/test-tag-path-pr-stamp-order.sh
sh scripts/test-releasable.sh
cargo xtask test-deploy-workflow
sh scripts/test-ssh-multiplex.sh
cargo xtask test-reuse-release
```

A pass is exit 0 from each command. Save combined output in `$VERIFY_EVIDENCE/auto-release-harness.txt`.

The tag-path dry run on pull requests stamps packages to a `-dryrun` tag locally and runs the same attest and reuse checks the release jobs use, without `gh release create` or deploy. On pull requests it also resolves the dry-run tag, builds release binaries, and runs `check-binary` before assembling a server upgrade tarball; `scripts/test-tag-path-pr-stamp-order.sh` fails if resolve/build/check order or the `v`-prefixed tag drifts.

## Gotchas

- Deploy never runs on pull requests; only `workflow_dispatch`, `workflow_call` from a successful main release, or tag-shaped refs reach `deploy/github/deploy.sh`.
- Parallel `scp` on separate connections used to compete with sshd `MaxStartups`; deploy and diagnose now reuse one control socket and upload serially.
- `release-plan` needs `contents: write` to create the git tag before downstream jobs read `needs.release-plan.outputs.tag`.
- Skipping `required` or any package job blocks `release-plan`, so a half-green main push does not publish.
- Identity and dashboard archives ride with the server deploy; identity is upgraded before `kiln-app` restarts (`deploy/github/install-identity.sh`).
- The dashboard bundle check compares the shipped tarball’s `index-*.js` with the live dashboard HTML (`deploy/github/check-dashboard-bundle.sh`, `KILN_DASHBOARD_LIVE_HTML`).

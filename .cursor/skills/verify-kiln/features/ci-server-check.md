# CI server-check evidence

When `server-check` runs **Test and lint**, the job wraps the step in a 25-minute timeout, tees all output to `server-check-test-and-lint.log`, and uploads that log as a workflow artifact on every outcome (`if: always()`, upload step uses `continue-on-error: true`). A step timeout or failure also prints process-tree and nextest diagnostics into the step log before exit.

Workflow: `.github/workflows/build.yml` job `server-check` calls `bash scripts/server-check-run-test-and-lint.sh`, which runs `scripts/server-check-test-and-lint.sh` under `timeout --signal=TERM --kill-after=30s`. Guards: `scripts/test-server-check-hang-evidence.sh`, `scripts/test-server-check-nextest.sh`.

Hermetic proofs use `ci-guards::hang_probe_sleep` with `KILN_SERVER_CHECK_HANG_PROBE_SECS` and a short `SERVER_CHECK_STEP_TIMEOUT_SEC` override.

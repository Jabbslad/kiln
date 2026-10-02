# Retained PID1 experiment

User approved the investigation's recommendation: measure warm reset subphases,
then compare an opt-in retained-PID1 mode without weakening the default.

## Design and constraints

- Add explicit `systemd_warm_shared` image mode. Its boxes deliberately share
  the public preparation machine ID, including cold boots/restarts. Host box IDs
  and hostnames remain unique. Existing modes and serialized snapshots keep
  their meanings; no protocol change.
- Only template preparation adds `kiln.retain_pid1=1`. The guest rejects a
  mismatched initialization identity before mutation. Retained PID1 uses
  daemon-reload rather than reexec; stopped daemons still start fresh.
- Preserve quiescence/credential/FD admission, private writable disks, checked
  entropy reseeding before activators, fresh workload-agent invocation and
  identity-bound handoff. This is pinned-image experimentation, not approval for
  arbitrary services, networking, credentials or production multi-tenancy.
- Emit nonoverlapping reset-step durations as one structured console line on
  success or failure. Preserve readiness protocol and host timing boundaries.
- Work in the current checkout with its uncommitted predecessor implementation.
  Do not touch dashboard work, push, or modify shared infrastructure.

## Execution

- [x] Add failing manifest/cold-boot and guest identity-policy tests; run targeted
  `cargo test -p kiln-runtime image::tests` and `cargo test -p kiln-guest warm::tests`.
- [x] Implement the mode in image/runtime/builder and guest warm reset. Record
  audit, provisioning, unmask, manager refresh, daemon startup and identity checks.
  Run unit tests, formatter and Clippy.
- [x] Extend raw-vsock tests to reject mismatched shared identity and preserve
  execution gating on failures. Build fresh matched images and run actual KVM
  tests for both policies, including identities, disk isolation and restart.
- [x] Benchmark 4 GiB/1-vCPU isolated overlay templates: 30 sequential and 16
  concurrency-four launches per policy, alternating order, no retries. Record
  create/first-exec timings and reset breakdowns. Keep all failures.
- [x] Document measured benefit or lack thereof, limitations and actual delivery
  state. Delete only disposable test resources, retain evidence under `.amp/in/`.

## Results and delivery

- Sequential create p50 517 → 475 ms; first-command p50 536 → 493 ms (8.0%).
  Concurrent create 618 → 581 ms; first-command 636 → 598 ms (6.0%). All 92
  attempts succeeded. The same-host comparison alternated policy order per batch.
- Manager refresh median 145 → 106 ms; admission still ~90 ms, service startup
  ~77 ms. Provisioning is <1 ms. Retention avoids only part of manager work:
  unmasked definitions still need reloading. No default policy was weakened.
- 68 ordinary tests, formatting and all-feature Clippy passed. Both modes passed
  all seven isolated lifecycle checks in final runs; the old systemd image passed
  clone/lifetime compatibility. All 11 raw-vsock admission/handoff cases passed.
- An initial shared-mode benchmark lifecycle test failed without its JSON error
  being printed. The helper now prints stdout and status. Three diagnostic reruns
  and the final full suite passed without a runtime fix. Original cause remains
  unknown, and its failed log remains separate from successful benchmark data.
- Measurements and independent validation: `.amp/in/artifacts/retained-pid1-*`.
  New images: `images/output/ubuntu-warm-{profile,shared}` with matching pinned
  source/builder/binary hashes. The isolated production CLI remains in the
  pre-rename overlay validation installation (no fault injection).
- Disposable boxes/templates removed; no Firecracker processes, test child
  cgroups, snapshot mappings or task-owned loop mappings remained. Pool unmounted.
  Final images/binaries and evidence retained. No vendor resources were launched.
- Changes remain local, uncommitted and unpushed. Dashboard work is untouched.

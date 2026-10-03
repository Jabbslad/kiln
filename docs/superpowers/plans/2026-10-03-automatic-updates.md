# Automatic Kiln updates

**Goal:** Re-running the installer installs missing Kiln components, updates
compatible installations, and leaves identical installations unchanged. No
upgrade flag, prompts, or boxd migration are required.

**Design:** The client uses its existing atomic replacement and `.previous`
backup. A verified identical client is a no-op, preserving the useful backup.
The server bootstrap detects existing Kiln paths before address detection or apt
and delegates to the privileged provisioner. That provisioner validates the
installed binaries, active standard services, private endpoint and release
compatibility; preserves all config, credentials, images and VM state; stages
binary backups; stops API then host with `KillMode=process`; replaces binaries
atomically; starts services and checks the authenticated template catalog.
Failure restores the previous binaries and services. A durable pending marker
blocks blind retries following interruption. Concurrent provisioning is locked.
Only explicitly reviewed compatible releases are accepted; no downgrade or
implicit guest-image/state migration. Old boxd installations are out of scope.

## Implementation and verification

1. Change `scripts/test-bootstrap.py` expectations first: ordinary reruns update,
   identical installs preserve backups, unsafe paths/failures preserve binaries,
   existing server setup skips apt and route detection and passes only explicit
   overrides. Retain public-download and noninteractive tests.
2. Implement the client/server dispatch in `deploy/bootstrap/install.sh`.
3. Test `deploy/install.py` with disposable paths and simulated systemd: update
   success, unchanged configuration/data, same-version no-op, incompatible or
   mixed versions, missing state, locks/interrupted updates, failures during file
   replacement/start/readiness, and rollback failure. Exercise actual atomic file
   replacement; mock only privileged system operations.
4. Add server update handling to `deploy/install.py`, keep fresh preflight intact,
   and update user-facing documentation. Package v0.3.3 without runtime changes.
5. Run Python installer/bootstrap/packaging tests, both supported ShellCheck
   versions, Rust fmt/clippy/workspace tests, and release CI. Inspect published
   archives; verify anonymous fresh client install, automatic upgrade from the
   previous release, and repeat no-op without a terminal. Publish reviewed pins
   and the two designated installer files. Do not upgrade the live server.

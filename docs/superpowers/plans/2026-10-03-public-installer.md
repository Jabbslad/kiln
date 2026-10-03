# Public unattended installation implementation plan

**Goal:** Publish Kiln source and releases and install without a GitHub token or terminal.

**Architecture:** Keep the existing POSIX bootstrap and Python provisioner. Download
versioned public GitHub assets with pinned SHA-256 checksums. The bootstrap detects
the route-selected private IPv4 unless `--address` overrides it. Guest networking
stays opt-in (`--network` detects the uplink, `--network-uplink` overrides it).
The explicit server command authorizes provisioning; every existing preflight
check remains. No existing server is upgraded or migrated.

**Tech stack:** POSIX shell, Python unittest, iproute2, GitHub Actions and Rust releases.

## Constraints

- No token prompt, controlling terminal, stdin consumption or sudo password prompt.
- Keep checksum/archive checks, atomic client upgrades and server conflict checks.
- Preserve unrelated `web/` and dashboard plan; do not deploy to the live host.
- Audit history and release data before making `Jabbslad/kiln` public.
- Publish only bootstrap script and README to `Jabbslad/kiln-install`.
- Public availability does not remove Kiln API authentication or grant a new license.

## Execution

1. In `scripts/test-bootstrap.py`, run the real piped shell in a new session with
   closed stdin. Assert public URLs/no credentials, correct platform archive,
   checksum rejection, safe upgrades, route detection/override/rejection, optional
   networking, and `sudo -n`/noninteractive apt. Run tests and observe failures.
2. In `deploy/bootstrap/install.sh`, remove PAT/TTY code, retain HTTPS-only
   redirects and digest/archive validation, add server options and route parsing,
   and invoke apt/provisioner with stdin closed. In `deploy/install.py`, make
   `--apply` the explicit authorization rather than calling `input()`; test
   preflight rejection, check-only and successful application without input.
3. Update current README/install/release guidance and `AGENTS.md` for public
   delivery. Permit reviewed tag releases in `.github/workflows/build.yml` on a
   public repository, retaining read-only PR jobs and draft publication.
4. Run `python3 scripts/test-bootstrap.py`, `python3 scripts/test-install.py`,
   `python3 scripts/test-release.py`, ShellCheck, fmt, clippy and workspace tests.
   Inspect diffs and the redacted publication audit. Commit v0.3.1, push tag and
   wait for native release CI. Review downloaded packages and their checksums.
5. Make the GitHub source public after audit, publish the reviewed release,
   update installer checksum/version pins, commit/push, and publish designated
   bootstrap files. Verify anonymous client installation/upgrade in an isolated
   HOME without a TTY and anonymous server download. Do not provision ser7.

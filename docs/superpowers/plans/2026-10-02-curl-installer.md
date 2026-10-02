# Curl installer implementation plan

> Execute inline with the executing-plans skill. Preserve unrelated dashboard work.

**Goal:** Publish a working curl installer backed by the approved private pilot release.
**Architecture:** A POSIX shell bootstrap contains pinned release asset IDs and
digests. It reads a token from the terminal, downloads without leaking credentials
across redirects, verifies/extracts the archive, and installs the client or invokes
the existing server installer after managing dependencies.
**Tech stack:** POSIX sh, curl, tar, SHA-256, Python unittest for development tests.

## Constraints

- Public bootstrap only; source and release packages stay private.
- No runtime Python/GitHub CLI/JSON parser on laptops.
- macOS ARM/Intel and Linux x86-64/glibc 2.39+; native Windows remains a manual zip.
- Server remains Ubuntu 24.04 x86-64/KVM, fresh-install only.
- No server deployment, firewall changes, or automatic upgrades.
- User approved the design and a private v0.1.0 prerelease on October 2.

## 1. Bootstrap and failure-path tests

Files: create `deploy/bootstrap/install.sh`, `scripts/test-bootstrap.py`.

- [x] Write subprocess/PTY tests with fake curl, platform, and privilege commands.
  Test correct installation/version, token hidden from argv/output, 302 download
  with no authorization, checksum failure, HTTP failure, malformed archives,
  existing destinations, OS rejection, signals/terminal restoration and cleanup.
- [x] Run `python3 scripts/test-bootstrap.py` and observe missing-script failures.
- [x] Implement `main`, platform checks, terminal prompts, private temporary files,
  pinned assets, HTTPS download, digest and archive checks, user install, and
  server prerequisite confirmation followed by `install.py --apply`.
- [x] Run tests and `shellcheck deploy/bootstrap/install.sh`; fix actual failures.

## 2. Package, document and validate

Files: `.github/workflows/build.yml`, `docs/releases.md`, `README.md`,
`deploy/bootstrap/README.md` (public allowlist documentation).

- [x] Add bootstrap tests and shell lint to existing CI without release/deployment
  permission changes. Document curl commands, fine-grained Contents: read token,
  server dependency management, platform constraints and installation limits.
- [x] Run `python3 scripts/test-release.py`, `python3 scripts/test-install.py`,
  `python3 scripts/test-bootstrap.py`, ShellCheck and `git diff --check`.

## 3. Approved publishing and live download verification

- [x] Download all five artifacts from successful run 36944964317 into excluded
  scratch space. Verify every package checksum and archive allowlist.
- [x] Create v0.1.0 at GitHub commit 2649ae09589d9e8f2907f7e58520684588dc6812,
  upload unchanged verified packages, and publish as a private prerelease with
  explicit pilot/fresh-host-validation caveats. Inspect state before any retry.
- [x] Read release asset IDs/digests and pin them in the bootstrap; rerun tests.
- [x] Exercise real authenticated client download and install in a disposable
  HOME, including `boxctl --version`. Never log or persist GitHub credentials.
- [x] Create public Jabbslad/boxd-install and publish only reviewed install.sh and
  README.md. Check visibility of both repos. Fetch the public script and compare
  its digest to the tested local copy, then exercise that copy with real assets.
- [x] Commit only this task's local changes. Do not push the private source
  repository without separate authorization. Report live command and remaining
  real-server/macOS verification limits. Remove scratch downloads and fixtures.

## Delivery and evidence

Published the private [v0.1.0 prerelease](https://github.com/Jabbslad/boxd/releases/tag/v0.1.0)
with all five original archives and their checksum files. All ten uploaded asset
digests match the verified successful build. Publishing the tag triggered a
redundant build (36996784311); it was cancelled before its release job could try
to recreate the existing release. The original successful run remains 36944964317.

Published only `install.sh` and `README.md` in public
[Jabbslad/boxd-install](https://github.com/Jabbslad/boxd-install), with independent
Git history. Verified the public script is byte-identical to the tested copy,
the source repository remains private, and unauthenticated asset access returns
404. Real Linux installs passed both before and after publication, including
the actual private download, digest verification, binary execution and cleanup.

15 bootstrap tests, 8 provisioner tests and 4 packaging tests pass. ShellCheck,
Ruff and diff whitespace checks pass. Review logs are retained under
`.amp/in/artifacts/curl-installer-{verification,live}.log`.

Source/test/docs changes are committed locally, not pushed to private GitHub.
New native bootstrap CI checks have not run on macOS yet; platform-selection
tests on Linux are not a substitute. No real Ubuntu 24.04/KVM provisioning or
reboot was performed, and no host services were changed. Unrelated dashboard
work remains untouched.

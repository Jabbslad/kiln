# Curl installation with private packages

## Outcome

Install the laptop client or provision a supported server from one shell command,
without asking the user to install GitHub CLI, Python, Rust, or a JSON parser.
The owner approved a public bootstrap script while keeping source and packages
private. No server deployment or private package release has been approved.

Proposed public location: a separate `Jabbslad/boxd-install` repository. Publish
only reviewed bootstrap files and installation metadata, never a copy of the
private repository. The command shapes are:

```sh
curl -fsSL https://raw.githubusercontent.com/Jabbslad/boxd-install/main/install.sh | sh
curl -fsSL https://raw.githubusercontent.com/Jabbslad/boxd-install/main/install.sh | sh -s -- server
```

These URLs are proposed, not currently available installations.

## Distribution

Use private GitHub release assets, not expiring Actions artifacts. There is no
published release today. The verified build run 36944964317 can supply an initial
pilot release, but publishing that private release requires owner approval. It
must retain the warning that fresh-host provisioning and reboot are unverified.

Keep asset metadata with the reviewed public bootstrap: version, supported
platform, GitHub release asset ID, archive filename, and SHA-256. This is public
metadata, not public package access. It avoids installing a JSON parser merely
to discover asset URLs. Do not execute downloaded metadata as shell code.
Pin each installation to one manifest/version; never mix latest components.

Alternative: use the current Actions artifact IDs and digests. Rejected as the
default because those artifacts expire on 2026-10-09. Another alternative is a
custom authenticated download service; that adds unnecessary infrastructure.

Maintain the authoritative bootstrap in the private workspace and publish only
an explicit allowlist to the separate public repository after review. Do not
add cross-repository write credentials to ordinary build jobs. Later releases
need an explicit reviewed metadata update; automatic release promotion is out
of scope.

## Authentication and download safety

Prompt on `/dev/tty` for a GitHub token with read-only access to this private
repository's release assets (fine-grained Contents: read). Disable terminal echo
while reading and restore it on success, cancellation, errors, and signals.
Never read interactive input from the pipe carrying the script.

Do not put tokens in URLs, command arguments, shell tracing, logs, or persistent
profiles. Pass authorization through a private temporary curl configuration and
delete it on exit. Do not forward authorization to another origin on redirects.
Use HTTPS with normal certificate verification; never use insecure TLS flags.
Disable user curl configuration so it cannot silently enable credential logging.

Verify the archive digest against the pinned metadata before extraction or
execution. These checks protect integrity under trust in the bootstrap publisher;
they do not claim independent binary signing. Use private temporary directories,
cleanup traps, and reject unexpected archive paths and links. Authentication,
missing assets, unsupported platforms, and checksum failures must stop without
installing or replacing anything.

## Client installation

Support macOS Apple Silicon/Intel and Linux x86-64 with glibc 2.39 or newer in
the shell bootstrap. Native Windows retains the existing downloadable client;
a PowerShell bootstrap is a separate task. Report unsupported platforms before
requesting credentials or downloading packages.

Install into the user's `~/.local/bin` without sudo, verify the downloaded
binary's version, and print PATH instructions when needed. Do not silently edit
shell startup files. Refuse an existing destination rather than implementing an
implicit upgrade. No Python, GitHub CLI, or Rust dependency on the laptop.

## Server installation

Reuse `deploy/install.py` rather than rewriting the tested provisioning logic.
The shell bootstrap handles required Ubuntu system packages, including Python,
after showing the changes and receiving confirmation. The user never manually
installs or invokes Python. Download and verify the server package before
requesting privileged setup; never send GitHub credentials into the services.

Retain Ubuntu 24.04 x86-64, KVM, cgroup v2, available RAM/disk, and fresh-install
restrictions. Reject unsupported hosts before package-manager changes. Prompt
for the private server IPv4 address and pass terminal input to the existing
installer so its INSTALL confirmation works under `curl | sh`.

Use sudo only for package installation and provisioning. Keep existing failure
handling: retain partial runtime state for diagnosis, never blindly delete it
or automatically retry setup. Print the enrollment bundle location on success.
Do not configure firewalls, VPNs, guest networking, or alter the disk backend.

## Verification and delivery

Test platform selection, unsupported hosts, hidden token prompting via a pseudo
terminal, interrupted prompt echo restoration, failed/expired authentication,
redirect credential handling, digest mismatch, unsafe archives, existing client
destinations, cleanup, and server confirmation/dependency ordering. Stub network
and privileged commands in tests; never install services on this runner.

Run ShellCheck and the existing packaging/installer suites. Exercise an actual
private asset download and Linux client installation in a disposable user
directory after a private release exists. Native macOS and real Ubuntu/KVM
provisioning remain distinct checks; mocked tests do not prove those outcomes.

Publishing the public bootstrap is authorized in principle. Publishing the
private pilot release and changing the private source remote remain separate
external actions. Do not publish a working-looking one-liner until its referenced
assets exist and its authenticated download path has been tested.

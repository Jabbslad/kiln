#!/usr/bin/env bash
# Install the Caddy release CI runs. The digest is the linux_amd64 line of caddy_2.10.2_checksums.txt.
set -euo pipefail
archive=$(mktemp -d)
trap 'rm -rf -- "$archive"' EXIT
curl --fail --location --retry 3 \
  -o "$archive/caddy.tar.gz" \
  https://github.com/caddyserver/caddy/releases/download/v2.10.2/caddy_2.10.2_linux_amd64.tar.gz
printf '%s  %s\n' \
  747df7ee74de188485157a383633a1a963fd9233b71fbb4a69ddcbcc589ce4e2cc82dacf5dbbe136cb51d17e14c59daeb5d9bc92487610b0f3b93680b2646546 \
  "$archive/caddy.tar.gz" | sha512sum -c -
tar -xzf "$archive/caddy.tar.gz" -C "$archive" caddy
sudo install -m 0755 "$archive/caddy" /usr/local/bin/caddy
caddy version

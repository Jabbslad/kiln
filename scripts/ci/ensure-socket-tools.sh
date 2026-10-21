#!/bin/sh
# server-check "Socket activation tools" step from .github/workflows/build.yml
set -eu

missing=
command -v systemd-socket-activate >/dev/null || missing=1
command -v ss >/dev/null || missing=1
if [ ! -x /lib/systemd/systemd-socket-proxyd ] && [ ! -x /usr/lib/systemd/systemd-socket-proxyd ]; then
  missing=1
fi
if [ -n "$missing" ]; then
  sudo apt-get update
  sudo apt-get install --no-install-recommends -y systemd iproute2
fi

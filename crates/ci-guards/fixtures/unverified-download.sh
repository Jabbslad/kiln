#!/bin/sh
curl --fail --location --retry 3 -o /tmp/caddy.tar.gz https://example.invalid/caddy.tar.gz
tar -xzf /tmp/caddy.tar.gz

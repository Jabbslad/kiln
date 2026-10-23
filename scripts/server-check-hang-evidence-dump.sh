#!/bin/sh
# Print diagnostics when server-check test-and-lint is stuck or timed out.
set -eu

printf '=== server-check hang evidence ===\n'
date -u +%Y-%m-%dT%H:%M:%SZ
printf '\n--- process tree ---\n'
if command -v pstree >/dev/null 2>&1; then
  pstree -ap || true
else
  ps -ef --forest 2>/dev/null || ps -ef
fi
printf '\n--- nextest / test processes ---\n'
ps -ef 2>/dev/null | grep -E '[n]extest|[c]argo nextest|[c]argo test' || true
printf '\n--- memory ---\n'
free -m || true
printf '\n--- disk ---\n'
df -h / || true
printf '=== end server-check hang evidence ===\n'

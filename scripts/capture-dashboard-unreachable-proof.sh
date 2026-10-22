#!/usr/bin/env sh
set -eu
ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
OUT_DIR="${1:-$ROOT/artifacts/dashboard-unreachable-proof}"
mkdir -p "$OUT_DIR"
cd "$ROOT/web"
npm ci
npm run build
PORT=4174
npm run preview -- --host 127.0.0.1 --port "$PORT" --strictPort &
PREVIEW_PID=$!
cleanup() {
  kill "$PREVIEW_PID" 2>/dev/null || true
}
trap cleanup EXIT INT TERM
ready=0
i=0
while [ "$i" -lt 60 ]; do
  if curl -sf "http://127.0.0.1:$PORT/" >/dev/null 2>&1; then
    ready=1
    break
  fi
  i=$((i + 1))
  sleep 0.5
done
if [ "$ready" -ne 1 ]; then
  echo "preview server did not become ready on port $PORT" >&2
  exit 1
fi
KILN_PROOF_BASE_URL="http://127.0.0.1:$PORT" KILN_SCREENSHOT_DIR="$OUT_DIR" \
  npx playwright test tests/network-unreachable-proof.spec.ts -c playwright.proof.config.ts
echo "Proof screenshots written to $OUT_DIR"

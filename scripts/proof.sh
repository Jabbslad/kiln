#!/bin/sh
# Capture rerunnable dashboard or CLI proof artifacts under artifacts/proof (or KILN_PROOF_DIR).
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

usage() {
  cat <<'EOF' >&2
usage: sh scripts/proof.sh dashboard [--phase before|after|smoke]
       sh scripts/proof.sh cli [--phase before|after] [-- kiln-args...]
EOF
  exit 2
}

fail() {
  printf 'proof: %s\n' "$*" >&2
  exit 1
}

proof_dir=${KILN_PROOF_DIR:-$root/artifacts/proof}
phase=after
mode=
cli_args=

while [ $# -gt 0 ]; do
  case "$1" in
    dashboard|cli)
      mode=$1
      shift
      ;;
    --phase)
      shift
      phase=${1:-}
      [ -n "$phase" ] || fail '--phase requires a value'
      shift
      ;;
    --)
      shift
      cli_args=$*
      break
      ;;
    -h|--help)
      usage
      ;;
    *)
      fail "unknown argument: $1"
      ;;
  esac
done

[ -n "$mode" ] || usage

case "$phase" in
  before|after|smoke) ;;
  *) fail "phase must be before, after, or smoke" ;;
esac

mkdir -p "$proof_dir"

build_kiln_app() {
  if [ "${KILN_PROOF_SKIP_KILN_APP:-}" = 1 ]; then
    return 0
  fi
  if ! command -v cargo >/dev/null 2>&1; then
    fail 'cargo is required to build kiln-api (kiln-app); set KILN_PROOF_SKIP_KILN_APP=1 to skip'
  fi
  cargo build --locked --release -p kiln-server --bin kiln-api
}

start_built_preview() {
  port=4174
  (
    cd "$root/web"
    npm run preview -- --host 127.0.0.1 --port "$port" --strictPort
  ) &
  preview_pid=$!
  cleanup_preview() {
    kill "$preview_pid" 2>/dev/null || true
  }
  trap cleanup_preview EXIT INT TERM
  ready=0
  i=0
  while [ "$i" -lt 60 ]; do
    if curl -sf "http://127.0.0.1:$port/" >/dev/null 2>&1; then
      ready=1
      break
    fi
    i=$((i + 1))
    sleep 0.5
  done
  [ "$ready" -eq 1 ] || fail "preview server did not become ready on port $port"
  KILN_PROOF_BASE_URL="http://127.0.0.1:$port"
  export KILN_PROOF_BASE_URL
}

dashboard_proof() {
  (
    cd "$root/web"
    if [ ! -d node_modules ]; then
      npm ci
    fi
    npm run build
  )
  build_kiln_app
  if ! command -v npx >/dev/null 2>&1; then
    fail 'npx is required for dashboard proof'
  fi
  start_built_preview
  (
    cd "$root/web"
    npx playwright install chromium
    KILN_SCREENSHOT_DIR="$proof_dir" \
      KILN_PROOF_PHASE="$phase" \
      KILN_PROOF_BASE_URL="$KILN_PROOF_BASE_URL" \
      npx playwright test tests/proof.spec.ts --config playwright.proof.config.ts
  )
}

cli_proof() {
  if ! command -v cargo >/dev/null 2>&1; then
    fail 'cargo is required for cli proof'
  fi
  cargo build --locked --release -p kiln-client --bin kiln
  kiln=$root/target/release/kiln
  out_dir=$proof_dir/cli
  mkdir -p "$out_dir"
  if [ -n "$cli_args" ]; then
    # shellcheck disable=SC2086
    $kiln $cli_args >"$out_dir/${phase}-command.txt" 2>&1
  else
    "$kiln" --version >"$out_dir/${phase}-version.txt"
    "$kiln" --help >"$out_dir/${phase}-help.txt"
  fi
}

case "$mode" in
  dashboard) dashboard_proof ;;
  cli) cli_proof ;;
esac

printf 'proof: wrote artifacts under %s\n' "$proof_dir"

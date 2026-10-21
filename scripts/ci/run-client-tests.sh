#!/bin/sh
# client "Native client TLS" and bootstrap steps from .github/workflows/build.yml
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"

target=${KILN_CI_CLIENT_TARGET:-}
if [ -z "$target" ]; then
  case "$(uname -s)" in
    Linux) target=x86_64-unknown-linux-gnu ;;
    Darwin)
      case "$(uname -m)" in
        arm64 | aarch64) target=aarch64-apple-darwin ;;
        *) target=x86_64-apple-darwin ;;
      esac
      ;;
    *)
      printf 'run-client-tests: unsupported OS for local client CI\n' >&2
      exit 1
      ;;
  esac
fi
export KILN_CI_CLIENT_TARGET=$target

cargo test --locked -p kiln-api -p kiln-client --target "$target"
cargo xtask test-bootstrap

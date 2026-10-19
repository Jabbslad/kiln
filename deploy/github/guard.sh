#!/bin/bash
# Sourced by the workflow entrypoints. A local proof leaves the GitHub
# environment variables unset and skips this gate.

fail() { printf 'deploy: %s\n' "$*" >&2; exit 1; }

guard_github_event() {
  mode=$1
  if [ -z "${GITHUB_EVENT_NAME:-}" ]; then
    return 0
  fi
  [ "${GITHUB_REPOSITORY:-}" = "Jabbslad/kiln" ] || fail 'refusing a fork'
  case "${mode}:${GITHUB_EVENT_NAME}" in
    deploy:workflow_dispatch|diagnose:workflow_dispatch)
      [ "${GITHUB_REF:-}" = "refs/heads/main" ] || fail 'manual runs only from main'
      ;;
    deploy:release) ;;
    *) fail "refusing event ${GITHUB_EVENT_NAME}" ;;
  esac
}

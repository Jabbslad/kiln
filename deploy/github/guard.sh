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
    deploy:release|deploy:workflow_call|deploy:push)
      if printf '%s\n' "${GITHUB_REF:-}" | grep -Eq '^refs/tags/v[0-9]+\.[0-9]+\.[0-9]+$'; then
        return 0
      fi
      if [ "${GITHUB_REF:-}" = "refs/heads/main" ]; then
        release_tag=${KILN_RELEASE_TAG:-${TAG:-}}
        printf '%s\n' "$release_tag" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+$' \
          || fail 'release runs only from a stable tag'
        return 0
      fi
      fail 'release runs only from a stable tag'
      ;;
    *) fail "refusing event ${GITHUB_EVENT_NAME}" ;;
  esac
}

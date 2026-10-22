#!/bin/sh
# Shared release-notes build mode. Source from other scripts; do not execute.
release_notes_strict_enabled() {
  if [ "${KILN_RELEASE_NOTES_STRICT:-}" = 1 ]; then
    return 0
  fi
  if [ "${KILN_RELEASE_NOTES_STRICT:-}" = 0 ]; then
    return 1
  fi
  case ${GITHUB_REF:-} in
    refs/tags/v*) return 0 ;;
  esac
  case ${REF_TYPE:-} in
    tag) return 0 ;;
  esac
  return 1
}

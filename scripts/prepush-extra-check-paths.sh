#!/bin/sh
# Read changed paths on stdin. Exit 0 when pre-push should run check.sh --release.
set -eu

while IFS= read -r path || [ -n "$path" ]; do
  path=${path#/}
  [ -n "$path" ] || continue
  case $path in
    deploy/*) exit 0 ;;
    .github/workflows/build.yml | .github/workflows/deploy.yml | .github/workflows/bootstrap-pins.yml) exit 0 ;;
    docs/releases.md) exit 0 ;;
    scripts/check-bootstrap* | scripts/check-release* | scripts/plan-release* | scripts/stamp-release* | scripts/tag-path* | scripts/releasable.sh | scripts/next-release-tag.sh | scripts/previous-release-tag.sh | scripts/published-release-tags.sh | scripts/release-notes* | scripts/test-bootstrap-pins* | scripts/test-installer-release* | scripts/test-next-release-tag.sh | scripts/test-plan-release.sh | scripts/test-releasable.sh | scripts/test-release* | scripts/verify-release* | scripts/write-release*) exit 0 ;;
  esac
done

exit 1

#!/bin/sh
# Same predicate as slow-lane-merge.yml job if (workflow_run on main push success).
set -eu

wr_conclusion=${WR_CONCLUSION:?}
wr_event=${WR_EVENT:?}
wr_branch=${WR_BRANCH:?}

if [ "$wr_conclusion" = success ] && [ "$wr_event" = push ] && [ "$wr_branch" = main ]; then
  exit 0
fi
exit 1

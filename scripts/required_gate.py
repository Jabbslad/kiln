#!/usr/bin/env python3

from __future__ import annotations

import json
import os
import sys
from dataclasses import dataclass

_PASSING = frozenset({"success", "skipped"})


@dataclass(frozen=True)
class Blocker:
    job_id: str
    result: str


@dataclass(frozen=True)
class Decision:
    blockers: tuple[Blocker, ...]

    @property
    def ok(self) -> bool:
        return not self.blockers


def evaluate(needs: object) -> Decision:
    if not isinstance(needs, dict):
        return Decision((Blocker("needs", "malformed"),))
    if not needs:
        return Decision((Blocker("needs", "empty"),))
    blockers: list[Blocker] = []
    for job_id, entry in needs.items():
        result = _result_of(entry)
        name = job_id if isinstance(job_id, str) else "needs"
        if isinstance(job_id, str) and result in _PASSING:
            continue
        blockers.append(Blocker(name, result))
    return Decision(tuple(blockers))


def _result_of(entry: object) -> str:
    if not isinstance(entry, dict):
        return "malformed"
    result = entry.get("result")
    if not isinstance(result, str) or not result:
        return "malformed"
    return result


def main() -> int:
    raw = os.environ.get("NEEDS_JSON", "")
    if not raw.strip():
        print("needs malformed")
        return 1
    try:
        payload = json.loads(raw)
    except json.JSONDecodeError:
        print("needs malformed")
        return 1
    decision = evaluate(payload)
    if decision.ok:
        return 0
    for blocker in decision.blockers:
        print(f"{blocker.job_id} {blocker.result}")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())

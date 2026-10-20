#!/usr/bin/env python3

from __future__ import annotations

import enum
import json
import os
import re
import sys
import urllib.error
import urllib.request
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass

_AUTHOR = "Jabbslad"
_PAGE_SIZE = 100
_MAX_PAGES = 20
_REPO = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
_HEAD = re.compile(r"^[0-9a-f]{40}$")
_VERDICT_SHA = re.compile(r"^[0-9a-f]{7,40}$")
_LINE = re.compile(
    r"^(?:(?:`+|\*{2,})\s*)?(PASS\+NOTES|FAIL|PASS)\s+at\s+([0-9a-fA-F]{7,40})(?![0-9a-fA-F])"
)


class StatusState(enum.Enum):
    PENDING = "pending"
    SUCCESS = "success"
    FAILURE = "failure"


class ProofError(RuntimeError):
    pass


class _Kind(enum.Enum):
    PASS = "PASS"
    PASS_NOTES = "PASS+NOTES"
    FAIL = "FAIL"


_KIND_BY_TOKEN = {
    "PASS": _Kind.PASS,
    "PASS+NOTES": _Kind.PASS_NOTES,
    "FAIL": _Kind.FAIL,
}
_MATCHED_STATE = {
    _Kind.PASS: StatusState.SUCCESS,
    _Kind.PASS_NOTES: StatusState.SUCCESS,
    _Kind.FAIL: StatusState.FAILURE,
}


@dataclass(frozen=True)
class HeadSha:
    value: str

    def __post_init__(self) -> None:
        if _HEAD.fullmatch(self.value) is None:
            raise ValueError("head sha must be 40 lowercase hex characters")


@dataclass(frozen=True)
class IssueComment:
    comment_id: int
    author: str
    body: str

    def __post_init__(self) -> None:
        if type(self.comment_id) is not int or self.comment_id <= 0:
            raise ValueError("comment id")
        if not isinstance(self.author, str) or not self.author:
            raise ValueError("comment author")
        if not isinstance(self.body, str):
            raise ValueError("comment body")


@dataclass(frozen=True)
class ProofStatus:
    state: StatusState
    description: str

    def __post_init__(self) -> None:
        if not isinstance(self.state, StatusState):
            raise ValueError("state")
        if not isinstance(self.description, str) or not 1 <= len(self.description) <= 140:
            raise ValueError("description")

    @property
    def context(self) -> str:
        return "proof"


@dataclass(frozen=True)
class _Verdict:
    kind: _Kind
    sha: str

    def __post_init__(self) -> None:
        if _VERDICT_SHA.fullmatch(self.sha) is None:
            raise ValueError("verdict sha")

    def matches(self, head: HeadSha) -> bool:
        return head.value.startswith(self.sha)


Fetch = Callable[[str, str], object]
Post = Callable[[HeadSha, ProofStatus], None]


def decide(head: HeadSha, comments: Sequence[IssueComment]) -> ProofStatus:
    seen: set[int] = set()
    chosen_id = 0
    chosen: _Verdict | None = None
    for comment in comments:
        if comment.comment_id in seen:
            raise ProofError("duplicate comment id")
        seen.add(comment.comment_id)
        if comment.author.casefold() != _AUTHOR.casefold():
            continue
        verdict = _parse_verdict(comment.body)
        if verdict is None:
            continue
        if chosen is None or comment.comment_id > chosen_id:
            chosen = verdict
            chosen_id = comment.comment_id
    head7 = head.value[:7]
    if chosen is None:
        return ProofStatus(StatusState.PENDING, f"awaiting Proof for {head7}")
    token = chosen.kind.value
    if chosen.matches(head):
        return ProofStatus(_MATCHED_STATE[chosen.kind], f"{token} at {chosen.sha}")
    return ProofStatus(StatusState.PENDING, f"{token} at {chosen.sha} does not match {head7}")


def sync_status(
    environ: Mapping[str, str],
    fetch: Fetch,
    post: Post,
) -> ProofStatus | None:
    event_name = environ.get("GITHUB_EVENT_NAME", "")
    event = _read_event(environ.get("GITHUB_EVENT_PATH", ""))
    if event_name == "issue_comment":
        issue = event.get("issue")
        if not isinstance(issue, dict):
            raise ProofError("issue is malformed")
        if not issue.get("pull_request"):
            return None
        number = _positive_int(issue.get("number"))
        repo, token, api = _github_target(environ)
        head = _head_sha(fetch(f"{api}/repos/{repo}/pulls/{number}", token))
    elif event_name == "pull_request_target":
        pull = event.get("pull_request")
        if not isinstance(pull, dict):
            raise ProofError("pull request is malformed")
        number = _positive_int(pull.get("number"))
        head = _head_sha(pull)
        repo, token, api = _github_target(environ)
    else:
        raise ProofError(f"unexpected event {event_name}")
    status = decide(head, _all_comments(api, repo, number, token, fetch))
    post(head, status)
    return status


def main() -> int:
    try:
        sync_status(os.environ, _github_fetch, _github_post)
    except Exception as error:
        message = str(error)
        token = os.environ.get("GITHUB_TOKEN", "")
        if token:
            message = message.replace(token, "")
        print(message, file=sys.stderr)
        return 1
    return 0


def _parse_verdict(body: str) -> _Verdict | None:
    matched = _LINE.match(_last_nonempty(body))
    if matched is None:
        return None
    sha = matched.group(2).lower()
    if _VERDICT_SHA.fullmatch(sha) is None:
        return None
    return _Verdict(_KIND_BY_TOKEN[matched.group(1)], sha)


def _last_nonempty(body: str) -> str:
    for line in reversed(body.splitlines()):
        stripped = line.strip()
        if stripped:
            return stripped
    return ""


def _read_event(path: str) -> Mapping[str, object]:
    try:
        with open(path, encoding="utf-8") as handle:
            payload = json.load(handle)
    except (OSError, json.JSONDecodeError) as error:
        raise ProofError("event payload is unreadable") from error
    if not isinstance(payload, dict):
        raise ProofError("event payload is malformed")
    return payload


def _all_comments(
    api: str,
    repo: str,
    number: int,
    token: str,
    fetch: Fetch,
) -> tuple[IssueComment, ...]:
    comments: list[IssueComment] = []
    for page in range(1, _MAX_PAGES + 1):
        payload = fetch(
            f"{api}/repos/{repo}/issues/{number}/comments?per_page={_PAGE_SIZE}&page={page}",
            token,
        )
        if not isinstance(payload, list):
            raise ProofError("comment list is malformed")
        if page == _MAX_PAGES and len(payload) >= _PAGE_SIZE:
            raise ProofError("comment list is truncated")
        for item in payload:
            comments.append(_parse_comment(item))
        if len(payload) < _PAGE_SIZE:
            return tuple(comments)
    raise ProofError("comment list is truncated")


def _parse_comment(item: object) -> IssueComment:
    if not isinstance(item, dict):
        raise ProofError("comment list is malformed")
    comment_id = item.get("id")
    user = item.get("user")
    body = item.get("body")
    if type(comment_id) is not int or comment_id <= 0:
        raise ProofError("comment list is malformed")
    if not isinstance(user, dict):
        raise ProofError("comment list is malformed")
    login = user.get("login")
    if not isinstance(login, str) or not login:
        raise ProofError("comment list is malformed")
    if not isinstance(body, str):
        raise ProofError("comment list is malformed")
    return IssueComment(comment_id, login, body)


def _head_sha(pull: object) -> HeadSha:
    if not isinstance(pull, dict):
        raise ProofError("pull head is malformed")
    head = pull.get("head")
    if not isinstance(head, dict):
        raise ProofError("pull head is malformed")
    sha = head.get("sha")
    if not isinstance(sha, str):
        raise ProofError("pull head is malformed")
    try:
        return HeadSha(sha.lower())
    except ValueError as error:
        raise ProofError("pull head is malformed") from error


def _positive_int(value: object) -> int:
    if type(value) is not int or value <= 0:
        raise ProofError("pull number is malformed")
    return value


def _github_target(environ: Mapping[str, str]) -> tuple[str, str, str]:
    repo = environ.get("GITHUB_REPOSITORY", "")
    token = environ.get("GITHUB_TOKEN", "")
    api = (environ.get("GITHUB_API_URL") or "https://api.github.com").rstrip("/")
    if _REPO.fullmatch(repo) is None:
        raise ProofError("GITHUB_REPOSITORY is not owner/name")
    if not token:
        raise ProofError("GITHUB_TOKEN is missing")
    return repo, token, api


def _github_headers(token: str) -> dict[str, str]:
    return {
        "Accept": "application/vnd.github+json",
        "Authorization": f"Bearer {token}",
        "User-Agent": "kiln-proof-status",
        "X-GitHub-Api-Version": "2022-11-28",
    }


def _github_fetch(url: str, token: str) -> object:
    request = urllib.request.Request(url, headers=_github_headers(token))
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            payload = json.load(response)
    except urllib.error.HTTPError as error:
        raise ProofError(f"GitHub API {error.code}") from error
    except urllib.error.URLError as error:
        raise ProofError("GitHub API request failed") from error
    except json.JSONDecodeError as error:
        raise ProofError("GitHub API returned malformed JSON") from error
    return payload


def _github_post(head: HeadSha, status: ProofStatus) -> None:
    repo, token, api = _github_target(os.environ)
    url = f"{api}/repos/{repo}/statuses/{head.value}"
    body = json.dumps(
        {
            "state": status.state.value,
            "context": status.context,
            "description": status.description,
        }
    ).encode()
    headers = _github_headers(token)
    headers["Content-Type"] = "application/json"
    request = urllib.request.Request(url, data=body, headers=headers, method="POST")
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            response.read()
            code = response.getcode()
    except urllib.error.HTTPError as error:
        raise ProofError(f"GitHub API {error.code}") from error
    except urllib.error.URLError as error:
        raise ProofError("GitHub API request failed") from error
    if code not in (200, 201):
        raise ProofError(f"GitHub API {code}")


if __name__ == "__main__":
    sys.exit(main())

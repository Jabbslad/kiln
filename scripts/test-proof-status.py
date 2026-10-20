#!/usr/bin/env python3

import contextlib
import importlib.util
import io
import json
import os
import re
import subprocess
import sys
import tempfile
import unittest
import urllib.error
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "proof_status.py"
PROOF = ROOT / ".github" / "workflows" / "proof.yml"
BUILD = ROOT / ".github" / "workflows" / "build.yml"
REPO = "Jabbslad/kiln"
TOKEN = "test-token"
API = "https://api.github.test"

LIVE = (
    ("539475e", "PASS at 539475e", "success", "PASS at 539475e"),
    ("d755dad", "PASS+NOTES at d755dad", "success", "PASS+NOTES at d755dad"),
    (
        "9bf4419",
        "PASS+NOTES at 9bf4419 (CI 39487224792).",
        "success",
        "PASS+NOTES at 9bf4419",
    ),
    (
        "8cf776c",
        "FAIL at 8cf776c (transcript edited; CI red).",
        "failure",
        "FAIL at 8cf776c",
    ),
    (
        "d1b89f0",
        "FAIL at d1b89f0 (CI 39484321370 red at docs-check). The runbook content would be PASS+NOTES once CI is green at the head.",
        "failure",
        "FAIL at d1b89f0",
    ),
    (
        "004deb5",
        "**FAIL at 004deb5** (CI run 39450414669)",
        "failure",
        "FAIL at 004deb5",
    ),
    ("f320845", "`FAIL at f320845`", "failure", "FAIL at f320845"),
    (
        "54645b1",
        "PASS+NOTES at 54645b1. This passes the runbook as a record only. It does not clear 8443 to close.",
        "success",
        "PASS+NOTES at 54645b1",
    ),
)


def load():
    spec = importlib.util.spec_from_file_location("proof_status", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    sys.modules["proof_status"] = module
    spec.loader.exec_module(module)
    return module


def full(prefix):
    return prefix + "0" * (40 - len(prefix))


def environ(directory, event_name, payload):
    path = Path(directory) / "event.json"
    path.write_text(json.dumps(payload), encoding="utf-8")
    return {
        "GITHUB_EVENT_NAME": event_name,
        "GITHUB_EVENT_PATH": str(path),
        "GITHUB_REPOSITORY": REPO,
        "GITHUB_TOKEN": TOKEN,
        "GITHUB_API_URL": API,
    }


def page_of(url):
    matched = re.search(r"[?&]page=(\d+)", url)
    if matched is None:
        raise AssertionError(url)
    return int(matched.group(1))


@contextlib.contextmanager
def swapped_env(updates):
    previous = {key: os.environ.get(key) for key in updates}
    os.environ.update(updates)
    try:
        yield
    finally:
        for key, value in previous.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value


class DecideTests(unittest.TestCase):
    def test_live_verdict_lines_match_the_head(self):
        module = load()
        for prefix, line, state, description in LIVE:
            with self.subTest(line=line):
                status = module.decide(
                    module.HeadSha(full(prefix)),
                    (module.IssueComment(1, "Jabbslad", "\n  " + line + "\n"),),
                )
                self.assertEqual(status.state, module.StatusState(state))
                self.assertEqual(status.state.value, state)
                self.assertEqual(status.description, description)
                self.assertEqual(status.context, "proof")
                self.assertLessEqual(len(status.description), 140)

    def test_full_sha_and_prefix_match(self):
        module = load()
        head = "a" * 40
        other = "b" * 40
        prefix = "a" * 39
        equal = module.decide(
            module.HeadSha(head),
            (module.IssueComment(1, "Jabbslad", f"PASS at {head.upper()}\n"),),
        )
        differ = module.decide(
            module.HeadSha(head),
            (module.IssueComment(1, "Jabbslad", f"FAIL at {other}\n"),),
        )
        shorter = module.decide(
            module.HeadSha(head),
            (module.IssueComment(1, "Jabbslad", f"PASS+NOTES at {prefix}\n"),),
        )
        self.assertEqual(equal.state, module.StatusState.SUCCESS)
        self.assertEqual(equal.description, f"PASS at {head}")
        self.assertEqual(differ.state, module.StatusState.PENDING)
        self.assertEqual(differ.description, f"FAIL at {other} does not match aaaaaaa")
        self.assertEqual(shorter.state, module.StatusState.SUCCESS)
        self.assertEqual(shorter.description, f"PASS+NOTES at {prefix}")
        self.assertLessEqual(len(differ.description), 140)
        self.assertLessEqual(len(shorter.description), 140)

    def test_a_sha_that_is_not_a_prefix_is_pending(self):
        module = load()
        status = module.decide(
            module.HeadSha(full("539475e")),
            (module.IssueComment(1, "Jabbslad", "PASS at d755dad\n"),),
        )
        self.assertEqual(status.state, module.StatusState.PENDING)
        self.assertEqual(status.description, "PASS at d755dad does not match 539475e")
        self.assertEqual(status.context, "proof")

    def test_uppercase_hex_matches_a_lowercase_head(self):
        module = load()
        status = module.decide(
            module.HeadSha(full("539475e")),
            (module.IssueComment(1, "Jabbslad", "PASS at 539475E\n"),),
        )
        self.assertEqual(status.state, module.StatusState.SUCCESS)
        self.assertEqual(status.description, "PASS at 539475e")

    def test_only_the_last_nonempty_line_can_be_a_verdict(self):
        module = load()
        head = module.HeadSha(full("539475e"))
        kept = module.decide(
            head,
            (module.IssueComment(1, "Jabbslad", "PASS at 539475e\n\n   \n"),),
        )
        prose = module.decide(
            head,
            (module.IssueComment(1, "Jabbslad", "PASS at 539475e\nOrdinary prose.\n"),),
        )
        failed = module.decide(
            head,
            (module.IssueComment(1, "Jabbslad", "PASS at 539475e\n\nFAIL at 539475e\n"),),
        )
        self.assertEqual(kept.description, "PASS at 539475e")
        self.assertEqual(prose.state, module.StatusState.PENDING)
        self.assertEqual(prose.description, "awaiting Proof for 539475e")
        self.assertEqual(failed.state, module.StatusState.FAILURE)
        self.assertEqual(failed.description, "FAIL at 539475e")

    def test_a_later_non_verdict_leaves_the_older_verdict(self):
        module = load()
        status = module.decide(
            module.HeadSha(full("539475e")),
            (
                module.IssueComment(4, "Jabbslad", "FAIL at 539475e\n"),
                module.IssueComment(8, "Jabbslad", "PASS+NOTES at 539475e\nThanks.\n"),
            ),
        )
        self.assertEqual(status.state, module.StatusState.FAILURE)
        self.assertEqual(status.description, "FAIL at 539475e")

    def test_the_greatest_comment_id_wins_in_either_order(self):
        module = load()
        head = module.HeadSha(full("539475e"))
        older = module.IssueComment(2, "Jabbslad", "PASS at 539475e\n")
        newer = module.IssueComment(9, "Jabbslad", "FAIL at 539475e\n")
        for comments in ((older, newer), (newer, older)):
            status = module.decide(head, comments)
            self.assertEqual(status.state, module.StatusState.FAILURE)
            self.assertEqual(status.description, "FAIL at 539475e")
        passed = module.decide(
            head,
            (
                module.IssueComment(9, "Jabbslad", "PASS+NOTES at 539475e\n"),
                module.IssueComment(2, "Jabbslad", "FAIL at 539475e\n"),
            ),
        )
        self.assertEqual(passed.state, module.StatusState.SUCCESS)
        self.assertEqual(passed.description, "PASS+NOTES at 539475e")

    def test_jabbslad_matches_by_casefold_and_other_logins_do_not(self):
        module = load()
        head = module.HeadSha(full("539475e"))
        lower = module.decide(
            head,
            (module.IssueComment(1, "jabbslad", "PASS at 539475e\n"),),
        )
        ignored = module.decide(
            head,
            (
                module.IssueComment(1, "jabbslad", "PASS at 539475e\n"),
                module.IssueComment(5, "proof-bot", "FAIL at 539475e\n"),
            ),
        )
        other = module.decide(
            head,
            (module.IssueComment(1, "proof-bot", "PASS at 539475e\n"),),
        )
        spaced = module.decide(
            head,
            (module.IssueComment(1, "Jabbslad ", "PASS at 539475e\n"),),
        )
        self.assertEqual(lower.description, "PASS at 539475e")
        self.assertEqual(ignored.description, "PASS at 539475e")
        self.assertEqual(other.description, "awaiting Proof for 539475e")
        self.assertEqual(spaced.description, "awaiting Proof for 539475e")

    def test_no_comments_and_a_blank_body_wait_for_proof(self):
        module = load()
        head = module.HeadSha(full("539475e"))
        empty = module.decide(head, ())
        blank = module.decide(head, (module.IssueComment(1, "Jabbslad", " \n\n\t"),))
        self.assertEqual(empty.state, module.StatusState.PENDING)
        self.assertEqual(empty.description, "awaiting Proof for 539475e")
        self.assertEqual(blank.description, empty.description)
        self.assertEqual(empty.context, "proof")

    def test_a_backticked_sha_and_a_bold_verdict_word(self):
        module = load()
        head = module.HeadSha(full("2724325"))
        cases = (
            ("PASS at `2724325`", "success", "PASS at 2724325"),
            ("**PASS** at 2724325", "success", "PASS at 2724325"),
            ("**PASS+NOTES** at `2724325`", "success", "PASS+NOTES at 2724325"),
            ("**FAIL** at `2724325`", "failure", "FAIL at 2724325"),
            ("PASS at `2724325`.", "success", "PASS at 2724325"),
        )
        for line, state, description in cases:
            with self.subTest(line=line):
                status = module.decide(
                    head,
                    (module.IssueComment(1, "Jabbslad", line + "\n"),),
                )
                self.assertEqual(status.state, module.StatusState(state))
                self.assertEqual(status.description, description)

    def test_a_single_asterisk_is_not_an_opener(self):
        module = load()
        head = module.HeadSha(full("539475e"))
        for line in ("*FAIL at 539475e", "* FAIL at 539475e"):
            status = module.decide(head, (module.IssueComment(1, "Jabbslad", line + "\n"),))
            self.assertEqual(status.description, "awaiting Proof for 539475e", line)

    def test_a_sha_longer_than_40_hex_digits_is_not_a_verdict(self):
        module = load()
        head = "a" * 40
        status = module.decide(
            module.HeadSha(head),
            (module.IssueComment(1, "Jabbslad", "PASS at " + ("a" * 41) + "\n"),),
        )
        self.assertEqual(status.state, module.StatusState.PENDING)
        self.assertEqual(status.description, "awaiting Proof for aaaaaaa")
        short = module.decide(
            module.HeadSha(head),
            (module.IssueComment(1, "Jabbslad", "PASS at abcdef\n"),),
        )
        self.assertEqual(short.description, "awaiting Proof for aaaaaaa")

    def test_duplicate_comment_id_raises(self):
        module = load()
        with self.assertRaises(module.ProofError):
            module.decide(
                module.HeadSha(full("539475e")),
                (
                    module.IssueComment(4, "Jabbslad", "PASS at 539475e\n"),
                    module.IssueComment(4, "other", "nope"),
                ),
            )

    def test_descriptions_stay_within_140_characters(self):
        module = load()
        sha = "b" * 40
        status = module.decide(
            module.HeadSha("a" * 40),
            (module.IssueComment(1, "Jabbslad", f"PASS+NOTES at {sha}\n"),),
        )
        self.assertEqual(status.description, f"PASS+NOTES at {sha} does not match aaaaaaa")
        self.assertLessEqual(len(status.description), 140)
        with self.assertRaises(ValueError):
            module.ProofStatus(module.StatusState.PENDING, "x" * 141)
        with self.assertRaises(ValueError):
            module.ProofStatus(module.StatusState.PENDING, "")

    def test_head_and_comment_shapes_reject_bad_values(self):
        module = load()
        with self.assertRaises(ValueError):
            module.HeadSha("539475e")
        with self.assertRaises(ValueError):
            module.HeadSha("A" * 40)
        with self.assertRaises(ValueError):
            module.IssueComment(True, "Jabbslad", "x")
        with self.assertRaises(ValueError):
            module.IssueComment(0, "Jabbslad", "x")
        status = module.ProofStatus(module.StatusState.PENDING, "awaiting Proof for 539475e")
        self.assertEqual(status.context, "proof")


class SyncStatusTests(unittest.TestCase):
    def test_pull_request_target_posts_the_event_head_without_fetching_the_pull(self):
        module = load()
        raw = "539475E" + "0" * 33
        urls = []
        posted = []

        def fetch(url, token):
            self.assertEqual(token, TOKEN)
            urls.append(url)
            self.assertNotIn("/pulls/", url)
            self.assertIn("/repos/Jabbslad/kiln/issues/12/comments", url)
            self.assertIn("per_page=100", url)
            return [
                {"id": 10, "user": {"login": "Jabbslad"}, "body": "PASS at 539475e\n"},
            ]

        def post(head, status):
            posted.append((head.value, status.state, status.context, status.description))

        with tempfile.TemporaryDirectory() as tmp:
            result = module.sync_status(
                environ(
                    tmp,
                    "pull_request_target",
                    {"pull_request": {"number": 12, "head": {"sha": raw}}},
                ),
                fetch,
                post,
            )
        self.assertEqual(
            posted,
            [(raw.lower(), module.StatusState.SUCCESS, "proof", "PASS at 539475e")],
        )
        self.assertEqual(result, module.ProofStatus(module.StatusState.SUCCESS, "PASS at 539475e"))
        self.assertTrue(urls)
        self.assertTrue(all("/pulls/" not in url for url in urls))

    def test_issue_comment_on_a_pull_posts_the_fetched_head(self):
        module = load()
        head = full("d755dad")
        urls = []
        posted = []

        def fetch(url, token):
            self.assertEqual(token, TOKEN)
            urls.append(url)
            if "/pulls/8" in url:
                self.assertNotIn("/comments", url)
                return {"number": 8, "head": {"sha": head}}
            self.assertIn("/repos/Jabbslad/kiln/issues/8/comments", url)
            return [
                {"id": 3, "user": {"login": "Jabbslad"}, "body": "PASS+NOTES at d755dad\n"},
            ]

        def post(got, status):
            posted.append((got.value, status.state.value, status.context, status.description))

        payload = {
            "issue": {
                "number": 8,
                "pull_request": {"url": "https://api.github.com/repos/Jabbslad/kiln/pulls/8"},
            },
            "comment": {"id": 99, "user": {"login": "Jabbslad"}, "body": "FAIL at d755dad\n"},
        }
        with tempfile.TemporaryDirectory() as tmp:
            result = module.sync_status(environ(tmp, "issue_comment", payload), fetch, post)
        self.assertEqual(posted, [(head, "success", "proof", "PASS+NOTES at d755dad")])
        self.assertEqual(result.description, "PASS+NOTES at d755dad")
        self.assertTrue(any("/pulls/8" in url for url in urls))
        self.assertTrue(any("/issues/8/comments" in url for url in urls))

    def test_issue_comment_on_an_issue_posts_nothing(self):
        module = load()
        for payload in ({"issue": {"number": 4}}, {"issue": {"number": 4, "pull_request": None}}):
            calls = []

            def fetch(url, token):
                calls.append(url)
                raise AssertionError(url)

            def post(head, status):
                raise AssertionError(status.description)

            with tempfile.TemporaryDirectory() as tmp:
                result = module.sync_status(environ(tmp, "issue_comment", payload), fetch, post)
            self.assertIsNone(result)
            self.assertEqual(calls, [])

    def test_pull_request_event_does_not_post(self):
        module = load()
        posted = []

        def fetch(url, token):
            raise AssertionError(url)

        def post(head, status):
            posted.append(status)

        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(module.ProofError):
                module.sync_status(
                    environ(
                        tmp,
                        "pull_request",
                        {"pull_request": {"number": 12, "head": {"sha": "a" * 40}}},
                    ),
                    fetch,
                    post,
                )
        self.assertEqual(posted, [])

    def test_a_full_twentieth_comment_page_does_not_post(self):
        module = load()
        pages = []
        posted = []

        def fetch(url, token):
            self.assertNotIn("/pulls/", url)
            page = page_of(url)
            pages.append(page)
            self.assertLessEqual(page, 20)
            start = (page - 1) * 100
            return [
                {
                    "id": start + offset + 1,
                    "user": {"login": "Jabbslad"},
                    "body": "PASS at 539475e\n",
                }
                for offset in range(100)
            ]

        def post(head, status):
            posted.append(status)

        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(module.ProofError):
                module.sync_status(
                    environ(
                        tmp,
                        "pull_request_target",
                        {"pull_request": {"number": 12, "head": {"sha": full("539475e")}}},
                    ),
                    fetch,
                    post,
                )
        self.assertEqual(posted, [])
        self.assertEqual(sorted(pages), list(range(1, 21)))

    def test_a_full_page_does_not_hide_a_later_verdict(self):
        module = load()
        posted = []

        def fetch(url, token):
            page = page_of(url)
            if page == 1:
                return [
                    {"id": item, "user": {"login": "Jabbslad"}, "body": "thanks"}
                    for item in range(1, 101)
                ]
            if page == 2:
                return [{"id": 250, "user": {"login": "Jabbslad"}, "body": "FAIL at 8cf776c\n"}]
            raise AssertionError(url)

        def post(head, status):
            posted.append((head.value, status.state.value, status.description))

        with tempfile.TemporaryDirectory() as tmp:
            module.sync_status(
                environ(
                    tmp,
                    "pull_request_target",
                    {"pull_request": {"number": 3, "head": {"sha": full("8cf776c")}}},
                ),
                fetch,
                post,
            )
        self.assertEqual(posted, [(full("8cf776c"), "failure", "FAIL at 8cf776c")])

    def test_a_malformed_later_page_does_not_post(self):
        module = load()
        posted = []

        def fetch(url, token):
            if page_of(url) == 1:
                return [
                    {"id": item, "user": {"login": "Jabbslad"}, "body": "PASS at 539475e"}
                    for item in range(1, 101)
                ]
            return {"message": "broken"}

        def post(head, status):
            posted.append(status)

        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(module.ProofError):
                module.sync_status(
                    environ(
                        tmp,
                        "pull_request_target",
                        {"pull_request": {"number": 3, "head": {"sha": full("539475e")}}},
                    ),
                    fetch,
                    post,
                )
        self.assertEqual(posted, [])

    def test_duplicate_comment_ids_do_not_post(self):
        module = load()
        posted = []

        def fetch(url, token):
            return [
                {"id": 1, "user": {"login": "Jabbslad"}, "body": "PASS at 539475e"},
                {"id": 1, "user": {"login": "Jabbslad"}, "body": "FAIL at 539475e"},
            ]

        def post(head, status):
            posted.append(status)

        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(module.ProofError):
                module.sync_status(
                    environ(
                        tmp,
                        "pull_request_target",
                        {"pull_request": {"number": 3, "head": {"sha": full("539475e")}}},
                    ),
                    fetch,
                    post,
                )
        self.assertEqual(posted, [])

    def test_a_fetch_error_does_not_post(self):
        module = load()
        posted = []

        def fetch(url, token):
            raise RuntimeError("network down")

        def post(head, status):
            posted.append(status)

        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(RuntimeError):
                module.sync_status(
                    environ(
                        tmp,
                        "pull_request_target",
                        {"pull_request": {"number": 3, "head": {"sha": full("539475e")}}},
                    ),
                    fetch,
                    post,
                )
        self.assertEqual(posted, [])

    def test_a_bad_head_does_not_post(self):
        module = load()
        posted = []

        def fetch(url, token):
            self.assertNotIn("not-a-sha", url)
            return []

        def post(head, status):
            posted.append(status)

        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(module.ProofError):
                module.sync_status(
                    environ(
                        tmp,
                        "pull_request_target",
                        {"pull_request": {"number": 3, "head": {"sha": "not-a-sha"}}},
                    ),
                    fetch,
                    post,
                )
        self.assertEqual(posted, [])

    def test_the_same_snapshot_posts_the_same_status_twice(self):
        module = load()
        head = full("8cf776c")
        posted = []

        def fetch(url, token):
            if "/pulls/" in url:
                return {"head": {"sha": head}}
            return [
                {
                    "id": 1,
                    "user": {"login": "Jabbslad"},
                    "body": "FAIL at 8cf776c (transcript edited; CI red).\n",
                }
            ]

        def post(got, status):
            posted.append((got.value, status.state.value, status.context, status.description))

        payload = {"issue": {"number": 5, "pull_request": {"url": "https://example.test/pulls/5"}}}
        with tempfile.TemporaryDirectory() as tmp:
            env = environ(tmp, "issue_comment", payload)
            first = module.sync_status(env, fetch, post)
            second = module.sync_status(env, fetch, post)
        self.assertEqual(len(posted), 2)
        self.assertEqual(posted[0], posted[1])
        self.assertEqual(posted[0], (head, "failure", "proof", "FAIL at 8cf776c"))
        self.assertEqual(first, second)

    def test_post_sends_the_proof_status_body(self):
        module = load()
        head = full("539475e")
        calls = []

        class Response(io.BytesIO):
            def __init__(self, status, payload):
                super().__init__(json.dumps(payload).encode())
                self.status = status

            def getcode(self):
                return self.status

            def __enter__(self):
                return self

            def __exit__(self, exc_type, exc, tb):
                return False

        def urlopen(request, timeout=30):
            calls.append(request)
            if request.get_method() == "POST":
                return Response(201, {})
            if "/pulls/" in request.full_url:
                return Response(200, {"head": {"sha": head}})
            return Response(
                200,
                [{"id": 10, "user": {"login": "Jabbslad"}, "body": "PASS at 539475e\n"}],
            )

        original = module.urllib.request.urlopen
        module.urllib.request.urlopen = urlopen
        payload = {
            "issue": {"number": 8, "pull_request": {"url": "https://example.test/pulls/8"}},
        }
        env = {
            "GITHUB_EVENT_NAME": "issue_comment",
            "GITHUB_REPOSITORY": REPO,
            "GITHUB_TOKEN": TOKEN,
            "GITHUB_API_URL": API,
        }
        try:
            with tempfile.TemporaryDirectory() as tmp:
                env["GITHUB_EVENT_PATH"] = environ(tmp, "issue_comment", payload)["GITHUB_EVENT_PATH"]
                with swapped_env(env):
                    result = module.sync_status(env, module._github_fetch, module._github_post)
        finally:
            module.urllib.request.urlopen = original
        self.assertEqual(result.description, "PASS at 539475e")
        methods = [request.get_method() for request in calls]
        self.assertEqual(methods, ["GET", "GET", "POST"])
        post = calls[-1]
        self.assertIn(f"/repos/{REPO}/statuses/{head}", post.full_url)
        self.assertNotIn(TOKEN, post.full_url)
        self.assertEqual(
            json.loads(post.data.decode()),
            {"state": "success", "context": "proof", "description": "PASS at 539475e"},
        )
        for request in calls:
            headers = {key.lower(): value for key, value in request.header_items()}
            self.assertEqual(headers["authorization"], f"Bearer {TOKEN}")
            self.assertEqual(headers["accept"], "application/vnd.github+json")
            self.assertEqual(headers["x-github-api-version"], "2022-11-28")
            self.assertEqual(headers["user-agent"], "kiln-proof-status")
            self.assertNotIn(TOKEN, request.full_url)

    def test_post_accepts_200_and_rejects_other_codes(self):
        module = load()
        head = module.HeadSha(full("539475e"))
        status = module.ProofStatus(module.StatusState.FAILURE, "FAIL at 539475e")

        class Response(io.BytesIO):
            def __init__(self, code):
                super().__init__(b"{}")
                self.status = code

            def getcode(self):
                return self.status

            def __enter__(self):
                return self

            def __exit__(self, exc_type, exc, tb):
                return False

        def install(code, error=False):
            def urlopen(request, timeout=30):
                if error:
                    raise urllib.error.HTTPError(request.full_url, code, "no", None, io.BytesIO(b""))
                return Response(code)

            module.urllib.request.urlopen = urlopen

        original = module.urllib.request.urlopen
        env = {
            "GITHUB_REPOSITORY": REPO,
            "GITHUB_TOKEN": TOKEN,
            "GITHUB_API_URL": API,
        }
        try:
            with swapped_env(env):
                install(200)
                module._github_post(head, status)
                install(201)
                module._github_post(head, status)
                install(202)
                with self.assertRaises(module.ProofError):
                    module._github_post(head, status)
                install(403, error=True)
                with self.assertRaises(module.ProofError):
                    module._github_post(head, status)
        finally:
            module.urllib.request.urlopen = original

    def test_main_exits_zero_for_failure_and_hides_the_token(self):
        module = load()
        token = "super-secret-token"
        head = full("8cf776c")
        posted = []

        def fetch(url, got):
            self.assertEqual(got, token)
            if "/pulls/" in url:
                return {"head": {"sha": head}}
            return [{"id": 1, "user": {"login": "Jabbslad"}, "body": "FAIL at 8cf776c\n"}]

        def post(got, status):
            posted.append(status.state.value)

        module._github_fetch = fetch
        module._github_post = post
        payload = {"issue": {"number": 5, "pull_request": {"url": "https://example.test/pulls/5"}}}
        with tempfile.TemporaryDirectory() as tmp:
            env = environ(tmp, "issue_comment", payload)
            env["GITHUB_TOKEN"] = token
            with swapped_env(env):
                stdout = io.StringIO()
                stderr = io.StringIO()
                with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                    code = module.main()
        self.assertEqual(code, 0, stderr.getvalue())
        self.assertEqual(posted, ["failure"])
        self.assertEqual(stdout.getvalue(), "")
        self.assertNotIn(token, stderr.getvalue())

        def explode(url, got):
            raise RuntimeError(f"network down {token}")

        module._github_fetch = explode
        with tempfile.TemporaryDirectory() as tmp:
            env = environ(tmp, "issue_comment", payload)
            env["GITHUB_TOKEN"] = token
            with swapped_env(env):
                stderr = io.StringIO()
                with contextlib.redirect_stderr(stderr):
                    code = module.main()
        self.assertEqual(code, 1)
        self.assertNotIn(token, stderr.getvalue())
        self.assertIn("network down", stderr.getvalue())

    def test_main_process_exits_zero_for_a_non_pull_and_one_for_pull_request(self):
        token = "process-secret-token"
        with tempfile.TemporaryDirectory() as tmp:
            event = Path(tmp) / "event.json"
            event.write_text(json.dumps({"issue": {"number": 4}}), encoding="utf-8")
            env = os.environ.copy()
            env.update(
                {
                    "GITHUB_EVENT_NAME": "issue_comment",
                    "GITHUB_EVENT_PATH": str(event),
                    "GITHUB_REPOSITORY": REPO,
                    "GITHUB_TOKEN": token,
                    "GITHUB_API_URL": "https://127.0.0.1:9",
                }
            )
            ok = subprocess.run(
                [sys.executable, str(SCRIPT)],
                cwd=ROOT,
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(ok.returncode, 0, ok.stderr)
            self.assertNotIn(token, ok.stdout)
            self.assertNotIn(token, ok.stderr)
            event.write_text(
                json.dumps({"pull_request": {"number": 1, "head": {"sha": "a" * 40}}}),
                encoding="utf-8",
            )
            env["GITHUB_EVENT_NAME"] = "pull_request"
            bad = subprocess.run(
                [sys.executable, str(SCRIPT)],
                cwd=ROOT,
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )
        self.assertEqual(bad.returncode, 1, bad.stderr)
        self.assertNotIn(token, bad.stderr)
        self.assertNotIn(token, bad.stdout)


class WorkflowTests(unittest.TestCase):
    def test_proof_workflow_uses_pull_request_target(self):
        text = PROOF.read_text(encoding="utf-8")
        triggers = text.split("on:\n", 1)[1].split("\npermissions:\n", 1)[0]
        keys = [
            line.strip()
            for line in triggers.splitlines()
            if line.startswith("  ") and not line.startswith("    ")
        ]
        self.assertEqual(keys, ["issue_comment:", "pull_request_target:"])
        self.assertNotIn("pull_request:", text)
        issue = text.split("issue_comment:\n", 1)[1].split("\n  pull_request_target:\n", 1)[0]
        target = text.split("pull_request_target:\n", 1)[1].split("\npermissions:\n", 1)[0]
        self.assertEqual(issue.strip(), "types: [created, edited]")
        self.assertEqual(target.strip(), "types: [opened, synchronize, reopened]")
        self.assertIn(
            "if: github.event_name == 'pull_request_target' || github.event.issue.pull_request",
            text,
        )
        self.assertNotRegex(text, r"github\.event_name == 'pull_request'(?!_)")
        permissions = text.split("permissions:\n", 1)[1].split("\nconcurrency:\n", 1)[0]
        self.assertEqual(
            permissions,
            "  contents: read\n  issues: read\n  pull-requests: read\n  statuses: write\n",
        )
        self.assertEqual(text.count("permissions:"), 1)
        for forbidden in (
            "pull-requests: write",
            "contents: write",
            "actions: write",
            "checks: write",
            "secrets.",
        ):
            self.assertNotIn(forbidden, text)
        self.assertIn("ref: ${{ github.event.repository.default_branch }}", text)
        self.assertIn("persist-credentials: false", text)
        self.assertIn("fetch-depth: 1", text)
        for forbidden in ("github.sha", "head.sha", "head.ref"):
            self.assertNotIn(forbidden, text)
        self.assertEqual(text.count("run: python scripts/proof_status.py"), 1)
        step = text.split("- name: Set proof status\n", 1)[1]
        self.assertTrue(
            step.startswith(
                "        env:\n"
                "          GITHUB_TOKEN: ${{ github.token }}\n"
                "        run: python scripts/proof_status.py\n"
            ),
            step,
        )
        self.assertIn("\n  status:\n", text)
        self.assertIn("name: proof-status", text)
        self.assertNotRegex(text, r"(?m)^  proof:")
        self.assertNotIn("gh pr merge", text)
        self.assertNotIn("--auto", text)
        self.assertIn("actions/checkout@11d5960a326750d5838078e36cf38b85af677262", text)
        self.assertIn("actions/setup-python@a26af69be951a213d495a4c3e4e4022e16d87065", text)
        self.assertIn("cancel-in-progress: false", text)

    def test_build_runs_the_proof_status_tests_twice(self):
        text = BUILD.read_text(encoding="utf-8")
        self.assertEqual(text.count("python scripts/test-proof-status.py"), 2)
        before, after = text.split("\n  server-check:\n", 1)
        self.assertEqual(before.count("python scripts/test-proof-status.py"), 1)
        self.assertEqual(after.count("python scripts/test-proof-status.py"), 1)
        self.assertEqual(
            text.count(
                "python scripts/test-merge-queue-tested.py\n          python scripts/test-proof-status.py\n"
            ),
            2,
        )


if __name__ == "__main__":
    unittest.main()

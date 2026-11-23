#!/usr/bin/env python3
"""Contract tests for scripts/ready-label.py against a stub `gh`."""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "ready-label.py"
LABEL = "ready for maintainer review"

# Answers the read of a pull request's later threads from $THREADS_FILE, the
# read of every pull request from $PAGES (failing every time, or the first two
# times, and counting each try in $TRIES), the runs held for approval from
# $RUNS_FILE unless $RUNS says the read fails, the head lookup from $HEAD, the
# label lookup from $LABEL_EXISTS, and records every write in $CALLS. A write
# to pull request $WRITE_FAIL and the head lookup of $HEAD_FAIL fail.
STUB = """#!/bin/sh
case "$*" in
*graphql*number=*endCursor=c*) cat "$THREADS_FILE" ;;
*graphql*)
  echo try >> "$TRIES"
  [ "$PAGES" = fail ] && { echo "gh: HTTP 502" >&2; exit 1; }
  [ "$PAGES" = flaky ] && [ "$(wc -l < "$TRIES")" -le 2 ] && exit 1
  cat "$PAGES_FILE" ;;
"api -X DELETE "*|*"labels[]="*)
  case "$*" in *"/issues/$WRITE_FAIL/"*) echo "gh: HTTP 403 write" >&2; exit 1 ;; esac
  echo "$*" >> "$CALLS" ;;
*actions/runs*)
  [ "$RUNS" = fail ] && { echo "gh: HTTP 403 actions" >&2; exit 1; }
  cat "$RUNS_FILE" ;;
*/pulls/*)
  case "$*" in *"/pulls/$HEAD_FAIL "*) echo "gh: HTTP 404 head" >&2; exit 1 ;; esac
  echo "$HEAD" ;;
*/labels/*) [ "$LABEL_EXISTS" = yes ] || { echo "gh: HTTP 403 rate limited" >&2; exit 1; } ;;
esac
"""


def thread(author, resolved=True, bot=False):
    kind = "Bot" if bot else "User"
    return {
        "isResolved": resolved,
        "comments": {"nodes": [{"author": {"__typename": kind, "login": author}}]},
    }


def run(name, conclusion="SUCCESS", status="COMPLETED", run_id=1):
    return {
        "__typename": "CheckRun",
        "databaseId": run_id,
        "name": name,
        "status": status,
        "conclusion": conclusion,
    }


def review(
    author,
    state="COMMENTED",
    on="old",
    threads=1,
    at="2026-01-01",
    bot=False,
    replies=0,
):
    """A review by AUTHOR left on commit ON, opening THREADS threads and
    replying REPLIES times to threads opened before it."""
    nodes = [{"replyTo": None}] * threads + [{"replyTo": {"id": "c0"}}] * replies
    return {
        "author": {"__typename": "Bot" if bot else "User", "login": author},
        "state": state,
        "submittedAt": at,
        "commit": {"oid": on},
        "comments": {"totalCount": len(nodes), "nodes": nodes},
    }


def status(context, state):
    return {"__typename": "StatusContext", "context": context, "state": state}


def pr(number=1, **over):
    """A pull request that is ready unless OVER says otherwise."""
    base = {
        "number": number,
        "isDraft": False,
        "mergeable": "MERGEABLE",
        "author": {"login": "alice"},
        "labels": {"nodes": []},
        "checks": [run("gates (rust)"), run("check")],
        "threads": [thread("maintainer")],
        "reviews": [review("maintainer")],
        "headRefOid": "head",
        "headRefName": "feature",
        "headRepository": {"nameWithOwner": "alice/r"},
    }
    base.update(over)
    checks = base.pop("checks")
    threads = base.pop("threads")
    reviews = base.pop("reviews")
    base["reviews"] = {
        "totalCount": base.pop("review_count", len(reviews)),
        "nodes": reviews,
    }
    base["commits"] = {
        "nodes": [
            {
                "commit": {
                    "statusCheckRollup": {
                        "contexts": {
                            "totalCount": base.pop("check_count", len(checks)),
                            "nodes": checks,
                        }
                    }
                }
            }
        ]
    }
    total = base.pop("thread_count", len(threads))
    base["reviewThreads"] = {
        "totalCount": total,
        "threadPages": {"hasNextPage": total > len(threads), "endCursor": "c"},
        "nodes": threads,
    }
    return base


def labeled(**over):
    return pr(labels={"nodes": [{"name": LABEL}]}, **over)


def sweep(
    prs,
    *args,
    pages="ok",
    label_exists=True,
    stderr=None,
    later_threads=(),
    head="head",
    tries=None,
    held=(),
    runs="ok",
    write_fail=0,
    head_fail=0,
):
    """Run the script over PRS; return (status, stdout, recorded writes).
    With STDERR, a list, the run's stderr is appended to it; with TRIES, the
    number of reads of every pull request. LATER_THREADS answer the read of
    a pull request's threads past the first 100, HEAD the head lookup, and
    HELD the runs held for approval."""
    with tempfile.TemporaryDirectory() as tmp:
        gh = Path(tmp, "gh")
        gh.write_text(STUB)
        gh.chmod(0o755)
        # A list of lists is one page each, printed back to back as --paginate
        # prints them.
        split = prs if prs and isinstance(prs[0], list) else [prs]
        Path(tmp, "pages.json").write_text(
            "\n".join(
                json.dumps({"data": {"repository": {"pullRequests": {"nodes": p}}}})
                for p in split
            )
        )
        Path(tmp, "threads.json").write_text(
            json.dumps(
                {
                    "data": {
                        "repository": {
                            "pullRequest": {
                                "reviewThreads": {"nodes": list(later_threads)}
                            }
                        }
                    }
                }
            )
        )
        Path(tmp, "runs.json").write_text("".join(json.dumps(r) + "\n" for r in held))
        calls = Path(tmp, "calls")
        calls.touch()
        Path(tmp, "tries").touch()
        env = {
            **os.environ,
            "PATH": f"{tmp}{os.pathsep}{os.environ['PATH']}",
            "PAGES": pages,
            "PAGES_FILE": str(Path(tmp, "pages.json")),
            "LABEL_EXISTS": "yes" if label_exists else "no",
            "CALLS": str(calls),
            "THREADS_FILE": str(Path(tmp, "threads.json")),
            "TRIES": str(Path(tmp, "tries")),
            "HEAD": head,
            "RUNS_FILE": str(Path(tmp, "runs.json")),
            "RUNS": runs,
            "WRITE_FAIL": str(write_fail),
            "HEAD_FAIL": str(head_fail),
            "READY_LABEL_PAUSE": "0",
        }
        result = subprocess.run(
            [sys.executable, str(SCRIPT), *args, "o/r"],
            env=env,
            capture_output=True,
            text=True,
            check=False,
        )
        if stderr is not None:
            stderr.append(result.stderr)
        if tries is not None:
            tries.append(len(Path(tmp, "tries").read_text().splitlines()))
        return result.returncode, result.stdout, calls.read_text().splitlines()


ADD = f"api repos/o/r/issues/1/labels -f labels[]={LABEL} --silent"
REMOVE = (
    "api -X DELETE repos/o/r/issues/1/labels/ready%20for%20maintainer%20review --silent"
)


class ReadyLabel(unittest.TestCase):
    # The exit status is asserted too: a run that crashes writes nothing, so a
    # bare check of the writes would pass every withholding case.
    def assert_adds(self, *prs):
        self.assertEqual(sweep(list(prs))[::2], (0, [ADD]))

    def assert_leaves(self, *prs, **kwargs):
        self.assertEqual(sweep(list(prs), **kwargs)[::2], (0, []))

    def test_every_reviewer_thread_resolved_and_green_adds_the_label(self):
        status, out, calls = sweep([pr()])
        self.assertEqual(status, 0)
        self.assertEqual(calls, [ADD])
        self.assertIn("#1: add", out)

    def test_a_label_already_there_is_not_added_again(self):
        self.assert_leaves(labeled())

    def test_an_open_reviewer_thread_withholds_the_label(self):
        self.assert_leaves(pr(threads=[thread("bob"), thread("bob", False)]))

    def test_an_open_bot_thread_withholds_the_label(self):
        bot = thread("cubic", False, bot=True)
        self.assert_leaves(pr(threads=[thread("bob"), bot]))

    def test_the_authors_own_open_thread_does_not_count(self):
        self.assert_adds(pr(threads=[thread("bob"), thread("alice", False)]))

    def test_only_the_authors_threads_and_reviews_are_not_a_review(self):
        self.assert_leaves(
            pr(threads=[thread("alice")], reviews=[review("alice", on="head")])
        )

    def test_no_review_at_all_withholds_the_label(self):
        self.assert_leaves(pr(threads=[], reviews=[]))

    def test_a_review_without_threads_answered_by_a_push_adds_the_label(self):
        self.assert_adds(
            pr(threads=[], reviews=[review("bob", "CHANGES_REQUESTED", threads=0)])
        )

    def test_a_review_on_this_head_without_a_thread_withholds_the_label(self):
        for state in ("APPROVED", "COMMENTED", "CHANGES_REQUESTED"):
            with self.subTest(state=state):
                reviews = [
                    review("bob"),
                    review("carol", state, "head", 0, "2026-02-01"),
                ]
                self.assert_leaves(pr(reviews=reviews))

    def test_a_new_review_takes_the_label_off(self):
        reviews = [review("bob"), review("bob", "APPROVED", "head", 0, "2026-02-01")]
        status, out, calls = sweep([labeled(reviews=reviews)])
        self.assertEqual((status, calls), (0, [REMOVE]))
        self.assertIn("reviewed by bob since the last push", out)

    def test_a_review_on_this_head_whose_threads_are_resolved_is_answered(self):
        self.assert_adds(pr(reviews=[review("bob", on="head", threads=2)]))

    def test_a_bot_or_dismissed_review_on_this_head_does_not_withhold(self):
        for late in (
            review("bot", on="head", threads=0, at="2026-02-01", bot=True),
            review("bob", "DISMISSED", "head", 0, "2026-02-01"),
            review("bob", "PENDING", "head", 0, "2026-02-01"),
        ):
            with self.subTest(late=late):
                self.assert_adds(pr(reviews=[review("bob"), late]))

    def test_a_draft_conflict_or_red_check_withholds_the_label(self):
        for over in (
            {"isDraft": True},
            {"mergeable": "CONFLICTING"},
            {"checks": [run("gates (rust)"), run("lint", "FAILURE")]},
            {"checks": [run("gates (rust)"), run("lint", None, "IN_PROGRESS")]},
            {"checks": [run("gates (rust)", "ACTION_REQUIRED")]},
            {"checks": [run("check")]},
            {"checks": [run("gates (rust)", "SKIPPED")]},
            {"checks": []},
        ):
            with self.subTest(over=over):
                self.assert_leaves(pr(**over))

    def test_a_rerun_that_passed_replaces_the_failure(self):
        checks = [
            run("gates (rust)", "FAILURE", run_id=1),
            run("gates (rust)", run_id=2),
        ]
        self.assert_adds(pr(checks=checks))
        self.assert_adds(pr(checks=checks[::-1]))

    def test_a_skipped_check_beside_a_passing_gates_lane_is_green(self):
        self.assert_adds(pr(checks=[run("gates (rust)"), run("mutants", "SKIPPED")]))

    def test_a_pr_that_stopped_being_ready_loses_the_label(self):
        for over in (
            {"threads": [thread("bob", False)]},
            {"isDraft": True},
            {"mergeable": "CONFLICTING"},
            {"checks": [run("gates (rust)", None, "QUEUED")]},
        ):
            with self.subTest(over=over):
                status, out, calls = sweep([labeled(**over)])
                self.assertEqual((status, calls), (0, [REMOVE]))
                self.assertIn("#1: remove", out)

    def test_an_answer_not_read_in_full_leaves_the_label_alone(self):
        for over in (
            {"mergeable": "UNKNOWN"},
            {"thread_count": 101},
            {"check_count": 51},
        ):
            with self.subTest(over=over):
                self.assert_leaves(pr(**over))
                self.assert_leaves(labeled(**over))

    def test_a_draft_or_conflict_loses_the_label_whatever_was_cut_off(self):
        for known in ({"isDraft": True}, {"mergeable": "CONFLICTING"}):
            for over in (
                {"thread_count": 101},
                {"check_count": 51},
                {"review_count": 101},
            ):
                with self.subTest(known=known, over=over):
                    self.assertEqual(sweep([labeled(**known, **over)])[2], [REMOVE])
        # A draft is known before GitHub has computed its mergeability.
        self.assertEqual(
            sweep([labeled(isDraft=True, mergeable="UNKNOWN")])[2], [REMOVE]
        )

    def test_dry_run_reports_without_writing(self):
        status, out, calls = sweep([pr()], "--dry-run", label_exists=False)
        self.assertEqual((status, calls), (0, []))
        self.assertIn("#1: add", out)

    def test_a_missing_label_or_api_error_writes_nothing(self):
        self.assertEqual(sweep([pr()], label_exists=False)[::2], (2, []))
        self.assertEqual(sweep([pr()], pages="fail")[::2], (2, []))

    def test_a_stale_check_is_not_a_pass(self):
        self.assert_leaves(pr(checks=[run("gates (rust)", "STALE")]))

    def test_a_status_context_is_read(self):
        for state, adds in (("SUCCESS", True), ("PENDING", False), ("ERROR", False)):
            with self.subTest(state=state):
                checks = [run("gates (rust)"), status("ci/x", state)]
                self.assertEqual(sweep([pr(checks=checks)])[2], [ADD] if adds else [])

    def test_a_failing_status_is_not_hidden_by_a_run_of_its_name(self):
        checks = [run("gates (rust)"), status("gates (rust)", "FAILURE")]
        for order in (checks, checks[::-1]):
            with self.subTest(first=order[0]["__typename"]):
                self.assert_leaves(pr(checks=order))

    def test_any_reviewer_on_this_head_without_a_thread_withholds(self):
        reviews = [
            review("bob", "APPROVED", "head", 0, "2026-02-01"),
            review("carol", on="head", at="2026-03-01"),
        ]
        self.assert_leaves(pr(reviews=reviews))

    def test_a_reviewers_own_later_review_supersedes_theirs(self):
        reviews = [
            review("bob", "APPROVED", "head", 0, "2026-02-01"),
            review("bob", on="head", at="2026-03-01"),
        ]
        self.assert_adds(pr(reviews=reviews))

    def test_a_deleted_reviewer_does_not_stop_the_run(self):
        ghost = review("x", on="head", threads=0, at="2026-02-01")
        ghost["author"] = None
        status_, out, calls = sweep([labeled(reviews=[review("bob"), ghost])])
        self.assertEqual((status_, calls), (0, [REMOVE]))
        self.assertIn("reviewed by ghost", out)

    def test_every_page_is_read(self):
        status_, out, calls = sweep([[pr(1)], [pr(2)]])
        self.assertEqual(status_, 0)
        self.assertEqual(len(calls), 2)
        self.assertIn("#2: add", out)

    def test_a_review_that_only_replies_withholds_the_label(self):
        late = review("bob", on="head", threads=0, replies=1, at="2026-02-01")
        self.assert_leaves(pr(reviews=[review("bob"), late]))
        status_, out, calls = sweep([labeled(reviews=[review("bob"), late])])
        self.assertEqual(calls, [REMOVE])
        self.assertIn("reviewed by bob since the last push", out)

    def test_a_review_that_replies_and_opens_a_thread_is_answered(self):
        late = review("bob", on="head", threads=1, replies=2, at="2026-02-01")
        self.assert_adds(pr(reviews=[review("bob"), late]))

    def test_replies_past_the_comments_read_leave_the_label_alone(self):
        late = review("bob", on="head", threads=0, replies=20, at="2026-02-01")
        late["comments"]["totalCount"] = 21
        self.assert_leaves(pr(reviews=[review("bob"), late]))
        self.assert_leaves(labeled(reviews=[review("bob"), late]))

    def test_a_cut_off_review_does_not_hide_another_reviewer_waiting(self):
        cut = review("bob", on="head", threads=0, replies=20, at="2026-02-01")
        cut["comments"]["totalCount"] = 21
        waiting = review("carol", "APPROVED", "head", 0, "2026-03-01")
        status_, out, calls = sweep([labeled(reviews=[review("bob"), cut, waiting])])
        self.assertEqual((status_, calls), (0, [REMOVE]))
        self.assertIn("reviewed by carol since the last push", out)
        # Nor a failing check.
        red = [run("gates (rust)", "FAILURE")]
        self.assertEqual(
            sweep([labeled(reviews=[review("bob"), cut], checks=red)])[2], [REMOVE]
        )

    def test_a_failed_label_lookup_keeps_ghs_reason(self):
        err = []
        self.assertEqual(sweep([pr()], label_exists=False, stderr=err)[::2], (2, []))
        self.assertIn("HTTP 403 rate limited", err[0])
        self.assertIn(f'cannot read label "{LABEL}"', err[0])

    def test_a_cut_off_count_does_not_hide_a_definite_not(self):
        open_thread = {"threads": [thread("bob"), thread("bob", False)]}
        red = {"checks": [run("gates (rust)", "FAILURE")]}
        late = {
            "reviews": [
                review("bob"),
                review("bob", "APPROVED", "head", 0, "2026-02-01"),
            ]
        }
        for over in (
            {"mergeable": "UNKNOWN"},
            {"thread_count": 101},
            {"review_count": 101},
            {"check_count": 51},
        ):
            for definite in (open_thread, red, late):
                with self.subTest(over=over, definite=definite):
                    self.assertEqual(
                        sweep([labeled(**over, **definite)])[::2], (0, [REMOVE])
                    )

    def test_cut_off_checks_with_no_gates_lane_seen_are_unknown(self):
        self.assert_leaves(labeled(checks=[run("check")], check_count=51))

    def test_a_cut_that_cannot_hide_the_answer_does_not_withhold_it(self):
        none_seen = {"threads": [], "reviews": []}
        no_gates = {"checks": [run("check")]}
        for case in (
            # Neither a mergeability nor a checks cut can hide a review.
            {"mergeable": "UNKNOWN", **none_seen},
            {"check_count": 51, **none_seen},
            # Nor can a threads or reviews cut hide a gates lane.
            {"review_count": 101, "checks": []},
            {"thread_count": 101, **no_gates},
            {"mergeable": "UNKNOWN", **no_gates},
        ):
            with self.subTest(case=case):
                self.assertEqual(sweep([labeled(**case)])[::2], (0, [REMOVE]))

    def test_a_cut_that_can_hide_the_answer_withholds_it(self):
        for case in (
            {"review_count": 101, "threads": [], "reviews": []},
            {"thread_count": 101, "threads": [], "reviews": []},
            {"check_count": 51, **{"checks": [run("check")]}},
        ):
            with self.subTest(case=case):
                self.assert_leaves(labeled(**case))

    def test_resolved_bot_threads_alone_are_not_a_review(self):
        bot = {"threads": [thread("cubic", bot=True)], "reviews": []}
        self.assert_leaves(pr(**bot))
        status_, out, calls = sweep([labeled(**bot)])
        self.assertEqual((status_, calls), (0, [REMOVE]))
        self.assertIn("not reviewed", out)

    def test_a_persons_thread_beside_a_bots_is_a_review(self):
        threads = [thread("cubic", bot=True), thread("bob")]
        self.assert_adds(pr(threads=threads, reviews=[]))

    def test_more_than_100_reviews_does_not_withhold_a_ready_answer(self):
        self.assert_adds(pr(review_count=101))
        # Unless no person's review was read: an older one may be the only one.
        self.assert_leaves(pr(review_count=101, threads=[], reviews=[]))

    def test_threads_past_the_first_100_are_read(self):
        first = [thread("bob")] * 100
        self.assertEqual(
            sweep([pr(threads=first, thread_count=101)], later_threads=[thread("bob")])[
                ::2
            ],
            (0, [ADD]),
        )
        late_open = [thread("bob", False)]
        self.assertEqual(
            sweep([labeled(threads=first, thread_count=101)], later_threads=late_open)[
                ::2
            ],
            (0, [REMOVE]),
        )

    def test_threads_that_changed_between_the_reads_leave_the_label_alone(self):
        first = [thread("bob")] * 100
        for later in ([thread("bob")] * 2, []):
            with self.subTest(later=len(later)):
                self.assert_leaves(
                    pr(threads=first, thread_count=101), later_threads=later
                )

    def test_a_run_held_for_approval_withholds_the_label(self):
        def held(**over):
            run_ = {
                "name": "check",
                "head_sha": "head",
                "head_branch": "feature",
                "head_repo": "alice/r",
            }
            return {**run_, **over}

        status_, out, calls = sweep([labeled()], held=[held()])
        self.assertEqual((status_, calls), (0, [REMOVE]))
        self.assertIn("held: check", out)
        # A run held on another pull request, or on an older head, is not this one's.
        for other in (
            held(head_sha="old"),
            held(head_branch="other"),
            held(head_repo="bob/r"),
        ):
            with self.subTest(other=other):
                self.assertEqual(sweep([pr()], held=[other])[::2], (0, [ADD]))

    def test_a_failed_read_of_the_held_runs_writes_nothing(self):
        err = []
        self.assertEqual(
            sweep([pr(), labeled(number=2, isDraft=True)], runs="fail", stderr=err)[
                ::2
            ],
            (2, []),
        )
        self.assertIn("HTTP 403 actions", err[0])

    def test_a_failed_write_or_head_lookup_fails_the_run_after_the_rest(self):
        add_2 = ADD.replace("issues/1/", "issues/2/")
        for name, prs, over, why in (
            ("add", [pr(1), pr(2)], {"write_fail": 1}, "HTTP 403 write"),
            (
                "remove",
                [labeled(isDraft=True), pr(2)],
                {"write_fail": 1},
                "HTTP 403 write",
            ),
            ("head", [pr(1), pr(2)], {"head_fail": 1}, "HTTP 404 head"),
        ):
            with self.subTest(failing=name):
                err = []
                self.assertEqual(sweep(prs, stderr=err, **over)[::2], (2, [add_2]))
                self.assertIn(why, err[0])

    def test_a_failed_read_is_tried_again(self):
        tries = []
        self.assertEqual(sweep([pr()], pages="flaky", tries=tries)[::2], (0, [ADD]))
        self.assertEqual(tries, [3])
        tries = []
        self.assertEqual(sweep([pr()], pages="fail", tries=tries)[::2], (2, []))
        self.assertEqual(tries, [3])

    def test_a_push_after_the_read_stops_the_add(self):
        status_, out, calls = sweep([pr()], head="newer")
        self.assertEqual((status_, calls), (0, []))
        self.assertIn("head moved", out)
        # A removal goes ahead: the next run settles it either way.
        self.assertEqual(
            sweep([labeled(isDraft=True)], head="newer")[::2], (0, [REMOVE])
        )


if __name__ == "__main__":
    unittest.main()

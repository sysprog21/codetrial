#!/usr/bin/env python3
"""Contract tests for the scripts the agent skills under .claude/skills run.

The sweeps read GitHub through `gh`, so each case serves them one GraphQL page
from a fake `gh` and asserts on the lists they print. The reference checker runs
in a scratch repository whose Makefile, workflow and skills are written here, so
a case names exactly what resolves and what does not.
"""

import fnmatch
import hashlib
import importlib.util
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path


SPEC = importlib.util.spec_from_file_location(
    "support", Path(__file__).with_name("support.py")
)
support = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(support)

ROOT = support.ROOT
SKILLS = ROOT / ".claude" / "skills"
ISSUE_SWEEP = SKILLS / "codetrial-triage" / "issue-sweep.sh"
PR_SWEEP = SKILLS / "codetrial-triage" / "pr-sweep.sh"
SKILL_REFS = ROOT / "scripts" / "check-skill-refs.py"
REVIEW_PICK = SKILLS / "codetrial-triage" / "review-pick.sh"
REVIEW_FINISH = SKILLS / "codetrial-triage" / "review-finish.sh"
APPLY_ACTIONS = SKILLS / "codetrial-triage" / "apply-pr-actions.sh"
POST_SIMILAR = SKILLS / "codetrial-triage" / "post-similar.sh"
CI_CACHED = SKILLS / "codetrial-verify" / "ci-cached.sh"
EXPERIENCE_CLOSE = SKILLS / "codetrial-review" / "experience-close.sh"
FEEDBACK_BUNDLE = SKILLS / "codetrial-contribute" / "feedback-bundle.sh"
REVIEW_BRIEF = SKILLS / "codetrial-triage" / "review-brief.sh"

# A gh that fails every call, so only a script's own check can refuse first.
FAKE_GH_FAILS = "#!/bin/sh\nexit 1\n"

# The viewer and the one account with push access. The sweeps ask `gh` for
# both before the GraphQL page; the runs listing is empty, so no PR is held.
FAKE_GH = support.stub("skill-scripts/gh.sh")

# Enough prose and a pasted --version line that no lint fires unless a case
# asks for it.
FILLER = (
    "\n\ncodetrial 0.1.0 (abc1234), and enough words that the body is not "
    "taken for an empty one."
)


def ago(days):
    moment = datetime.now(timezone.utc) - timedelta(days=days)
    return moment.strftime("%Y-%m-%dT%H:%M:%SZ")


def comment(login, days, body="Please post the `--version` line."):
    return {
        "author": {"__typename": "User", "login": login},
        "authorAssociation": "MEMBER" if login == "maint" else "NONE",
        "createdAt": ago(days),
        "body": body,
    }


def issue(number, body, **fields):
    node = {
        "number": number,
        "title": f"Interview stalls in case {number}",
        "body": body,
        "createdAt": ago(30),
        "authorAssociation": "NONE",
        "author": {"login": "reporter"},
        "issueType": {"name": "Bug"},
        "userContentEdits": {"totalCount": 0, "nodes": []},
        "assignees": {"totalCount": 0, "nodes": []},
        "assigned": {"totalCount": 0, "nodes": []},
        "comments": {"totalCount": 0, "nodes": []},
        "timelineItems": {"totalCount": 0, "nodes": []},
    }
    return node | fields


def listed(*nodes):
    return {"totalCount": len(nodes), "nodes": list(nodes)}


def checked(*runs):
    """A head pushed ten days ago whose check runs ended as (name, conclusion).

    A conclusion of None is a run still in progress.
    """
    return {
        "nodes": [
            {
                "commit": {
                    "committedDate": ago(10),
                    "checkSuites": {"nodes": [{"createdAt": ago(10)}]},
                    "statusCheckRollup": {
                        "contexts": {
                            "nodes": [
                                {
                                    "__typename": "CheckRun",
                                    "databaseId": index,
                                    "name": name,
                                    "status": "IN_PROGRESS"
                                    if conclusion is None
                                    else "COMPLETED",
                                    "conclusion": conclusion,
                                }
                                for index, (name, conclusion) in enumerate(runs)
                            ]
                        }
                    },
                }
            }
        ]
    }


def pull(number, **fields):
    node = {
        "number": number,
        "title": f"Fix case {number}",
        "isDraft": False,
        "authorAssociation": "CONTRIBUTOR",
        "headRefOid": f"head{number}",
        "mergeable": "MERGEABLE",
        "baseRefName": "main",
        "updatedAt": ago(1),
        "headRefName": "topic",
        "headRepository": {"nameWithOwner": "contributor/codetrial"},
        "author": {"login": "contributor"},
        "closingIssuesReferences": listed(),
        "files": listed(),
        # The head was pushed ten days ago, which dates every request.
        "commits": checked(),
        "reviews": listed(),
        "comments": listed(),
        "reviewThreads": listed(),
        "timelineItems": {"nodes": []},
    }
    return node | fields


def review(login, state, days, body="", oid="head1"):
    return {
        "id": f"R_{login}",
        "author": {"__typename": "User", "login": login},
        "authorAssociation": "MEMBER",
        "state": state,
        "submittedAt": ago(days),
        "isMinimized": False,
        "body": body,
        "commit": {"oid": oid},
        "comments": {"totalCount": 0},
    }


def request(node_id, days, body):
    return {
        "id": node_id,
        "author": {"__typename": "User", "login": "maint"},
        "authorAssociation": "MEMBER",
        "createdAt": ago(days),
        "isMinimized": False,
        "body": body,
    }


FIXED = (
    (pathlib.Path(__file__).parent.parent / ".claude/skills/codetrial-triage/rebase.md")
    .read_text(encoding="utf-8")
    .strip()
)


GREEN = checked(("check", "SUCCESS"), ("gates (rust)", "SUCCESS"))


def thread(kind="User", login="maint"):
    return {
        "id": f"T_{login}",
        "isResolved": False,
        "isOutdated": False,
        "path": "web/runners.js",
        "comments": {"nodes": [{"author": {"__typename": kind, "login": login}}]},
    }


def run_fake(script, args, fake_gh, files, cwd=None, env=()):
    """Run a skill script against a fake gh, with files it or the fake reads."""
    with tempfile.TemporaryDirectory() as scratch:
        scratch = Path(scratch)
        (scratch / "bin").mkdir()
        support.write_executable(scratch / "bin" / "gh", fake_gh)
        paths = {}
        for variable, content in files.items():
            paths[variable] = scratch / variable.lower()
            paths[variable].write_text(content)
        # An argument "$NAME" is the path of the file given as NAME.
        args = [str(paths[a[1:]]) if str(a).startswith("$") else str(a) for a in args]
        return subprocess.run(
            ["sh", str(script), *args],
            cwd=cwd,
            capture_output=True,
            text=True,
            env=os.environ
            | {"PATH": f"{scratch / 'bin'}:{os.environ['PATH']}"}
            | {k: str(v) for k, v in paths.items()}
            | dict(env),
        )


@unittest.skipUnless(shutil.which("jq"), "jq is not installed")
class SweepCase(unittest.TestCase):
    def sweep(self, script, page, previous=None, runs=(), again=(), env=(), diffs=None):
        files = {
            "FAKE_PAGE": json.dumps({"data": page}),
            # gh applies --jq itself, so the fake prints one object per run.
            "FAKE_RUNS": "".join(json.dumps(run) + "\n" for run in runs),
            "FAKE_AGAIN": json.dumps(list(again)),
        }
        args = ["sysprog21/codetrial"]
        # ci-failure.sh as the PR sweep runs it: a PR in $STALE is a stale base.
        files["PR_SWEEP_CI"] = (
            'case " $STALE " in *" $1 "*) s=true ;; *) s=false ;; esac\n'
            'printf \'{"pr":%s,"head":"%s","stale_base":%s}\\n\' '
            '"$1" "${STALE_HEAD:-head$1}" "$s"\n'
        )
        if diffs is not None:
            files["FAKE_DIFFS"] = json.dumps(diffs)
        if previous is not None:
            files["PREVIOUS"] = json.dumps(previous)
            args += ["7", "$PREVIOUS"]
        result = run_fake(script, args, FAKE_GH, files, env=env)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)


class IssueSweepTests(SweepCase):
    def run_issues(self, *issues):
        page = {
            "repository": {
                "issues": {"pageInfo": {"hasNextPage": False}, "nodes": list(issues)}
            }
        }
        return self.sweep(ISSUE_SWEEP, page)

    def gaps(self, body):
        lint = self.run_issues(issue(1, body))["lint"]
        return lint[0]["gaps"] if lint else []

    def test_heading_followed_by_a_higher_level_one_is_an_empty_section(self):
        self.assertIn(
            "empty_section", self.gaps("## Steps\n# Environment\nmacOS" + FILLER)
        )

    def test_heading_followed_by_its_own_subsection_is_not_empty(self):
        self.assertEqual(self.gaps("## Steps\n### First\nOpen the lobby." + FILLER), [])

    def test_comments_in_a_fenced_shell_session_are_not_headings(self):
        body = "## Steps\n```sh\n# start it\n# then\n```\nOpen the lobby."
        self.assertEqual(self.gaps(body + FILLER), [])

    def test_a_shorter_fence_inside_a_longer_one_does_not_close_it(self):
        body = "## Steps\n````md\n```\n# not a heading\n# nor this\n````\nOpen it."
        self.assertEqual(self.gaps(body + FILLER), [])

    def test_heading_on_the_last_line_is_an_empty_section(self):
        self.assertIn(
            "empty_section", self.gaps("## Steps\nOpen it." + FILLER + "\n## Logs")
        )

    def test_uppercase_hash_counts_as_a_revision(self):
        body = "Seen on build DEADBEEF1 after the reconnect, with enough prose here."
        self.assertNotIn("no_revision", self.gaps(body))

    def test_bug_without_a_revision_is_flagged(self):
        body = "The interview stalls after reconnecting, every time, on two machines."
        self.assertIn("no_revision", self.gaps(body))

    def test_unanswered_question_is_waiting(self):
        swept = self.run_issues(
            issue(1, "Body" + FILLER, comments=listed(comment("maint", 10)))
        )
        self.assertEqual([entry["issue"] for entry in swept["waiting"]], [1])

    def test_reporter_edit_hidden_by_a_later_maintainer_edit_still_answers(self):
        edits = listed(
            {"editedAt": ago(2), "editor": {"login": "maint"}},
            {"editedAt": ago(5), "editor": {"login": "reporter"}},
        )
        swept = self.run_issues(
            issue(
                1,
                "Body" + FILLER,
                comments=listed(comment("maint", 10)),
                userContentEdits=edits,
            )
        )
        self.assertEqual(swept["waiting"], [])

    def test_idle_days_is_unknown_when_no_act_of_the_assignee_was_read(self):
        swept = self.run_issues(
            issue(
                1,
                "Body" + FILLER,
                assignees=listed({"login": "student"}),
                comments=listed(comment("maint", 10, "Assigned to you.")),
            )
        )
        [claim] = swept["claimed"]
        self.assertIsNone(claim["idle_days"])
        self.assertIsNone(claim["checked_at"])
        self.assertIsNone(claim["stale"])

    def test_an_open_pr_that_closes_the_issue_is_named_on_its_entry(self):
        here = {"nameWithOwner": "sysprog21/codetrial"}
        fork = {"nameWithOwner": "someone/codetrial"}
        closing = {
            "nodes": [
                {"number": 7, "repository": here},
                {"number": 9, "repository": fork},
            ]
        }
        swept = self.run_issues(
            issue(1, "Body" + FILLER, closedByPullRequestsReferences=closing),
            issue(2, "Body" + FILLER, comments=listed(comment("maint", 10))),
            issue(
                3,
                "Body" + FILLER,
                comments=listed(comment("maint", 10)),
                closedByPullRequestsReferences=closing,
            ),
        )
        self.assertEqual(
            {e["issue"]: e["closing_prs"] for e in swept["unanswered"]}, {1: [7]}
        )
        self.assertEqual(
            {e["issue"]: e["closing_prs"] for e in swept["waiting"]}, {2: [], 3: [7]}
        )

    def test_more_closing_prs_than_read_marks_the_issue_truncated(self):
        closing = {"totalCount": 11, "nodes": []}
        swept = self.run_issues(
            issue(1, "Body" + FILLER, closedByPullRequestsReferences=closing)
        )
        self.assertEqual(swept["truncated"], [1])

    def test_more_body_edits_than_read_marks_the_issue_truncated(self):
        edits = {"totalCount": 21, "nodes": []}
        swept = self.run_issues(issue(1, "Body" + FILLER, userContentEdits=edits))
        self.assertEqual(swept["truncated"], [1])


class PrSweepTests(SweepCase):
    def run_pulls(self, *pulls, **options):
        page = {
            "repository": {
                "defaultBranchRef": {"name": "main"},
                "pullRequests": {
                    "pageInfo": {"hasNextPage": False},
                    "nodes": list(pulls),
                },
            }
        }
        return self.sweep(PR_SWEEP, page, **options)

    def grades(self, *pulls):
        return {
            entry["pr"]: entry["grade"] for entry in self.run_pulls(*pulls)["status"]
        }

    def test_status_ranks_by_size_and_splits_out_tests(self):
        files = listed(
            {"path": "web/runners.js", "additions": 2, "deletions": 1},
            {"path": "tests/browser/runners.test.js", "additions": 40, "deletions": 0},
        )
        swept = self.run_pulls(
            pull(1, additions=90, deletions=10, commits=GREEN),
            pull(2, additions=42, deletions=1, files=files, commits=GREEN),
        )
        self.assertEqual([entry["pr"] for entry in swept["status"]], [2, 1])
        self.assertEqual(
            swept["status"][0]["size"], {"total": 43, "source": 3, "tests": 40}
        )

    def test_grade_follows_what_github_records(self):
        cases = {
            "high": pull(1, commits=GREEN),
            "low": pull(2, commits=checked(("gates (rust)", "FAILURE"))),
            "blocked": pull(
                3,
                commits=GREEN,
                mergeable="CONFLICTING",
                reviewThreads=listed(thread()),
            ),
            "medium": pull(4, commits=checked(("check", "SUCCESS"))),
            "medium-high": pull(5, commits=GREEN, reviewDecision="CHANGES_REQUESTED"),
        }
        grades = self.grades(*cases.values())
        for grade, node in cases.items():
            with self.subTest(grade=grade):
                self.assertEqual(grades[node["number"]], grade)

    def test_waits_on_names_whose_move_it_is_and_review_queues_the_maintainers(self):
        cases = {
            1: (pull(1, commits=GREEN), "maintainer"),
            2: (pull(2, commits=checked(("gates (rust)", "FAILURE"))), "author"),
            3: (pull(3, commits=checked(("gates (rust)", None))), "ci"),
            4: (
                pull(
                    4,
                    commits=checked(
                        ("gates (rust)", "SUCCESS"), ("check", "ACTION_REQUIRED")
                    ),
                ),
                "approval",
            ),
            # The author pushed past the changes request: a re-review is owed.
            5: (
                pull(5, commits=GREEN, reviewDecision="CHANGES_REQUESTED"),
                "maintainer",
            ),
            6: (pull(6, commits=GREEN, reviewThreads=listed(thread())), "author"),
            7: (pull(7, commits=GREEN, isDraft=True), "author"),
            8: (pull(8, commits=GREEN, mergeable="CONFLICTING"), "author"),
            9: (pull(9, commits=checked(("check", "SUCCESS"))), "author"),
            12: (pull(12, commits=GREEN, author={"login": "viewer"}), "maintainer"),
            10: (pull(10, commits=GREEN, mergeable="UNKNOWN"), "ci"),
            # A held run outranks the conflict: nothing runs until approved.
            11: (
                pull(
                    11,
                    mergeable="CONFLICTING",
                    commits=checked(("check", "ACTION_REQUIRED")),
                ),
                "approval",
            ),
        }
        swept = self.run_pulls(
            *(node for node, _ in cases.values()),
            env={"PR_SWEEP_RETRY_DELAY": "0"},
        )
        self.assertEqual(
            {entry["pr"]: entry["waits_on"] for entry in swept["status"]},
            {number: who for number, (_, who) in cases.items()},
        )
        # The viewer's own PR waits on nobody else's review.
        self.assertEqual(sorted(swept["review"]), [1, 5])

    def test_a_stale_base_is_asked_to_rebase_under_the_conflicting_limits(self):
        failing = checked(("gates (portable)", "FAILURE"), ("check", "FAILURE"))
        swept = self.run_pulls(
            pull(1, commits=failing),
            # ci-failure.sh says its failure is its own.
            pull(2, commits=failing),
            pull(
                3, commits=failing, comments=listed(request("C3", 1, "Rebase, please."))
            ),
            pull(4, commits=failing, baseRefName="feature"),
            pull(5, commits=failing, isDraft=True),
            pull(6, commits=failing, author={"login": "viewer"}),
            pull(7, commits=failing, comments={"totalCount": 101, "nodes": []}),
            # Already listed for its conflict, so asked once.
            pull(8, commits=failing, mergeable="CONFLICTING"),
            # Green: nothing to rebase for.
            pull(9, commits=GREEN),
            env={"STALE": "1 3 4 5 6 7 8 9"},
        )
        self.assertEqual(
            sorted((entry["pr"], entry["reason"]) for entry in swept["rebase"]),
            [(1, "stale_base"), (8, "conflicts")],
        )

    def test_a_job_main_also_fails_stays_the_authors_and_says_so(self):
        # The lane runs several gates, so a shared name is not a shared cause.
        swept = self.run_pulls(
            pull(1, commits=checked(("gates (rust)", "FAILURE"), ("check", "FAILURE"))),
            env={"FAKE_MAIN_FAILED": json.dumps(["gates (rust)", "check"])},
        )
        [entry] = swept["status"]
        self.assertEqual(entry["waits_on"], "author")
        self.assertIn(
            "failing: check, gates (rust); main also fails check, gates (rust)",
            entry["why"],
        )
        self.assertEqual(swept["review"], [])

    def test_a_failed_diagnosis_never_asks_for_a_rebase(self):
        failing = checked(("gates (portable)", "FAILURE"))
        cases = {
            # Says stale, then exits non-zero: not trusted.
            "exit": 'echo \'{"pr":1,"head":"head1","stale_base":true}\'; exit 1\n',
            # Answers for another PR.
            "other pr": 'echo \'{"pr":2,"head":"head1","stale_base":true}\'\n',
            # Diagnosed a head pushed after the sweep read this one.
            "other head": 'echo \'{"pr":1,"head":"later","stale_base":true}\'\n',
        }
        for name, stub in cases.items():
            with self.subTest(name):
                page = {
                    "repository": {
                        "defaultBranchRef": {"name": "main"},
                        "pullRequests": {
                            "pageInfo": {"hasNextPage": False},
                            "nodes": [pull(1, commits=failing)],
                        },
                    }
                }
                files = {
                    "FAKE_PAGE": json.dumps({"data": page}),
                    "FAKE_RUNS": "",
                    "FAKE_AGAIN": "[]",
                    "PR_SWEEP_CI": stub,
                }
                result = run_fake(PR_SWEEP, ["sysprog21/codetrial"], FAKE_GH, files)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout)["rebase"], [])

    def test_moved_names_a_change_of_whose_move_it_is(self):
        node = pull(1, commits=GREEN)
        first = self.run_pulls(node)
        earlier = first | {
            "status": [entry | {"waits_on": "author"} for entry in first["status"]]
        }
        swept = self.run_pulls(node, previous=earlier)
        self.assertEqual(
            swept["moved"],
            [{"pr": 1, "changed": {"waits_on": ["author", "maintainer"]}}],
        )

    def test_refs_only_lists_issues_the_merge_leaves_open(self):
        asker = request("C1", 1, "Make the last line `Closes #7`.")
        outsider = comment("student", 1, "Closes #8 would be better.") | {
            "id": "C2",
            "isMinimized": False,
        }
        swept = self.run_pulls(
            pull(1, commits=GREEN, body="Shows the camera.\n\nRefs #141"),
            # It closes #5, so its Refs #6 stands (CONTRIBUTING.md).
            pull(
                2,
                commits=GREEN,
                body="Refs #6",
                closingIssuesReferences=listed({"number": 5}),
            ),
            pull(3, commits=GREEN, body="Refs: #7", comments=listed(asker)),
            pull(4, commits=GREEN, body="Refs #8", comments=listed(outsider)),
            # Asked about #70, which is not #7.
            pull(
                5,
                commits=GREEN,
                body="Refs #7",
                comments=listed(request("C3", 1, "Closes #70")),
            ),
            pull(6, commits=GREEN, body=None),
            pull(
                7,
                commits=GREEN,
                body="Refs sysprog21/codetrial#9\n"
                "Refs https://github.com/sysprog21/codetrial/issues/10",
                comments=listed(
                    request(
                        "C4",
                        1,
                        "Fixes https://github.com/sysprog21/codetrial/issues/10",
                    )
                ),
            ),
            pull(8, commits=GREEN, body="Refs #11, #12 and #13"),
            # The form the log prefers counts as a request as much as Closes.
            pull(
                9,
                commits=GREEN,
                body="Refs #14",
                comments=listed(request("C5", 1, "Make the last line `Close #14`.")),
            ),
        )
        self.assertEqual(
            swept["refs_only"],
            [
                {"pr": 1, "issue": 141, "asked": False},
                {"pr": 3, "issue": 7, "asked": True},
                {"pr": 4, "issue": 8, "asked": False},
                {"pr": 5, "issue": 7, "asked": False},
                {"pr": 7, "issue": 9, "asked": False},
                {"pr": 7, "issue": 10, "asked": True},
                {"pr": 8, "issue": 11, "asked": False},
                {"pr": 8, "issue": 12, "asked": False},
                {"pr": 8, "issue": 13, "asked": False},
                {"pr": 9, "issue": 14, "asked": True},
            ],
        )

    def test_human_thread_lowers_the_grade_and_a_bot_thread_only_to_medium(self):
        grades = self.grades(
            pull(1, commits=GREEN, reviewThreads=listed(thread())),
            pull(2, commits=GREEN, reviewThreads=listed(thread("Bot", "cubic-dev-ai"))),
        )
        self.assertEqual(grades, {1: "low", 2: "medium"})

    def test_a_review_whose_threads_are_all_settled_is_hidden_as_resolved(self):
        def mine(rid, state="COMMENTED", kind="User", body="", count=1):
            return review("maint", state, 1, body) | {
                "id": rid,
                "author": {"__typename": kind, "login": "maint"},
                "comments": {"totalCount": count},
            }

        def thread_of(*rids, resolved=False, outdated=False, total=None):
            nodes = [
                {"author": {"login": "maint"}, "pullRequestReview": {"id": r}}
                for r in rids
            ]
            return {
                "id": "T_" + "_".join(rids),
                "isResolved": resolved,
                "isOutdated": outdated,
                "path": "a.js",
                "comments": {"totalCount": total or len(nodes), "nodes": nodes},
            }

        reviews = [
            mine("R_bot", kind="Bot", body="summary"),
            mine("R_out"),
            mine("R_text", body="please look"),
            mine("R_open"),
            mine("R_cr", state="CHANGES_REQUESTED"),
            mine("R_split", count=2),
            mine("R_none", count=0),
        ]
        threads = [
            thread_of("R_bot", resolved=True),
            thread_of("R_out", outdated=True),
            thread_of("R_text", resolved=True),
            thread_of("R_open"),
            thread_of("R_cr", resolved=True),
            thread_of("R_split", resolved=True),
            thread_of("R_split"),
        ]
        swept = self.run_pulls(
            pull(1, reviews=listed(*reviews), reviewThreads=listed(*threads)),
            pull(
                2,
                reviews=listed(mine("R_long")),
                reviewThreads=listed(thread_of("R_long", resolved=True, total=21)),
            ),
        )
        self.assertEqual(
            [(h["pr"], h["id"]) for h in swept["hide_resolved"]],
            [(1, "R_bot"), (1, "R_out")],
        )

    def test_a_refine_comment_is_stale_once_no_thread_is_open(self):
        text = (SKILLS / "codetrial-triage" / "refine.md").read_text()
        mine = comment("viewer", 1, text) | {"id": "IC_mine"}
        theirs = comment("maint", 1, text) | {"id": "IC_theirs"}
        other = comment("viewer", 1, "Thanks!") | {"id": "IC_other"}
        comments = listed(mine, theirs, other)
        settled = listed(thread() | {"isResolved": True})
        swept = self.run_pulls(
            pull(1, comments=comments, reviewThreads=settled),
            pull(2, comments=comments, reviewThreads=listed(thread())),
        )
        self.assertEqual(swept["refine_stale"], [{"pr": 1, "id": "IC_mine"}])

    def test_a_thread_the_author_opened_is_not_a_reviewer_thread(self):
        grades = self.grades(
            pull(1, commits=GREEN, reviewThreads=listed(thread(login="contributor")))
        )
        self.assertEqual(grades, {1: "high"})

    def test_a_failure_rerun_green_is_not_failing(self):
        rerun = checked(
            ("gates (rust)", "FAILURE"),
            ("gates (rust)", "SUCCESS"),
            ("check", "SUCCESS"),
        )
        self.assertEqual(self.grades(pull(1, commits=rerun)), {1: "high"})

    def test_a_failing_status_is_not_hidden_by_a_check_run_of_its_name(self):
        commits = checked(("lint", "SUCCESS"), ("gates (rust)", "SUCCESS"))
        contexts = commits["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]
        contexts["nodes"].append(
            {"__typename": "StatusContext", "context": "lint", "state": "ERROR"}
        )
        swept = self.run_pulls(pull(1, commits=commits))
        entry = swept["status"][0]
        self.assertEqual(
            (entry["checks"]["failing"], entry["grade"]), (["lint"], "low")
        )

    def test_a_pr_past_100_reviews_is_not_graded_as_fully_read(self):
        swept = self.run_pulls(
            pull(1, commits=GREEN, reviews={"totalCount": 101, "nodes": []})
        )
        entry = swept["status"][0]
        self.assertEqual(entry["grade"], "medium")
        self.assertIn("more than 100 reviews or threads, not all read", entry["why"])

    def test_a_held_run_is_not_a_failure(self):
        grades = self.grades(
            pull(
                1,
                commits=checked(
                    ("gates (rust)", "SUCCESS"), ("check", "ACTION_REQUIRED")
                ),
            )
        )
        self.assertEqual(grades, {1: "medium"})

    def test_problem_files_named_with_test_inside_a_word_are_source(self):
        files = listed(
            {
                "path": "web/problems/tightest-channel-spacing.json",
                "additions": 30,
                "deletions": 0,
            },
            {"path": "tests/test_gen_problems.py", "additions": 5, "deletions": 0},
            {"path": "web/lib.test.js", "additions": 4, "deletions": 0},
        )
        swept = self.run_pulls(pull(1, additions=39, deletions=0, files=files))
        self.assertEqual(
            swept["status"][0]["size"], {"total": 39, "source": 30, "tests": 9}
        )

    def test_one_approval_does_not_hide_another_maintainers_request(self):
        reviews = listed(
            review("maint", "CHANGES_REQUESTED", 3, "Fix the null case."),
            review("viewer", "APPROVED", 1),
        )
        grades = self.grades(
            pull(1, commits=GREEN, reviews=reviews, reviewDecision="CHANGES_REQUESTED")
        )
        self.assertEqual(grades, {1: "blocked"})

    def test_only_a_request_that_opens_with_rebase_is_a_rebase(self):
        def asked(number, body):
            request = review("maint", "CHANGES_REQUESTED", 1, body, f"head{number}")
            return pull(
                number,
                commits=GREEN,
                reviewDecision="CHANGES_REQUESTED",
                reviews=listed(request),
            )

        grades = self.grades(
            asked(1, FIXED), asked(2, "Fix the null dereference; no rebase is needed.")
        )
        self.assertEqual(grades, {1: "medium", 2: "blocked"})

    def test_adding_a_failing_check_never_raises_a_blocked_grade(self):
        grades = self.grades(
            pull(
                1,
                commits=checked(("gates (rust)", "FAILURE")),
                mergeable="CONFLICTING",
                reviewThreads=listed(thread()),
            )
        )
        self.assertEqual(grades, {1: "blocked"})

    def test_a_run_waiting_for_approval_with_no_check_run_is_held(self):
        run = {
            "id": 1,
            "name": "check",
            "head_sha": "head1",
            "head_branch": "topic",
            "head_repo": "contributor/codetrial",
            "actor": "contributor",
        }
        swept = self.run_pulls(pull(1, commits=GREEN), runs=[run])
        entry = swept["status"][0]
        self.assertEqual(
            (entry["checks"]["held"], entry["grade"]), (["check"], "medium")
        )

    def test_previous_sweep_reports_only_what_moved_and_what_left(self):
        first = self.run_pulls(
            pull(1, commits=GREEN), pull(2, commits=GREEN), pull(9, commits=GREEN)
        )
        swept = self.run_pulls(
            pull(1, commits=GREEN),
            pull(2, commits=GREEN, mergeable="CONFLICTING"),
            previous=first,
        )
        self.assertEqual(
            swept["moved"],
            [
                {
                    "pr": 2,
                    "changed": {
                        "mergeable": ["MERGEABLE", "CONFLICTING"],
                        "grade": ["high", "medium"],
                        "waits_on": ["maintainer", "author"],
                    },
                }
            ],
        )
        self.assertEqual(swept["gone"], [{"pr": 9, "state": "MERGED", "merge": "m9"}])

    def test_a_failed_lookup_marks_one_departed_pr_and_keeps_the_sweep(self):
        first = self.run_pulls(pull(1), pull(9), pull(404))
        swept = self.run_pulls(pull(1), previous=first)
        self.assertEqual(
            swept["gone"],
            [
                {"pr": 9, "state": "MERGED", "merge": "m9"},
                {"pr": 404, "state": "UNKNOWN"},
            ],
        )

    def test_a_check_finishing_moves_a_pr_even_when_its_grade_holds(self):
        waiting = checked(("gates (rust)", "SUCCESS"), ("check", None))
        decided = {"reviewDecision": "CHANGES_REQUESTED"}
        first = self.run_pulls(pull(1, commits=waiting, **decided))
        swept = self.run_pulls(pull(1, commits=GREEN, **decided), previous=first)
        self.assertEqual([entry["pr"] for entry in swept["moved"]], [1])
        # The grade holds; the move passes from CI to a re-review.
        self.assertEqual(list(swept["moved"][0]["changed"]), ["waits_on", "checks"])
        self.assertEqual(swept["moved"][0]["changed"]["waits_on"], ["ci", "maintainer"])

    def test_unknown_mergeability_is_asked_again_for_those_prs_only(self):
        swept = self.run_pulls(
            pull(1, commits=GREEN, mergeable="UNKNOWN"),
            pull(2, commits=GREEN),
            again=[{"number": 1, "mergeable": "CONFLICTING"}],
            env={"PR_SWEEP_RETRY_DELAY": "0"},
        )
        self.assertEqual(swept["unknown"], [])
        self.assertEqual(
            {entry["pr"]: entry["mergeable"] for entry in swept["status"]},
            {1: "CONFLICTING", 2: "MERGEABLE"},
        )

    def test_runs_awaiting_approval_on_old_heads_are_only_counted(self):
        def run(sha):
            return {
                "id": sha,
                "name": "check",
                "head_sha": sha,
                "head_branch": "topic",
                "head_repo": "contributor/codetrial",
                "actor": "contributor",
            }

        swept = self.run_pulls(pull(1, commits=GREEN), runs=[run("head1"), run("old")])
        self.assertEqual(
            ([entry["run"] for entry in swept["approve"]], swept["superseded"]),
            (["head1"], 1),
        )

    def test_failed_gates_line_matches_what_the_test_gate_prints(self):
        # ci-failure.sh reads the gate names from the last line test.sh prints
        # on failure; a change to that line has to break this test too.
        self.assertIn(
            'echo "failed gates:$failed"', (ROOT / "scripts" / "test.sh").read_text()
        )
        self.assertIn(
            'startswith("failed gates:")', (VERIFY / "ci-failure.sh").read_text()
        )

    def test_workflow_facts_the_review_checklist_states_still_hold(self):
        # codetrial-review's workflow rules say check.yml runs on pull_request
        # with contents: read, and that within check.yml only release has
        # contents: write, running on a push to main.
        workflow = (ROOT / ".github" / "workflows" / "check.yml").read_text()
        self.assertNotIn("pull_request_target", workflow)
        self.assertNotIn("workflow_run", workflow)
        self.assertNotIn("issue_comment", workflow)
        self.assertIn("\npermissions:\n  contents: read\n", workflow)
        self.assertEqual(workflow.count("contents: write"), 1)
        release = workflow[workflow.index("\n  release:\n") :]
        release = release[: release.index("steps:")]
        self.assertIn("contents: write", release)
        self.assertIn(
            "if: github.event_name == 'push' && github.ref == 'refs/heads/main'",
            release,
        )

    def test_desktop_workflow_publishes_from_main_or_a_tag_only(self):
        # desktop.yml also runs on pull_request, and a job's permissions
        # apply to a pull request from a branch of this repository, so the two
        # jobs granted contents: write each carry the guard that keeps a pull
        # request from publishing.
        workflow = (ROOT / ".github" / "workflows" / "desktop.yml").read_text()
        self.assertNotIn("pull_request_target", workflow)
        self.assertNotIn("workflow_run", workflow)
        self.assertIn("\npermissions:\n  contents: read\n", workflow)
        self.assertEqual(workflow.count("contents: write"), 2)
        for job, guard in (
            (
                "rolling",
                "if: github.event_name == 'push' && github.ref == 'refs/heads/main'",
            ),
            ("release", "if: startsWith(github.ref, 'refs/tags/desktop-v')"),
        ):
            with self.subTest(job=job):
                body = workflow[workflow.index(f"\n  {job}:\n") :]
                body = body[: body.index("steps:")]
                self.assertIn("contents: write", body)
                self.assertIn(guard, body)

    def test_review_memory_publisher_keeps_privileges_off_collection_and_prs(self):
        workflow = (ROOT / ".github/workflows/review-memory.yml").read_text()
        for event in ("pull_request", "workflow_run", "issue_comment"):
            self.assertNotIn(event, workflow)
        self.assertIn("  schedule:\n", workflow)
        self.assertIn("  workflow_dispatch:\n", workflow)
        self.assertIn("permissions:\n  contents: read", workflow)
        self.assertIn(
            "github.repository == 'sysprog21/codetrial' && github.ref == 'refs/heads/main'",
            workflow,
        )
        self.assertIn("cancel-in-progress: false", workflow)
        self.assertEqual(workflow.count("contents: write"), 2)
        prepare = workflow.split("\n  prepare:\n", 1)[1].split(
            "\n  review-memory-sign-checkpoint:", 1
        )[0]
        self.assertNotIn("id-token: write", prepare)
        self.assertNotIn("attestations: write", prepare)
        signer = workflow.split("\n  review-memory-sign-checkpoint:\n", 1)[1].split(
            "\n  publish:", 1
        )[0]
        self.assertNotIn("contents: write", signer)
        self.assertIn("id-token: write", signer)
        self.assertIn("publish-memory.py verify", signer)
        self.assertNotIn("publish-memory.py prepare", signer)
        self.assertNotIn("actions/cache", workflow)
        lines = workflow.splitlines()
        for number, line in enumerate(lines):
            if line.lstrip().startswith(("run:", "- run:")):
                indent = len(line) - len(line.lstrip())
                block = [line]
                for following in lines[number + 1 :]:
                    if following.strip() and (
                        len(following) - len(following.lstrip()) <= indent
                    ):
                        break
                    block.append(following)
                self.assertNotIn("${{", "\n".join(block), line)
        product = (ROOT / ".github/workflows/check.yml").read_text()
        self.assertIn(
            'gh api --paginate "repos/$GH_REPO/releases?per_page=100"', product
        )

    def test_hook_installer_prints_the_labels_conventions_explains(self):
        # codetrial-conventions tells an agent to read make hooks' output by
        # these labels, so renaming one has to move that text too.
        installer = (ROOT / "scripts" / "install-git-hooks.sh").read_text()
        for label in ("OK", "HOOK", "FOREIGN", "SHARED"):
            with self.subTest(label=label):
                self.assertIn(f"  {label} ", installer)

    def test_a_skipped_gates_lane_is_not_a_pass_and_a_skipped_shard_is_nothing(self):
        skipped = checked(("check", "SUCCESS"), ("gates (rust)", "SKIPPED"))
        shard = checked(
            ("check", "SUCCESS"),
            ("gates (rust)", "SUCCESS"),
            ("mutants (0)", "SKIPPED"),
        )
        swept = self.run_pulls(pull(1, commits=skipped), pull(2, commits=shard))
        status = {entry["pr"]: entry for entry in swept["status"]}
        self.assertEqual(status[1]["grade"], "medium")
        self.assertEqual(status[1]["checks"]["skipped"], ["gates (rust)"])
        self.assertEqual(
            status[1]["why"], ["no gates lane passed (skipped: gates (rust))"]
        )
        self.assertEqual((status[2]["grade"], status[2]["why"]), ("high", []))

    def test_an_approval_left_before_the_last_push_waits_on_a_re_review(self):
        old = review("maint", "APPROVED", 2, oid="older")
        here = review("maint", "APPROVED", 2, oid="head2")
        swept = self.run_pulls(
            pull(1, commits=GREEN, reviewDecision="APPROVED", reviews=listed(old)),
            pull(2, commits=GREEN, reviewDecision="APPROVED", reviews=listed(here)),
        )
        status = {entry["pr"]: entry for entry in swept["status"]}
        self.assertEqual(status[1]["grade"], "medium-high")
        self.assertEqual(status[1]["why"], ["approved before the last push"])
        self.assertEqual(status[1]["waits_on"], "maintainer")
        self.assertEqual((status[2]["grade"], status[2]["why"]), ("high", []))

    def test_waits_on_all_keeps_the_author_behind_a_pending_check(self):
        asked = review("maint", "CHANGES_REQUESTED", 1, "Handle the empty list.")
        node = pull(
            1,
            commits=checked(("gates (rust)", "SUCCESS"), ("check", None)),
            reviewDecision="CHANGES_REQUESTED",
            reviews=listed(asked),
        )
        entry = self.run_pulls(node)["status"][0]
        self.assertEqual(entry["waits_on"], "ci")
        self.assertEqual(entry["waits_on_all"], ["ci", "author"])
        # A gates lane still running is the CI's move, not the author's.
        running = self.run_pulls(
            pull(1, commits=checked(("check", "SUCCESS"), ("gates (rust)", None)))
        )["status"][0]
        self.assertEqual(running["waits_on_all"], ["ci"])
        self.assertEqual(running["grade"], "medium-high")
        self.assertEqual(
            self.run_pulls(pull(1, commits=GREEN))["status"][0]["waits_on_all"],
            ["maintainer"],
        )

    def test_the_viewer_and_a_maintainers_own_pr_are_named(self):
        swept = self.run_pulls(
            pull(1, author={"login": "maint"}), pull(2, author={"login": "contributor"})
        )
        self.assertEqual(swept["viewer"], "viewer")
        mine = {entry["pr"]: entry["author_is_maintainer"] for entry in swept["status"]}
        self.assertEqual(mine, {1: True, 2: False})

    def test_prerequisites_are_a_stacked_branch_or_a_stated_dependency(self):
        base = pull(
            1,
            headRefName="feature-a",
            headRepository={"nameWithOwner": "sysprog21/codetrial"},
        )
        stacked = pull(2, baseRefName="feature-a")
        stated = pull(3, body="Depends on #1 for the queue bound.")
        mention = pull(4, body="Unlike #1, this keeps the order, broken after #1.")
        swept = self.run_pulls(base, stacked, stated, mention)
        self.assertEqual(
            swept["prerequisites"],
            [
                {"pr": 2, "on": 1, "how": "stacked"},
                {"pr": 3, "on": 1, "how": "stated"},
            ],
        )

    def test_collide_pairs_prs_that_change_the_same_base_lines(self):
        def diff(old, new, start):
            return (
                "diff --git a/web/a.js b/web/a.js\n--- a/web/a.js\n+++ b/web/a.js\n"
                f"@@ -{start},3 +{start},3 @@ function walk(node) {{\n"
                f" keep\n-{old}\n+{new}\n keep\n"
            )

        shared = listed({"path": "web/a.js", "additions": 1, "deletions": 1})
        swept = self.run_pulls(
            pull(1, files=shared),
            pull(2, files=shared),
            pull(3, files=shared),
            pull(4, files=shared),
            diffs={
                "1": diff("a", "b", 10),
                "2": diff("a", "c", 10),
                "3": diff("x", "y", 40),
            },
        )
        self.assertEqual(
            swept["collide"],
            [
                {
                    "a": 1,
                    "b": 2,
                    "path": "web/a.js",
                    "lines": [[11, 11], [11, 11]],
                    "context": ["function walk(node) {"],
                }
            ],
        )
        self.assertEqual(swept["collide_unread"], [4])

    def test_a_binary_file_both_prs_change_collides_whole(self):
        # A name holding " b/" still splits right, since both halves match.
        binary = (
            "diff --git a/web/a b/logo.png b/web/a b/logo.png\n"
            "index 1..2 100644\nBinary files differ\n"
        )
        shared = listed({"path": "web/a b/logo.png", "additions": 0, "deletions": 0})
        swept = self.run_pulls(
            pull(1, files=shared),
            pull(2, files=shared),
            diffs={"1": binary, "2": binary},
        )
        self.assertEqual(
            [(c["a"], c["b"], c["path"]) for c in swept["collide"]],
            [(1, 2, "web/a b/logo.png")],
        )

    def test_a_rename_collides_under_its_new_name(self):
        # The diff --git line alone would split this name at its last " b/".
        rename = (
            "diff --git a/web/a b/old.js b/web/a b/new.js\nsimilarity index 100%\n"
            "rename from web/a b/old.js\nrename to web/a b/new.js\n"
        )
        shared = listed({"path": "web/a b/new.js", "additions": 0, "deletions": 0})
        swept = self.run_pulls(
            pull(1, files=shared),
            pull(2, files=shared),
            diffs={"1": rename, "2": rename},
        )
        self.assertEqual(
            [(c["a"], c["b"], c["path"]) for c in swept["collide"]],
            [(1, 2, "web/a b/new.js")],
        )

    def test_gates_prefix_matches_the_check_workflow_job_name(self):
        # The grade reads "no gates lane passed" from this prefix; renaming the
        # job in check.yml without the script would mark every PR as such.
        workflow = (ROOT / ".github" / "workflows" / "check.yml").read_text()
        self.assertIn("name: gates (", workflow)
        self.assertIn('startswith("gates (")', PR_SWEEP.read_text())

    def test_a_previous_file_that_is_not_one_sweep_is_refused(self):
        # An earlier sweep that failed while redirected leaves the file empty.
        for previous in (
            "",
            " \n\t",
            "null",
            "{}",
            "[]",
            "broken",
            "{}\n{}",
            '{"status": 1}',
            '{"status": {}}',
        ):
            with self.subTest(previous=previous):
                result = run_fake(
                    PR_SWEEP,
                    ["sysprog21/codetrial", "7", "$PREVIOUS"],
                    FAKE_GH_FAILS,
                    {"PREVIOUS": previous},
                )
                self.assertEqual((result.returncode, result.stdout), (2, ""))
                self.assertIn("is not an earlier pr-sweep.sh output", result.stderr)

    def test_without_a_previous_sweep_moved_and_gone_are_null(self):
        swept = self.run_pulls(pull(1))
        self.assertIsNone(swept["moved"])
        self.assertIsNone(swept["gone"])

    def test_pr_with_no_commits_does_not_end_the_sweep(self):
        swept = self.run_pulls(pull(1, commits={"nodes": []}), pull(2))
        self.assertEqual([entry["number"] for entry in swept["prs"]], [1, 2])

    def test_conflicting_pr_without_a_request_is_asked_to_rebase(self):
        swept = self.run_pulls(pull(1, mergeable="CONFLICTING"))
        self.assertEqual(
            swept["rebase"],
            [{"pr": 1, "author": "contributor", "reason": "conflicts"}],
        )

    def test_a_later_rebase_review_does_not_withdraw_an_earlier_request(self):
        # The sweep posts the rebase request as the maintainer account, so it
        # lands after that account's own substantive request on the same head.
        work = review("maint", "CHANGES_REQUESTED", 5, "Handle the empty list.")
        rebase = review("maint", "CHANGES_REQUESTED", 1, FIXED) | {"id": "R_2"}
        node = pull(1, mergeable="CONFLICTING", reviews=listed(work, rebase))
        self.assertEqual(self.grades(node), {1: "blocked"})
        # An approval after both answers them.
        ok = review("maint", "APPROVED", 0) | {"id": "R_3"}
        node = pull(1, reviews=listed(work, rebase, ok))
        self.assertNotEqual(self.grades(node)[1], "blocked")

    def test_current_fixed_review_is_not_repeated_or_hidden(self):
        reviews = listed(review("maint", "CHANGES_REQUESTED", 5, FIXED))
        swept = self.run_pulls(pull(1, mergeable="CONFLICTING", reviews=reviews))
        self.assertEqual(swept["rebase"], [])
        self.assertEqual(swept["hide"], [])

    def test_current_fixed_comment_still_gets_the_review_and_is_hidden(self):
        swept = self.run_pulls(
            pull(1, mergeable="CONFLICTING", comments=listed(request("IC_1", 5, FIXED)))
        )
        self.assertEqual([entry["pr"] for entry in swept["rebase"]], [1])
        self.assertEqual([entry["id"] for entry in swept["hide"]], ["IC_1"])

    def test_a_request_the_author_pushed_past_is_outdated_once_the_branch_merges(self):
        onto = "Rebase onto latest `main` branch and resolve conflicts."
        old = review("maint", "CHANGES_REQUESTED", 20, FIXED, oid="old") | {
            "id": "R_old"
        }
        now = review("maint", "CHANGES_REQUESTED", 2, FIXED, oid="head2") | {
            "id": "R_now"
        }
        swept = self.run_pulls(
            # Pushed since: nothing is current, so every copy is outdated.
            pull(1, reviews=listed(old), comments=listed(request("IC_1", 15, onto))),
            # A request on the current head still stands.
            pull(2, reviews=listed(now)),
            # A conflict nobody can be asked about yet keeps its newest copy.
            pull(3, mergeable="CONFLICTING", isDraft=True, reviews=listed(old)),
        )
        hidden = {(h["pr"], h["id"]): h["why"] for h in swept["hide"]}
        self.assertEqual(hidden, {(1, "R_old"): "outdated", (1, "IC_1"): "outdated"})

    def test_a_dismissed_request_in_a_known_wording_is_hidden_as_outdated(self):
        dismissed = review("maint", "DISMISSED", 3, FIXED) | {"id": "R_dismissed"}
        inline = dismissed | {"id": "R_inline", "comments": {"totalCount": 1}}
        other = review("maint", "DISMISSED", 3, "Please also fix the docs") | {
            "id": "R_other"
        }
        swept = self.run_pulls(
            pull(1, reviews=listed(dismissed, inline, other)),
        )
        self.assertEqual(
            [(h["id"], h["kind"], h["why"]) for h in swept["hide"]],
            [("R_dismissed", "review", "outdated")],
        )

    def test_a_repeated_request_is_replaced_by_the_newest_and_says_it_is_repeated(self):
        copies = listed(request("IC_a", 8, FIXED), request("IC_b", 6, FIXED))
        swept = self.run_pulls(pull(1, mergeable="CONFLICTING", comments=copies))
        self.assertEqual(
            sorted((h["id"], h["kind"], h["why"], h["act"]) for h in swept["hide"]),
            [
                ("IC_a", "comment", "repeated", "hide"),
                ("IC_b", "comment", "repeated", "hide"),
            ],
        )
        # The viewer own repeated comment is the one a sweep may delete.
        mine = request("IC_mine", 8, FIXED) | {
            "author": {"__typename": "User", "login": "viewer"}
        }
        swept = self.run_pulls(
            pull(
                1,
                mergeable="CONFLICTING",
                comments=listed(mine, request("IC_b", 6, FIXED)),
            )
        )
        self.assertEqual(
            sorted((h["id"], h["act"]) for h in swept["hide"]),
            [("IC_b", "hide"), ("IC_mine", "delete")],
        )
        # On a merging branch the newest current copy stays and only older go.
        swept = self.run_pulls(
            pull(
                1,
                comments=listed(request("IC_a", 8, FIXED), request("IC_b", 6, FIXED)),
                commits=checked(),
            )
        )
        self.assertEqual(
            [(h["id"], h["why"]) for h in swept["hide"]],
            [("IC_a", "repeated")],
        )

    def test_earlier_copies_of_the_ci_wording_and_the_old_text_are_hidden(self):
        old = "Rebase latest `main` branch and resolve conflicts."
        ci = "Before asking someone to review, you MUST resolve any regressions flagged by GitHub Actions."
        reviews = listed(
            review("maint", "CHANGES_REQUESTED", 20, ci, oid="old") | {"id": "R_ci"}
        )
        comments = listed(request("IC_old", 10, old), request("IC_new", 1, FIXED))
        swept = self.run_pulls(pull(1, reviews=reviews, comments=comments))
        self.assertEqual(
            sorted(entry["id"] for entry in swept["hide"]), ["IC_old", "R_ci"]
        )

    def test_a_later_comment_never_hides_the_review_that_holds_the_request(self):
        reviews = listed(review("maint", "CHANGES_REQUESTED", 5, FIXED))
        comments = listed(request("IC_1", 1, FIXED))
        swept = self.run_pulls(
            pull(1, mergeable="CONFLICTING", reviews=reviews, comments=comments)
        )
        self.assertEqual(swept["rebase"], [])
        self.assertEqual([entry["id"] for entry in swept["hide"]], ["IC_1"])

    def test_current_hand_written_request_supersedes_an_older_fixed_one(self):
        comments = listed(
            request("IC_old", 20, FIXED),
            request("IC_new", 5, "Rebase latest `main` to pick up the zip fix."),
        )
        swept = self.run_pulls(pull(1, comments=comments))
        self.assertEqual([entry["id"] for entry in swept["hide"]], ["IC_old"])
        self.assertEqual(swept["hide_ask"], [])
        self.assertEqual(swept["rebase"], [])
        # Conflicting, it still gets the review, and the hand-written comment is
        # offered for hiding with its body rather than hidden.
        swept = self.run_pulls(pull(1, mergeable="CONFLICTING", comments=comments))
        self.assertEqual([entry["pr"] for entry in swept["rebase"]], [1])
        self.assertEqual([entry["id"] for entry in swept["hide"]], ["IC_old"])
        self.assertEqual([entry["id"] for entry in swept["hide_ask"]], ["IC_new"])


RELATED = SKILLS / "codetrial-contribute" / "related.sh"

# The viewer, one issue held by someone else, and the open PRs; gh applies
# --jq itself, so the issue view answers in the shape related.sh asks for.
FAKE_GH_RELATED = support.stub("skill-scripts/gh-related.sh")


def open_pr(number, author, branch, paths, closes=(), head=""):
    return {
        "number": number,
        "title": f"PR {number}",
        "author": {"login": author},
        "headRefName": branch,
        "headRefOid": head,
        "files": [{"path": path} for path in paths],
        "closingIssuesReferences": [{"number": n} for n in closes],
    }


@unittest.skipUnless(shutil.which("jq") and shutil.which("git"), "jq or git missing")
class RelatedTests(unittest.TestCase):
    def test_branch_meets_the_same_issue_shared_files_and_a_held_issue(self):
        with tempfile.TemporaryDirectory() as scratch:
            scratch = Path(scratch)
            repo = scratch / "repo"
            git = ["git", "-C", str(repo), "-c", "user.name=t", "-c", "user.email=t@t"]
            subprocess.run(["git", "init", "-q", "-b", "main", str(repo)], check=True)
            (repo / "web").mkdir()
            (repo / "web" / "runners.js").write_text("a\n")
            subprocess.run([*git, "add", "."], check=True)
            subprocess.run([*git, "commit", "-qm", "base"], check=True)
            subprocess.run([*git, "switch", "-qc", "topic"], check=True)
            (repo / "web" / "runners.js").write_text("b\n")
            # A qualified close counts, another repository's does not, and a
            # reference to an issue the branch also closes is a close.
            message = (
                "Fix it\n\nCloses #5\nFixes sysprog21/codetrial#12\n"
                "Fixes other/repo#9\nRefs #5\nRefs #8"
            )
            subprocess.run([*git, "commit", "-qam", message], check=True)
            head = subprocess.run(
                [*git, "rev-parse", "HEAD"], capture_output=True, text=True, check=True
            ).stdout.strip()
            prs = [
                open_pr(7, "other", "x", ["web/runners.js"], closes=[5]),
                open_pr(8, "other", "y", ["web/styles.css"]),
                # Pushed under another name, found by its head commit.
                open_pr(9, "viewer", "pushed-as", ["web/runners.js"], head=head),
            ]
            result = run_fake(
                RELATED,
                [],
                FAKE_GH_RELATED,
                {"FAKE_PRS": json.dumps(prs)},
                cwd=repo,
                env={"RELATED_BASE": "main"},
            )
        self.assertEqual(result.returncode, 0, result.stderr)
        found = json.loads(result.stdout)
        self.assertEqual(found["files"], ["web/runners.js"])
        self.assertEqual((found["closes"], found["refs"]), ([5, 12], [8]))
        self.assertEqual(
            {entry["number"]: entry["others_hold"] for entry in found["issues"]},
            {5: True, 8: False, 12: False},
        )
        self.assertEqual(found["own_pr"], 9)
        self.assertEqual([entry["pr"] for entry in found["same_issue"]], [7])
        self.assertEqual(
            found["same_files"],
            [
                {
                    "pr": 7,
                    "title": "PR 7",
                    "author": "other",
                    "shared": ["web/runners.js"],
                }
            ],
        )


VERIFY = SKILLS / "codetrial-verify"
REVIEW = SKILLS / "codetrial-review"

# One PR at head "h1": a gates lane whose log names two gates, the aggregate job
# main also fails, and a check from an app outside GitHub Actions.
FAKE_GH_CI = support.stub("skill-scripts/gh-ci.sh")

# A step's first log line carries a byte order mark before its timestamp, in
# the message column, as the real log of run 39712346073 does.
BOM = "\ufeff"
CI_LOG = "".join(
    f"gates (portable)\trun\t{BOM if n == 0 else ''}2026-10-22T00:00:0{n}.0Z {line}\n"
    for n, line in enumerate(
        [
            "Ran 24 tests",
            "error: 3 vulnerabilities found!",
            "failed gates: cargo-audit indent",
            "##[error]Process completed with exit code 1.",
        ]
    )
)

# A modified file whose hunk deletes a line reading "-- x", and a deleted file.
DIFF = """\
diff --git a/web/a.js b/web/a.js
index 1..2 100644
--- a/web/a.js
+++ b/web/a.js
@@ -10,3 +10,3 @@ function f() {
 keep
--- x
+new
 tail
diff --git a/web/a b.txt b/web/a b.txt
index 1..2 100644
--- a/web/a b.txt\t
+++ b/web/a b.txt\t
@@ -1,1 +1,1 @@
-old
+new
diff --git a/web/old.js b/web/old.js
deleted file mode 100644
--- a/web/old.js
+++ /dev/null
@@ -1,1 +0,0 @@
-gone
"""


@unittest.skipUnless(shutil.which("jq"), "jq is not installed")
class VerifyScriptTests(unittest.TestCase):
    def test_ci_failure_names_the_gates_and_what_main_shares(self):
        checks = [
            {
                "name": "gates (portable)",
                "state": "FAILURE",
                "bucket": "fail",
                "link": "https://github.com/sysprog21/codetrial/actions/runs/7/job/70",
            },
            {
                "name": "check",
                "state": "FAILURE",
                "bucket": "fail",
                "link": "https://github.com/sysprog21/codetrial/actions/runs/7/job/71",
            },
            # Cancelled at its time limit: no failed step, so only the whole
            # job log shows why (#300).
            {
                "name": "gates (browser)",
                "state": "CANCELLED",
                "bucket": "cancel",
                "link": "https://github.com/sysprog21/codetrial/actions/runs/7/job/73",
            },
            # gh files a startup failure as pending; it never ran.
            {
                "name": "gates (mutants)",
                "state": "STARTUP_FAILURE",
                "bucket": "pending",
                "link": "https://github.com/sysprog21/codetrial/actions/runs/7/job/74",
            },
            # Its log can no longer be read.
            {
                "name": "gates (wasm-old)",
                "state": "FAILURE",
                "bucket": "fail",
                "link": "https://github.com/sysprog21/codetrial/actions/runs/7/job/75",
            },
            {
                "name": "cubic",
                "state": "FAILURE",
                "bucket": "fail",
                "link": "https://www.cubic.dev/pr/5",
            },
            {
                "name": "gates (rust)",
                "state": "PENDING",
                "bucket": "pending",
                "link": "https://github.com/sysprog21/codetrial/actions/runs/7/job/72",
            },
            # gh files a held check under fail; it has no log to read.
            {
                "name": "gates (wasm)",
                "state": "ACTION_REQUIRED",
                "bucket": "fail",
                "link": "https://github.com/sysprog21/codetrial/actions/runs/8/job/80",
            },
        ]
        result = run_fake(
            VERIFY / "ci-failure.sh",
            [5],
            FAKE_GH_CI,
            {"FAKE_CHECKS": json.dumps(checks), "FAKE_LOG": CI_LOG},
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        found = json.loads(result.stdout)
        self.assertEqual(found["pending"], ["gates (rust)"])
        self.assertEqual(found["held"], ["gates (wasm)"])
        lane, collector, cancelled, startup, unread, app = found["failing"]
        # Log order, not alphabetical, which would put "Process" first.
        self.assertEqual(
            lane["errors"],
            ["error: 3 vulnerabilities found!", "Process completed with exit code 1."],
        )
        self.assertEqual(
            (unread["gates"], unread["errors"], unread["excerpt"]), (None, None, None)
        )
        self.assertTrue(lane["excerpt"].startswith("Ran 24 tests"), lane["excerpt"])
        self.assertEqual(cancelled["errors"], ["SyntaxError: Unexpected token"])
        self.assertEqual((startup["name"], startup["errors"]), ("gates (mutants)", []))
        self.assertEqual(
            (lane["gates"], lane["current"], lane["on_main"]),
            (["cargo-audit", "indent"], True, False),
        )
        self.assertIn("error: 3 vulnerabilities found!", lane["errors"])
        self.assertEqual((collector["name"], collector["on_main"]), ("check", True))
        self.assertEqual(app, {"name": "cubic", "link": "https://www.cubic.dev/pr/5"})
        # indent failed beside cargo-audit, so a rebase would not clear it.
        self.assertIs(found["stale_base"], False)

    def test_ci_failure_reports_the_advisory_from_an_audit_annotation(self):
        checks = [
            {
                "name": "audit",
                "state": "FAILURE",
                "bucket": "fail",
                "link": "https://github.com/sysprog21/codetrial/actions/runs/7/job/70",
            }
        ]
        message = "RUSTSEC-2026-0385 ctutils 0.4.3"
        result = run_fake(
            VERIFY / "ci-failure.sh",
            [5],
            FAKE_GH_CI,
            {
                "FAKE_CHECKS": json.dumps(checks),
                "FAKE_LOG": f"audit\tscan\t2026-10-22T00:00:00.0Z ##[error]{message}\n",
            },
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        found = json.loads(result.stdout)
        self.assertEqual(found["failing"][0]["errors"], [message])
        self.assertFalse(found["stale_base"])

    def test_an_audit_failure_main_has_fixed_is_a_stale_base(self):
        checks = [
            {
                "name": name,
                "state": "FAILURE",
                "bucket": "fail",
                "link": f"https://github.com/sysprog21/codetrial/actions/runs/7/job/{job}",
            }
            for name, job in (("gates (portable)", 70), ("check", 71))
        ]

        def pr_with(*paths, changed=None):
            return json.dumps(
                {
                    "headRefOid": "h1",
                    "files": [{"path": path} for path in paths],
                    "changedFiles": len(paths) if changed is None else changed,
                }
            )

        cubic = {
            "name": "cubic",
            "state": "FAILURE",
            "bucket": "fail",
            "link": "https://www.cubic.dev/pr/5",
        }
        # main skipped the lane: absent from its failures, yet not a pass.
        skipped = json.dumps([{"name": "gates (portable)", "conclusion": "skipped"}])
        log = CI_LOG.replace("cargo-audit indent", "cargo-audit")
        cases = {
            "stale": (checks, {}, True),
            "touches Cargo": (checks, {"FAKE_PR": pr_with("Cargo.lock")}, False),
            "changes the gate": (
                checks,
                {"FAKE_PR": pr_with("scripts/test.sh")},
                False,
            ),
            "changes CI": (
                checks,
                {"FAKE_PR": pr_with(".github/workflows/check.yml")},
                False,
            ),
            # gh read 100 of 101 files, so the lockfile may be the one cut.
            "more files than read": (
                checks,
                {
                    "FAKE_PR": pr_with(
                        *(f"web/f{n}.js" for n in range(100)), changed=101
                    )
                },
                False,
            ),
            "main skipped the lane": (checks, {"FAKE_MAIN": skipped}, False),
            "a check outside Actions fails": (checks + [cubic], {}, False),
            # main last ran before the PR did, so a new advisory fails both.
            "main ran first": (
                checks,
                {"FAKE_MAIN_AT": "2026-10-21T00:00:00Z"},
                False,
            ),
            "a lane is still pending": (
                checks
                + [
                    {
                        "name": "gates (rust)",
                        "state": "IN_PROGRESS",
                        "bucket": "pending",
                        "link": "",
                    }
                ],
                {},
                False,
            ),
        }
        for name, (listed_checks, env, stale) in cases.items():
            with self.subTest(name):
                result = run_fake(
                    VERIFY / "ci-failure.sh",
                    [5],
                    FAKE_GH_CI,
                    {"FAKE_CHECKS": json.dumps(listed_checks), "FAKE_LOG": log},
                    env=env,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIs(json.loads(result.stdout)["stale_base"], stale)

    def test_a_checks_lookup_that_fails_is_not_read_as_a_clean_pr(self):
        # gh can fail without JSON; whitespace must not count as a response.
        for checks in ("", " \n\t", "null", "{}", "broken", "[]\n[]"):
            with self.subTest(checks=checks):
                result = run_fake(
                    VERIFY / "ci-failure.sh",
                    [5],
                    FAKE_GH_CI,
                    {"FAKE_CHECKS": checks, "FAKE_LOG": ""},
                )
                self.assertEqual(result.returncode, 1, result.stdout)
                self.assertIn("could not read the checks of #5", result.stderr)

    def test_a_successful_empty_checks_lookup_is_a_clean_pr(self):
        result = run_fake(
            VERIFY / "ci-failure.sh",
            [5],
            FAKE_GH_CI,
            {"FAKE_CHECKS": "[]", "FAKE_LOG": ""},
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["failing"], [])

    def test_a_pr_that_is_not_a_number_is_refused_before_any_call(self):
        for script, args in (
            (VERIFY / "ci-failure.sh", ["abc"]),
            (REVIEW / "review-anchors.sh", ["abc", "payload.json"]),
            (
                REVIEW / "post-review.sh",
                ["abc", "payload.json", "2026-10-22T00:00:00Z"],
            ),
        ):
            with self.subTest(script=script.name):
                # The fake gh fails every call, so only the script's own
                # check can produce exit 2 and its message.
                result = run_fake(script, args, FAKE_GH_FAILS, {})
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertIn("is not a pull request number", result.stderr)

    def test_review_anchors_keep_diff_lines_inline_and_move_the_rest(self):
        def note(path, line, side, text):
            return {"path": path, "line": line, "side": side, "body": text}

        payload = {
            "event": "COMMENT",
            "commit_id": "h1",
            "body": "Notes.",
            "comments": [
                note("web/a.js", 11, "RIGHT", "added line"),
                note("web/a.js", 11, "LEFT", "the deleted -- x line"),
                note("web/a.js", 12, "RIGHT", "context on the right"),
                # Left line 12 exists only if the "-- x" line was counted.
                note("web/a.js", 12, "LEFT", "context on the left"),
                note("web/old.js", 1, "LEFT", "deleted file"),
                # git appends a tab to a header path containing a space.
                note("web/a b.txt", 1, "RIGHT", "a name with a space"),
                note("web/a.js", 40, "RIGHT", "outside every hunk"),
                note("web/a.js", 11, "RIGHT", "starts outside") | {"start_line": 2},
            ],
        }
        result = run_fake(
            REVIEW / "review-anchors.sh",
            [5, "$FAKE_PAYLOAD"],
            FAKE_GH_CI,
            {"FAKE_DIFF": DIFF, "FAKE_PAYLOAD": json.dumps(payload)},
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        checked = json.loads(result.stdout)
        self.assertEqual(
            [c["body"] for c in checked["comments"]],
            [
                "added line",
                "the deleted -- x line",
                "context on the right",
                "context on the left",
                "deleted file",
                "a name with a space",
            ],
        )
        self.assertEqual(
            checked["body"],
            "Notes.\n\nweb/a.js:40: outside every hunk\n\nweb/a.js:11: starts outside",
        )

    def test_review_anchors_place_by_quote_and_never_by_a_guessed_line(self):
        # Two hunks of one file that each add the line "dup".
        diff = DIFF + (
            "diff --git a/web/b.js b/web/b.js\n--- a/web/b.js\n+++ b/web/b.js\n"
            "@@ -1,2 +1,3 @@\n one\n+dup\n two\n"
            "@@ -20,2 +21,3 @@\n three\n+dup\n four\n"
            "diff --git a/web/c.js b/web/c.js\n--- a/web/c.js\n+++ b/web/c.js\n"
            "@@ -1,3 +1,3 @@\n foo\n \n-old\n+baz\n"
        )

        def note(text, **fields):
            return {"path": fields.pop("path", "web/a.js"), "body": text} | fields

        payload = {
            "event": "COMMENT",
            "commit_id": "h1",
            "body": "Notes.",
            "comments": [
                # Blank lines and indentation do not count; two lines span.
                note("span", quote="  keep\n\n  new  "),
                note("blank span", path="web/c.js", quote="foo\n\n baz"),
                # A miscounted line inside the diff is corrected by the quote;
                # a line outside every hunk is about unchanged code, so the
                # quote cannot carry it into the diff.
                note("quote wins", quote="tail", line=10),
                # A start_line given with a quote that lands on one line is
                # not kept: the span the quote found is the span.
                note("stale start", quote="tail", line=10, start_line=3),
                note("unchanged code", quote="tail", line=40),
                # A deleted or context line on the left is matched on the left.
                note("left side", quote="tail", line=11, side="LEFT"),
                # The same text in two hunks: the line picks the second match,
                # and a line in neither cannot pick.
                note("picked", path="web/b.js", quote="dup", line=22),
                note("unpicked", path="web/b.js", quote="dup", line=5),
                # A quote found nowhere does not fall back to its line, which
                # sits in a hunk and would post on the wrong code.
                note("misquoted", quote="tail;", line=11),
                # A run across two hunks is not one place.
                note("across hunks", path="web/b.js", quote="two\nthree"),
                note("nowhere", quote="absent"),
            ],
        }
        result = run_fake(
            REVIEW / "review-anchors.sh",
            [5, "$FAKE_PAYLOAD"],
            FAKE_GH_CI,
            {"FAKE_DIFF": diff, "FAKE_PAYLOAD": json.dumps(payload)},
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        checked = json.loads(result.stdout)
        placed = {
            c["body"]: (c.get("start_line"), c["line"], c.get("side", "RIGHT"))
            for c in checked["comments"]
        }
        self.assertEqual(
            placed,
            {
                "span": (10, 11, "RIGHT"),
                "blank span": (1, 3, "RIGHT"),
                "quote wins": (None, 12, "RIGHT"),
                "stale start": (None, 12, "RIGHT"),
                "left side": (None, 12, "LEFT"),
                "picked": (None, 22, "RIGHT"),
            },
        )
        self.assertFalse(any("quote" in c for c in checked["comments"]))
        # The posting scripts act on moved; the body is how the user reads it.
        self.assertEqual(
            [c["body"] for c in checked["moved"]],
            ["unchanged code", "unpicked", "misquoted", "across hunks", "nowhere"],
        )
        self.assertTrue(all(c["line"] is None for c in checked["moved"]))
        self.assertEqual(
            checked["body"].split("\n\n"),
            [
                "Notes.",
                "web/a.js: unchanged code",
                "web/b.js: unpicked",
                "web/a.js: misquoted",
                "web/b.js: across hunks",
                "web/a.js: nowhere",
            ],
        )


# The PR as post-review.sh rereads it before posting, and the post itself.
FAKE_GH_POST = support.stub("skill-scripts/gh-post.sh")


@unittest.skipUnless(shutil.which("jq"), "jq is not installed")
class PostReviewTests(unittest.TestCase):
    def post(self, line=11, env=(), **fields):
        payload = {
            "event": "COMMENT",
            "commit_id": "h1",
            "body": "Notes.",
            "comments": [
                {"path": "web/a.js", "line": line, "side": "RIGHT", "body": "x"}
            ],
        } | fields
        return run_fake(
            REVIEW / "post-review.sh",
            [5, "$FAKE_PAYLOAD", "T1"],
            FAKE_GH_POST,
            {"FAKE_DIFF": DIFF, "FAKE_PAYLOAD": json.dumps(payload)},
            env=env,
        )

    def test_posts_when_the_pr_is_where_the_user_read_it(self):
        result = self.post()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("pullrequestreview-h1", result.stdout)

    def test_refuses_a_payload_without_a_commit_event_or_body(self):
        # Without an event GitHub would keep a pending review only its writer
        # sees; without a body it refuses the review.
        cases = {
            "commit_id": (None, "names no commit_id"),
            "event": (None, "names no event"),
            "body": ("", "has no body"),
        }
        for field, (value, said) in cases.items():
            with self.subTest(field):
                result = self.post(**{field: value})
                self.assertEqual((result.returncode, result.stdout), (2, ""))
                self.assertIn(said, result.stderr)

    def test_refuses_a_payload_that_is_not_one_object(self):
        # The fake gh fails every call, so only the payload check can produce
        # exit 2 and its message.
        for payload in ("", " \n\t", "null", "[]", "broken", "{}\n{}"):
            with self.subTest(payload=payload):
                result = run_fake(
                    REVIEW / "post-review.sh",
                    [5, "$FAKE_PAYLOAD", "T1"],
                    FAKE_GH_FAILS,
                    {"FAKE_PAYLOAD": payload},
                )
                self.assertEqual((result.returncode, result.stdout), (2, ""))
                self.assertIn("is not one JSON object", result.stderr)

    def test_refuses_without_posting_when_anything_moved(self):
        cases = {
            "head": ({"FAKE_HEAD": "h2"}, 11, "moved from h1 to h2"),
            # Also what a second run meets after a post: the post itself
            # moves updatedAt, so a rerun refuses instead of posting twice.
            "updated": ({"FAKE_UPDATED": "T2"}, 11, "was updated at T2"),
            "anchor": ({}, 40, "outside the diff"),
        }
        for name, (env, line, said) in cases.items():
            with self.subTest(name):
                result = self.post(line, env)
                # The fake answers a post with the URL, so empty stdout means
                # nothing was posted.
                self.assertEqual((result.returncode, result.stdout), (3, ""))
                self.assertIn(said, result.stderr)


POST_INLINE = REVIEW / "post-inline.sh"

# The PR's head, diff and review threads, and each posted comment echoed back
# as "posted path:line" in place of its URL.
FAKE_GH_INLINE = support.stub("skill-scripts/gh-inline.sh")
FAKE_GH_APPLY = support.stub("skill-scripts/gh-apply.sh")
FAKE_GH_SIMILAR = support.stub("skill-scripts/gh-similar.sh")
FAKE_GH_FEEDBACK = support.stub("skill-scripts/gh-feedback.sh")
FAKE_GH_CACHED = support.stub("skill-scripts/gh-cached.sh")
FAKE_GH_LIVE = support.stub("skill-scripts/gh-live.sh")
FAKE_INLINE_LOG = support.stub("skill-scripts/inline-log.sh")
FAKE_GH_REFINE = support.stub("skill-scripts/gh-refine.sh")
FAKE_CI_COUNT = support.stub("skill-scripts/ci-count.sh")


def review_comment(login, path, line, at="h1", body="x"):
    return {
        "user": {"login": login},
        "path": path,
        "line": line,
        "side": "RIGHT",
        "commit_id": at,
        "body": body,
    }


@unittest.skipUnless(shutil.which("jq"), "jq is not installed")
class PostInlineTests(unittest.TestCase):
    def post(self, comments, threads=(), summary=None, env=()):
        draft = {"sha": "h1", "comments": comments, "summary": summary}
        return run_fake(
            POST_INLINE,
            [5, "$FAKE_DRAFT"],
            FAKE_GH_INLINE,
            {
                "FAKE_DIFF": DIFF,
                "FAKE_DRAFT": json.dumps(draft),
                "FAKE_THREADS": json.dumps(list(threads)),
            },
            env=env,
        )

    def test_a_hunk_cubic_commented_on_is_skipped_and_the_file_is_not(self):
        def note(path, line):
            return {"path": path, "line": line, "body": "x"}

        result = self.post(
            [note("web/a.js", 12), note("web/a b.txt", 1), note("web/a.js", 40)],
            [
                review_comment("cubic-dev-ai[bot]", "web/a.js", 11),
                # Outdated: GitHub keeps its old line, which must not count
                # against this head's hunk. A human's claims none either.
                review_comment("cubic-dev-ai[bot]", "web/a b.txt", 1, at="h0"),
                review_comment("maint", "web/a b.txt", 1),
            ],
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "posted web/a b.txt:1 at h1\n")
        self.assertIn("dropped web/a.js:12, in a hunk cubic", result.stderr)
        self.assertIn("dropped web/a.js:40, outside the diff", result.stderr)

    def test_a_rerun_posts_only_what_did_not_land(self):
        notes = [
            {"path": "web/a.js", "line": 11, "body": "landed"},
            {"path": "web/a.js", "line": 12, "body": "refused last time"},
        ]
        result = self.post(
            notes, [review_comment("viewer", "web/a.js", 11, body="landed")]
        )
        self.assertEqual(result.stdout, "posted web/a.js:12 at h1\n")
        self.assertIn("dropped web/a.js:11, already posted", result.stderr)

    def test_a_push_mid_way_stops_the_rest(self):
        notes = [
            {"path": "web/a.js", "line": 11, "body": "first"},
            {"path": "web/a.js", "line": 12, "body": "second"},
        ]
        # Only the fallback posts one at a time, so only it can be split.
        with tempfile.TemporaryDirectory() as scratch:
            result = self.post(
                notes,
                env={
                    "FAKE_PUSHED_AFTER": f"{scratch}/pushed",
                    "FAKE_REFUSE_REVIEW": "1",
                },
            )
        self.assertEqual(result.returncode, 3)
        self.assertEqual(result.stdout, "posted web/a.js:11 at h1\n")
        self.assertIn("moved from h1 to h2", result.stderr)

    def test_a_refused_comment_leaves_failure_status_and_posts_the_rest(self):
        result = self.post(
            [
                {"path": "web/a.js", "line": 11, "body": "refused"},
                {"path": "web/a.js", "line": 12, "body": "accepted"},
            ],
            env={"FAKE_REFUSE_LINE": "11"},
        )
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, "posted web/a.js:12 at h1\n")
        self.assertIn("refused web/a.js:11", result.stderr)

    def test_posts_the_summary_only_when_the_draft_has_one(self):
        note = {"path": "web/a.js", "line": 11, "body": "x"}
        self.assertNotIn("summary", self.post([note]).stdout)
        result = self.post([note], summary="This loses the report.")
        self.assertEqual(result.stdout, "posted web/a.js:11 at h1\nsummary posted\n")

    def test_the_summary_waits_until_a_finding_lands(self):
        outside = {"path": "web/a.js", "line": 40, "body": "x"}
        result = self.post([outside], summary="This loses the report.")
        self.assertEqual(result.stdout, "")
        self.assertIn("summary held back", result.stderr)
        refused = {"path": "web/a.js", "line": 11, "body": "x"}
        result = self.post(
            [refused], summary="This loses the report.", env={"FAKE_REFUSE_LINE": "11"}
        )
        self.assertNotIn("summary posted", result.stdout)
        # One finding landed, but the summary may name the one that did not.
        landed = {"path": "web/a.js", "line": 11, "body": "x"}
        result = self.post([landed, outside], summary="This loses the report.")
        self.assertEqual(result.stdout, "posted web/a.js:11 at h1\n")
        self.assertIn("summary held back", result.stderr)

    def test_findings_go_out_as_one_review(self):
        notes = [
            {"path": "web/a.js", "line": 11, "body": "first"},
            {"path": "web/a.js", "line": 12, "body": "second"},
        ]
        result = self.post(notes)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            result.stdout, "posted web/a.js:11 at h1\nposted web/a.js:12 at h1\n"
        )
        self.assertNotIn("posting each finding", result.stderr)

    def test_a_review_whose_answer_was_lost_is_not_posted_again(self):
        notes = [{"path": "web/a.js", "line": 11, "body": "once"}]
        result = self.post(notes, env={"FAKE_LOSE_REVIEW": "1"})
        self.assertEqual((result.returncode, result.stdout), (0, ""))
        self.assertIn("posting each finding", result.stderr)

    def test_a_quote_places_the_finding_and_never_reaches_github(self):
        result = self.post(
            [
                # The reviewer miscounted; the quote names line 12.
                {"path": "web/a.js", "quote": "  tail", "line": 10, "body": "x"},
                {"path": "web/a.js", "quote": "not in the diff", "body": "y"},
            ]
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "posted web/a.js:12 at h1\n")
        self.assertIn("dropped web/a.js:unplaced, outside the diff", result.stderr)
        self.assertNotIn("draft field posted", result.stderr)

    def test_a_moved_head_posts_nothing(self):
        result = self.post(
            [{"path": "web/a.js", "line": 11, "body": "x"}], env={"FAKE_HEAD": "h2"}
        )
        self.assertEqual((result.returncode, result.stdout), (3, ""))
        self.assertIn("moved from h1 to h2", result.stderr)


BUNDLE = REVIEW / "review-bundle.sh"

# The PR read on both sides of its diff, its files at the head, its threads,
# and the comparison with an earlier review's head. FAKE_AFTER is what the
# second read sees.
FAKE_GH_BUNDLE = support.stub("skill-scripts/gh-bundle.sh")

BUNDLE_PR = {
    "title": "Fix the thing",
    "author": {"login": "contributor"},
    "body": "Closes #9",
    "headRefOid": "h1",
    "baseRefOid": "b1",
    "updatedAt": "T1",
    "closingIssuesReferences": [{"number": 9}],
    "reviews": [],
    "comments": [],
}


# As the REST listing pages them; web/gone.js is changed but unreadable.
BUNDLE_FILES = [
    {"path": "web/a.js", "additions": 1, "deletions": 1, "status": "modified"},
    {"path": "web/a b.txt", "additions": 1, "deletions": 1, "status": "modified"},
    {"path": "web/old.js", "additions": 0, "deletions": 1, "status": "removed"},
    {"path": "web/gone.js", "additions": 1, "deletions": 0, "status": "added"},
]


@unittest.skipUnless(shutil.which("jq"), "jq is not installed")
class ReviewBundleTests(unittest.TestCase):
    def bundle(self, *args, env=(), files=BUNDLE_FILES, out=None, pr=BUNDLE_PR):
        with tempfile.TemporaryDirectory() as scratch:
            out = out or scratch
            result = run_fake(
                BUNDLE,
                [5, out, *args],
                FAKE_GH_BUNDLE,
                {
                    "FAKE_PR": json.dumps(pr),
                    "FAKE_DIFF": DIFF,
                    "FAKE_FILES": "".join(json.dumps(f) + "\n" for f in files),
                },
                env=env,
            )
            written = {
                str(p.relative_to(out)): p.read_text()
                for p in Path(out).rglob("*")
                if p.is_file()
            }
        return result, written

    def test_maps_files_to_rules_and_numbers_the_right_side(self):
        result, written = self.bundle()
        self.assertEqual(result.returncode, 0, result.stderr)
        manifest = json.loads(result.stdout)
        self.assertEqual(json.loads(written["manifest.json"]), manifest)
        self.assertEqual((manifest["head"], manifest["closes"]), ("h1", [9]))
        rules = {f["path"]: f["rules"] for f in manifest["files"]}
        self.assertEqual(rules["web/a.js"], ["browser.md"])
        # No pattern covers a .txt under web/, so it is named, not skipped.
        self.assertEqual(manifest["unruled"], ["web/a b.txt"])
        self.assertEqual(manifest["updatedAt"], "T1")
        # A contributor's diff never goes to an outside reviewer; the
        # account's own PR may.
        self.assertEqual(
            (manifest["author"], manifest["outside_reviewers"]),
            ("contributor", "refused"),
        )
        own, _ = self.bundle(env={"FAKE_VIEWER": "contributor"})
        self.assertEqual(json.loads(own.stdout)["outside_reviewers"], "allowed")
        self.assertTrue(manifest["files_complete"])
        self.assertEqual(
            json.loads(written["talk.json"]), {"reviews": [], "comments": []}
        )
        # Every changed file but the deleted one is read once, here.
        self.assertEqual(written["head/web/a.js"], "at head\n")
        self.assertEqual(written["head/web/a b.txt"], "spaced\n")
        self.assertEqual(manifest["not_at_head"], ["web/gone.js", "web/old.js"])
        self.assertNotIn("not read", result.stderr)
        self.assertEqual(len(written["threads.tsv"].splitlines()), 2)
        # A point made in a reply counts as raised, so replies are kept.
        self.assertEqual(json.loads(written["comments.json"])[0]["reply"], True)
        self.assertIsNone(manifest["since"])
        numbered = written["numbered.txt"]
        self.assertIn("    10  keep\n       --- x\n    11 +new\n", numbered)
        self.assertIn("## web/old.js\n(deleted)\n", numbered)
        self.assertNotIn("since.diff", written)
        # Synthetic h1 is deliberately not a full SHA: optional private
        # experience preparation must report its limitation without failing
        # the otherwise valid review bundle.
        for status in ("experience-status.json", "experience-context-status.json"):
            self.assertEqual(json.loads(written[status])["outcome"], "unavailable")
        self.assertNotIn("experience-start.json", written)
        self.assertIn("full manifest head", written["experience-begin.stderr"])
        self.assertIn("Local experience unavailable", written["experience.md"])

    def test_since_needs_enough_of_a_sha_to_name_a_commit(self):
        # A 1-digit prefix of the head would read as the head and review all.
        result, _ = self.bundle("--since", "a")
        self.assertEqual(result.returncode, 2)
        self.assertIn("at least 7 hex digits", result.stderr)

    def test_a_pr_past_the_file_listing_limit_says_it_is_partial(self):
        result, _ = self.bundle(pr=dict(BUNDLE_PR, changedFiles=3001))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(json.loads(result.stdout)["files_complete"])
        self.assertIn("changes 3001 files and GitHub listed 4", result.stderr)

    def test_each_path_a_rule_applies_to_loads_that_rule(self):
        # A rule's "Applies to" line scopes it; rules/paths is what the bundle
        # routes by. A path named in the first and not routed by the second is
        # a rule no reviewer of that file ever reads.
        tracked = subprocess.run(
            ["git", "ls-files"], cwd=ROOT, capture_output=True, text=True, check=True
        ).stdout.split()
        routes = [
            line.split()
            for line in (REVIEW / "rules" / "paths").read_text().splitlines()
            if line.strip() and not line.startswith("#")
        ]
        for rule in sorted((REVIEW / "rules").glob("*.md")):
            if rule.name == "general.md":
                continue
            text = rule.read_text()
            for scope in re.findall(r"^Applies to: (.*?)\n\n", text, re.M | re.S):
                # Paths only: a constant named beside its file is not one.
                for named in re.findall(r"`([^`]*[/.][^`]*)`", scope):
                    files = [t for t in tracked if fnmatch.fnmatchcase(t, named)]
                    with self.subTest(rule=rule.name, path=named):
                        self.assertTrue(files, "names no tracked file")
                        for path in files:
                            self.assertIn(
                                rule.name,
                                {r for p, r in routes if fnmatch.fnmatchcase(path, p)},
                                path,
                            )

    def test_an_earlier_head_gives_the_new_work_only_when_it_is_an_ancestor(self):
        result, written = self.bundle("--since", "c0ffee1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            json.loads(result.stdout)["since"], {"from": "c0ffee1", "status": "ahead"}
        )
        self.assertIn("since.txt", written)
        result, written = self.bundle(
            "--since", "c0ffee1", env={"FAKE_COMPARE": "diverged"}
        )
        since = json.loads(result.stdout)["since"]
        self.assertEqual(since["status"], "diverged")
        self.assertIn("whole diff", since["why"])
        self.assertNotIn("since.diff", written)
        # The base merged in keeps the earlier head an ancestor, but the
        # comparison then carries the base's own work too.
        result, written = self.bundle(
            "--since", "c0ffee1", env={"FAKE_COMPARE": "ahead 1"}
        )
        since = json.loads(result.stdout)["since"]
        self.assertEqual(since["status"], "merged")
        self.assertIn("whole diff", since["why"])
        self.assertNotIn("since.diff", written)

    def test_a_refused_comment_read_fails_the_bundle(self):
        # An empty list would read as a PR nobody has commented on.
        result, written = self.bundle(env={"FAKE_NO_COMMENTS": "1"})
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("manifest.json", written)

    def test_an_empty_repo_argument_keeps_since(self):
        result, written = self.bundle("", "--since", "c0ffee1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["since"]["status"], "ahead")
        self.assertIn("since.txt", written)

    def test_a_rerun_into_one_directory_keeps_nothing_stale(self):
        with tempfile.TemporaryDirectory() as out:
            self.bundle("--since", "c0ffee1", out=out)
            # The next head deletes the only file the first one changed.
            only = [BUNDLE_FILES[2]]
            result, written = self.bundle(files=only, out=out)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["not_at_head"], ["web/old.js"])
        self.assertFalse([name for name in written if name.startswith("head/")])
        self.assertNotIn("since.txt", written)
        # A PR that only deletes runs no read at all, and says nothing.
        self.assertEqual(result.stderr, "")

    def test_a_pr_that_keeps_moving_is_refused(self):
        result, _ = self.bundle(env={"FAKE_AFTER": "h2 b1"})
        self.assertEqual((result.returncode, result.stdout), (3, ""))
        self.assertIn("moved twice", result.stderr)

    def test_bad_arguments_are_refused_before_any_call(self):
        for args in (["abc", "out"], [5, "out", "--since", "not-a-sha"]):
            with self.subTest(args=args):
                result = run_fake(BUNDLE, args, FAKE_GH_FAILS, {})
                self.assertEqual(result.returncode, 2, result.stderr)

    def test_local_reviews_the_dirty_tree_or_the_branch(self):
        with (
            tempfile.TemporaryDirectory() as repo,
            tempfile.TemporaryDirectory() as out,
        ):

            def git(*args):
                subprocess.run(
                    ["git", "-c", "user.name=t", "-c", "user.email=t@t", *args],
                    cwd=repo,
                    check=True,
                    capture_output=True,
                )

            git("init", "-q")
            git("config", "user.email", "t@t")
            Path(repo, "web").mkdir()
            Path(repo, "web", "a.js").write_text("one\n")
            git("add", ".")
            git("commit", "-qm", "base")
            base = subprocess.run(
                ["git", "rev-parse", "HEAD"], cwd=repo, capture_output=True, text=True
            ).stdout.strip()
            Path(repo, "web", "a.js").write_text("one\ntwo\n")
            git("commit", "-qam", "branch")

            def local(*args):
                # A gh that fails every call: local mode reads only git.
                result = run_fake(
                    BUNDLE, ["--local", out, *args], FAKE_GH_FAILS, {}, cwd=repo
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                return json.loads(result.stdout)

            clean = local(base)
            start_path = Path(out, "experience-start.json")
            first_start = json.loads(start_path.read_text())
            self.assertRegex(first_start["run_id"], r"^[0-9a-f]{32}$")
            self.assertEqual(first_start["head"], clean["head"])
            self.assertEqual(
                first_start["diff_hash"],
                hashlib.sha256(Path(out, "pr.diff").read_bytes()).hexdigest(),
            )
            self.assertEqual(
                first_start["manifest_hash"],
                hashlib.sha256(Path(out, "manifest.json").read_bytes()).hexdigest(),
            )
            self.assertEqual(
                json.loads(Path(out, "experience-status.json").read_text()), first_start
            )
            context_status = json.loads(
                Path(out, "experience-context-status.json").read_text()
            )
            self.assertEqual(context_status["outcome"], "ready")
            self.assertEqual(context_status["cases"], 0)
            self.assertIn(
                "not evidence of absence", Path(out, "experience.md").read_text()
            )
            for name in (
                "experience-start.json",
                "experience-status.json",
                "experience.md",
            ):
                self.assertEqual(Path(out, name).stat().st_mode & 0o777, 0o600)
            self.assertEqual(local(base)["head"], clean["head"])
            self.assertEqual(json.loads(start_path.read_text()), first_start)
            # Every commit carries the configured email: the account's own.
            self.assertEqual(clean["outside_reviewers"], "allowed")
            self.assertEqual(clean["against"], f"{base}...HEAD")
            self.assertTrue(clean["committed"])
            self.assertTrue(clean["commit_sizes"]["complete"])
            self.assertEqual(clean["commit_sizes"]["head"], clean["head"])
            self.assertEqual(clean["commit_sizes"]["commits"][0]["known_lines"], 1)
            self.assertEqual(clean["files"][0]["rules"], ["browser.md"])
            self.assertIn("     2 +two", Path(out, "numbered.txt").read_text())

            Path(repo, "web", "b.js").write_text("other\n")
            git("add", ".")
            git("commit", "-qm", "theirs", "--author", "o <o@o>")
            # A branch holding someone else's commit goes to no outside reviewer.
            self.assertEqual(local(base)["outside_reviewers"], "refused")
            git("reset", "-q", "--hard", "HEAD~1")

            Path(repo, "notes.txt").write_text("x\n")
            untracked = local()
            self.assertEqual(untracked["against"], "HEAD")
            self.assertFalse(untracked["committed"])
            self.assertFalse(untracked["commit_sizes"]["complete"])
            self.assertEqual(untracked["files"], [])
            self.assertEqual(untracked["untracked"], ["notes.txt"])
            self.assertEqual(Path(out, "pr.diff").read_text(), "")
            # A base named is the base, untracked files or not.
            named = local(base)
            self.assertEqual(named["against"], f"{base}...HEAD")
            self.assertTrue(named["committed"])
            self.assertIn("     2 +two", Path(out, "numbered.txt").read_text())

            Path(repo, "web", "a.js").write_text("one\ntwo\nthree\n")
            Path(repo, "notes.txt").write_text("x\n")
            dirty = local()
            dirty_start = json.loads(start_path.read_text())
            Path(repo, "web", "a.js").write_text("one\ntwo\nfour\n")
            same_head_new_diff = local()
            new_start = json.loads(start_path.read_text())
            self.assertEqual(same_head_new_diff["head"], dirty["head"])
            self.assertNotEqual(new_start["diff_hash"], dirty_start["diff_hash"])
            self.assertNotEqual(new_start["run_id"], dirty_start["run_id"])
            Path(repo, "web", "a.js").write_text("one\ntwo\nthree\n")
            self.assertEqual(dirty["against"], "HEAD")
            self.assertEqual(dirty["size"]["additions"], 1)
            # With a base and uncommitted work, the base to the working tree:
            # both commits' lines, and nothing a push of HEAD would carry alone.
            whole = local(base)
            self.assertEqual(whole["against"], base)
            self.assertFalse(whole["committed"])
            self.assertEqual(whole["size"]["additions"], 2)
            self.assertEqual(dirty["untracked"], ["notes.txt"])
            self.assertEqual(dirty["unruled"], [])

            # A name git would quote reaches the rule patterns as it is.
            git("add", ".")
            Path(repo, "web", "\u00e9.js").write_text("x\n")
            git("add", ".")
            quoted = local()
            self.assertIn("web/\u00e9.js", [f["path"] for f in quoted["files"]])
            self.assertEqual(quoted["unruled"], ["notes.txt"])


ODDS = SKILLS / "codetrial-odds"


@unittest.skipUnless(shutil.which("jq"), "jq is not installed")
class OddsTests(unittest.TestCase):
    def test_it_is_typed_only_and_named_for_its_directory(self):
        # Routing never offers it, so the plain-language ranking request
        # stays with codetrial-triage.
        front = (ODDS / "SKILL.md").read_text().split("---", 2)[1]
        self.assertIn("name: codetrial-odds\n", front)
        self.assertIn("disable-model-invocation: true\n", front)

    def test_only_a_whole_sweep_replaces_the_snapshot(self):
        # Records its arguments in the snapshot, or fails as FAKE_SWEEP says.
        sweep = (
            'case "${FAKE_SWEEP:-}" in\n'
            "  fail) echo '{\"status\": [' ; exit 1 ;;\n"
            "  junk) echo '{}' ;;\n"
            "  *) jq -n '{status: [], args: $ARGS.positional}' --args \"$@\" ;;\n"
            "esac\n"
        )
        snapshot = ".git/codetrial-triage/pr-sweep-sysprog21-codetrial.json"
        with tempfile.TemporaryDirectory() as scratch:
            repo = Path(scratch)
            subprocess.run(["git", "init", "-q", str(repo)], check=True)
            saved = repo / snapshot

            def run(mode=""):
                return run_fake(
                    ODDS / "snapshot.sh",
                    [],
                    FAKE_GH_FAILS,
                    {"ODDS_SWEEP": sweep},
                    cwd=repo,
                    env={"FAKE_SWEEP": mode},
                )

            first = run()
            self.assertEqual((first.returncode, first.stdout), (0, f"{snapshot}\n"))
            # An empty stale-days leaves the sweep its own default.
            self.assertEqual(
                json.loads(saved.read_text())["args"], ["sysprog21/codetrial", ""]
            )
            self.assertEqual(run().returncode, 0)
            self.assertEqual(
                json.loads(saved.read_text())["args"],
                ["sysprog21/codetrial", "", snapshot],
            )
            kept = saved.read_text()
            for mode in ("fail", "junk"):
                with self.subTest(mode=mode):
                    result = run(mode)
                    self.assertEqual((result.returncode, result.stdout), (1, ""))
                    self.assertIn("the last snapshot is kept", result.stderr)
                    self.assertEqual(saved.read_text(), kept)
                    self.assertEqual(
                        [p.name for p in saved.parent.iterdir()], [saved.name]
                    )


HYGIENE = SKILLS / "codetrial-triage" / "hygiene.sh"

# The PR and its commits; every other call fails, so ci-failure.sh yields no
# diagnosis and no CI item, which is also what a failed lookup must do.
FAKE_GH_HYGIENE = support.stub("skill-scripts/gh-hygiene.sh")

GOOD_MESSAGE = "Add a thing\n\nThe lobby needed it, and this says why in one line.\n"


def commit(message, parents=1, at="2026-10-20T00:00:00Z"):
    return {
        "sha": "h1",
        "author": {"login": "contributor"},
        "parents": [{}] * parents,
        "commit": {"message": message, "committer": {"date": at}},
    }


# One commit for each commit rule: a bad message, an agent co-author, a merge.
EVERY_RULE = [
    commit("added a thing."),
    commit(GOOD_MESSAGE + "\nCo-authored-by: Claude <noreply@anthropic.com>\n"),
    commit("Merge branch 'main' into topic", parents=2),
]


def failing_job(gates, on_main=False, name="gates (portable)"):
    return {"name": name, "current": True, "gates": gates, "on_main": on_main}


def agent_marked(messages):
    """Whether the commit-msg hook credits an AI tool in each message: bit 2."""
    hook = ROOT / "scripts" / "git-commit-msg.sh"
    return [
        bool(
            subprocess.run(
                ["sh", str(hook), "--stored", "-"],
                input=message,
                capture_output=True,
                text=True,
            ).returncode
            & 2
        )
        for message in messages
    ]


@unittest.skipUnless(shutil.which("jq"), "jq is not installed")
class HygieneTests(unittest.TestCase):
    def hygiene(self, commits, said=(), env=(), ci=None, **fields):
        pr = {
            "number": 5,
            "headRefOid": "h1",
            "headRefName": "topic",
            "isCrossRepository": True,
            "isDraft": False,
            "baseRefName": "main",
            "mergeable": "MERGEABLE",
            "author": {"login": "contributor"},
            "body": "Adds a thing.",
            "files": [{"path": "src/a.rs"}],
            "reviews": [],
            "comments": list(said),
        } | fields
        result = run_fake(
            HYGIENE,
            [5],
            FAKE_GH_HYGIENE,
            {"FAKE_PR": json.dumps(pr), "FAKE_COMMITS": json.dumps(commits)}
            | (
                {"FAKE_CI": json.dumps(ci), "HYGIENE_CI": 'cat "$FAKE_CI"\n'}
                if ci is not None
                else {}
            ),
            env=env,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        out = json.loads(result.stdout)
        self.assertEqual(out["body"], "\n\n".join(i["text"] for i in out["items"]))
        return out

    def keys(self, *args, **kwargs):
        out = self.hygiene(*args, **kwargs)
        return out["skip"], [item["key"] for item in out["items"]]

    def comment_keys(self, *args, **kwargs):
        return [c["key"] for c in self.hygiene(*args, **kwargs)["comments"]]

    def test_each_rule_fires_on_its_own_evidence(self):
        commits = EVERY_RULE
        skip, keys = self.keys(
            commits, headRefName="main", files=[{"path": "web/index.html"}]
        )
        self.assertIsNone(skip)
        self.assertEqual(
            keys, ["fork_main", "merge_commit", "commit_rules", "co_authored"]
        )
        # A clean PR is asked for nothing.
        self.assertEqual(self.keys([commit(GOOD_MESSAGE)]), (None, []))

    def test_a_page_change_without_a_screenshot_is_asked_in_a_plain_comment(self):
        good = [commit(GOOD_MESSAGE)]
        text = (
            "Add screenshots to the pull request description to illustrate your "
            "work, as @ColtenOuO suggests. Example: #349"
        )
        out = self.hygiene(good, files=[{"path": "web/index.html"}])
        self.assertEqual(out["comments"], [{"key": "screenshot", "text": text}])
        # It is a comment, so it does not turn the PR into a blocked review.
        self.assertEqual((out["items"], out["body"]), ([], ""))
        shown = "Before and after:\n\n![lobby](https://github.com/user-attachments/assets/x)"
        for name, fields in {
            "an image in the description": {
                "files": [{"path": "web/styles.css"}],
                "body": shown,
            },
            "no page file": {"files": [{"path": "web/problems/a.json"}]},
            "a vendored script": {"files": [{"path": "web/vendor/x.js"}]},
        }.items():
            with self.subTest(name):
                self.assertEqual(self.comment_keys(good, **fields), [])

    def test_a_script_counts_as_a_page_change_when_it_writes_to_the_dom(self):
        good = [commit(GOOD_MESSAGE)]
        files = [{"path": "web/app.js"}]

        def diff(line, path="web/app.js"):
            return f"diff --git a/{path} b/{path}\n+++ b/{path}\n+{line}"

        env = {"FAKE_DIFF": diff("el.textContent = label;")}
        self.assertEqual(self.comment_keys(good, files=files, env=env), ["screenshot"])
        for name, text in {
            "logic only": diff("const total = a + b;"),
            "a test file": diff("el.textContent = 1;", "web/app.test.js"),
            "vendored code": diff("el.innerHTML = 1;", "web/vendor/x.js"),
        }.items():
            with self.subTest(name):
                self.assertEqual(
                    self.comment_keys(good, files=files, env={"FAKE_DIFF": text}), []
                )
        # A diff GitHub will not serve leaves only HTML and CSS as evidence.
        self.assertEqual(self.comment_keys(good, files=files), [])

    def test_a_removed_dom_write_is_a_page_change_too(self):
        good = [commit(GOOD_MESSAGE)]
        gone = "diff --git a/web/app.js b/web/app.js\n+++ b/web/app.js\n-  el.textContent = label;"
        self.assertEqual(
            self.comment_keys(
                good, files=[{"path": "web/app.js"}], env={"FAKE_DIFF": gone}
            ),
            ["screenshot"],
        )

    def test_a_screenshot_already_asked_for_is_not_asked_again(self):
        good = [commit(GOOD_MESSAGE)]
        files = [{"path": "web/index.html"}]
        ask = {
            "author": {"login": "maint"},
            "createdAt": "2026-10-19T00:00:00Z",
            "body": "Add screenshots to the pull request description.",
        }
        # Before the last push or not: the description is what is missing.
        self.assertEqual(self.comment_keys(good, [ask], files=files), [])
        self.assertEqual(
            self.comment_keys(good, [ask | {"isMinimized": True}], files=files),
            ["screenshot"],
        )
        old = review("maint", "CHANGES_REQUESTED", 0, "Show a screenshot.", oid="h0")
        self.assertEqual(self.comment_keys(good, files=files, reviews=[old]), [])
        # A review hidden by hand asks nothing: gh pr view cannot say so.
        hidden = {"FAKE_MINIMIZED": json.dumps([old["id"]])}
        self.assertEqual(
            self.comment_keys(good, files=files, reviews=[old], env=hidden),
            ["screenshot"],
        )
        # Asked on a line of the diff rather than in a comment.
        inline = [
            {"user": {"login": "maint"}, "commit_id": "h0", "body": "A screenshot?"}
        ]
        self.assertEqual(
            self.comment_keys(
                good, files=files, env={"FAKE_INLINE": json.dumps(inline)}
            ),
            [],
        )

    def test_only_an_agent_address_is_an_attribution_marker(self):
        agents = (
            "Claude <noreply@anthropic.com>",
            "Claude Opus 5 (1M context) <noreply@anthropic.com>",
            "Agent <codex@openai.com>",
            "Agent <cursoragent@cursor.com>",
            "Agent <devin-ai-integration[bot]@users.noreply.github.com>",
            # GitHub's noreply form for an app or bot account: ID+login.
            "Agent <158243242+devin-ai-integration[bot]@users.noreply.github.com>",
            "Copilot <198982749+Copilot@users.noreply.github.com>",
            "Agent <copilot@github.com>",
        )
        people = (
            "Ada Lovelace <ada@example.com>",
            "Claude Shannon <claude@example.com>",
            "Ada Lovelace <ada@openai.com>",
            "Ada Lovelace <ada@anthropic.com>",
            "Ada Lovelace <ada@cursor.com>",
            "Ada Lovelace <ada@cursor.sh>",
            "Ada Lovelace <ada@devin.ai>",
            "Ada Lovelace <copilot-research@example.com>",
            # A person's own noreply address has the same ID+login shape,
            # and a bot that is not an agent is credited for other reasons.
            "Ada Lovelace <12345+ada@users.noreply.github.com>",
            "CI <1+github-actions[bot]@users.noreply.github.com>",
        )
        marked = agent_marked(
            [f"{GOOD_MESSAGE}\nCo-authored-by: {who}\n" for who in agents + people]
        )
        self.assertEqual(marked, [True] * len(agents) + [False] * len(people))
        # The footer an agent writes, against prose that names a tool.
        footer = f"{GOOD_MESSAGE}\n\U0001f916 Generated with [Claude Code](https://x)\n"
        prose = f"{GOOD_MESSAGE}\nFix the parser for files generated by codex.\n"
        self.assertEqual(agent_marked([footer, prose]), [True, False])

    def test_a_request_already_made_at_this_head_is_not_repeated(self):
        def said(at):
            return {
                "author": {"login": "maint"},
                "createdAt": at,
                "body": "Hint: run `make hooks`.",
            }

        bad = [commit("added a thing.")]
        self.assertEqual(self.keys(bad, [said("2026-10-21T00:00:00Z")]), (None, []))
        # Said before the last push: the new commits have not heard it.
        self.assertEqual(
            self.keys(bad, [said("2026-10-19T00:00:00Z")]), (None, ["commit_rules"])
        )
        # Dependabot writes its own messages, and CI skips its commits by
        # address, so the same address is never asked to reword one here.
        bot = commit("bump the thing.")
        bot["commit"]["author"] = {
            "email": "49699333+dependabot[bot]@users.noreply.github.com"
        }
        self.assertEqual(self.keys([bot]), (None, []))
        # Asked on a line of this head, rather than in a review body; a line
        # comment left on an older head has not been heard by this one.
        inline = [
            {"user": {"login": "maint"}, "commit_id": head, "body": "Run `make hooks`."}
            for head in ("h1",)
        ]
        self.assertEqual(
            self.keys(bad, env={"FAKE_INLINE": json.dumps(inline)}), (None, [])
        )
        inline[0]["commit_id"] = "h0"
        self.assertEqual(
            self.keys(bad, env={"FAKE_INLINE": json.dumps(inline)}),
            (None, ["commit_rules"]),
        )
        # The push is dated by GitHub's first check suite, not the commit: a
        # commit made before the request but pushed after it has not heard it.
        self.assertEqual(
            self.keys(
                bad,
                [said("2026-10-21T00:00:00Z")],
                env={"FAKE_PUSHED": "2026-10-22T00:00:00Z"},
            ),
            (None, ["commit_rules"]),
        )

    def test_a_broken_rule_and_an_attribution_in_one_message_are_both_asked_for(self):
        both = "added a thing.\n\nCo-authored-by: Claude <noreply@anthropic.com>\n"
        self.assertEqual(
            self.keys([commit(both)]), (None, ["commit_rules", "co_authored"])
        )

    def test_attribution_is_checked_in_merges_and_bot_commits(self):
        message = (
            "Merge branch 'main'\n\nCo-authored-by: Claude <noreply@anthropic.com>\n"
        )
        self.assertEqual(
            self.keys([commit(message, parents=2)]),
            (None, ["merge_commit", "co_authored"]),
        )
        bot = commit(
            GOOD_MESSAGE + "\nCo-authored-by: Claude <noreply@anthropic.com>\n"
        )
        bot["commit"]["author"] = {
            "email": "49699333+dependabot[bot]@users.noreply.github.com"
        }
        self.assertEqual(self.keys([bot]), (None, ["co_authored"]))

    def test_a_reference_on_the_last_line_breaks_a_commit_rule(self):
        refs = GOOD_MESSAGE + "\nRefs #12\n"
        closes = GOOD_MESSAGE + "\nCloses #12\n"
        self.assertEqual(self.keys([commit(refs)]), (None, ["commit_rules"]))
        self.assertEqual(self.keys([commit(closes)]), (None, []))

    def test_each_commit_rule_fires_on_its_own_evidence_alone(self):
        # One rule's evidence at a time, so a predicate that leaned on another
        # rule's evidence would fire on the wrong case or miss its own.
        agent = GOOD_MESSAGE + "\nCo-authored-by: Claude <noreply@anthropic.com>\n"
        cases = {
            "commit_rules": ([commit("added a thing.")], {}),
            "co_authored": ([commit(agent)], {}),
            "merge_commit": ([commit(GOOD_MESSAGE), commit("Merge it", parents=2)], {}),
            "fork_main": ([commit(GOOD_MESSAGE)], {"headRefName": "main"}),
        }
        for key, (commits, fields) in cases.items():
            with self.subTest(key):
                self.assertEqual(self.keys(commits, **fields), (None, [key]))

    def test_ci_items_need_a_failure_the_branch_made_at_this_head(self):
        good = [commit(GOOD_MESSAGE)]
        job = failing_job

        def keys(*jobs, head="h1", **fields):
            ci = {"head": head, "stale_base": False, "failing": list(jobs)}
            return self.keys(good, ci=ci, **fields)[1]

        self.assertEqual(keys(job(["indent"])), ["indent"])
        self.assertEqual(keys(job(["clippy"])), ["ci_regression"])
        # A mutation job names no gate and still failed for this branch.
        self.assertEqual(keys(job([], name="mutants (0)")), ["ci_regression"])
        # main fails it too, the log could not be read, only the aggregate
        # check failed, the diagnosis read another head, or a rebase comes
        # first: none of these is the author's to fix here.
        self.assertEqual(keys(job(["indent"], on_main=True)), [])
        self.assertEqual(keys(job(None)), [])
        self.assertEqual(keys(job([], name="check")), [])
        self.assertEqual(keys(job(["clippy"]), head="h0"), [])
        self.assertEqual(keys(job(["clippy"]), mergeable="CONFLICTING"), [])
        self.assertEqual(keys(job(["clippy"]), mergeable="UNKNOWN"), [])

    def test_a_ci_regression_is_asked_in_the_fixed_rebase_text(self):
        ci = {"head": "h1", "stale_base": False, "failing": [failing_job(["clippy"])]}
        out = self.hygiene([commit(GOOD_MESSAGE)], ci=ci)
        self.assertEqual(out["body"], FIXED)

    def test_a_posted_review_holds_back_every_item_it_carried(self):
        # Each item's marker has to recognize its own text, or the next sweep
        # posts the same review again at the same head.
        commits = EVERY_RULE
        fields = {"headRefName": "main", "files": [{"path": "web/index.html"}]}
        ci = {
            "head": "h1",
            "failing": [
                failing_job(["indent"], name="lint"),
                failing_job(["rust"], name="unit"),
            ],
        }
        out = self.hygiene(commits, ci=ci, **fields)
        self.assertEqual(len(out["items"]), 6)
        posted = review("maint", "CHANGES_REQUESTED", 0, out["body"], oid="h1")
        self.assertEqual(
            self.keys(commits, ci=ci, reviews=[posted], **fields), (None, [])
        )

    def test_a_request_stands_across_pushes_while_its_evidence_holds(self):
        good = [commit(GOOD_MESSAGE)]
        ci = {"head": "h1", "failing": [failing_job(["indent"])]}
        text = self.hygiene(good, ci=ci)["body"]

        def copy(name, days, body=text, oid="h0"):
            return review("maint", "CHANGES_REQUESTED", days, body, oid=oid) | {
                "id": name
            }

        # The copy was left on an earlier head and the gate still fails: no
        # second review, and nothing to hide.
        out = self.hygiene(good, ci=ci, reviews=[copy("R_old", 3)])
        self.assertEqual((out["items"], out["hide"]), ([], []))
        # Hidden by hand, it no longer stands and the request goes out again.
        out = self.hygiene(
            good,
            ci=ci,
            reviews=[copy("R_old", 3)],
            env={"FAKE_MINIMIZED": '["R_old"]'},
        )
        self.assertEqual([i["key"] for i in out["items"]], ["indent"])
        # Two copies left by earlier sweeps: the newest stays.
        out = self.hygiene(good, ci=ci, reviews=[copy("R_a", 3), copy("R_b", 1)])
        self.assertEqual(
            out["hide"], [{"id": "R_a", "key": "indent", "why": "repeated"}]
        )
        self.assertEqual(out["items"], [])
        # A comment carries the request too, and is hidden the same way.
        said = {
            "id": "IC_x",
            "author": {"login": "maint"},
            "createdAt": ago(5),
            "isMinimized": False,
            "body": text,
        }
        out = self.hygiene(good, said=[said], ci=ci, reviews=[copy("R_b", 1)])
        self.assertEqual([h["id"] for h in out["hide"]], ["IC_x"])

    def test_a_request_whose_evidence_is_gone_is_hidden_and_not_repeated(self):
        good = [commit(GOOD_MESSAGE)]
        both = [commit("added a thing.")]
        ci = {"head": "h1", "failing": [failing_job(["indent"])]}
        text = self.hygiene(both, ci=ci)["body"]
        old = review("maint", "CHANGES_REQUESTED", 2, text, oid="h0") | {"id": "R_old"}
        # The gate passes now: the copy describes a PR that no longer exists.
        out = self.hygiene(good, ci={"head": "h1", "failing": []}, reviews=[old])
        self.assertEqual(out["items"], [])
        self.assertEqual(
            out["hide"], [{"id": "R_old", "key": "commit_rules,indent", "why": "stale"}]
        )
        # The commit messages are fixed and the gate still fails: the review
        # that carried both is replaced by one that carries what holds.
        out = self.hygiene(good, ci=ci, reviews=[old])
        self.assertEqual([i["key"] for i in out["items"]], ["indent"])
        self.assertEqual([h["id"] for h in out["hide"]], ["R_old"])
        # A head that moved while it was read hides nothing.
        out = self.hygiene([commit(GOOD_MESSAGE) | {"sha": "h2"}], reviews=[old])
        self.assertEqual(
            (out["skip"], out["hide"]), ("the head moved while it was read", [])
        )

    def test_no_ci_evidence_keeps_the_copy_that_rests_on_it(self):
        good = [commit(GOOD_MESSAGE)]
        failing = {"head": "h1", "failing": [failing_job(["indent"])]}
        text = self.hygiene(good, ci=failing)["body"]
        old = review("maint", "CHANGES_REQUESTED", 2, text, oid="h0") | {"id": "R_old"}
        # None of these says the gate passes: the diagnosis failed, it read
        # another head, a check is still running or held, or a log was unread.
        unknown = {
            "failed": {},
            "another head": {"head": "h0", "failing": []},
            "pending": {"head": "h1", "failing": [], "pending": ["gates"]},
            "held": {"head": "h1", "failing": [], "held": ["gates"]},
            "unread": {"head": "h1", "failing": [failing_job(None)]},
        }
        # A review that carried a request about the messages too, now fixed:
        # it still rests on CI, so it stays while CI is unknown.
        mixed_text = self.hygiene([commit("added a thing.")], ci=failing)["body"]
        mixed = review("maint", "CHANGES_REQUESTED", 2, mixed_text, oid="h0") | {
            "id": "R_mixed"
        }
        for name, ci in unknown.items():
            out = self.hygiene(good, ci=ci, reviews=[old])
            self.assertEqual((out["items"], out["hide"]), ([], []), name)
            out = self.hygiene(good, ci=ci, reviews=[mixed])
            self.assertEqual((out["items"], out["hide"]), ([], []), name)
        # A complete read with nothing failing is the evidence that it is gone.
        out = self.hygiene(good, ci={"head": "h1", "failing": []}, reviews=[old])
        self.assertEqual([h["why"] for h in out["hide"]], ["stale"])

    def test_a_review_with_feedback_of_its_own_is_never_hidden(self):
        good = [commit(GOOD_MESSAGE)]
        failing = {"head": "h1", "failing": [failing_job(["indent"])]}
        text = self.hygiene(good, ci=failing)["body"]
        mixed = review(
            "maint", "CHANGES_REQUESTED", 2, "The API in foo.rs is wrong.\n\n" + text
        ) | {"id": "R_human", "commit": {"oid": "h0"}}
        out = self.hygiene(good, ci={"head": "h1", "failing": []}, reviews=[mixed])
        self.assertEqual(out["hide"], [])

    def test_a_review_hidden_by_hand_is_not_what_was_said(self):
        good = [commit(GOOD_MESSAGE)]
        failing = {"head": "h1", "failing": [failing_job(["indent"])]}
        text = self.hygiene(good, ci=failing)["body"]
        here = review("maint", "CHANGES_REQUESTED", 0, text, oid="h1") | {
            "id": "R_here"
        }
        out = self.hygiene(good, ci=failing, reviews=[here])
        self.assertEqual(out["items"], [])
        out = self.hygiene(
            good, ci=failing, reviews=[here], env={"FAKE_MINIMIZED": '["R_here"]'}
        )
        self.assertEqual([i["key"] for i in out["items"]], ["indent"])

    def test_a_partial_review_listing_skips_the_pr(self):
        out = self.hygiene([commit(GOOD_MESSAGE)], env={"FAKE_MINIMIZED_MORE": "1"})
        self.assertEqual(out["skip"], "the review listing failed")

    def test_a_pr_nobody_should_be_asked_about_is_skipped(self):
        bad = [commit("added a thing.")]
        self.assertEqual(self.keys(bad, isDraft=True), ("draft", []))
        # gh names a bot app/<slug> and flags it, never <slug>[bot].
        bot = {"login": "app/dependabot", "is_bot": True}
        self.assertEqual(self.keys(bad, author=bot), ("a bot's pull request", []))
        self.assertEqual(
            self.keys(bad, author={"login": "maint"}),
            ("a maintainer's pull request", []),
        )
        # Nobody can be told apart as a maintainer, even with comments to read.
        said = [
            {
                "author": {"login": "maint"},
                "createdAt": "2026-10-21T00:00:00Z",
                "body": "x",
            }
        ]
        self.assertEqual(
            self.keys(bad, said, env={"FAKE_NO_PUSHERS": "1"}),
            ("the push listing failed", []),
        )
        # Nothing dates the push, so no request can be read as already asked.
        self.assertEqual(
            self.keys(bad, env={"FAKE_NO_SUITE": "1"}),
            ("no check suite dates the push", []),
        )
        # Commits listed after a push belong to another head.
        moved = [commit("added a thing.") | {"sha": "h2"}]
        self.assertEqual(self.keys(moved), ("the head moved while it was read", []))


GOOD_TARGET = """\
---
name: fixture
description: front matter is not a heading
---
# Foo, bar (baz)
## The `make indent` target/lane
## Repeat
## Repeat
## Closing hashes ##
   ## Indented heading
Setext heading
--------------
## See [the guide](guide.md)
## Émigré café
## a_b & c—d
## The `<id>` and `__init__` hooks
````md
```
# inside four-backtick fence
```
````
~~~
# inside tilde fence
~~~
"""

RESOLVING = """\
# Fixture

[a](target.md#foo-bar-baz) [b](target.md#the-make-indent-targetlane)
[c](target.md#repeat-1) [d](target.md#closing-hashes)
[e](target.md#indented-heading) [f](target.md#setext-heading)
[g](target.md#see-the-guide) [h](target.md#émigré-café) [i](target.md#a_b--cd)
[j](target.md "Title") [k](../../../src/main.rs#L10) [l](https://example.com/x)
[self](#fixture) `[in code](nowhere.md)` [r](target.md#the-id-and-__init__-hooks)
`src/main.rs:12` `src/main.rs#L5` `Cargo.toml` `TODO.md` `make fmt_all`
`make lint` `make -j4 V=1 lint` `scripts/test.sh --fixture` `gates (rust)`

```sh
./scripts/test.sh
cat src/main.rs; ls src/main.rs, src/
make hooks
[in a fence](nowhere.md)
```
"""

DANGLING = """\
# Fixture

[m](target.md#inside-four-backtick-fence) [n](target.md#inside-tilde-fence)
[o](target.md#repeat-2) [p](target.md#description-front-matter-is-not-a-heading)
[q](missing.md)

[ref]: missing-ref.md

`src/nope.rs:3` `Nope.toml` `make test2` `make indent-check`
`scripts/test.sh --fix` `gates (lane-2)` `make -k test3`

```sh
scripts/missing.sh
make no_such_target
```
"""

DANGLING_REPORT = {
    f".claude/skills/fixture/refs.md: {what}"
    for what in (
        "anchor target.md#inside-four-backtick-fence",
        "anchor target.md#inside-tilde-fence",
        "anchor target.md#repeat-2",
        "anchor target.md#description-front-matter-is-not-a-heading",
        "link missing.md",
        "link missing-ref.md",
        "path src/nope.rs",
        "path scripts/missing.sh",
        "file Nope.toml",
        "make test2",
        "make test3",
        "make indent-check",
        "make no_such_target",
        "flag scripts/test.sh --fix",
        "CI lane lane-2",
    )
}


def skill(name, description="Does a thing.", body="# Skill\n"):
    return f"---\nname: {name}\ndescription: {description}\n---\n\n{body}"


class SkillRefsTests(unittest.TestCase):
    def check(self, refs, extra=()):
        with tempfile.TemporaryDirectory() as repo:
            repo = Path(repo)
            files = dict(extra) | {
                "Makefile": "hooks:\n\ttrue\nindent:\n\ttrue\nfmt_all:\n\ttrue\n"
                "test lint: hooks\n\ttrue\nindent-check:= yes\n",
                ".github/workflows/check.yml": "jobs:\n  gates:\n"
                "    strategy:\n      matrix:\n        include:\n"
                "          - lane: rust\n          - lane: portable\n",
                "scripts/test.sh": "#!/bin/sh\ncase $1 in --fixture) ;; esac\n",
                "src/main.rs": "fn main() {}\n",
                "Cargo.toml": "[package]\n",
                ".claude/skills/codetrial-verify/SKILL.md": skill("codetrial-verify"),
                ".claude/skills/fixture/guide.md": "# Guide\n",
                ".claude/skills/fixture/target.md": GOOD_TARGET,
                ".claude/skills/fixture/refs.md": refs,
            }
            for name, text in files.items():
                (repo / name).parent.mkdir(parents=True, exist_ok=True)
                (repo / name).write_text(text)
            script = repo / "scripts/check-skill-refs.py"
            shutil.copy(SKILL_REFS, script)
            # A rules map is checked against what git tracks.
            subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
            subprocess.run(["git", "add", "."], cwd=repo, check=True)
            return subprocess.run(
                [sys.executable, str(script)], cwd=repo, capture_output=True, text=True
            )

    def test_references_that_resolve_print_nothing(self):
        result = self.check(RESOLVING)
        self.assertEqual((result.returncode, result.stdout), (0, ""), result.stderr)

    def test_each_dangling_reference_is_named_once(self):
        result = self.check(DANGLING)
        self.assertEqual(result.returncode, 1, result.stderr)
        lines = result.stdout.splitlines()
        self.assertEqual(set(lines), DANGLING_REPORT)
        self.assertEqual(len(lines), len(DANGLING_REPORT))

    def test_an_unquoted_description_may_not_hold_a_colon_or_a_comment(self):
        base = ".claude/skills/"
        result = self.check(
            RESOLVING,
            {
                base + "colon/SKILL.md": skill("colon", "Read rust.md: then edit."),
                base + "hash/SKILL.md": skill("hash", "Review PR #N and more."),
                base + "both/SKILL.md": skill("both", "Use it: see #N."),
                # The same hazards on a wrapped plain scalar's second line.
                base + "wrapped/SKILL.md": skill(
                    "wrapped", "Fine first line.\n  Then see #N."
                ),
                base + "wrapcolon/SKILL.md": skill("wrapcolon", "Fine.\n  Then: this."),
                base + "trail/SKILL.md": skill("trail", "Fine.\n  Use it for:"),
                # Spellings a loader reads whole: backticks, quotes, a block
                # scalar, and a colon or a hash with no space before it.
                base + "ticks/SKILL.md": skill("ticks", "Review PR `#N` and more."),
                base + "quoted/SKILL.md": skill("quoted", '"Use it: see #N."'),
                base + "single/SKILL.md": skill("single", "'Use it: see #N.'"),
                base + "folded/SKILL.md": skill("folded", ">\n  Use it: see #N."),
                base + "tight/SKILL.md": skill(
                    "tight", "A url http://x.y/a#b, key:value."
                ),
            },
        )
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(
            result.stdout.splitlines(),
            [
                f"{base}both/SKILL.md: description has a colon and a space; quote it or reword",
                f"{base}both/SKILL.md: description has a space before #, a YAML comment; use backticks",
                f"{base}colon/SKILL.md: description has a colon and a space; quote it or reword",
                f"{base}hash/SKILL.md: description has a space before #, a YAML comment; use backticks",
                f"{base}trail/SKILL.md: description ends with a colon; quote it or reword",
                f"{base}wrapcolon/SKILL.md: description has a colon and a space; quote it or reword",
                f"{base}wrapped/SKILL.md: description has a space before #, a YAML comment; use backticks",
            ],
        )

    def test_frontmatter_must_name_the_directory_and_describe_it(self):
        base = ".claude/skills/"
        result = self.check(
            RESOLVING,
            {
                base + "good/SKILL.md": skill("good", ">\n  Folded\n  text."),
                base + "renamed/SKILL.md": skill("other"),
                base + "Bad_Name/SKILL.md": skill("Bad_Name"),
                base + "mute/SKILL.md": skill("mute", ""),
                base + "bare/SKILL.md": "# No frontmatter\n",
                # Valid YAML spellings: a quoted name and a trailing comment
                # pass, and an empty quoted or comment-only description fails.
                base + "quoted/SKILL.md": skill('"quoted"', "Quoted name."),
                base + "noted/SKILL.md": skill("noted # routing name"),
                base + "dq/SKILL.md": skill("dq", '""'),
                base + "sq/SKILL.md": skill("sq", "''"),
                base + "todo/SKILL.md": skill("todo", "# TODO"),
            },
        )
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(
            result.stdout.splitlines(),
            [
                f"{base}Bad_Name/SKILL.md: name 'Bad_Name' is not lowercase-hyphen",
                f"{base}bare/SKILL.md: frontmatter missing",
                f"{base}dq/SKILL.md: description empty",
                f"{base}mute/SKILL.md: description empty",
                f"{base}renamed/SKILL.md: name other does not match its directory",
                f"{base}sq/SKILL.md: description empty",
                f"{base}todo/SKILL.md: description empty",
            ],
        )

    def test_a_rules_map_names_rules_that_exist_and_paths_that_do(self):
        rules = ".claude/skills/fixture/rules/"
        result = self.check(
            RESOLVING,
            {
                rules + "paths": "# comment\n  # indented\nsrc/* code.md\n"
                "src/* gone.md\nweb/*.mjs code.md\nsrc/* code.md extra.md\n",
                # Rule files sit a level down and are checked like any other.
                rules + "code.md": "# Code\n\n[gone](../missing.md)\n",
            },
        )
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(
            result.stdout.splitlines(),
            [
                f"{rules}code.md: link ../missing.md",
                f"{rules}paths: rule gone.md",
                f"{rules}paths: pattern web/*.mjs",
                f"{rules}paths: line src/* code.md extra.md",
            ],
        )


ROUTING_SPEC = importlib.util.spec_from_file_location(
    "check_skill_routing", ROOT / "scripts" / "check-skill-routing.py"
)
routing = importlib.util.module_from_spec(ROUTING_SPEC)
ROUTING_SPEC.loader.exec_module(routing)


class SkillRoutingTests(unittest.TestCase):
    def test_a_skill_fires_by_invocation_or_by_reading_its_file(self):
        skills = ["codetrial-web", "codetrial-review"]

        def used(name, **args):
            content = [{"type": "tool_use", "name": name, "input": args}]
            return routing.first_skill(
                {"type": "assistant", "message": {"content": content}}, skills
            )

        self.assertEqual(used("Skill", skill="codetrial-web"), "codetrial-web")
        self.assertEqual(used("Skill", skill="proj:codetrial-web"), "codetrial-web")
        path = "/tmp/x/.claude/skills/codetrial-review/lenses.md"
        self.assertEqual(used("Read", file_path=path), "codetrial-review")
        # Another skill, another file, or a user message decides nothing.
        self.assertIsNone(used("Skill", skill="gh-review"))
        self.assertIsNone(used("Read", file_path="/tmp/x/docs/install.md"))
        self.assertIsNone(routing.first_skill({"type": "user"}, skills))

    def test_a_run_that_failed_is_an_error_not_a_near_miss(self):
        skill_use = {
            "type": "assistant",
            "message": {
                "content": [
                    {
                        "type": "tool_use",
                        "name": "Skill",
                        "input": {"skill": "codetrial-web"},
                    }
                ]
            },
        }
        cases = {
            "fired": (json.dumps(skill_use), 0, "codetrial-web"),
            "nothing fired": ('{"type": "result", "is_error": false}', 0, "none"),
            "error result": ('{"type": "result", "is_error": true}', 0, "error"),
            "crashed": ("", 1, "error"),
            "never finished": ('{"type": "system"}', 0, "error"),
        }
        for name, (line, code, want) in cases.items():
            with self.subTest(name), tempfile.TemporaryDirectory() as bin_dir:
                support.write_executable(
                    Path(bin_dir) / "claude",
                    f"#!/bin/sh\nprintf '%s\\n' '{line}'\nexit {code}\n",
                )
                # The fake by path and the timeout as an argument: the gate
                # runs cases in one process at once, so a patched PATH or
                # module global would reach another case, or the real claude.
                fake = str(Path(bin_dir) / "claude")
                got = routing.run_query("q", bin_dir, ["codetrial-web"], 30, fake)
                self.assertEqual(got, want)

    def test_a_claude_that_cannot_start_is_an_error(self):
        self.assertEqual(
            routing.run_query("q", ".", [], 1, "/nonexistent/claude"), "error"
        )

    def test_zero_runs_is_refused_rather_than_passed(self):
        result = subprocess.run(
            [
                sys.executable,
                str(ROOT / "scripts" / "check-skill-routing.py"),
                "--runs",
                "0",
            ],
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("--runs must be at least 1", result.stderr)

    def test_a_run_past_the_timeout_is_an_error(self):
        with tempfile.TemporaryDirectory() as bin_dir:
            support.write_executable(
                Path(bin_dir) / "claude", "#!/bin/sh\nexec sleep 30\n"
            )
            fake = str(Path(bin_dir) / "claude")
            self.assertEqual(routing.run_query("q", bin_dir, [], 0.5, fake), "error")

    def test_the_fixture_carries_the_skills_and_nothing_ignored(self):
        names = routing.fixture_names()
        self.assertIn(".claude/skills/codetrial-verify/SKILL.md", names)
        # No project hook or setting, no build output, and of the env files only
        # the tracked template, never a per-person credential file.
        claude = {n for n in names if n.startswith(".claude/")}
        self.assertTrue(all(n.startswith(".claude/skills/") for n in claude))
        self.assertFalse(any(n.startswith("target/") for n in names))
        env = [n for n in names if n.startswith("config/") and ".env." in n]
        self.assertEqual(env, ["config/codetrial.env.example"])


class MemoryPreparationFlowTests(unittest.TestCase):
    def test_documented_review_flow_handles_empty_interrupted_and_degraded_results(
        self,
    ):
        text = (SKILLS / "codetrial-review/SKILL.md").read_text()
        snippet = next(
            block.split("```", 1)[0]
            for block in text.split("```sh\n")[1:]
            if block.startswith("bundle=")
        )
        triage_text = (SKILLS / "codetrial-triage/review-prs.md").read_text()
        triage_snippet = next(
            block.split("```", 1)[0]
            for block in triage_text.split("```sh\n")[1:]
            if block.lstrip().startswith("history_ok=")
        )
        flows = (("review", snippet), ("triage", triage_snippet))
        cases = [
            ("ready", 0, {"outcome": "ready", "code": "ready"}, True, False),
            ("partial", 2, {"outcome": "degraded", "code": "budget"}, True, False),
            ("busy", 1, {"outcome": "error", "code": "busy"}, True, False),
            ("storage", 1, {"outcome": "error", "code": "storage"}, False, False),
            ("invalid", 2, "invalid", False, False),
            ("interrupted", 130, "", False, False),
            ("multiple", 0, '{"outcome":"ready"} {"outcome":"ready"}', False, False),
            ("wrong-exit", 2, {"outcome": "ready", "code": "ready"}, False, False),
            ("context-failed", 0, {"outcome": "ready", "code": "ready"}, True, True),
        ]
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            (base / "bin").mkdir()
            (base / "review-7").mkdir()
            support.write_executable(
                base / "bin/python3",
                "#!"
                + sys.executable
                + "\n"
                + """import os,sys
from pathlib import Path
if 'ensure' in sys.argv:
    print(os.environ['CASE_JSON'])
    sys.exit(int(os.environ['CASE_EXIT']))
with open(os.environ['CALL_LOG'], 'a') as out:
    out.write(' '.join(sys.argv[1:]) + '\\n')
if os.environ['CONTEXT_FAIL'] == 'true':
    sys.exit(1)
Path(sys.argv[-1], 'history.md').write_text('fixture context')
""",
            )
            for flow, snippet in flows:
                for name, code, value, context_called, context_failed in cases:
                    with self.subTest(flow=flow, case=name):
                        log = base / "calls"
                        if log.exists():
                            log.unlink()
                        env = dict(
                            os.environ,
                            PATH=str(base / "bin") + ":" + os.environ["PATH"],
                            SCRATCH=str(base),
                            dir=str(base / "review-7"),
                            shared_memory_status=str(
                                base / "review-7/memory-status.json"
                            ),
                            shared_memory_exit=str(code),
                            pr="7",
                            CASE_EXIT=str(code),
                            CASE_JSON=json.dumps(value)
                            if isinstance(value, dict)
                            else value,
                            CALL_LOG=str(log),
                            CONTEXT_FAIL=str(context_failed).lower(),
                        )
                        (base / "review-7/memory-status.json").write_text(
                            env["CASE_JSON"]
                        )
                        result = subprocess.run(
                            ["bash", "-e", "-u", "-o", "pipefail", "-c", snippet],
                            env=env,
                            capture_output=True,
                            text=True,
                        )
                        self.assertEqual(result.returncode, 0, result.stderr)
                        self.assertEqual(log.exists(), context_called)
                        if context_called:
                            calls = log.read_text().split()
                            self.assertIn("context", calls)
                            self.assertIn("--no-migrate", calls)
                            self.assertNotIn("experience", calls)
                        history = (base / "review-7/history.md").read_text()
                        self.assertEqual(
                            "fixture context" in history,
                            context_called and not context_failed,
                        )


@unittest.skipUnless(shutil.which("jq"), "jq is not installed")
class ReviewQueueTests(unittest.TestCase):
    """review-pick.sh and review-finish.sh share the ledger in the git dir."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.repo = Path(self.tmp.name)
        subprocess.run(["git", "init", "-q", str(self.repo)], check=True)
        self.ledger = self.repo / ".git" / "codetrial-triage" / "reviewed"

    def pick(self, sweep):
        path = self.repo / "sweep.json"
        path.write_text(json.dumps(sweep))
        result = subprocess.run(
            ["sh", str(REVIEW_PICK), str(path)],
            cwd=self.repo,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return [json.loads(line) for line in result.stdout.splitlines()]

    def finish(self, draft, live, inline_rc=0, gh=None, **env):
        path = self.repo / "draft.json"
        path.write_text(json.dumps(draft))
        with tempfile.TemporaryDirectory() as scratch:
            support.write_executable(Path(scratch) / "gh", gh or FAKE_GH_LIVE)
            # The stand-in for post-inline.sh logs that it was asked to post.
            support.write_executable(Path(scratch) / "inline.sh", FAKE_INLINE_LOG)
            return subprocess.run(
                ["sh", str(REVIEW_FINISH), "7", str(path)],
                cwd=self.repo,
                capture_output=True,
                text=True,
                env=os.environ
                | {
                    "PATH": f"{scratch}:{os.environ['PATH']}",
                    "REVIEW_FINISH_INLINE": f"{scratch}/inline.sh",
                    "FAKE_LIVE": live,
                    "FAKE_INLINE_LOG": f"{self.repo}/inline.log",
                    "FAKE_INLINE_RC": str(inline_rc),
                }
                | env,
            )

    def posted(self):
        log = self.repo / "inline.log"
        return log.read_text() if log.exists() else ""

    def test_brief_reads_a_crashed_or_empty_status_as_unavailable(self):
        pick = {
            "pr": 1,
            "head": "abc",
            "title": "T",
            "why": [],
            "since": None,
            "outdated": [],
            "collide": [],
        }
        (self.repo / "pick.json").write_text(json.dumps(pick))
        for text in ("", "{}", "not json", '{"outcome": null}'):
            (self.repo / "status.json").write_text(text)
            result = subprocess.run(
                [
                    "sh",
                    str(REVIEW_BRIEF),
                    str(self.repo / "pick.json"),
                    "/s",
                    str(self.repo / "status.json"),
                    "1",
                ],
                capture_output=True,
                text=True,
            )
            self.assertIn("Outcome/reason/coverage: unavailable.", result.stdout, text)

    @staticmethod
    def status_entry(pr, grade="low", head="h", author="a", human=0, bot=0):
        return {
            "pr": pr,
            "grade": grade,
            "head": head,
            "author": author,
            "title": "t",
            "why": [],
            "threads": {"human": human, "bot": bot, "by": []},
        }

    def one_pr_sweep(self, human):
        return {
            "viewer": "maint",
            "outdated": [],
            "collide": [],
            "status": [self.status_entry(1, human=human)],
        }

    def test_pick_includes_a_draft_with_an_open_thread(self):
        entry = self.status_entry
        sweep = {
            "viewer": "maint",
            "outdated": [],
            "collide": [],
            "status": [
                entry(1, "draft", human=2),
                entry(2, "draft"),
                entry(3, "medium"),
                entry(4, "medium", bot=1),
                entry(5, "draft", author="maint", human=1),
            ],
        }
        self.assertEqual([p["pr"] for p in self.pick(sweep)], [1, 4])

    def test_pick_rereads_a_head_only_when_open_threads_grew(self):
        self.ledger.parent.mkdir(parents=True, exist_ok=True)
        self.ledger.write_text("1 h 2\n")
        self.assertEqual(self.pick(self.one_pr_sweep(2)), [])
        self.assertEqual(self.pick(self.one_pr_sweep(1)), [])
        self.assertEqual(len(self.pick(self.one_pr_sweep(3))), 1)
        self.ledger.write_text("1 h\n")
        self.assertEqual(self.pick(self.one_pr_sweep(3)), [])
        # Only the newest line for the head counts, so 2 -> 1 -> 2 is growth.
        self.ledger.write_text("1 h 2\n1 h 1\n")
        self.assertEqual(len(self.pick(self.one_pr_sweep(2))), 1)

    def test_finish_records_the_open_thread_count_from_the_sweep(self):
        scratch = self.repo / "scratch"
        scratch.mkdir()
        entry = self.status_entry
        sweep = {
            "status": [
                entry(7, head="old", human=9),
                entry(7, head="abc", human=3, bot=1),
            ]
        }
        (scratch / "sweep.json").write_text(json.dumps(sweep))
        draft = {"pr": 7, "sha": "abc", "comments": [], "similar": [], "resolve": []}
        self.assertEqual(self.finish(draft, "abc", SCRATCH=str(scratch)).returncode, 0)
        self.assertEqual(self.ledger.read_text(), "7 abc 4\n")

    def test_pick_hands_the_newest_reviewed_head_to_the_bundle(self):
        sweep = {
            "viewer": "maint",
            "outdated": [],
            "collide": [],
            "status": [
                {
                    "pr": 1,
                    "grade": "low",
                    "head": "h3",
                    "author": "a",
                    "title": "t",
                    "why": [],
                }
            ],
        }
        self.ledger.parent.mkdir(parents=True)
        self.ledger.write_text("1 h1\n1 h2\n")
        self.assertEqual(self.pick(sweep)[0]["since"], "h2")

    def test_brief_names_the_pr_its_history_and_its_collisions(self):
        pick = {
            "pr": 519,
            "head": "abc",
            "title": "T",
            "why": ["5 open thread(s)"],
            "since": "old",
            "outdated": [{"thread": "T1", "path": "a.js"}],
            "collide": [{"other": 110, "path": "web/x.js", "context": ["f"]}],
        }
        (self.repo / "pick.json").write_text(json.dumps(pick))
        (self.repo / "status.json").write_text(
            json.dumps({"outcome": "degraded", "reason": "busy"})
        )
        result = subprocess.run(
            [
                "sh",
                str(REVIEW_BRIEF),
                str(self.repo / "pick.json"),
                "/s",
                str(self.repo / "status.json"),
                "2",
            ],
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        for want in (
            "PR #519",
            "head abc",
            "shared_memory_exit=2",
            "degraded (busy)",
            "collides with #110 at web/x.js (f)",
            "reviewed head: old",
            "T1 (a.js)",
        ):
            self.assertIn(want.lower(), result.stdout.lower())

    def test_pick_skips_graded_up_prs_the_viewers_and_reviewed_heads(self):
        def row(pr, grade, head, author="someone"):
            return {
                "pr": pr,
                "grade": grade,
                "head": head,
                "author": author,
                "title": f"t{pr}",
                "why": [],
            }

        sweep = {
            "viewer": "maint",
            "status": [
                row(1, "low", "h1"),
                row(2, "high", "h2"),
                row(3, "blocked", "h3", author="maint"),
                row(4, "blocked", "h4new"),
                row(5, "low", "h5"),
            ],
            "outdated": [{"pr": 1, "thread": "T1", "path": "a.js"}],
            "collide": [{"a": 1, "b": 5, "path": "web/x.js", "context": ["f"]}],
        }
        self.ledger.parent.mkdir(parents=True)
        self.ledger.write_text("4 h4old\n5 h5\n")
        picked = self.pick(sweep)
        self.assertEqual([p["pr"] for p in picked], [1, 4])
        self.assertIsNone(picked[0]["since"])
        self.assertEqual(picked[0]["outdated"], [{"thread": "T1", "path": "a.js"}])
        self.assertEqual(picked[0]["collide"][0]["other"], 5)
        self.assertEqual(picked[1]["since"], "h4old")

    def test_finish_records_the_head_only_when_nothing_is_left(self):
        draft = {"pr": 7, "sha": "abc", "comments": [], "similar": [], "resolve": []}
        self.assertEqual(self.finish(draft, "abc").returncode, 0)
        self.assertEqual(self.ledger.read_text(), "7 abc\n")

    def refine_draft(self, refine):
        return {
            "pr": 7,
            "sha": "abc",
            "comments": [],
            "similar": [],
            "resolve": [],
            "refine": refine,
        }

    def finish_refine(self, refine, **env):
        env = {"FAKE_HISTORY": "", "FAKE_LOG": f"{self.repo}/gh.log"} | env
        return self.finish(self.refine_draft(refine), "abc", gh=FAKE_GH_REFINE, **env)

    def test_finish_posts_the_refine_comment_with_every_listed_thread(self):
        threads = [{"url": "https://github.com/o/r/pull/7#c1", "path": "a.js"}]
        self.assertEqual(self.finish_refine(threads).returncode, 0)
        log = (self.repo / "gh.log").read_text()
        self.assertIn("refine the change", log)
        self.assertIn("pull/7#c1", log)
        self.assertIn("Checked at abc.", log)
        self.assertNotIn("DELETE", log)
        self.assertEqual(self.ledger.read_text(), "7 abc\n")

    def test_finish_deletes_the_stale_copies_once_the_new_reminder_is_posted(self):
        text = (SKILLS / "codetrial-triage" / "refine.md").read_text()
        history = json.dumps(
            [
                {"id": 11, "user": {"login": "viewer"}, "body": text + "\nOld list"},
                {"id": 12, "user": {"login": "someone"}, "body": text},
                {"id": 13, "user": {"login": "viewer"}, "body": "Thanks!"},
            ]
        )
        threads = [{"url": "https://github.com/o/r/pull/7#c1", "path": "a.js"}]
        self.assertEqual(
            self.finish_refine(threads, FAKE_HISTORY=history).returncode, 0
        )
        log = (self.repo / "gh.log").read_text()
        self.assertIn("issues/comments/11", log)
        self.assertNotIn("issues/comments/12", log)
        self.assertNotIn("issues/comments/13", log)
        self.assertLess(log.index("comment"), log.index("issues/comments/11"))

        # A reminder that did not post leaves the old one standing.
        (self.repo / "gh.log").unlink()
        failed = self.finish_refine(
            threads, FAKE_HISTORY=history, FAKE_COMMENT_FAIL="1"
        )
        self.assertEqual(failed.returncode, 4)
        self.assertFalse((self.repo / "gh.log").exists())

    def test_finish_retry_keeps_the_identical_reminder_and_clears_the_rest(self):
        text = (SKILLS / "codetrial-triage" / "refine.md").read_text()
        current = (
            text
            + "\nNot addressed at the current head:\n"
            + "- https://github.com/o/r/pull/7#c1 (`a.js`)\n"
            + "\nChecked at abc.\n"
        )
        history = json.dumps(
            [
                {"id": 20, "user": {"login": "viewer"}, "body": current},
                {"id": 21, "user": {"login": "viewer"}, "body": text + "\nOld"},
            ]
        )
        threads = [{"url": "https://github.com/o/r/pull/7#c1", "path": "a.js"}]
        self.assertEqual(
            self.finish_refine(threads, FAKE_HISTORY=history).returncode, 0
        )
        log = (self.repo / "gh.log").read_text()
        self.assertNotIn("comment\n", log)
        self.assertIn("issues/comments/21", log)
        self.assertNotIn("issues/comments/20", log)

    def test_finish_keeps_one_identical_reminder_and_deletes_a_second(self):
        text = (SKILLS / "codetrial-triage" / "refine.md").read_text()
        current = (
            text
            + "\nNot addressed at the current head:\n"
            + "- https://github.com/o/r/pull/7#c1 (`a.js`)\n"
            + "\nChecked at abc.\n"
        )
        history = json.dumps(
            [
                {"id": 30, "user": {"login": "viewer"}, "body": current},
                {"id": 31, "user": {"login": "viewer"}, "body": current},
            ]
        )
        threads = [{"url": "https://github.com/o/r/pull/7#c1", "path": "a.js"}]
        self.assertEqual(
            self.finish_refine(threads, FAKE_HISTORY=history).returncode, 0
        )
        log = (self.repo / "gh.log").read_text()
        self.assertIn("issues/comments/31", log)
        self.assertNotIn("issues/comments/30", log)
        self.assertNotIn("comment\n", log)

    def test_finish_keeps_the_head_out_when_a_stale_copy_cannot_be_deleted(self):
        text = (SKILLS / "codetrial-triage" / "refine.md").read_text()
        history = json.dumps(
            [{"id": 40, "user": {"login": "viewer"}, "body": text + "\nOld"}]
        )
        threads = [{"url": "https://github.com/o/r/pull/7#c1", "path": "a.js"}]
        result = self.finish_refine(threads, FAKE_HISTORY=history, FAKE_DELETE_FAIL="1")
        self.assertEqual(result.returncode, 4)
        self.assertFalse(self.ledger.exists() and self.ledger.read_text())

    def test_finish_asks_the_author_to_resolve_an_addressed_thread(self):
        threads = [
            {"url": "https://github.com/o/r/pull/7#c1", "path": "a.js"},
            {
                "url": "https://github.com/o/r/pull/7#c2",
                "path": "b.js",
                "kind": "resolve",
            },
        ]
        self.assertEqual(self.finish_refine(threads).returncode, 0)
        log = (self.repo / "gh.log").read_text()
        self.assertIn("resolve", log.split("Not addressed")[0])
        self.assertLess(log.index("#c1"), log.index("reply with what changed"))
        self.assertLess(log.index("reply with what changed"), log.index("#c2"))

    def test_finish_keeps_the_head_out_when_the_refine_comment_fails(self):
        threads = [{"url": "https://github.com/o/r/pull/7#c1", "path": "a.js"}]
        for fail in ("FAKE_COMMENT_FAIL", "FAKE_API_FAIL"):
            result = self.finish_refine(threads, **{fail: "1"})
            self.assertEqual(result.returncode, 4, fail)
            self.assertFalse(self.ledger.exists() and self.ledger.read_text(), fail)

    def test_finish_refuses_a_malformed_refine_before_posting(self):
        for refine in (
            "x",
            [{}],
            [{"url": "u"}],
            [{"url": 1, "path": "p"}],
            [{"url": "u", "path": "p", "kind": "other"}],
        ):
            result = self.finish_refine(refine)
            self.assertEqual(result.returncode, 2, refine)
            self.assertFalse((self.repo / "gh.log").exists())

    def test_finish_refuses_a_moved_head_and_posts_nothing(self):
        draft = {
            "pr": 7,
            "sha": "abc",
            "comments": [{"path": "a"}],
            "similar": [],
            "resolve": [],
        }
        self.assertEqual(self.finish(draft, "def").returncode, 3)
        self.assertEqual(self.posted(), "")
        self.assertFalse(self.ledger.exists())

    def test_finish_posts_the_inline_comments_and_records_only_after_they_land(self):
        draft = {
            "pr": 7,
            "sha": "abc",
            "comments": [{"path": "a"}],
            "similar": [],
            "resolve": [],
        }
        self.assertEqual(self.finish(draft, "abc").returncode, 0)
        self.assertIn("7 ", self.posted())
        self.assertEqual(self.ledger.read_text(), "7 abc\n")
        self.ledger.unlink()
        result = self.finish(draft, "abc", inline_rc=1)
        self.assertEqual(result.returncode, 1)
        self.assertFalse(self.ledger.exists())

    def test_finish_refuses_a_sha_that_would_split_into_other_fields(self):
        draft = {
            "pr": 7,
            "sha": "abc 0 0",
            "comments": [{"path": "a"}],
            "similar": [],
            "resolve": [],
        }
        result = self.finish(draft, "abc 0 0")
        self.assertEqual(result.returncode, 2)
        self.assertEqual(self.posted(), "")
        self.assertFalse(self.ledger.exists())

    def test_finish_leaves_a_thread_to_resolve_to_the_caller(self):
        draft = {
            "pr": 7,
            "sha": "abc",
            "comments": [],
            "similar": [],
            "resolve": [{"thread": "T1", "path": "a.js", "why": "fixed"}],
        }
        result = self.finish(draft, "abc")
        self.assertEqual(result.returncode, 4)
        self.assertIn("T1", result.stdout)
        self.assertFalse(self.ledger.exists())

    def test_finish_refuses_unpostable_notices_before_posting_anything(self):
        draft = {
            "pr": 7,
            "sha": "abc",
            "comments": [{"path": "a"}],
            "resolve": [],
            "similar": [
                {
                    "kind": "pr",
                    "number": 8,
                    "notice": "see #8",
                    "other_notice": "see #7",
                    "other_sha": "h8",
                }
            ],
        }
        # No SCRATCH for the pair ledger, then a notice that names no thread.
        for env, similar in (
            ({"SCRATCH": ""}, draft["similar"]),
            (
                {"SCRATCH": str(self.repo)},
                [draft["similar"][0] | {"notice": "no link"}],
            ),
        ):
            result = self.finish(draft | {"similar": similar}, "abc", **env)
            self.assertEqual(result.returncode, 2, env)
            self.assertEqual(self.posted(), "")
            self.assertFalse(self.ledger.exists())

    def test_finish_refuses_a_malformed_draft_before_recording_anything(self):
        good = {"pr": 7, "sha": "abc", "comments": [], "similar": [], "resolve": []}
        for field, value in (
            ("similar", True),
            ("comments", "x"),
            ("pr", 8),
            ("sha", None),
        ):
            result = self.finish(good | {field: value}, "abc")
            self.assertEqual(result.returncode, 2, field)
            self.assertFalse(self.ledger.exists(), field)


@unittest.skipUnless(shutil.which("jq"), "jq is not installed")
class ApplyPrActionsTests(unittest.TestCase):
    """apply-pr-actions.sh against a gh that logs every write it is asked for."""

    FAKE = FAKE_GH_APPLY
    QUIET = {"approve": [], "rebase": [], "hide": []}

    def sweep(self, **fields):
        base = {
            "maintainers_known": True,
            "prs": [{"number": n, "head": f"h{n}"} for n in (1, 2, 3, 4)],
            "approve": [
                self.approval()["approve"][0],
                self.approval(pr=2, run=12, first_time=False)["approve"][0],
            ],
            "rebase": [{"pr": 3, "reason": "conflicts"}],
            "hide": [
                {"pr": 3, "id": "IC_3", "by": "maint"},
                {"pr": 4, "id": "IC_4", "by": "maint"},
            ],
            "hide_resolved": [],
            "refine_stale": [],
        }
        return json.dumps(base | fields)

    def run_actions(self, sweep, workflows=None, repo=None, hygiene=None, **env):
        with tempfile.TemporaryDirectory() as scratch:
            scratch = Path(scratch)
            (scratch / "bin").mkdir()
            support.write_executable(scratch / "bin" / "gh", self.FAKE)
            (scratch / "sweep.json").write_text(sweep)
            (scratch / "log").write_text("")
            # A workflow directory the guard reads; None leaves the real one.
            if workflows is not None:
                (scratch / "wf").mkdir()
                for name, text in workflows.items():
                    (scratch / "wf" / name).write_text(text)
                env["WORKFLOWS_DIR"] = str(scratch / "wf")
            # A hygiene.sh stand-in: $1 is the PR, and it prints that PR's result.
            if hygiene is not None:
                (scratch / "hygiene.sh").write_text(hygiene)
                env["APPLY_HYGIENE"] = str(scratch / "hygiene.sh")
            result = subprocess.run(
                ["sh", str(APPLY_ACTIONS), str(scratch / "sweep.json")]
                + ([repo] if repo else [])
                + (["--hygiene"] if hygiene is not None else []),
                capture_output=True,
                text=True,
                env=os.environ
                | {
                    "PATH": f"{scratch / 'bin'}:{os.environ['PATH']}",
                    "FAKE_LOG": str(scratch / "log"),
                }
                | env,
            )
            log = (scratch / "log").read_text()
        receipts = [json.loads(line) for line in result.stdout.splitlines()]
        return result, {(r["action"], r["pr"]): r["result"] for r in receipts}, log

    def approval(self, **fields):
        entry = {
            "pr": 1,
            "run": 11,
            "first_time": True,
            "touches_workflows": False,
            "files_truncated": False,
        }
        return {"approve": [entry | fields]}

    HYGIENE = (
        'case "$1" in\n'
        '  1) echo \'{"pr":1,"head":"h1","items":[{"key":"indent"}],"body":"fix"}\' ;;\n'
        '  2) echo \'{"pr":2,"head":"h2","items":[],"body":""}\' ;;\n'
        '  3) echo \'{"pr":3,"head":"h3","items":[{"key":"k"}],"body":"old"}\' ;;\n'
        "  *) exit 1 ;;\n"
        "esac\n"
    )

    def test_hygiene_reviews_post_only_a_body_read_at_the_live_head(self):
        quiet = self.QUIET
        result, got, log = self.run_actions(
            self.sweep(**quiet), hygiene=self.HYGIENE, FAKE_MOVED="3"
        )
        self.assertEqual(
            got,
            {
                ("hygiene", 1): "applied",
                ("hygiene", 3): "skipped",
                ("hygiene", 4): "failed",
            },
        )
        # The empty body of PR 2 posts nothing, and a failed read exits 1.
        self.assertEqual(result.returncode, 1)
        self.assertIn("pulls/1/reviews -f commit_id=h1", log)
        self.assertNotIn("pulls/2/reviews", log)
        self.assertNotIn("pulls/3/reviews", log)

    def test_hygiene_hides_the_replaced_copies_after_the_new_review_is_up(self):
        old = '"hide":[{"id":"PRR_old","key":"indent","why":"stale"}]'
        hygiene = (
            'case "$1" in\n'
            f'  1) echo \'{{"pr":1,"head":"h1","items":[{{"key":"indent"}}],"body":"fix",{old}}}\' ;;\n'
            f'  2) echo \'{{"pr":2,"head":"h2","items":[],"body":"",{old}}}\' ;;\n'
            f'  3) echo \'{{"pr":3,"head":"h3","items":[],"body":"",{old}}}\' ;;\n'
            "  *) exit 1 ;;\n"
            "esac\n"
        )
        result, got, log = self.run_actions(
            self.sweep(**self.QUIET), hygiene=hygiene, FAKE_MOVED="3"
        )
        # PR 1 posts and then hides; PR 2 has nothing new and only hides; PR 3
        # moved after it was read and is left alone.
        self.assertEqual(got[("hygiene", 1)], "applied")
        self.assertEqual(got[("hygiene_hide", 1)], "applied")
        self.assertEqual(got[("hygiene_hide", 2)], "applied")
        self.assertNotIn(("hygiene_hide", 3), got)
        self.assertEqual(log.count("subjectId: $id"), 2)
        self.assertLess(log.index("pulls/1/reviews"), log.index("minimizeComment"))

    def test_hygiene_keeps_the_old_copy_when_the_new_review_is_not_posted(self):
        hygiene = (
            'case "$1" in\n'
            '  1) echo \'{"pr":1,"head":"h1","items":[{"key":"indent"}],"body":"fix",'
            '"hide":[{"id":"PRR_old","key":"indent","why":"stale"}]}\' ;;\n'
            "  *) exit 1 ;;\n"
            "esac\n"
        )
        result, got, log = self.run_actions(
            self.sweep(**self.QUIET), hygiene=hygiene, FAKE_MOVED="1"
        )
        self.assertEqual(got[("hygiene", 1)], "skipped")
        self.assertNotIn(("hygiene_hide", 1), got)
        self.assertNotIn("minimizeComment", log)

    def test_a_hygiene_comment_is_posted_as_a_comment_at_the_live_head(self):
        quiet = self.QUIET
        shot = (
            'echo \'{"pr":%s,"head":"h%s","items":[],"body":"",'
            '"comments":[{"key":"screenshot","text":"Add screenshots now."}]}\' ;;\n'
        )
        hygiene = 'case "$1" in\n  1) ' + shot % (1, 1) + "  3) " + shot % (3, 3)
        hygiene += "  *) exit 1 ;;\nesac\n"
        _, got, log = self.run_actions(
            self.sweep(**quiet), hygiene=hygiene, FAKE_MOVED="3"
        )
        self.assertEqual(got[("hygiene_screenshot", 1)], "applied")
        self.assertEqual(got[("hygiene_screenshot", 3)], "skipped")
        self.assertIn("pr comment 1 ", log)
        self.assertIn("Add screenshots now.", log)
        self.assertNotIn("pr comment 3 ", log)
        self.assertNotIn("pulls/1/reviews", log)

    def test_a_merged_pr_of_a_contributor_is_thanked_once(self):
        def merged(number, login, body=None, bot=False):
            return {
                "number": number,
                "author": {"login": login, "is_bot": bot},
                "comments": [{"author": {"login": "maint"}, "body": body}]
                if body
                else [],
            }

        quiet = self.QUIET
        listing = [
            merged(9, "newbie"),
            merged(10, "thanked", "Thanks @thanked, merged."),
            merged(13, "later", "Please thank @later when it lands."),
            merged(11, "maint"),
            merged(12, "app/dependabot", bot=True),
        ]
        _, got, log = self.run_actions(
            self.sweep(**quiet),
            hygiene=self.HYGIENE,
            FAKE_MERGED=json.dumps(listing),
        )
        thanks = {pr for (action, pr) in got if action == "thanks"}
        self.assertEqual(thanks, {9, 13})
        self.assertIn("Thank @newbie for contributing!", log)
        self.assertNotIn("pr comment 10 ", log)

    def test_resolved_reviews_are_hidden_and_a_stale_refine_comment_deleted(self):
        quiet = self.QUIET
        _, got, log = self.run_actions(
            self.sweep(
                **quiet,
                hide_resolved=[{"pr": 2, "id": "PRR_2", "by": "cubic-dev-ai"}],
                refine_stale=[{"pr": 2, "id": "IC_9"}],
            )
        )
        self.assertEqual(
            got, {("hide_resolved", 2): "applied", ("refine_stale", 2): "applied"}
        )
        self.assertIn("PRR_2", log)
        self.assertIn("classifier: RESOLVED", log)
        self.assertIn("deleteIssueComment", log)

    def test_a_write_that_timed_out_is_tried_again_before_it_is_reported(self):
        quiet = self.QUIET
        _, got, log = self.run_actions(
            self.sweep(**quiet, hide_resolved=[{"pr": 2, "id": "PRR_2"}]),
            FAKE_FLAKY="1",
            RETRY_DELAY="0",
        )
        self.assertEqual(got, {("hide_resolved", 2): "applied"})
        self.assertEqual(log.count("classifier: RESOLVED"), 2)

    def test_each_review_is_checked_again_after_a_previous_hide(self):
        def state(reopened):
            threads = [
                {
                    "isResolved": not (reopened and review == "PRR_3"),
                    "isOutdated": False,
                    "comments": {
                        "totalCount": 1,
                        "nodes": [{"pullRequestReview": {"id": review}}],
                    },
                }
                for review in ("PRR_2", "PRR_3")
            ]
            return json.dumps(
                {
                    "data": {
                        "repository": {
                            "pullRequest": {
                                "reviewThreads": {"totalCount": 2, "nodes": threads}
                            }
                        }
                    }
                }
            )

        result, _, log = self.run_actions(
            self.sweep(
                **self.QUIET,
                hide_resolved=[{"pr": 2, "id": "PRR_2"}, {"pr": 2, "id": "PRR_3"}],
            ),
            FAKE_THREADS=state(False),
            FAKE_THREADS_AFTER_WRITE=state(True),
        )
        receipts = [json.loads(line) for line in result.stdout.splitlines()]
        self.assertEqual([r["result"] for r in receipts], ["applied", "skipped"])
        self.assertEqual(log.count("classifier: RESOLVED"), 1)

    def test_an_entry_the_sweep_marks_delete_is_deleted_and_the_rest_hidden(self):
        def entry(pr, node, act):
            return {"pr": pr, "id": node, "by": "maint", "act": act}

        hide = [
            entry(1, "IC_mine", "delete"),
            entry(2, "IC_theirs", "hide"),
            entry(3, "R_mine", "hide"),
        ]
        _, got, log = self.run_actions(self.sweep(**(self.QUIET | {"hide": hide})))
        self.assertEqual(set(got.values()), {"applied"})
        self.assertEqual(log.count("deleteIssueComment"), 1)
        self.assertRegex(
            log, r"id=IC_mine -f query=\s+mutation\(\$id: ID!\) \{ deleteIssueComment"
        )
        self.assertEqual(log.count("classifier: OUTDATED"), 2)

    def test_a_node_gone_after_a_timed_out_attempt_counts_as_done(self):
        _, got, _ = self.run_actions(
            self.sweep(**self.QUIET, refine_stale=[{"pr": 2, "id": "IC_9"}]),
            FAKE_FLAKY="1",
            FAKE_GONE="1",
            RETRY_DELAY="0",
        )
        self.assertEqual(got, {("refine_stale", 2): "applied"})

    def test_a_refused_write_is_not_tried_again(self):
        _, got, log = self.run_actions(
            self.sweep(**self.QUIET, hide_resolved=[{"pr": 2, "id": "PRR_2"}]),
            FAKE_REFUSED="1",
            RETRY_DELAY="0",
        )
        self.assertEqual(got, {("hide_resolved", 2): "failed"})
        self.assertEqual(log.count("classifier: RESOLVED"), 1)

    def test_changed_or_incomplete_threads_prevent_mutations(self):
        quiet = self.QUIET
        thread = {
            "isResolved": False,
            "isOutdated": False,
            "comments": {
                "totalCount": 1,
                "nodes": [{"pullRequestReview": {"id": "PRR_2"}}],
            },
        }
        for total in (1, 2):
            state = {
                "data": {
                    "repository": {
                        "pullRequest": {
                            "reviewThreads": {"totalCount": total, "nodes": [thread]}
                        }
                    }
                }
            }
            _, got, log = self.run_actions(
                self.sweep(
                    **quiet,
                    hide_resolved=[{"pr": 2, "id": "PRR_2"}],
                    refine_stale=[{"pr": 2, "id": "IC_9"}],
                ),
                FAKE_THREADS=json.dumps(state),
            )
            self.assertEqual(
                got, {("hide_resolved", 2): "skipped", ("refine_stale", 2): "skipped"}
            )
            self.assertNotIn("classifier: RESOLVED", log)
            self.assertNotIn("deleteIssueComment", log)

    def test_hygiene_is_off_without_the_flag(self):
        quiet = self.QUIET
        _, got, log = self.run_actions(self.sweep(**quiet))
        self.assertEqual((got, "reviews" in log), ({}, False))

    def test_an_approval_needs_each_guard_to_hold(self):
        for field, value in (
            ("touches_workflows", True),
            ("files_truncated", True),
            ("first_time", False),
        ):
            _, got, log = self.run_actions(
                self.sweep(**self.approval(**{field: value}))
            )
            self.assertEqual(got[("approve", 1)], "confirm", field)
            self.assertNotIn("/approve", log, field)

    def test_a_workflow_on_a_base_token_trigger_confirms_every_approval(self):
        for trigger in ("pull_request_target", "workflow_run"):
            _, got, log = self.run_actions(
                self.sweep(**self.approval()), workflows={"w.yml": f"on: {trigger}\n"}
            )
            self.assertEqual(got[("approve", 1)], "confirm", trigger)
            self.assertNotIn("/approve", log, trigger)

    def test_the_reviewed_description_check_does_not_confirm_every_approval(self):
        # pr-description.yml runs on pull_request_target on purpose: the base
        # branch's copy, a read-only token, the pull request only as data.
        reviewed = {"pr-description.yml": "on:\n  pull_request_target:\n"}
        _, got, log = self.run_actions(
            self.sweep(**self.approval()), workflows=reviewed
        )
        self.assertEqual(got[("approve", 1)], "applied")
        # Any other workflow on that trigger still does, alongside it.
        _, got, log = self.run_actions(
            self.sweep(**self.approval()),
            workflows={**reviewed, "other.yml": "on: pull_request_target\n"},
        )
        self.assertEqual(got[("approve", 1)], "confirm")
        self.assertNotIn("/approve", log)
        # The review covered one trigger; the same name on another is no exception.
        _, got, _ = self.run_actions(
            self.sweep(**self.approval()),
            workflows={"pr-description.yml": "on: workflow_run\n"},
        )
        self.assertEqual(got[("approve", 1)], "confirm")

    def test_workflows_that_cannot_be_inspected_confirm_the_approval(self):
        _, got, _ = self.run_actions(self.sweep(**self.approval()), workflows={})
        self.assertEqual(got[("approve", 1)], "confirm")
        _, got, log = self.run_actions(
            self.sweep(**self.approval()),
            workflows={"w.yml": "on: push\n"},
            repo="other/repo",
        )
        self.assertEqual(got[("approve", 1)], "confirm")
        self.assertNotIn("/approve", log)

    @unittest.skipIf(os.geteuid() == 0, "root reads an unreadable file")
    def test_a_workflow_directory_grep_cannot_read_confirms_the_approval(self):
        # An unreadable file makes grep exit 2, which is no answer either way.
        with tempfile.TemporaryDirectory() as wf:
            unreadable = Path(wf) / "w.yml"
            unreadable.write_text("on: pull_request\n")
            unreadable.chmod(0)
            try:
                _, got, log = self.run_actions(
                    self.sweep(**self.approval()), WORKFLOWS_DIR=wf
                )
            finally:
                unreadable.chmod(0o644)
        self.assertEqual(got[("approve", 1)], "confirm")
        self.assertNotIn("/approve", log)

    def test_a_hide_with_no_author_evidence_is_left_to_confirm(self):
        _, got, log = self.run_actions(self.sweep(hide=[{"pr": 4, "id": "IC_4"}]))
        self.assertEqual(got[("hide", 4)], "confirm")
        self.assertNotIn("minimizeComment", log)

    def test_clean_workflows_leave_the_approval_to_the_sweep(self):
        _, got, _ = self.run_actions(
            self.sweep(**self.approval()), workflows={"w.yml": "on: pull_request\n"}
        )
        self.assertEqual(got[("approve", 1)], "applied")

    def test_a_sweep_of_the_wrong_shape_writes_nothing_and_exits_2(self):
        bad = (
            self.approval(first_time="false"),
            {"approve": [{"pr": 1, "run": 11, "first_time": True}]},
            {"rebase": None},
            {"rebase": [{"pr": 3}]},
            {"rebase": [{"pr": 3, "reason": "confict"}]},
            {"hide": [{"pr": 3}]},
            {"maintainers_known": None},
        )
        for fields in bad:
            result, got, log = self.run_actions(self.sweep(**fields))
            self.assertEqual((result.returncode, got), (2, {}), fields)
            self.assertEqual(log, "", fields)

    def test_applies_what_a_sweep_may_and_leaves_the_rest_to_confirm(self):
        result, got, log = self.run_actions(self.sweep())
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            got,
            {
                ("approve", 1): "applied",
                ("approve", 2): "confirm",
                ("rebase", 3): "applied",
                ("hide", 3): "applied",
                ("hide", 4): "applied",
            },
        )
        self.assertIn("actions/runs/11/approve", log)
        self.assertNotIn("actions/runs/12/approve", log)

    def test_a_moved_head_posts_nothing_and_keeps_the_old_request_visible(self):
        result, got, log = self.run_actions(self.sweep(), FAKE_VIEW="CONFLICTING h9")
        self.assertEqual(got[("rebase", 3)], "skipped")
        self.assertEqual(got[("hide", 3)], "skipped")
        self.assertEqual(got[("hide", 4)], "applied")
        self.assertNotIn("pulls/3/reviews", log)
        result, got, _ = self.run_actions(self.sweep(), FAKE_VIEW="MERGEABLE h3")
        self.assertEqual(got[("rebase", 3)], "skipped")

    def test_a_run_approved_meanwhile_is_not_a_failure(self):
        result, got, _ = self.run_actions(self.sweep(), FAKE_NOT_WAITING="1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(got[("approve", 1)], "already")

    def test_unknown_maintainers_require_confirmation_before_hiding(self):
        result, got, log = self.run_actions(self.sweep(maintainers_known=False))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(got[("hide", 3)], "confirm")
        self.assertEqual(got[("hide", 4)], "confirm")
        self.assertNotIn("minimizeComment", log)

    def test_a_run_no_longer_waiting_needs_a_verified_running_status(self):
        for status in ("queued", "completed", "unread"):
            with self.subTest(status=status):
                result, got, log = self.run_actions(
                    self.sweep(), FAKE_NOT_WAITING="1", FAKE_RUN_STATUS=status
                )
                self.assertIn("run view 11", log)
                self.assertEqual(
                    got[("approve", 1)], "already" if status == "queued" else "failed"
                )
                self.assertEqual(result.returncode, 0 if status == "queued" else 1)


@unittest.skipUnless(shutil.which("jq"), "jq is not installed")
class PostSimilarTests(unittest.TestCase):
    """post-similar.sh: both threads read first, heads checked, pairs once."""

    FAKE = FAKE_GH_SIMILAR

    def run_similar(
        self,
        views,
        similar,
        preposted="",
        shell="sh",
        after=None,
        claimed=None,
        live=None,
    ):
        draft = {
            "pr": 7,
            "sha": "h7",
            "comments": [],
            "resolve": [],
            "similar": similar,
        }
        with tempfile.TemporaryDirectory() as scratch:
            scratch = Path(scratch)
            (scratch / "bin").mkdir()
            support.write_executable(scratch / "bin" / "gh", self.FAKE)
            for number, view in views.items():
                view = {"body": "", "comments": []} | view
                (scratch / f"{number}.json").write_text(json.dumps(view))
            (scratch / "draft.json").write_text(json.dumps(draft))
            (scratch / "similar-posted").write_text(preposted)
            if claimed:
                (scratch / f"similar-posted.claim-{claimed}").mkdir()
            # A push to PR b that lands once a comment on PR a has posted.
            for (a, b), view in (after or {}).items():
                (scratch / f"after-{a}-{b}").write_text(json.dumps(view))
            # A head that moved after its view was read, though the view says h.
            for number, head in (live or {}).items():
                (scratch / f"{number}.live").write_text(head + "\n")
            result = subprocess.run(
                [shell, str(POST_SIMILAR), "7", str(scratch / "draft.json")],
                capture_output=True,
                text=True,
                env=os.environ
                | {
                    "PATH": f"{scratch / 'bin'}:{os.environ['PATH']}",
                    "FAKE_DIR": str(scratch),
                    "SCRATCH": str(scratch),
                },
            )
            posted = scratch / "posted"
            posted = posted.read_text().split() if posted.exists() else []
            pairs = (scratch / "similar-posted").read_text()
            # Claims the run left behind, which only a posted pair keeps.
            self.claims = [c.name for c in scratch.glob("similar-posted.claim-*")]
            calls = scratch / "calls"
            self.calls = calls.read_text().splitlines() if calls.exists() else []
        receipts = [json.loads(line)["result"] for line in result.stdout.splitlines()]
        return result.returncode, receipts, posted, pairs

    def entry(self, **fields):
        return {
            "kind": "pr",
            "number": 8,
            "notice": "Overlaps #8.",
            "other_notice": "Overlaps #7.",
            "other_sha": "h8",
        } | fields

    def test_each_thread_is_decided_on_its_own_and_the_pair_recorded(self):
        views = {
            7: {"headRefOid": "h7", "comments": [{"body": "see #8"}]},
            8: {"headRefOid": "h8"},
        }
        code, receipts, posted, pairs = self.run_similar(views, [self.entry()])
        # #7 already links #8, so only the mirror goes to #8.
        self.assertEqual((code, receipts, posted), (0, ["applied"], ["8"]))
        self.assertEqual(pairs, "7 8\n")

    def test_a_linked_pair_and_a_pair_already_posted_post_nothing(self):
        views = {
            7: {"headRefOid": "h7", "body": "#8"},
            8: {"headRefOid": "h8", "comments": [{"body": "x #7."}]},
        }
        code, receipts, posted, _ = self.run_similar(views, [self.entry()])
        self.assertEqual((code, receipts, posted), (0, ["linked"], []))
        views = {7: {"headRefOid": "h7"}, 8: {"headRefOid": "h8"}}
        code, receipts, posted, _ = self.run_similar(
            views, [self.entry()], preposted="7 8\n"
        )
        self.assertEqual((code, receipts, posted), (0, ["pair"], []))

    def test_a_number_inside_a_longer_one_is_not_a_link(self):
        views = {7: {"headRefOid": "h7", "body": "closes #80"}, 8: {"headRefOid": "h8"}}
        _, receipts, posted, _ = self.run_similar(views, [self.entry()])
        self.assertEqual((receipts, posted), (["applied"], ["7", "8"]))

    def test_a_moved_head_on_either_side_posts_nothing_and_exits_1(self):
        for moved in (7, 8):
            views = {7: {"headRefOid": "h7"}, 8: {"headRefOid": "h8"}}
            views[moved] = {"headRefOid": "new"}
            code, receipts, posted, pairs = self.run_similar(views, [self.entry()])
            self.assertEqual((code, receipts, posted, pairs), (1, ["moved"], [], ""))
            # A pair that leaves without a post gives its claim back.
            self.assertEqual(self.claims, [], moved)

    def test_an_issue_notice_goes_to_this_pr_only(self):
        views = {7: {"headRefOid": "h7"}}
        issue = {"kind": "issue", "number": 5, "notice": "Delivers #5."}
        code, receipts, posted, _ = self.run_similar(views, [issue])
        self.assertEqual((code, receipts, posted), (0, ["applied"], ["7"]))

    def test_missing_required_mirror_notice_keeps_the_pair_pending(self):
        views = {7: {"headRefOid": "h7"}, 8: {"headRefOid": "h8"}}
        for notice in (None, "", "   "):
            with self.subTest(notice=notice):
                code, receipts, posted, pairs = self.run_similar(
                    views, [self.entry(other_notice=notice)]
                )
                self.assertEqual((code, receipts, pairs), (1, ["failed"], ""))
                self.assertNotIn("8", posted)
                # A pair that leaves without a post gives its claim back.
                self.assertEqual(self.claims, [])

    def test_missing_mirror_text_is_allowed_when_other_thread_already_links(self):
        views = {
            7: {"headRefOid": "h7"},
            8: {"headRefOid": "h8", "body": "see #7"},
        }
        code, receipts, posted, pairs = self.run_similar(
            views, [self.entry(other_notice=None)]
        )
        self.assertEqual(
            (code, receipts, posted, pairs), (0, ["applied"], ["7"], "7 8\n")
        )

    @unittest.skipUnless(shutil.which("dash"), "dash is not installed")
    def test_the_pair_key_is_the_same_whichever_side_the_draft_is_for(self):
        # The ledger holds the pair as "3 9"; a draft for PR 9 must find it.
        views = {7: {"headRefOid": "h7"}, 3: {"headRefOid": "h3"}}
        entry = self.entry(number=3, notice="Overlaps #3.", other_sha="h3")
        code, receipts, posted, _ = self.run_similar(views, [entry], preposted="3 7\n")
        self.assertEqual((code, receipts, posted), (0, ["pair"], []))

    def test_a_draft_whose_notices_do_not_name_the_other_thread_is_refused(self):
        views = {7: {"headRefOid": "h7"}, 8: {"headRefOid": "h8"}}
        for fields in ({"notice": "no number"}, {"other_notice": "see #9"}):
            code, receipts, posted, pairs = self.run_similar(
                views, [self.entry(**fields)]
            )
            self.assertEqual((code, receipts, posted, pairs), (2, [], [], ""), fields)

    def test_a_mirror_owed_but_not_drafted_fails_before_the_first_post(self):
        views = {7: {"headRefOid": "h7"}, 8: {"headRefOid": "h8"}}
        code, receipts, posted, pairs = self.run_similar(
            views, [self.entry(other_notice=None)]
        )
        self.assertEqual((code, receipts, posted, pairs), (1, ["failed"], [], ""))
        self.assertEqual(self.claims, [])

    def test_a_pair_another_finish_holds_is_not_posted_twice(self):
        views = {7: {"headRefOid": "h7"}, 8: {"headRefOid": "h8"}}
        code, receipts, posted, _ = self.run_similar(
            views, [self.entry()], claimed="7-8"
        )
        # Held is not posted, so the finish stays pending instead of recording.
        self.assertEqual((code, receipts, posted), (1, ["held"], []))
        # The pair is owned before either thread is read: a read taken earlier
        # would be stale once its owner gave the claim back after a failed post.
        self.assertEqual(self.calls, [])

    def test_a_push_during_the_first_post_stops_the_mirror_and_frees_the_pair(self):
        views = {7: {"headRefOid": "h7"}, 8: {"headRefOid": "h8"}}
        moved = {"body": "", "comments": [], "headRefOid": "new"}
        for pushed in ((7, 8), (7, 7)):
            code, receipts, posted, pairs = self.run_similar(
                views, [self.entry()], after={pushed: moved}
            )
            self.assertEqual(
                (code, receipts, posted, pairs), (1, ["moved"], ["7"], ""), pushed
            )
            # Only a pair that posted stays claimed, so a rerun can try again.
            self.assertEqual(self.claims, [], pushed)

    def test_the_other_head_is_checked_before_the_first_notice_too(self):
        views = {7: {"headRefOid": "h7"}, 8: {"headRefOid": "h8"}}
        code, receipts, posted, pairs = self.run_similar(
            views, [self.entry()], live={8: "new"}
        )
        self.assertEqual((code, receipts, posted, pairs), (1, ["moved"], [], ""))
        # Also when the other thread already links this PR and only this side
        # would be posted.
        views[8]["body"] = "see #7"
        code, receipts, posted, _ = self.run_similar(
            views, [self.entry(other_notice=None)], live={8: "new"}
        )
        self.assertEqual((code, receipts, posted), (1, ["moved"], []))

    def test_a_thread_read_without_its_comments_is_failed_not_unlinked(self):
        views = {7: {"headRefOid": "h7", "comments": None}, 8: {"headRefOid": "h8"}}
        code, receipts, posted, _ = self.run_similar(views, [self.entry()])
        self.assertEqual((code, receipts, posted), (1, ["failed"], []))
        self.assertEqual(self.claims, [])

    def test_multiline_notices_preserve_json_escapes_under_dash(self):
        views = {7: {"headRefOid": "h7"}, 8: {"headRefOid": "h8"}}
        code, receipts, posted, pairs = self.run_similar(
            views,
            [
                self.entry(
                    notice="first #8\nsecond", other_notice="path \\notes #7\nmore"
                )
            ],
            shell="dash",
        )
        self.assertEqual(
            (code, receipts, posted, pairs), (0, ["applied"], ["7", "8"], "7 8\n")
        )


@unittest.skipUnless(shutil.which("jq"), "jq is not installed")
class CiCachedTests(unittest.TestCase):
    """ci-cached.sh reads the job logs once per head, and never a live check."""

    SETTLED = {"pr": 7, "pending": [], "held": [], "failing": []}
    # A PR with a failed check, the only kind whose diagnosis is cached.
    RED = '[{"name":"lint","state":"FAILURE","bucket":"fail","link":"run/1"}]'

    def run_cached(
        self,
        heads,
        diagnosis=None,
        repos=("a/b", "a/b"),
        checks=None,
        reported=None,
        age=False,
        ci_fails=False,
    ):
        """Run ci-cached.sh once per head; return (outputs, runs, results)."""
        diagnosis = diagnosis or self.SETTLED
        with tempfile.TemporaryDirectory() as scratch:
            scratch = Path(scratch)
            (scratch / "bin").mkdir()
            support.write_executable(scratch / "bin" / "gh", FAKE_GH_CACHED)
            # The stand-in for ci-failure.sh counts its runs.
            support.write_executable(scratch / "ci.sh", FAKE_CI_COUNT)
            env = os.environ | {
                "PATH": f"{scratch / 'bin'}:{os.environ['PATH']}",
                "FAKE_DIR": str(scratch),
                "CI_FAILURE_CACHE": str(scratch / "cache"),
                "CI_FAILURE_SCRIPT": str(scratch / "ci.sh"),
                "FAKE_CI_FAILS": "1" if ci_fails else "",
            }
            results = []
            for head, repo, listed in zip(heads, repos, checks or (self.RED,) * 9):
                (scratch / "head").write_text(head + "\n")
                (scratch / "checks").write_text(listed)
                out = diagnosis | {"head": reported or head}
                (scratch / "out").write_text(json.dumps(out))
                results.append(
                    subprocess.run(
                        ["sh", str(CI_CACHED), "7", repo],
                        capture_output=True,
                        text=True,
                        env=env,
                    )
                )
                # An hour is past the 15 minutes an entry is served for.
                for entry in (scratch / "cache").glob("*.json") if age else ():
                    hour_ago = time.time() - 3600
                    os.utime(entry, (hour_ago, hour_ago))
            runs = scratch / "runs"
            count = len(runs.read_text().split()) if runs.exists() else 0
            # No temp file is left behind, and every stored entry is whole.
            cache = scratch / "cache"
            left = [f.name for f in cache.glob("ci.*")] if cache.exists() else []
            stored = list(cache.glob("*.json")) if cache.exists() else []
            for entry in stored:
                json.loads(entry.read_text())
            self.assertEqual(left, [])
        return results, count, len(stored)

    def heads(self, results):
        return [json.loads(r.stdout)["head"] for r in results]

    def test_a_settled_diagnosis_is_served_again_only_for_the_same_head(self):
        results, runs, _ = self.run_cached(["a", "a"])
        self.assertEqual((self.heads(results), runs), (["a", "a"], 1))
        results, runs, _ = self.run_cached(["a", "b"])
        self.assertEqual((self.heads(results), runs), (["a", "b"], 2))

    def test_a_rerun_of_a_job_on_the_same_head_is_not_served_the_old_diagnosis(self):
        red = '[{"name":"lint","state":"FAILURE","bucket":"fail","link":"run/1"}]'
        green = '[{"name":"lint","state":"SUCCESS","bucket":"pass","link":"run/2"}]'
        _, runs, _ = self.run_cached(["a", "a"], checks=(red, green))
        self.assertEqual(runs, 2)

    def test_a_changed_failed_list_on_the_same_head_is_a_new_entry(self):
        one = '[{"name":"lint","state":"FAILURE","bucket":"fail","link":"run/1"}]'
        two = '[{"name":"lint","state":"FAILURE","bucket":"fail","link":"run/2"}]'
        _, runs, stored = self.run_cached(["a", "a"], checks=(one, two))
        self.assertEqual((runs, stored), (2, 2))

    def test_a_list_with_no_failed_check_or_none_readable_bypasses_the_cache(self):
        green = '[{"name":"lint","state":"SUCCESS","bucket":"pass","link":"run/1"}]'
        for listed in ("[]", green, "oops"):
            _, runs, stored = self.run_cached(["a", "a"], checks=(listed, listed))
            self.assertEqual((runs, stored), (2, 0), listed)

    def test_repositories_that_differ_by_one_character_do_not_share_entries(self):
        _, runs, _ = self.run_cached(["a", "a"], repos=("a-b/c", "a/b-c"))
        self.assertEqual(runs, 2)

    def test_a_diagnosis_with_a_running_or_held_check_is_never_stored(self):
        for field in ("pending", "held"):
            live = self.SETTLED | {field: ["gates (rust)"]}
            _, runs, stored = self.run_cached(["a", "a"], live)
            self.assertEqual((runs, stored), (2, 0), field)

    def test_a_diagnosis_of_another_head_is_served_but_never_stored(self):
        results, runs, stored = self.run_cached(["a", "a"], reported="b")
        self.assertEqual((self.heads(results), runs, stored), (["b", "b"], 2, 0))

    def test_an_entry_older_than_the_ttl_is_read_again(self):
        _, runs, _ = self.run_cached(["a", "a"], age=True)
        self.assertEqual(runs, 2)

    def test_a_head_that_is_not_a_commit_id_never_reaches_a_file_name(self):
        results, runs, stored = self.run_cached(["../x"], repos=("a/b",))
        self.assertEqual((results[0].returncode, runs, stored), (1, 0, 0))

    def test_a_failed_diagnosis_is_a_failure_not_an_empty_one(self):
        results, _, stored = self.run_cached(["a"], repos=("a/b",), ci_fails=True)
        self.assertNotEqual(results[0].returncode, 0)
        self.assertEqual((results[0].stdout, stored), ("", 0))


@unittest.skipUnless(shutil.which("jq"), "jq is not installed")
class ExperienceCloseTests(unittest.TestCase):
    """experience-close.sh fills the fixed fields and refuses an unstated outcome."""

    HEAD = "a" * 40

    def close(self, fields, head=None, same_path=False):
        with tempfile.TemporaryDirectory() as scratch:
            scratch = Path(scratch)
            output = scratch / "out"
            output.mkdir()
            source = (
                output / "experience.json" if same_path else scratch / "fields.json"
            )
            source.write_text(json.dumps(fields))
            # A record command that echoes the report it was handed.
            (scratch / "memory.py").write_text(
                "import sys\nprint(open(sys.argv[sys.argv.index('--report') + 1]).read())\n"
                "print([a[8:] for a in sys.argv if a.startswith('--skill=')][0], file=sys.stderr)\n"
            )
            result = subprocess.run(
                [
                    "sh",
                    str(EXPERIENCE_CLOSE),
                    str(source),
                    head or self.HEAD,
                ],
                capture_output=True,
                text=True,
                env=os.environ
                | {
                    "SCRATCH": str(output),
                    "EXPERIENCE_MEMORY": str(scratch / "memory.py"),
                },
            )
        return result

    def test_fills_the_fixed_fields_and_keeps_the_invocations_own(self):
        fields = {
            "outcome": "complete",
            "skills_used": ["codetrial-triage"],
            "coverage": "Swept 17 PRs.",
            "questions": [{"q": "x"}],
        }
        result = self.close(fields)
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads(result.stdout)
        self.assertEqual(report["head"], self.HEAD)
        self.assertEqual(report["schema_version"], 1)
        self.assertEqual(report["findings"], [])
        self.assertEqual(report["questions"], [{"q": "x"}])
        self.assertIn("codetrial-triage", result.stderr)

    def test_a_report_that_does_not_state_its_outcome_is_refused(self):
        base = {"outcome": "complete", "skills_used": ["s"], "coverage": "c"}
        for drop in ("outcome", "skills_used", "coverage"):
            result = self.close({k: v for k, v in base.items() if k != drop})
            self.assertEqual(result.returncode, 2, drop)
        self.assertEqual(self.close(base, head="abc123").returncode, 2)
        self.assertEqual(self.close(base | {"coverage": "  \n"}).returncode, 2)
        self.assertEqual(self.close(base, head="z" * 40).returncode, 2)
        self.assertEqual(self.close(base | {"outcome": "done"}).returncode, 2)

    def test_fixed_fields_cannot_override_the_examined_head_or_schema(self):
        result = self.close(
            {
                "outcome": "complete",
                "skills_used": ["codetrial-review"],
                "coverage": "local diff",
                "head": "b" * 40,
                "schema_version": 99,
            }
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads(result.stdout)
        self.assertEqual(report["head"], self.HEAD)
        self.assertEqual(report["schema_version"], 1)

    def test_an_existing_report_can_be_used_as_the_fields_file(self):
        result = self.close(
            {"outcome": "complete", "skills_used": ["s"], "coverage": "kept"},
            same_path=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads(result.stdout)
        self.assertEqual(report["coverage"], "kept")
        self.assertEqual(report["head"], self.HEAD)


@unittest.skipUnless(shutil.which("jq"), "jq is not installed")
class FeedbackBundleTests(unittest.TestCase):
    """feedback-bundle.sh refetches a cut thread and never reads a failure as empty."""

    FAKE = FAKE_GH_FEEDBACK

    def run_bundle(self, env=(), bulk=0):
        def node(author, body):
            return {"id": body, "author": {"login": author}, "body": body, "url": "u"}

        page = {
            "data": {
                "repository": {
                    "pullRequest": {
                        "reviewThreads": {
                            "nodes": [
                                {
                                    "id": "T1",
                                    "isResolved": False,
                                    "isOutdated": False,
                                    "path": "a.js",
                                    "line": 3,
                                    "originalLine": 3,
                                    "comments": {
                                        "pageInfo": {"hasNextPage": True},
                                        "nodes": [node("m", "first")],
                                    },
                                },
                                {
                                    "id": "T2",
                                    "isResolved": True,
                                    "isOutdated": True,
                                    "path": "b.js",
                                    "line": None,
                                    "originalLine": 9,
                                    "comments": {
                                        "pageInfo": {"hasNextPage": False},
                                        "nodes": [node("m", "done")],
                                    },
                                },
                            ]
                        }
                    }
                }
            }
        }
        rest = {
            "data": {
                "node": {
                    "comments": {
                        "nodes": [
                            node("m", "first"),
                            node("a", "reply" + "x" * bulk),
                            node("m", "last"),
                        ]
                    }
                }
            }
        }
        pr = {"number": 7, "url": "u", "headRefOid": "h", "comments": [], "reviews": []}
        with tempfile.TemporaryDirectory() as scratch:
            scratch = Path(scratch)
            (scratch / "bin").mkdir()
            support.write_executable(scratch / "bin" / "gh", self.FAKE)
            for name, value in (("pr", pr), ("threads", page), ("thread", rest)):
                (scratch / f"{name}.json").write_text(json.dumps(value))
            return subprocess.run(
                ["sh", str(FEEDBACK_BUNDLE), "7"],
                capture_output=True,
                text=True,
                env=os.environ
                | {
                    "PATH": f"{scratch / 'bin'}:{os.environ['PATH']}",
                    "FAKE_DIR": str(scratch),
                }
                | dict(env),
            )

    def test_a_cut_thread_is_refetched_whole_and_open_requests_are_listed(self):
        result = self.run_bundle()
        self.assertEqual(result.returncode, 0, result.stderr)
        out = json.loads(result.stdout)
        first = out["threads"][0]
        self.assertEqual(
            [c["body"] for c in first["comments"]], ["first", "reply", "last"]
        )
        self.assertTrue(out["complete"])
        self.assertEqual([t["id"] for t in out["open"]], ["T1"])

    def test_a_refetched_thread_past_the_argument_limit_still_makes_a_bundle(self):
        # Two megabytes is past what one argument can carry on every platform
        # this runs on, which a thread of many long replies can reach.
        result = self.run_bundle(bulk=2_000_000)
        self.assertEqual(result.returncode, 0, result.stderr[-200:])
        out = json.loads(result.stdout)
        self.assertEqual(len(out["threads"][0]["comments"][1]["body"]), 2_000_005)
        self.assertTrue(out["complete"])

    def test_a_thread_that_cannot_be_refetched_marks_the_bundle_incomplete(self):
        out = json.loads(self.run_bundle({"FAKE_NO_THREAD": "1"}).stdout)
        self.assertFalse(out["complete"])
        self.assertFalse(out["threads"][0]["complete"])

    def test_no_sign_in_is_an_error_not_an_empty_pull_request(self):
        result = self.run_bundle({"FAKE_NO_AUTH": "1"})
        self.assertEqual((result.returncode, result.stdout), (1, ""))


class BadArgumentTests(unittest.TestCase):
    """Every new script's header says a bad argument exits 2, not just fails."""

    def test_a_missing_or_malformed_argument_exits_2_before_any_call(self):
        scripts = (
            APPLY_ACTIONS,
            POST_SIMILAR,
            REVIEW_BRIEF,
            REVIEW_FINISH,
            REVIEW_PICK,
            EXPERIENCE_CLOSE,
            FEEDBACK_BUNDLE,
        )
        with tempfile.TemporaryDirectory() as scratch:
            support.write_executable(Path(scratch) / "gh", FAKE_GH_FAILS)
            env = os.environ | {
                "PATH": f"{scratch}:{os.environ['PATH']}",
                "SCRATCH": scratch,
            }
            for script in scripts:
                for args in ([], ["not-a-number"]):
                    for shell in ("sh", "bash"):
                        result = subprocess.run(
                            [shell, str(script), *args],
                            capture_output=True,
                            text=True,
                            env=env,
                            cwd=ROOT,
                        )
                        self.assertEqual(
                            result.returncode,
                            2,
                            (script.name, args, shell, result.stderr),
                        )


if __name__ == "__main__":
    unittest.main()

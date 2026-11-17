#!/usr/bin/env python3
"""Contract tests for approving a held run only on gate evidence.

gate-evidence.sh gathers a pull request's head tree, changed paths, toolchain
files and gate note as data and asks scripts/check-gate-evidence.py. These cases
serve it a fake `gh` and a fake note fetcher, then check what the sweep reports
and what apply-pr-actions.sh writes: an approval only for a pass, rechecked at
the head the sweep read immediately before the call.
"""

import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location(
    "support", Path(__file__).with_name("support.py")
)
support = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(support)

TRIAGE = support.ROOT / ".claude" / "skills" / "codetrial-triage"
EVIDENCE = TRIAGE / "gate-evidence.sh"
APPLY = TRIAGE / "apply-pr-actions.sh"
PR_SWEEP = TRIAGE / "pr-sweep.sh"
FAKE_GH = support.stub("gate-approve/gh.sh")
FAKE_FETCH = support.stub("gate-approve/fetch.sh")
HEAD = "a" * 40
MOVED = "b" * 40
TREE = "c" * 40
PIN = '[toolchain]\nchannel = "1.100.0"\n'
RUSTC = "rustc 1.100.0 (abcdef123 2026-10-01)"


def note(tree=TREE, rustc=RUSTC, passed=True, lane="rust"):
    return json.dumps(
        {
            "version": 1,
            "worktree_tree": tree,
            "receipts": [
                {
                    "lane": lane,
                    "passed": passed,
                    "skipped": 0,
                    "time": "2026-11-16T12:34:56Z",
                    "rustc": rustc,
                    "cargo": "cargo 1.100.0 (abcdef123 2026-09-01)",
                }
            ],
        }
    )


def entry(path, previous=None):
    return (
        {"filename": path, "previous_filename": previous}
        if previous
        else {"filename": path}
    )


@unittest.skipUnless(shutil.which("jq"), "jq is not installed")
class GateCase(unittest.TestCase):
    """A scratch directory holding what the fake gh and fetcher serve."""

    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.scratch = Path(scratch.name)
        (self.scratch / "bin").mkdir()
        support.write_executable(self.scratch / "bin" / "gh", FAKE_GH)
        support.write_executable(self.scratch / "fetch.sh", FAKE_FETCH)
        self.dir = self.scratch / "fixture"
        self.dir.mkdir()
        self.log = self.scratch / "log"
        self.log.write_text("")
        self.env = os.environ | {
            "PATH": f"{self.scratch / 'bin'}:{os.environ['PATH']}",
            "FAKE_LOG": str(self.log),
            "FAKE_DIR": str(self.dir),
            "GATE_EVIDENCE_FETCH": str(self.scratch / "fetch.sh"),
            "GATE_EVIDENCE_PYTHON": sys.executable,
        }
        self.serve()

    def pull(self, sha=HEAD, **fields):
        node = {
            "state": "open",
            "head": {
                "sha": sha,
                "repo": {"name": "codetrial", "owner": {"login": "contrib"}},
            },
            "base": {"ref": "main"},
            "user": {"type": "User"},
            "changed_files": 1,
        }
        return node | fields

    def serve(self, files=(entry("src/main.rs"),), note_text=None, **pull):
        """What GitHub says about the pull request; a note unless None."""
        pull.setdefault("changed_files", len(files))
        (self.dir / "pull.json").write_text(json.dumps(self.pull(**pull)))
        # --paginate prints one array per page, back to back.
        (self.dir / "files.json").write_text("".join(json.dumps([f]) for f in files))
        (self.dir / "tree").write_text(TREE + "\n")
        (self.dir / "base-toolchain").write_text(PIN)
        (self.dir / "head-toolchain").write_text(PIN)
        if note_text is not None:
            (self.dir / "note").write_text(note_text)

    def calls(self):
        return self.log.read_text().splitlines()

    def evidence(self, want=HEAD, **env):
        result = subprocess.run(
            ["sh", str(EVIDENCE), "sysprog21/codetrial", "7", want],
            capture_output=True,
            text=True,
            env=self.env | env,
        )
        out = json.loads(result.stdout) if result.stdout.strip() else {}
        return result.returncode, out.get("verdict"), out.get("reason", ""), result


class FakeGhTests(GateCase):
    def test_a_review_post_without_x_post_is_logged_as_a_write(self):
        # `gh api` posts when it is given a field and no method, as the review
        # requests in apply-pr-actions.sh do; the stub must not read it as a GET.
        for flag in ("-f", "-F"):
            with self.subTest(flag=flag):
                self.log.write_text("")
                subprocess.run(
                    ["gh", "api", "repos/o/r/pulls/7/reviews", flag, "event=X"],
                    env=self.env,
                    capture_output=True,
                    check=True,
                )
                self.assertIn("WRITE", self.calls())


class GateEvidenceTests(GateCase):
    def test_a_note_for_the_head_tree_passes(self):
        self.serve(note_text=note())
        code, verdict, reason, _ = self.evidence()
        self.assertEqual((code, verdict), (0, "pass"), reason)
        self.assertIn("FETCH contrib codetrial " + HEAD, self.calls())

    def test_no_note_asks_the_contributor_for_one(self):
        self.serve()
        for status in ("3", "6", "7"):
            with self.subTest(status=status):
                code, verdict, reason, _ = self.evidence(FAKE_NOTE_RC=status)
                self.assertEqual((code, verdict), (1, "ask"), reason)
                self.assertIn("no receipt", reason)

    def test_a_deleted_fork_is_indeterminate_when_a_receipt_is_owed(self):
        self.serve(note_text=note(), head={"sha": HEAD, "repo": None})
        code, verdict, reason, _ = self.evidence()
        self.assertEqual((code, verdict), (2, "indeterminate"), reason)
        # Nothing was fetched for a repository the API does not name.
        self.assertFalse([c for c in self.calls() if c.startswith("FETCH")])

    def test_a_docs_only_change_needs_no_note_even_from_a_deleted_fork(self):
        self.serve(
            files=(entry("docs/x.md"),),
            head={"sha": HEAD, "repo": None},
        )
        code, verdict, reason, _ = self.evidence()
        self.assertEqual((code, verdict), (0, "pass"), reason)
        self.assertFalse([c for c in self.calls() if c.startswith("FETCH")])
        self.assertFalse([c for c in self.calls() if "contents" in c])

    def test_a_failed_or_refused_fetch_is_indeterminate(self):
        self.serve()
        for status in ("2", "4", "5"):
            with self.subTest(status=status):
                code, verdict, reason, _ = self.evidence(FAKE_NOTE_RC=status)
                self.assertEqual((code, verdict), (2, "indeterminate"), reason)
                self.assertIn(status, reason)

    def test_a_note_for_another_tree_asks(self):
        self.serve(note_text=note(tree="d" * 40))
        code, verdict, reason, _ = self.evidence()
        self.assertEqual((code, verdict), (1, "ask"), reason)

    def test_a_note_made_with_another_compiler_asks(self):
        self.serve(note_text=note(rustc="rustc 1.99.0 (abcdef123 2026-10-01)"))
        self.assertEqual(self.evidence()[:2], (1, "ask"))

    def test_a_truncated_list_is_indeterminate_before_anything_is_fetched(self):
        self.serve(note_text=note(), changed_files=3)
        code, verdict, reason, _ = self.evidence()
        self.assertEqual((code, verdict), (2, "indeterminate"), reason)
        self.assertIn("truncated", reason)
        self.assertFalse([c for c in self.calls() if c.startswith("FETCH")])

    def test_an_unreadable_checker_answer_is_indeterminate(self):
        # A pinned channel that names no exact version cannot be compared.
        self.serve(note_text=note())
        (self.dir / "base-toolchain").write_text('[toolchain]\nchannel = "stable"\n')
        code, verdict, reason, _ = self.evidence()
        self.assertEqual((code, verdict), (2, "indeterminate"), reason)

    def test_any_failed_read_is_indeterminate(self):
        self.serve(note_text=note())
        for variable in (
            "FAKE_PULL_FAIL",
            "FAKE_FILES_FAIL",
            "FAKE_TREE_FAIL",
            "FAKE_BASE_TOOLCHAIN_FAIL",
        ):
            with self.subTest(variable=variable):
                code, verdict, _, _ = self.evidence(**{variable: "1"})
                self.assertEqual((code, verdict), (2, "indeterminate"))

    def test_a_head_that_differs_from_the_one_asked_about_is_indeterminate(self):
        self.serve(note_text=note(), sha=MOVED)
        code, verdict, reason, _ = self.evidence(want=HEAD)
        self.assertEqual((code, verdict), (2, "indeterminate"), reason)
        self.assertIn("moved", reason)
        self.assertFalse([c for c in self.calls() if c.startswith("FETCH")])

    def test_a_push_while_the_evidence_is_gathered_is_indeterminate(self):
        self.serve(note_text=note())
        (self.dir / "pull-after.json").write_text(json.dumps(self.pull(sha=MOVED)))
        code, verdict, reason, _ = self.evidence()
        self.assertEqual((code, verdict), (2, "indeterminate"), reason)
        self.assertIn("moved", reason)

    def test_a_closed_pull_request_is_indeterminate(self):
        self.serve(note_text=note(), state="closed")
        self.assertEqual(self.evidence()[:2], (2, "indeterminate"))

    def test_the_base_toolchain_comes_from_the_base_branch_not_the_head(self):
        self.serve(note_text=note())
        self.evidence()
        reads = [c for c in self.calls() if "rust-toolchain.toml" in c]
        self.assertEqual(len(reads), 1)
        self.assertTrue(reads[0].endswith("-f ref=main"), reads)

    def test_the_head_toolchain_is_read_only_when_the_change_edits_it(self):
        files = (entry("src/main.rs"), entry("rust-toolchain.toml"))
        self.serve(files=files, note_text=note(rustc="rustc 1.101.0 (abc 2026-11-01)"))
        (self.dir / "head-toolchain").write_text('[toolchain]\nchannel = "1.101.0"\n')
        code, verdict, reason, _ = self.evidence()
        self.assertEqual((code, verdict), (0, "pass"), reason)
        reads = [c for c in self.calls() if "rust-toolchain.toml" in c]
        self.assertEqual(len(reads), 2)
        self.assertTrue(reads[1].endswith(f"-f ref={HEAD}"), reads)

    def test_a_rename_out_of_the_rust_scope_still_needs_a_receipt(self):
        # The new path alone owes nothing, so only the old path can be what asks.
        self.serve(files=(entry("docs/main.md"),))
        self.assertEqual(self.evidence(FAKE_NOTE_RC="6")[:2], (0, "pass"))
        self.serve(files=(entry("docs/main.md", previous="src/main.rs"),))
        code, verdict, reason, _ = self.evidence(FAKE_NOTE_RC="6")
        self.assertEqual((code, verdict), (1, "ask"), reason)
        self.assertIn("no receipt", reason)
        self.assertTrue([c for c in self.calls() if c.startswith("FETCH")])

    def test_a_bot_author_owes_no_receipt(self):
        self.serve(user={"type": "Bot"})
        code, verdict, reason, _ = self.evidence()
        self.assertEqual((code, verdict), (0, "pass"), reason)
        self.assertFalse([c for c in self.calls() if c.startswith("FETCH")])

    def test_a_path_with_a_newline_stays_one_path(self):
        self.serve(files=(entry("docs/x\nsrc/main.rs"),))
        self.assertEqual(self.evidence()[:2], (0, "pass"))

    def test_a_workflow_path_is_noted_but_changes_no_verdict(self):
        self.serve(files=(entry(".github/workflows/x.yml"),))
        code, verdict, _, result = self.evidence()
        self.assertEqual((code, verdict), (0, "pass"))

    def test_bad_arguments_never_reach_gh(self):
        for args in (
            ("not a repo", "7", HEAD),
            ("sysprog21/codetrial", "7;id", HEAD),
            ("sysprog21/codetrial", "7", "abc"),
            ("sysprog21/codetrial", "7", HEAD.upper()),
        ):
            with self.subTest(args=args):
                result = subprocess.run(
                    ["sh", str(EVIDENCE), *args],
                    capture_output=True,
                    text=True,
                    env=self.env,
                )
                self.assertEqual(result.returncode, 2)
                self.assertEqual(json.loads(result.stdout)["verdict"], "indeterminate")
        self.assertEqual(self.calls(), [])


class ApplyApprovalTests(GateCase):
    """apply-pr-actions.sh with the real evidence check behind it."""

    def apply(self, head=HEAD, **env):
        sweep = {
            "maintainers_known": True,
            "prs": [],
            "approve": [
                {
                    "pr": 7,
                    "run": 11,
                    "head": head,
                    "first_time": True,
                    "touches_workflows": False,
                    "files_truncated": False,
                }
            ],
            "rebase": [],
            "hide": [],
            "hide_resolved": [],
            "refine_stale": [],
        }
        (self.scratch / "sweep.json").write_text(json.dumps(sweep))
        result = subprocess.run(
            ["sh", str(APPLY), str(self.scratch / "sweep.json")],
            capture_output=True,
            text=True,
            env=self.env | env,
        )
        self.assertIn(result.returncode, (0, 1), result.stderr)
        receipts = [json.loads(line) for line in result.stdout.splitlines()]
        return receipts[0], "WRITE" in self.calls()

    def test_a_pass_approves(self):
        self.serve(note_text=note())
        receipt, wrote = self.apply()
        self.assertEqual((receipt["result"], wrote), ("applied", True), receipt)

    def test_a_missing_note_goes_on_the_confirmed_list_unapproved(self):
        self.serve()
        receipt, wrote = self.apply(FAKE_NOTE_RC="3")
        self.assertEqual((receipt["result"], wrote), ("confirm", False), receipt)
        self.assertIn("toolchain comment", receipt["why"])

    def test_an_indeterminate_answer_never_approves(self):
        self.serve(note_text=note())
        for variable in ("FAKE_FILES_FAIL", "FAKE_PULL_FAIL", "FAKE_TREE_FAIL"):
            with self.subTest(variable=variable):
                receipt, wrote = self.apply(**{variable: "1"})
                self.assertEqual(
                    (receipt["result"], wrote), ("skipped", False), receipt
                )
        receipt, wrote = self.apply(FAKE_NOTE_RC="5")
        self.assertEqual((receipt["result"], wrote), ("skipped", False), receipt)

    def test_a_head_that_changed_since_the_sweep_is_not_approved(self):
        # The sweep read HEAD; the pull request is now at MOVED.
        self.serve(note_text=note(), sha=MOVED)
        receipt, wrote = self.apply(head=HEAD)
        self.assertEqual((receipt["result"], wrote), ("skipped", False), receipt)
        self.assertIn("moved", receipt["why"])

    def test_the_evidence_is_read_after_the_sweep_not_taken_from_it(self):
        # A sweep that recorded a pass for this head carries no weight: the
        # note is fetched again, and now it is for another tree.
        self.serve(note_text=note(tree="d" * 40))
        receipt, wrote = self.apply()
        self.assertEqual((receipt["result"], wrote), ("confirm", False), receipt)
        self.assertTrue([c for c in self.calls() if c.startswith("FETCH")])

    def test_the_check_runs_immediately_before_the_approval_call(self):
        self.serve(note_text=note())
        self.apply()
        calls = self.calls()
        approve = next(i for i, c in enumerate(calls) if "runs/11/approve" in c)
        fetched = max(i for i, c in enumerate(calls) if c.startswith("FETCH"))
        reread = max(
            i for i, c in enumerate(calls) if c.endswith("/pulls/7 --jq .head.sha")
        )
        self.assertLess(fetched, reread)
        self.assertLess(reread, approve)
        self.assertEqual([c for c in calls[reread + 1 : approve] if c != "WRITE"], [])


class SweepEvidenceTests(unittest.TestCase):
    """pr-sweep.sh puts the verdict beside each held run."""

    @unittest.skipUnless(shutil.which("jq"), "jq is not installed")
    def test_each_held_run_carries_the_verdict_for_its_head(self):
        sys.modules.pop("skill_scripts_for_gate", None)
        spec = importlib.util.spec_from_file_location(
            "skill_scripts_for_gate", Path(__file__).with_name("test_skill_scripts.py")
        )
        helpers = importlib.util.module_from_spec(spec)
        sys.modules["skill_scripts_for_gate"] = helpers
        spec.loader.exec_module(helpers)
        node = helpers.pull(1, headRefOid=HEAD, commits=helpers.GREEN)
        page = {
            "repository": {
                "defaultBranchRef": {"name": "main"},
                "pullRequests": {"pageInfo": {"hasNextPage": False}, "nodes": [node]},
            }
        }
        held = {
            "id": 11,
            "name": "check",
            "head_sha": HEAD,
            "head_branch": "topic",
            "head_repo": "contributor/codetrial",
            "actor": "contributor",
        }
        checks = {
            "FAKE_PAGE": json.dumps({"data": page}),
            "FAKE_RUNS": json.dumps(held) + "\n",
            "FAKE_AGAIN": "[]",
            "PR_SWEEP_CI": 'echo "{}"\n',
            "PR_SWEEP_EVIDENCE": (
                'echo "$*" >&2\n'
                'echo \'{"verdict":"ask","reason":"no receipt","head":"\'"$3"\'"}\'\n'
                "exit 1\n"
            ),
        }
        result = helpers.run_fake(
            PR_SWEEP, ["sysprog21/codetrial"], helpers.FAKE_GH, checks
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        approve = json.loads(result.stdout)["approve"]
        self.assertEqual(len(approve), 1)
        self.assertEqual(approve[0]["head"], HEAD)
        self.assertEqual(
            approve[0]["evidence"],
            {"verdict": "ask", "reason": "no receipt", "head": HEAD},
        )


if __name__ == "__main__":
    unittest.main()

"""Explanation revisions, reviewer feedback and local HTTP access boundaries."""

import errno
import importlib.util
import http.client
import json
import os
from pathlib import Path
import select
import socket
import subprocess
import sys
import tempfile
import unittest
from urllib.parse import urlsplit


HELPER = (
    Path(__file__).resolve().parents[1] / ".claude/skills/codetrial-review/workbench.py"
)
SPEC = importlib.util.spec_from_file_location("review_workbench", HELPER)
workbench = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(workbench)
COMMIT_SIZES = HELPER.with_name("commit-sizes.py")


class WorkbenchTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.manifest = {
            "head": "a" * 40,
            "against": "b" * 40 + "...HEAD",
            "pr": None,
            "committed": True,
            "files": [{"path": "src/example.rs", "additions": 300, "deletions": 201}],
        }
        self.write_manifest()
        (self.root / "pr.diff").write_text(
            "diff --git a/src/example.rs b/src/example.rs\n+new\n-old\n"
        )
        self.questions = self.root / "questions.json"
        self.questions.write_text(
            json.dumps(
                [
                    {
                        "id": "alternative",
                        "path": "src/example.rs",
                        "prompt": "Why persist before replying?",
                    },
                ]
            )
        )
        self.app = workbench.Workbench(self.root, self.questions)

    def write_manifest(self):
        (self.root / "manifest.json").write_text(json.dumps(self.manifest))

    def test_rename_quotes_use_metadata_without_matching_path_suffixes_or_hunk_text(
        self,
    ):
        diff = (
            "diff --git a/old.rs b/a b/src/example.rs\n"
            "--- a/old.rs\n+++ b/a b/src/example.rs\n@@ -1 +1 @@\n+elsewhere\n"
            "diff --git a/old.rs b/src/example.rs\n"
            "--- a/old.rs\n+++ b/src/example.rs\n@@ -1 +1 @@\n+new\n"
        )
        self.assertNotIn("+elsewhere", workbench.file_patch(diff, "src/example.rs"))
        (self.root / "pr.diff").write_text(diff)
        q = json.loads(self.questions.read_text())
        q[0]["quote"] = "+new"
        self.questions.write_text(json.dumps(q))
        app = workbench.Workbench(self.root, self.questions)
        self.assertEqual(app.questions[0]["quote"], "+new")

    def test_recreated_path_prefers_its_current_block_over_the_rename_source(self):
        diff = (
            "diff --git a/src/old.rs b/src/aaa.rs\n"
            "similarity index 90%\nrename from src/old.rs\nrename to src/aaa.rs\n"
            "--- a/src/old.rs\n+++ b/src/aaa.rs\n@@ -1 +1 @@\n+moved\n"
            "diff --git a/src/old.rs b/src/old.rs\nnew file mode 100644\n"
            "--- /dev/null\n+++ b/src/old.rs\n@@ -0,0 +1 @@\n+recreated\n"
        )
        self.assertIn("+recreated", workbench.file_patch(diff, "src/old.rs"))
        self.assertIn(
            "+moved",
            workbench.file_patch(
                diff[: diff.index("diff --git a/src/old.rs b/src/old.rs")], "src/old.rs"
            ),
        )

    def test_identical_feedback_retry_preserves_confirmation_revision_and_history(self):
        self.save()
        self.request_review()
        self.app.feedback(self.feedback())
        s = self.app.snapshot()
        self.app.reflect(
            {
                "identity": s["identity"],
                "revision": s["revision"],
                "feedback_digest": s["feedback_digest"],
                "grade": "got",
                "correction": "",
            }
        )
        before = self.app.state_path.read_bytes()
        self.app.feedback(self.app.snapshot()["feedback"])
        self.assertEqual(self.app.state_path.read_bytes(), before)
        self.assertEqual(self.app.snapshot()["status"], "explanation_complete")

    def test_multibyte_answers_fit_their_first_submission_and_oversize_drafts_are_not_saved(
        self,
    ):
        self.manifest["files"].extend(
            [
                {"path": "b.rs", "additions": 1, "deletions": 0},
                {"path": "c.rs", "additions": 1, "deletions": 0},
            ]
        )
        self.write_manifest()
        q = json.loads(self.questions.read_text()) + [
            {"id": "b", "path": "b.rs", "prompt": "Why this boundary?"},
            {"id": "c", "path": "c.rs", "prompt": "What regression matters?"},
        ]
        self.questions.write_text(json.dumps(q))
        self.app = workbench.Workbench(self.root, self.questions)
        # The three-byte fixture pins byte budgeting rather than character counts.
        text = "\u20ac" * 40000
        self.save({"motivation": text, "reading:b": text})
        self.request_review()
        self.assertEqual(self.app.snapshot()["status"], "awaiting_review")
        self.assertLess(self.app.state_path.stat().st_size, workbench.LIMIT)
        before = self.app.state_path.read_bytes()
        with self.assertRaisesRegex(ValueError, "submission budget"):
            self.save({"motivation": text, "reading:b": text, "reading:c": text})
        self.assertEqual(self.app.state_path.read_bytes(), before)

    def test_submission_reserves_history_slots_through_confirmation(self):
        state = self.app.state()
        state["history"] = [{"kind": "fixture", "value": {}} for _ in range(61)]
        self.app.write(state)
        self.save()
        self.request_review()
        self.app.feedback(self.feedback())
        state = self.app.snapshot()
        self.app.reflect(
            {
                "identity": state["identity"],
                "revision": state["revision"],
                "feedback_digest": state["feedback_digest"],
                "grade": "got",
                "correction": "",
            }
        )
        self.assertEqual(len(self.app.snapshot()["history"]), 64)
        before = self.app.state_path.read_bytes()
        self.app.feedback(self.app.snapshot()["feedback"])
        self.assertEqual(self.app.snapshot()["status"], "explanation_complete")
        with self.assertRaisesRegex(ValueError, "history is full"):
            self.save(
                {"motivation": "A new explanation cannot silently overflow history."}
            )
        self.assertEqual(self.app.state_path.read_bytes(), before)

    def test_one_motivation_is_sufficient_without_a_follow_up(self):
        self.questions.write_text("[]")
        self.app = workbench.Workbench(self.root, self.questions)
        self.assertEqual(self.app.keys, ["motivation"])
        self.assertEqual(self.app.snapshot()["missing"], ["motivation"])
        self.save(
            {"motivation": "The current workflow hides the reason for the change."}
        )
        self.request_review()
        self.app.feedback(self.feedback())
        self.assertEqual(self.app.snapshot()["status"], "reflection_needed")

    def test_key_quote_must_belong_to_its_named_file(self):
        with (self.root / "pr.diff").open("a") as patch:
            patch.write("diff --git a/other.rs b/other.rs\n+elsewhere\n")
        question = json.loads(self.questions.read_text())
        question[0]["quote"] = "+elsewhere"
        self.questions.write_text(json.dumps(question))
        with self.assertRaisesRegex(ValueError, "pinned diff"):
            workbench.Workbench(self.root, self.questions)

    def test_reading_question_budget_is_bounded_without_changing_the_draft(self):
        self.save()
        before = self.app.state_path.read_bytes()
        self.questions.write_text(
            json.dumps(
                [
                    {
                        "id": str(index),
                        "path": "src/example.rs",
                        "prompt": "Why change this?",
                    }
                    for index in range(4)
                ]
            )
        )
        with self.assertRaisesRegex(ValueError, "at most three"):
            workbench.Workbench(self.root, self.questions)
        self.assertEqual(self.app.state_path.read_bytes(), before)

    def test_file_reading_answers_are_optional_and_retained_in_adjustment_context(self):
        self.manifest["files"].append(
            {"path": "tests/example.rs", "additions": 1, "deletions": 0}
        )
        self.write_manifest()
        with (self.root / "pr.diff").open("a") as stream:
            stream.write(
                "diff --git a/tests/example.rs b/tests/example.rs\n+assert_saved()\n"
            )
        questions = json.loads(self.questions.read_text())
        questions.append(
            {
                "id": "regression",
                "path": "tests/example.rs",
                "quote": "+assert_saved()",
                "prompt": "What failure would this assertion expose?",
            }
        )
        self.questions.write_text(json.dumps(questions))
        self.app = workbench.Workbench(self.root, self.questions)
        self.assertEqual(self.app.snapshot()["missing"], ["motivation"])
        self.save({"motivation": "Preserve the previous saved record."})
        self.assertEqual(self.app.snapshot()["missing"], [])
        self.request_review()
        self.assertNotIn(
            "reading:regression", self.app.basis()["submission"]["answers"]
        )
        self.save(
            {
                "motivation": "Preserve the previous saved record.",
                "reading:regression": "This assertion does not exercise write failure yet.",
            }
        )
        self.request_review()
        basis = self.app.basis()
        self.assertEqual(basis["submission"]["focus"], questions)
        self.assertIn(
            "write failure", basis["submission"]["answers"]["reading:regression"]
        )
        questions[1]["path"] = questions[0]["path"]
        questions[1].pop("quote")
        self.questions.write_text(json.dumps(questions))
        with self.assertRaisesRegex(ValueError, "one question per file"):
            workbench.Workbench(self.root, self.questions)

    def test_old_questionnaire_answers_are_preserved_instead_of_silently_dropped(self):
        state = self.app.state()
        state["answers"] = {"problem": "A saved explanation from the previous form"}
        self.app.write(state)
        before = self.app.state_path.read_bytes()
        with self.assertRaisesRegex(ValueError, "older questionnaire"):
            self.app.snapshot()
        self.assertEqual(self.app.state_path.read_bytes(), before)

    def save(self, answers=None):
        state = self.app.snapshot()
        return self.app.save(
            {
                "identity": state["identity"],
                "revision": state["revision"],
                "answers": answers
                if answers is not None
                else {k: "A concrete explanation" for k in self.app.keys},
            }
        )

    def feedback(self, approved=True):
        state = self.app.snapshot()
        return {
            "identity": state["identity"],
            "answer_digest": state["answer_digest"],
            "notes": [
                {
                    "field": "motivation",
                    "text": "Traced the error path at src/example.rs:8.",
                }
            ],
        }

    def test_answers_survive_restart_and_old_tab_cannot_overwrite(self):
        stale = self.app.snapshot()
        self.save({"motivation": "The actual problem"})
        restarted = workbench.Workbench(self.root, self.questions)
        self.assertEqual(
            restarted.snapshot()["answers"], {"motivation": "The actual problem"}
        )
        with self.assertRaisesRegex(ValueError, "Session changed"):
            self.app.save(
                {
                    "identity": stale["identity"],
                    "revision": stale["revision"],
                    "answers": {},
                }
            )
        self.assertEqual(
            restarted.snapshot()["answers"]["motivation"], "The actual problem"
        )

    def test_reflection_requires_submission_and_revisions_invalidate_feedback(self):
        with self.assertRaisesRegex(ValueError, "motivation"):
            self.request_review()
        self.assertEqual(self.save()["status"], "ready_to_send")
        self.request_review()
        feedback = self.feedback()
        self.app.feedback(feedback)
        self.assertEqual(self.app.snapshot()["status"], "reflection_needed")
        answers = self.app.snapshot()["answers"]
        answers["motivation"] = "An updated failure analysis"
        self.assertEqual(self.save(answers)["status"], "ready_to_send")
        with self.assertRaisesRegex(ValueError, "different|not been sent"):
            self.app.feedback(feedback)

    def test_one_key_question_uses_one_answer_and_preserves_adjustment_basis(self):
        self.assertEqual(self.app.keys, ["motivation"])
        self.save(
            {
                "motivation": "Keep the old record if writing fails.",
                "confidence": "probably",
            }
        )
        self.request_review()
        self.app.feedback(self.feedback())
        state = self.app.snapshot()
        frame = {
            "identity": state["identity"],
            "revision": state["revision"],
            "feedback_digest": state["feedback_digest"],
            "grade": "missed",
            "correction": "",
        }
        with self.assertRaisesRegex(ValueError, "uncertainty"):
            self.app.reflect(frame)
        frame["correction"] = "I have not checked the caller's rollback path yet."
        self.app.reflect(frame)
        self.assertEqual(self.app.snapshot()["status"], "explanation_complete")
        basis = self.app.basis()
        self.assertEqual(basis["authority"], "context_only")
        self.assertEqual(basis["submission"]["focus"], self.app.questions)
        self.assertEqual(basis["submission"]["answers"]["confidence"], "probably")
        self.assertEqual(
            [event["kind"] for event in basis["history"]],
            ["author", "reference", "reflection"],
        )
        self.save({"motivation": "A different goal"})
        self.assertFalse(self.app.basis()["current"])
        self.assertIsNone(self.app.snapshot()["reflection"])
        with self.assertRaises(ValueError):
            self.app.reflect(frame)

    def test_restored_answer_requires_a_new_explicit_submission(self):
        original = {"motivation": "A concrete goal"}
        self.save(original)
        self.request_review()
        self.app.feedback(self.feedback())
        previous = self.app.snapshot()
        self.save({"motivation": "A different goal"})
        self.save(original)
        state = self.app.snapshot()
        self.assertEqual(state["status"], "ready_to_send")
        with self.assertRaises(ValueError):
            self.app.reflect(
                {
                    "identity": state["identity"],
                    "revision": state["revision"],
                    "feedback_digest": state["feedback_digest"],
                    "grade": "got",
                    "correction": "",
                }
            )
        self.request_review()
        self.assertEqual(self.app.snapshot()["status"], "awaiting_review")
        self.assertNotEqual(self.app.snapshot()["request_id"], previous["request_id"])

    def test_model_cannot_score_or_complete_reflection(self):
        self.save()
        self.request_review()
        feedback = self.feedback()
        feedback["approved"] = True
        with self.assertRaisesRegex(ValueError, "not a score"):
            self.app.feedback(feedback)
        del feedback["approved"]
        self.app.feedback(feedback)
        self.assertEqual(self.app.snapshot()["status"], "reflection_needed")

    def test_changed_code_or_questions_cannot_reuse_answers(self):
        self.save()
        (self.root / "pr.diff").write_text("different code")
        with self.assertRaisesRegex(ValueError, "Bundle changed"):
            self.app.snapshot()
        with self.assertRaisesRegex(ValueError, "Bundle or questions changed"):
            workbench.Workbench(self.root, self.questions).snapshot()

    def test_unknown_counts_and_untracked_files_remain_incomplete(self):
        self.assertEqual(self.app.snapshot()["total"], 501)
        self.manifest["files"][0]["deletions"] = 200
        self.write_manifest()
        self.assertEqual(
            workbench.Workbench(self.root, self.questions).snapshot()["total"], 500
        )
        self.manifest["files"][0]["additions"] = None
        self.write_manifest()
        snapshot = workbench.Workbench(self.root, self.questions).snapshot()
        self.assertIsNone(snapshot["total"])
        self.assertFalse(snapshot["coverage_complete"])
        self.manifest["files"][0]["additions"] = 300
        self.manifest["untracked"] = ["new.py"]
        self.write_manifest()
        self.assertFalse(
            workbench.Workbench(self.root, self.questions).snapshot()[
                "coverage_complete"
            ]
        )

    def test_feedback_can_request_another_iteration(self):
        self.save()
        self.request_review()
        self.app.feedback(self.feedback(approved=False))
        self.assertEqual(self.app.snapshot()["status"], "reflection_needed")
        self.assertEqual(
            self.app.snapshot()["feedback"]["notes"][0]["field"], "motivation"
        )

    def test_commit_200_triggers_even_when_the_final_diff_has_one_line(self):
        repo = self.root / "repo"
        (repo / "src").mkdir(parents=True)

        def git(*args):
            return (
                subprocess.check_output(
                    [
                        "git",
                        "-c",
                        "user.name=Test",
                        "-c",
                        "user.email=test@example.test",
                        "-c",
                        "commit.gpgsign=false",
                        *args,
                    ],
                    cwd=repo,
                    stderr=subprocess.PIPE,
                )
                .decode()
                .strip()
            )

        def collect(base):
            return json.loads(
                subprocess.check_output(
                    [sys.executable, str(COMMIT_SIZES), "--base", base],
                    cwd=repo,
                )
            )

        path = repo / "src/example.rs"
        git("init", "-q")
        path.write_text("base\n")
        git("add", ".")
        git("commit", "-qm", "Base fixture")
        base = git("rev-parse", "HEAD")
        path.write_text("base\n" + "old\n" * 199)
        git("commit", "-qam", "Add 199 lines")
        sizes = collect(base)
        self.manifest.update(head=sizes["head"], commit_sizes=sizes)
        self.manifest["files"][0].update(additions=199, deletions=0)
        self.write_manifest()
        snapshot = workbench.Workbench(self.root, self.questions).snapshot()
        self.assertEqual(snapshot["large_commits"], [])
        self.assertTrue(snapshot["commit_coverage_complete"])

        path.write_text("base\nlast\n")
        git("commit", "-qam", "Replace 199 lines with one")
        sizes = collect(base)
        self.assertEqual([c["known_lines"] for c in sizes["commits"]], [199, 200])
        self.manifest.update(head=sizes["head"], commit_sizes=sizes)
        self.manifest["files"][0].update(additions=1)
        self.write_manifest()
        snapshot = workbench.Workbench(self.root, self.questions).snapshot()
        self.assertEqual(snapshot["total"], 1)
        self.assertEqual([c["sha"] for c in snapshot["large_commits"]], [sizes["head"]])

        (repo / ".gitattributes").write_text("src/example.rs -diff\n")
        git("add", ".gitattributes")
        git("commit", "-qm", "Set later diff attributes")
        (repo / ".git/info/attributes").write_text("*.rs -diff\n")
        self.assertEqual(
            [c["known_lines"] for c in collect(base)["commits"][:2]], [199, 200]
        )

        (repo / "binary.dat").write_bytes(b"\0binary")
        git("add", ".")
        git("commit", "-qm", "Add binary fixture")
        self.assertFalse(collect(base)["complete"])

    def test_unknown_or_wrong_head_commit_counts_cannot_clear_coverage(self):
        self.assertFalse(self.app.snapshot()["commit_coverage_complete"])
        self.manifest["commit_sizes"] = {
            "head": "b" * 40,
            "complete": True,
            "commits": [{"sha": "b" * 40, "known_lines": 200}],
        }
        self.write_manifest()
        snapshot = workbench.Workbench(self.root, self.questions).snapshot()
        self.assertEqual(snapshot["large_commits"], [])
        self.assertFalse(snapshot["commit_coverage_complete"])

    def test_manual_feedback_refuses_a_stale_request(self):
        self.save({"motivation": "First explanation"})
        self.request_review()
        stale = self.app.snapshot()["request_id"]
        stale_feedback = self.feedback()
        self.save({"motivation": "Second explanation"})
        self.save({"motivation": "First explanation"})
        self.request_review()
        with self.assertRaisesRegex(ValueError, "changed or already has feedback"):
            self.app.feedback(self.feedback(), request={"request_id": stale})
        self.assertIsNone(self.app.snapshot()["feedback"])
        self.app.feedback(
            self.feedback(),
            request={"request_id": self.app.snapshot()["request_id"]},
        )
        with self.assertRaisesRegex(ValueError, "changed or already has feedback"):
            self.app.feedback(
                stale_feedback,
                request={"request_id": self.app.snapshot()["request_id"]},
            )

    def test_feedback_command_requires_the_request_id(self):
        result = subprocess.run(
            [
                sys.executable,
                str(HELPER),
                "feedback",
                "--bundle",
                str(self.root),
                "--questions",
                str(self.questions),
                "--file",
                str(self.root / "feedback.json"),
            ],
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("--request-id", result.stderr)

    def test_merging_upstream_does_not_count_incoming_lines_as_the_merge(self):
        repo = self.root / "merge-repo"
        repo.mkdir()
        env = {
            **os.environ,
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_CONFIG_NOSYSTEM": "1",
        }

        def git(*args):
            return subprocess.check_output(
                ["git", *args], cwd=repo, env=env, text=True
            ).strip()

        git("init", "-q", "-b", "main")
        git("config", "user.name", "Merge Test")
        git("config", "user.email", "merge@example.test")
        git("config", "commit.gpgsign", "false")
        (repo / "a.txt").write_text("a\n")
        git("add", ".")
        git("commit", "-qm", "Base")
        git("checkout", "-qb", "topic")
        (repo / "b.txt").write_text("b\n")
        git("add", ".")
        git("commit", "-qm", "Topic change")
        git("checkout", "-q", "main")
        (repo / "a.txt").write_text("a\n" + "upstream\n" * 250)
        git("commit", "-qam", "Upstream work")
        upstream = git("rev-parse", "HEAD")
        git("checkout", "-q", "topic")
        git("merge", "-q", "--no-ff", "-m", "Merge upstream", "main")
        sizes = json.loads(
            subprocess.check_output(
                [sys.executable, str(COMMIT_SIZES), "--base", upstream], cwd=repo
            )
        )
        self.assertEqual([c["known_lines"] for c in sizes["commits"]], [1, 0])

    def request_review(self):
        snapshot = self.app.snapshot()
        return self.app.request_review(
            {"identity": snapshot["identity"], "revision": snapshot["revision"]}
        )

    def test_agent_wait_only_receives_explicit_complete_submissions(self):
        with self.assertRaisesRegex(ValueError, "Explain your motivation"):
            self.request_review()
        saved = self.save()
        self.assertIsNone(self.app.wait(-1, 0))
        self.assertEqual(self.app.snapshot()["reviewer"], "offline")
        submitted = self.request_review()
        self.assertEqual(self.request_review()["revision"], submitted["revision"])
        received = self.app.wait(saved["revision"], 0)
        self.assertEqual(received["answers"], submitted["answers"])
        self.assertEqual(self.app.snapshot()["reviewer"], "reading")
        self.assertIsNone(self.app.wait(submitted["revision"], 0))
        self.app.feedback(self.feedback(approved=False))
        self.assertEqual(self.app.snapshot()["reviewer"], "offline")
        with self.assertRaisesRegex(ValueError, "already have feedback"):
            self.request_review()

    def test_changed_questions_refused_in_running_session(self):
        self.save()
        self.questions.write_text(
            self.questions.read_text().replace("Why persist", "Why write")
        )
        with self.assertRaisesRegex(ValueError, "Bundle changed"):
            self.app.snapshot()

    def test_wait_timeout_is_explicit_pending_not_a_submission(self):
        self.save()
        result = subprocess.run(
            [
                sys.executable,
                str(HELPER),
                "wait",
                "--bundle",
                str(self.root),
                "--questions",
                str(self.questions),
                "--timeout",
                "0",
            ],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 2)
        self.assertEqual(json.loads(result.stdout)["outcome"], "pending")

    def test_failed_replace_preserves_saved_answers(self):
        from unittest.mock import patch

        self.save({"motivation": "The first version"})
        with patch.object(workbench.os, "replace", side_effect=OSError("Disk failure")):
            with self.assertRaisesRegex(OSError, "Disk failure"):
                self.save({"motivation": "Must not replace the first version"})
        self.assertEqual(
            self.app.snapshot()["answers"]["motivation"], "The first version"
        )
        self.assertEqual(list(self.root.glob(".explanation-*")), [])

    def start_server(self, port=0):
        process = subprocess.Popen(
            [
                sys.executable,
                str(HELPER),
                "serve",
                "--port",
                str(port),
                "--bundle",
                str(self.root),
                "--questions",
                str(self.questions),
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )

        def stop():
            process.terminate()
            process.communicate(timeout=5)

        self.addCleanup(stop)
        self.assertTrue(
            select.select([process.stdout], [], [], 5)[0], "Server did not start"
        )
        url = urlsplit(process.stdout.readline().strip())
        self.assertEqual(url.hostname, "127.0.0.1")
        connection = http.client.HTTPConnection(url.hostname, url.port, timeout=5)
        self.addCleanup(connection.close)
        self.assertTrue(url.fragment.startswith("token="))
        return connection, url.fragment[len("token=") :]

    def test_occupied_port_advances_sequentially_and_prints_the_selected_port(self):
        listener = socket.socket()
        self.addCleanup(listener.close)
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        port = listener.getsockname()[1]
        if port == 65535:
            listener.close()
            listener = socket.socket()
            self.addCleanup(listener.close)
            listener.bind(("127.0.0.1", 0))
            listener.listen()
            port = listener.getsockname()[1]
        connection, token = self.start_server(port)
        self.assertGreater(connection.port, port)
        connection.request(
            "GET", "/api/session", headers={"Authorization": "Bearer " + token}
        )
        response = connection.getresponse()
        self.assertEqual(response.status, 200)
        self.assertEqual(
            json.loads(response.read())["manifest"]["head"], self.manifest["head"]
        )

    def test_cli_defaults_to_7777(self):
        from unittest.mock import patch

        with (
            patch.object(
                sys,
                "argv",
                [
                    str(HELPER),
                    "serve",
                    "--bundle",
                    str(self.root),
                    "--questions",
                    str(self.questions),
                ],
            ),
            patch.object(workbench, "server") as serve,
        ):
            workbench.main()
        self.assertEqual(serve.call_args.args[1], 7777)

    def test_binding_errors_do_not_skip_permissions_or_overflow_the_port_range(self):
        from unittest.mock import patch

        for port, code in [(7777, errno.EACCES), (65535, errno.EADDRINUSE)]:
            with patch.object(
                workbench,
                "ThreadingHTTPServer",
                side_effect=OSError(code, "fixture failure"),
            ) as factory:
                with self.assertRaisesRegex(ValueError, "Cannot bind review server"):
                    workbench.server(self.app, port)
                factory.assert_called_once()

    def test_busy_binding_attempts_only_consecutive_ports(self):
        from unittest.mock import MagicMock, patch

        httpd = MagicMock(server_port=7779)
        httpd.serve_forever.side_effect = KeyboardInterrupt
        with (
            patch.object(
                workbench,
                "ThreadingHTTPServer",
                side_effect=[
                    OSError(errno.EADDRINUSE, "busy"),
                    OSError(errno.EADDRINUSE, "busy"),
                    httpd,
                ],
            ) as factory,
            patch.object(sys, "stdout"),
            patch.object(sys, "stderr"),
        ):
            workbench.server(self.app, 7777)
        self.assertEqual(
            [call.args[0][1] for call in factory.call_args_list], [7777, 7778, 7779]
        )
        httpd.server_close.assert_called_once()

    def test_http_refuses_missing_token_foreign_origin_and_foreign_host(self):
        connection, token = self.start_server()
        for headers in (
            {},
            {"Authorization": "Bearer " + token, "Origin": "https://example.org"},
            {"Authorization": "Bearer " + token, "Host": "example.org"},
        ):
            connection.request("GET", "/api/session", headers=headers)
            response = connection.getresponse()
            response.read()
            self.assertEqual(response.status, 403)
        connection.request(
            "GET", "/api/session", headers={"Authorization": "Bearer " + token}
        )
        response = connection.getresponse()
        self.assertEqual(response.status, 200)
        self.assertEqual(json.loads(response.read())["status"], "draft")

    def test_http_save_and_payload_limit(self):
        connection, token = self.start_server()
        state = self.app.snapshot()
        payload = {
            "identity": state["identity"],
            "revision": 0,
            "answers": {"motivation": "Explain the failure"},
        }
        headers = {
            "Authorization": "Bearer " + token,
            "Content-Type": "application/json",
        }
        connection.request("PUT", "/api/answers", json.dumps(payload), headers)
        response = connection.getresponse()
        self.assertEqual(response.status, 200)
        self.assertEqual(
            json.loads(response.read())["answers"]["motivation"], "Explain the failure"
        )
        self.assertEqual(
            self.app.snapshot()["answers"]["motivation"], "Explain the failure"
        )
        connection.request(
            "PUT",
            "/api/answers",
            "",
            {**headers, "Content-Length": str(workbench.LIMIT + 1)},
        )
        response = connection.getresponse()
        response.read()
        self.assertEqual(response.status, 413)

    def test_finishing_needs_a_confirmed_explanation(self):
        with self.assertRaisesRegex(ValueError, "Confirm your reflection"):
            self.app.finish()
        self.save()
        self.request_review()
        self.app.feedback(self.feedback())
        with self.assertRaisesRegex(ValueError, "Confirm your reflection"):
            self.app.finish()
        s = self.app.snapshot()
        self.app.reflect(
            {
                "identity": s["identity"],
                "revision": s["revision"],
                "feedback_digest": s["feedback_digest"],
                "grade": "got",
                "correction": "",
            }
        )
        self.assertEqual(self.app.finish(), {"finished": True})

    def test_words_count_each_cjk_character(self):
        u = workbench.load_understanding()
        self.assertEqual(u.words("fix the stale cache"), 4)
        self.assertEqual(u.words("修正快取失效的問題"), 9)
        self.assertEqual(u.words("修正 cache 失效"), 5)

    def test_pasted_question_or_code_is_not_the_contributors_own_words(self):
        u = workbench.load_understanding()
        prompt = "Why should missing review evidence prevent this push and what problem does that solve"
        self.assertTrue(u.copied(prompt, [prompt]))
        self.assertTrue(u.copied("Because " + prompt + " matters", [prompt]))
        own = "Evidence is local, so the hook is the only place it can be required before review"
        self.assertFalse(u.copied(own, [prompt]))
        quoted = "It first reads `missing review evidence prevent this push and what problem` again"
        self.assertFalse(u.copied(quoted, [prompt]))
        cjk = "為什麼缺少審查證據時要阻止這次推送並說明貢獻者的問題"
        self.assertTrue(u.copied(cjk, [cjk]))

    def test_unsure_answers_are_always_sendable(self):
        u = workbench.load_understanding()
        for text in ("I don't know", "Not sure what the retry covers.", "不知道"):
            self.assertIsNone(u.problem(u.check(text, None, "")), text)
        self.assertIsNotNone(u.problem(u.check("Because it is better", None, "")))

    def test_send_refuses_thin_or_copied_answers_and_reports_what_is_unnamed(self):
        prompt = "Why does the retry budget stop persisting records before the reply is sent to the caller"
        self.questions.write_text(
            json.dumps([{"id": "core", "path": "src/example.rs", "prompt": prompt}])
        )
        (self.root / "pr.diff").write_text(
            "diff --git a/src/example.rs b/src/example.rs\n"
            "--- a/src/example.rs\n+++ b/src/example.rs\n@@ -1 +1 @@\n"
            "-spend_budget(retries)\n+persist_record(budget)\n"
        )
        self.app = workbench.Workbench(self.root, self.questions)
        connection, token = self.start_server()
        headers = {
            "Authorization": "Bearer " + token,
            "Content-Type": "application/json",
        }

        def put(path, payload):
            connection.request("PUT", path, json.dumps(payload), headers)
            response = connection.getresponse()
            return response.status, json.loads(response.read())

        def send(text):
            state = self.app.snapshot()
            status, _ = put(
                "/api/answers",
                {
                    "identity": state["identity"],
                    "revision": state["revision"],
                    "answers": {"motivation": text},
                },
            )
            self.assertEqual(status, 200)
            state = self.app.snapshot()
            return put(
                "/api/review",
                {"identity": state["identity"], "revision": state["revision"]},
            )

        status, body = send("Because it is better")
        self.assertEqual(
            (status, "A little more, please" in body["error"]), (409, True)
        )
        status, body = send(prompt)
        self.assertEqual(status, 409)
        self.assertIn("word for word", body["error"])
        thin = "The reply must not wait, so the work is deferred until the caller has what it asked for"
        status, _ = send(thin)
        self.assertEqual(status, 200)
        checks = self.app.snapshot()["submission"]["checks"]
        self.assertIs(checks["grounded"], False)
        self.assertIn("persist_record", checks["anchors"])
        status, _ = send(
            "The reply waits on `persist_record` so a crash cannot lose what the caller was told"
        )
        self.assertEqual(status, 200)
        self.assertIs(self.app.snapshot()["submission"]["checks"]["grounded"], True)

    def test_draft_checks_report_where_the_answer_stands_without_saving(self):
        (self.root / "pr.diff").write_text(
            "diff --git a/src/example.rs b/src/example.rs\n"
            "--- a/src/example.rs\n+++ b/src/example.rs\n@@ -1 +1 @@\n"
            "-spend_budget(retries)\n+persist_record(budget)\n"
        )
        app = workbench.Workbench(self.root, self.questions)

        def state(text):
            return app.check_draft({"identity": app.identity, "text": text})["state"]

        self.assertEqual(state(""), "empty")
        self.assertEqual(state("Too short"), "short")
        self.assertEqual(state("I don't know"), "unsure")
        long = "The reply must not wait, so the work is deferred until the caller has what it asked for"
        self.assertEqual(state(long), "ungrounded")
        self.assertEqual(state(long + " using `persist_record`"), "good")
        self.assertEqual(app.snapshot()["answers"], {})
        with self.assertRaisesRegex(ValueError, "Session changed"):
            app.check_draft({"identity": "x", "text": "y"})

    def test_message_names_unmentioned_code_only_when_there_is_something_to_name(self):
        (self.root / "pr.diff").write_text(
            "diff --git a/src/example.rs b/src/example.rs\n"
            "--- a/src/example.rs\n+++ b/src/example.rs\n@@ -1 +1 @@\n"
            "-spend_budget(retries)\n+persist_record(budget)\n"
        )
        app = workbench.Workbench(self.root, self.questions)
        checks = app.reported_checks({"motivation": "short"})
        self.assertEqual(checks["state"], "short")
        text = "The reply must not wait, so the work is deferred until the caller has what it asked for"
        checks = app.reported_checks({"motivation": text})
        self.assertEqual(checks["state"], "ungrounded")
        self.assertIn("persist_record", checks["anchors"])
        self.assertIn("Nothing from the changed lines", checks["message"])
        checks = app.reported_checks({"motivation": text + " via `persist_record`"})
        self.assertEqual(checks["state"], "good")

    def test_saying_unsure_does_not_excuse_pasting_the_question(self):
        u = workbench.load_understanding()
        prompt = "Why should missing review evidence prevent this push and what problem does that solve"
        pasted = "Not sure. " + prompt
        checks = u.check(pasted, {"prompt": prompt}, "")
        self.assertEqual(u.state(checks), "copied")
        self.assertIsNotNone(u.problem(checks))
        honest = u.check(
            "I don't know, I would read the caller first", {"prompt": prompt}, ""
        )
        self.assertEqual(u.state(honest), "unsure")
        self.assertIsNone(u.problem(honest))

    def test_punctuation_or_emoji_alone_is_not_an_answer(self):
        u = workbench.load_understanding()
        for text in ("!!!", "...", "\U0001f642\U0001f642\U0001f642"):
            checks = u.check(text, None, "")
            self.assertEqual(u.state(checks), "empty", text)
            self.assertIsNotNone(u.problem(checks), text)

    def test_cjk_characters_around_other_text_are_not_a_consecutive_copy(self):
        u = workbench.load_understanding()
        prompt = "證據 and 審查 and 推送 and 寫入 and 快取 and 提交 are the terms"
        own = "證據 first, 審查 next, 推送 then, 寫入 later, 快取 after, 提交 last"
        self.assertFalse(u.copied(own, [prompt]))
        cjk = "為什麼缺少審查證據時要阻止這次推送並說明貢獻者的問題"
        self.assertTrue(u.copied("因為" + cjk + "所以", [cjk]))

    def test_grounding_counts_every_changed_identifier_not_only_the_five_shown(self):
        u = workbench.load_understanding()
        names = [
            "alpha_one",
            "beta_two",
            "gamma_three",
            "delta_four",
            "eps_five",
            "zeta_six",
        ]
        patch = "".join(f"+{name}(1)\n" for name in names)
        checks = u.check(
            "The sixth step matters because zeta_six runs before the rest do",
            None,
            patch,
        )
        self.assertEqual(len(checks["anchors"]), 5)
        self.assertNotIn("zeta_six", checks["anchors"])
        self.assertIs(checks["grounded"], True)

    def test_a_typographic_apostrophe_still_says_unsure(self):
        u = workbench.load_understanding()
        for text in ("I don't know", "I don\u2019t know", "I do not know"):
            self.assertEqual(u.state(u.check(text, None, "")), "unsure", text)

    def test_wrapping_the_question_in_backticks_does_not_make_it_an_answer(self):
        u = workbench.load_understanding()
        prompt = "Why should missing review evidence prevent this push and what problem does that solve"
        for pasted in (f"`{prompt}`", f"```\n{prompt}\n```", f"See `{prompt}` ok"):
            checks = u.check(pasted, {"prompt": prompt}, "")
            self.assertIn(u.state(checks), ("empty", "short"), pasted)
            self.assertIsNotNone(u.problem(checks), pasted)
        own = "It reads `persist_record(budget)` first, then it decides whether the reply may be sent at all"
        self.assertIsNone(u.problem(u.check(own, None, "")))

    def test_ordinary_ways_of_saying_unsure_are_accepted(self):
        u = workbench.load_understanding()
        for text in (
            "I'm not sure",
            "I am not sure about the retry",
            "I have no idea",
            "我不知道",
            "我不太清楚這段",
            "不确定",
            "沒概念",
        ):
            self.assertEqual(u.state(u.check(text, None, "")), "unsure", text)
        self.assertNotEqual(u.state(u.check("I know exactly why", None, "")), "unsure")

    def test_naming_a_function_the_question_names_still_counts_as_naming_code(self):
        u = workbench.load_understanding()
        patch = "-old_call(budget)\n+persist_record(budget)\n"
        question = {"prompt": "Why does persist_record replace old_call here"}
        text = "persist_record runs first so a crash cannot lose what the caller was already told"
        self.assertIs(u.check(text, question, patch)["grounded"], True)
        # The hint still leaves out what the question already says.
        self.assertEqual(u.check(text, question, patch)["anchors"], ("budget",))

    def test_a_changed_line_that_starts_with_two_plus_signs_is_still_a_line(self):
        u = workbench.load_understanding()
        patch = "--- a/x.c\n+++ b/x.c\n@@ -1 +1 @@\n-old_name\n+++counter_value\n"
        names = u.identifiers(patch)
        self.assertIn("counter_value", names)
        self.assertNotIn("x", names)

    def test_the_snapshot_does_not_carry_the_checks(self):
        # They depend on the whole patch and nothing polls for them.
        self.assertNotIn("checks", self.app.snapshot())


if __name__ == "__main__":
    unittest.main()

"""Private lifecycle durability and reviewed-marker boundary tests (no network)."""

import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

HELPER = (
    Path(__file__).resolve().parents[1]
    / ".claude/skills/codetrial-review/review-lifecycle.py"
)
SPEC = importlib.util.spec_from_file_location("review_lifecycle", HELPER)
lifecycle = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(lifecycle)


class LifecycleTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bundle = self.root / "bundle"
        self.bundle.mkdir()
        self.head = "a" * 40
        self.manifest = {"head": self.head, "committed": True, "pr": None, "files": []}
        self.write_manifest()
        (self.bundle / "pr.diff").write_text("a diff\n")
        self.report = self.root / "report.json"
        self.report.write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "head": self.head,
                    "outcome": "complete",
                    "skills_used": ["codetrial-review"],
                    "paths": [],
                    "findings": [],
                    "refuted": [],
                    "questions": [],
                    "history_evidence": {},
                }
            )
        )
        lifecycle.begin(self.bundle)
        self.api = lifecycle.memory_api()
        self.output = patch("builtins.print")
        self.output.start()
        self.addCleanup(self.output.stop)
        self.common = self.root / ".git"
        self.common.mkdir()
        self.args = type(
            "Args",
            (),
            {
                "bundle": str(self.bundle),
                "report": str(self.report),
                "repo": "sysprog21/codetrial",
                "skill": "codetrial-review",
                "db": str(self.root / "memory.db"),
            },
        )()

    def write_manifest(self):
        (self.bundle / "manifest.json").write_text(json.dumps(self.manifest))

    def finish(self):
        with patch.object(lifecycle, "common_dir", return_value=self.common):
            return lifecycle.finish(self.args, self.api)

    def pending(self):
        return (
            self.common
            / "codetrial-review/pending"
            / lifecycle.load(self.bundle / "experience-start.json")["run_id"]
        )

    def test_begin_idempotent_and_changes_get_new_run(self):
        first = lifecycle.begin(self.bundle)
        self.assertEqual(first, lifecycle.begin(self.bundle))
        self.assertEqual(
            0o600, (self.bundle / "experience-start.json").stat().st_mode & 0o777
        )
        (self.bundle / "pr.diff").write_text("another diff")
        self.assertNotEqual(first["run_id"], lifecycle.begin(self.bundle)["run_id"])

    def test_real_record_and_retry_marker_deduplicated(self):
        self.assertEqual(0, self.finish())
        self.assertEqual(0, lifecycle.retry(self.pending(), self.api))
        marker = self.common / "codetrial-review/reviewed"
        self.assertEqual(self.head + "\n", marker.read_text())
        self.assertEqual(0o700, self.pending().stat().st_mode & 0o777)
        for filename in ["report.json", "pending.json", "attempt.json", "receipt.json"]:
            self.assertEqual(0o600, (self.pending() / filename).stat().st_mode & 0o777)
        self.assertEqual(
            self.head, lifecycle.load(self.pending() / "receipt.json")["head"]
        )

    def test_pr_dirty_and_incomplete_never_mark(self):
        for change in ["pr", "dirty", "incomplete"]:
            with self.subTest(change=change):
                self.manifest["pr"] = 12 if change == "pr" else None
                self.manifest["committed"] = change != "dirty"
                self.write_manifest()
                (self.bundle / "pr.diff").write_text(change)
                lifecycle.begin(self.bundle)
                report = lifecycle.load(self.report)
                report["outcome"] = (
                    "incomplete" if change == "incomplete" else "complete"
                )
                self.report.write_text(json.dumps(report))
                self.assertEqual(0, self.finish())
                self.assertFalse((self.common / "codetrial-review/reviewed").exists())

    def test_non_review_skill_records_without_certifying_code_review(self):
        self.args.skill = "codetrial-verify"
        report = lifecycle.load(self.report)
        report["skills_used"] = ["codetrial-verify"]
        report["coverage"] = (
            "Test and lint validation only. No source review performed."
        )
        self.report.write_text(json.dumps(report))
        self.assertEqual(0, self.finish())
        self.assertFalse((self.common / "codetrial-review/reviewed").exists())
        self.assertTrue((self.pending() / "receipt.json").exists())
        with sqlite3.connect(self.args.db) as db:
            rows = db.execute("SELECT skill,skills FROM local_review_runs").fetchall()
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0][0], "codetrial-verify")
        self.assertEqual(json.loads(rows[0][1]), ["codetrial-verify"])

    def test_combined_skill_with_explicit_review_can_certify_reviewed_head(self):
        self.args.skill = "codetrial-triage"
        report = lifecycle.load(self.report)
        report["skills_used"] = ["codetrial-triage", "codetrial-review"]
        self.report.write_text(json.dumps(report))
        self.assertEqual(0, self.finish())
        self.assertEqual(
            (self.common / "codetrial-review/reviewed").read_text(), self.head + "\n"
        )
        with sqlite3.connect(self.args.db) as db:
            rows = db.execute("SELECT skill,skills FROM local_review_runs").fetchall()
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0][0], "codetrial-triage")
        self.assertEqual(
            set(json.loads(rows[0][1])), {"codetrial-triage", "codetrial-review"}
        )

    def test_failure_keeps_sanitized_pending_and_retry_checks_bundle(self):
        report = lifecycle.load(self.report)
        report["coverage"] = "token ghp_" + "A" * 36
        self.report.write_text(json.dumps(report))
        failure = subprocess.CompletedProcess([], 1, "", "database busy")
        with patch.object(lifecycle.subprocess, "run", return_value=failure):
            self.assertEqual(1, self.finish())
        pending = self.pending()
        self.assertNotIn("ghp_" + "A" * 36, (pending / "report.json").read_text())
        self.assertTrue((pending / "attempt.json").exists())
        self.assertFalse((pending / "receipt.json").exists())
        self.assertEqual(0, lifecycle.retry(pending, self.api))
        (self.bundle / "pr.diff").write_text("changed")
        with self.assertRaises(ValueError):
            lifecycle.retry(pending, self.api)
        (self.bundle / "manifest.json").unlink()
        with self.assertRaises(OSError):
            lifecycle.retry(pending, self.api)

    def outside_scope_report(self):
        report = lifecycle.load(self.report)
        report["findings"] = [
            {
                "verdict": "confirmed",
                "path": "src/outside.rs",
                "claim": "c",
                "trigger": "t",
                "evidence": ["reproduced"],
            }
        ]
        return report

    def test_invalid_report_leaves_no_pending_and_correction_records(self):
        report = lifecycle.load(self.report)
        report["findings"] = [
            {"verdict": "confirmed", "claim": "c", "trigger": "t", "evidence": []}
        ]
        self.report.write_text(json.dumps(report))
        with self.assertRaises(ValueError):
            self.finish()
        self.assertFalse(self.pending().exists())
        report["findings"][0]["evidence"] = ["reproduced"]
        self.report.write_text(json.dumps(report))
        self.assertEqual(0, self.finish())
        self.assertTrue((self.pending() / "receipt.json").exists())

    def test_large_unicode_report_survives_retry(self):
        report = lifecycle.load(self.report)
        report["coverage"] = "\u00e9" * 200000
        self.report.write_text(json.dumps(report, ensure_ascii=False))
        self.assertEqual(0, self.finish())
        self.assertLessEqual(
            (self.pending() / "report.json").stat().st_size, 1024 * 1024
        )
        self.assertEqual(0, lifecycle.retry(self.pending(), self.api))

    def test_case_path_outside_empty_bundle_scope_is_refused(self):
        report = self.outside_scope_report()
        self.report.write_text(json.dumps(report))
        with self.assertRaisesRegex(ValueError, "outside reviewed scope"):
            self.finish()
        self.assertFalse(self.pending().exists())

    def test_new_store_file_is_private_from_creation(self):
        # Without the later chmod, only the creation mode can make it private.
        previous = os.umask(0o022)
        self.addCleanup(os.umask, previous)
        with patch.object(os, "chmod"):
            self.assertEqual(0, self.finish())
        self.assertEqual(0o600, (self.root / "memory.db").stat().st_mode & 0o777)

    def test_corrected_report_replaces_a_failed_unrecorded_attempt(self):
        failure = subprocess.CompletedProcess([], 1, "", "source missing")
        with patch.object(lifecycle.subprocess, "run", return_value=failure):
            self.assertEqual(1, self.finish())
        report = lifecycle.load(self.report)
        report["coverage"] = "corrected"
        self.report.write_text(json.dumps(report))
        self.assertEqual(0, self.finish())
        self.assertEqual(
            "corrected", lifecycle.load(self.pending() / "report.json")["coverage"]
        )
        kept = list((self.pending() / "superseded").iterdir())
        self.assertEqual(1, len(kept))
        self.assertTrue((kept[0] / "report.json").is_file())

    def test_case_path_without_a_declared_scope_is_refused(self):
        report = self.outside_scope_report()
        self.report.write_text(json.dumps(report))
        with self.assertRaisesRegex(ValueError, "outside reviewed scope"):
            self.finish()

    def test_case_path_without_bundle_or_declared_scope_is_refused(self):
        helper = self.api.experience_module()
        report = self.outside_scope_report()
        args = type(
            "Args",
            (),
            {
                "bundle": None,
                "skill": "codetrial-review",
                "run_id": None,
                "expected_manifest_hash": None,
                "expected_diff_hash": None,
            },
        )()
        with self.assertRaisesRegex(helper.Invalid, "outside reviewed scope"):
            helper.normalize(report, args, self.api)

    def test_stored_json_is_readable_up_to_the_limit_and_refused_past_it(self):
        limit = lifecycle.MAX_JSON
        overhead = len('{"k":""}') + 1
        path = self.root / "stored.json"
        for padding, accepted in (
            (limit - overhead - 1, True),
            (limit - overhead, True),
            (limit - overhead + 1, False),
        ):
            with self.subTest(padding=padding):
                value = {"k": "x" * padding}
                if accepted:
                    lifecycle.atomic(path, value, compact=True)
                    self.assertLessEqual(path.stat().st_size, limit)
                    self.assertEqual(value, lifecycle.load(path))
                else:
                    with self.assertRaises(ValueError):
                        lifecycle.atomic(path, value, compact=True)

    def test_case_provenance_is_kept_and_a_bad_lens_is_refused(self):
        helper = self.api.experience_module()
        args = type(
            "Args",
            (),
            {
                "bundle": None,
                "skill": "codetrial-review",
                "run_id": None,
                "expected_manifest_hash": None,
                "expected_diff_hash": None,
            },
        )()
        report = lifecycle.load(self.report)
        report["paths"] = ["src/a.rs"]
        case = {
            "verdict": "confirmed",
            "path": "src/a.rs",
            "claim": "c",
            "trigger": "t",
            "evidence": ["reproduced"],
        }
        report["findings"] = [
            dict(case, rule="judge", lens="failure", reviewer="codex")
        ]
        _, _, payload = helper.normalize(report, args, self.api)
        kept = payload["cases"][0]
        self.assertEqual(
            ("judge", "failure", "codex"),
            (kept["rule"], kept["lens"], kept["reviewer"]),
        )
        report["findings"] = [dict(case, lens="bogus")]
        with self.assertRaisesRegex(helper.Invalid, "unknown review lens"):
            helper.normalize(report, args, self.api)
        report["findings"] = [case]
        _, _, plain = helper.normalize(report, args, self.api)
        self.assertNotIn("lens", plain["cases"][0])

    def test_changed_report_cannot_replace_pending(self):
        self.assertEqual(0, self.finish())
        report = lifecycle.load(self.report)
        report["coverage"] = "different"
        self.report.write_text(json.dumps(report))
        with self.assertRaises(ValueError):
            self.finish()

    def test_retry_rejects_changed_manifest_eligibility(self):
        failure = subprocess.CompletedProcess([], 1, "", "busy")
        with patch.object(lifecycle.subprocess, "run", return_value=failure):
            self.assertEqual(1, self.finish())
        self.manifest["committed"] = False
        self.write_manifest()
        with patch.object(lifecycle.subprocess, "run") as run:
            with self.assertRaises(ValueError):
                lifecycle.retry(self.pending(), self.api)
            run.assert_not_called()
        self.assertFalse((self.common / "codetrial-review/reviewed").exists())

    def test_malformed_report_preserves_safe_pending_without_marker(self):
        failure = subprocess.CompletedProcess([], 1, "", "busy")
        with patch.object(lifecycle.subprocess, "run", return_value=failure):
            self.assertEqual(1, self.finish())
        pending = self.pending()
        original_report = (pending / "report.json").read_bytes()
        original_config = (pending / "pending.json").read_bytes()
        invalid = [
            '{"head":',
            '{"head":"' + self.head + '","head":"' + "b" * 40 + '"}',
            '{"number":NaN}',
            '{"number":Infinity}',
            '{"number":-Infinity}',
            '{"padding":"' + "x" * (1024 * 1024) + '"}',
        ]
        for text in invalid:
            with self.subTest(prefix=text[:50]):
                self.report.write_text(text)
                with patch.object(lifecycle.subprocess, "run") as run:
                    self.assertEqual(
                        1,
                        lifecycle.main(
                            [
                                "finish",
                                "--bundle",
                                str(self.bundle),
                                "--report",
                                str(self.report),
                            ]
                        ),
                    )
                    run.assert_not_called()
                self.assertEqual(
                    original_report, (pending / "report.json").read_bytes()
                )
                self.assertEqual(
                    original_config, (pending / "pending.json").read_bytes()
                )
                self.assertFalse((self.common / "codetrial-review/reviewed").exists())

    def test_hidden_report_path_is_refused_before_sanitization(self):
        original = lifecycle.load(self.report)
        unsafe = "src/mi\u200bc.rs"
        for field in ("paths", "findings", "refuted", "questions"):
            report = dict(original)
            report[field] = [unsafe] if field == "paths" else [{"path": unsafe}]
            self.report.write_text(json.dumps(report))
            with self.subTest(field=field):
                with patch.object(lifecycle.subprocess, "run") as run:
                    with self.assertRaisesRegex(ValueError, "path identity"):
                        self.finish()
                    run.assert_not_called()
                self.assertFalse((self.common / "codetrial-review/reviewed").exists())
                self.assertFalse((self.common / "codetrial-review/pending").exists())
        self.manifest["files"] = [{"path": unsafe}]
        self.write_manifest()
        with self.assertRaisesRegex(ValueError, "path identity"):
            lifecycle.begin(self.bundle)

    def test_manifest_has_separate_bounded_input_limit(self):
        self.manifest["metadata"] = "x" * (1024 * 1024 + 1)
        self.write_manifest()
        self.assertEqual(self.head, lifecycle.begin(self.bundle)["head"])
        self.assertEqual(0, self.finish())
        self.assertEqual(
            lifecycle.load(self.pending() / "receipt.json")["record_outcome"],
            "recorded",
        )
        self.manifest["metadata"] = "x" * (8 * 1024 * 1024)
        self.write_manifest()
        with self.assertRaisesRegex(ValueError, "size limit"):
            lifecycle.begin(self.bundle)

    def test_bundle_flip_before_real_record_never_persists_misbound_run(self):
        original = (self.bundle / "pr.diff").read_bytes()
        execute = subprocess.run

        def moved_bundle(command, **kwargs):
            (self.bundle / "pr.diff").write_bytes(
                b"changed before the recorder reads it"
            )
            return execute(command, **kwargs)

        with patch.object(lifecycle.subprocess, "run", side_effect=moved_bundle):
            self.assertEqual(1, self.finish())
        pending = self.pending()
        self.assertTrue((pending / "attempt.json").exists())
        self.assertFalse((pending / "receipt.json").exists())
        self.assertFalse((self.common / "codetrial-review/reviewed").exists())
        with sqlite3.connect(self.args.db) as db:
            self.assertEqual(
                db.execute("SELECT count(*) FROM local_review_runs").fetchone()[0], 0
            )
        (self.bundle / "pr.diff").write_bytes(original)
        self.assertEqual(0, lifecycle.retry(pending, self.api))
        with sqlite3.connect(self.args.db) as db:
            self.assertEqual(
                db.execute("SELECT count(*) FROM local_review_runs").fetchone()[0], 1
            )

    def test_finish_requires_unchanged_explicit_start(self):
        original = (self.bundle / "experience-start.json").read_bytes()
        for change in ("diff", "manifest"):
            with self.subTest(change=change):
                (self.bundle / "pr.diff").write_text(
                    "changed" if change == "diff" else "a diff\n"
                )
                if change == "manifest":
                    self.manifest["committed"] = False
                    self.write_manifest()
                with patch.object(lifecycle.subprocess, "run") as run:
                    with self.assertRaisesRegex(ValueError, "changed since begin"):
                        self.finish()
                    run.assert_not_called()
                self.assertEqual(
                    original, (self.bundle / "experience-start.json").read_bytes()
                )
                self.assertFalse((self.common / "codetrial-review/reviewed").exists())
                self.assertFalse((self.common / "codetrial-review/pending").exists())

    def test_finish_refuses_missing_start(self):
        (self.bundle / "experience-start.json").unlink()
        with patch.object(lifecycle.subprocess, "run") as run:
            with self.assertRaisesRegex(ValueError, "missing experience start"):
                self.finish()
            run.assert_not_called()
        self.assertFalse((self.bundle / "experience-start.json").exists())
        self.assertFalse((self.common / "codetrial-review/reviewed").exists())

    def test_cli_environment_store_semantics(self):
        environment = dict(
            os.environ, CODETRIAL_MEMORY=str(self.root / "environment.db")
        )
        subprocess.run(["git", "init", "-q", str(self.root / "checkout")], check=True)
        result = subprocess.run(
            [
                sys.executable,
                str(HELPER),
                "finish",
                "--bundle",
                str(self.bundle),
                "--report",
                str(self.report),
            ],
            cwd=str(self.root / "checkout"),
            env=environment,
            capture_output=True,
            text=True,
        )
        self.assertEqual(0, result.returncode, result.stderr)
        self.assertTrue((self.root / "environment.db").exists())


class ExplainedTests(unittest.TestCase):
    """What closeout makes of an explanation file, whatever is in it."""

    @staticmethod
    def digest(answers):
        import hashlib

        return hashlib.sha256(json.dumps(answers, sort_keys=True).encode()).hexdigest()

    def test_feedback_and_a_reflection_for_the_current_answers_count(self):
        answers = {"motivation": "why"}
        digest = self.digest(answers)
        state = {
            "answers": answers,
            "feedback": {"answer_digest": digest},
            "reflection": {"answer_digest": digest},
        }
        self.assertTrue(lifecycle.explained(state))
        state["answers"] = {"motivation": "changed"}
        self.assertFalse(lifecycle.explained(state))

    def test_a_malformed_file_is_not_explained_and_is_not_a_crash(self):
        digest = self.digest({})
        for state in (
            [],
            "text",
            None,
            {"feedback": [], "reflection": {"answer_digest": digest}},
            {"feedback": {"answer_digest": digest}, "reflection": "yes"},
            {"feedback": {}, "reflection": {}, "answers": {1, 2}},
        ):
            self.assertFalse(lifecycle.explained(state), state)


if __name__ == "__main__":
    unittest.main()

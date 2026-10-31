#!/usr/bin/env python3
"""Contract tests for scripts/check-recovery-matrix.py.

The real ledger is private working evidence and stays outside the checkout, so
every ledger here is synthetic: two phases crossed with the six cause classes
(twelve rows) under a baseline file of its own, a hand-written test list in
`cargo test -- --list` format, a scratch source tree and, for `record`, a fake
cargo. Nothing here runs cargo or touches a credential.

Every failure fixture starts from a ledger that passes strict mode and changes
one thing, so the class named in its label is the only thing that can fail it.
"""

import copy
import hashlib
import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TOOL = ROOT / "scripts" / "check-recovery-matrix.py"

SUPPORT_SPEC = importlib.util.spec_from_file_location(
    "support", Path(__file__).with_name("support.py")
)
support = importlib.util.module_from_spec(SUPPORT_SPEC)
SUPPORT_SPEC.loader.exec_module(support)

SPEC = importlib.util.spec_from_file_location("check_recovery_matrix", TOOL)
crm = importlib.util.module_from_spec(SPEC)
sys.modules["check_recovery_matrix"] = crm
SPEC.loader.exec_module(crm)

PHASES = ("alpha_wait", "beta_write")
CAUSES = crm.CAUSES
MOD = "livekit::recovery::tests::"
TESTS = ["alpha_cancels", "beta_cancels", "shared_notice", "pinned_finish"]
RUN, ACTION = "run-1", "end"

N_FAILURE_FIXTURES = 8


def sha256(data):
    return hashlib.sha256(data).hexdigest()


class Fixture:
    """A scratch tree holding one passing ledger and everything it resolves to."""

    def __init__(self, directory):
        self.dir = Path(directory)
        self.src = self.dir / "src"
        self.src.mkdir()
        (self.dir / "Cargo.toml").write_text('[package]\nname = "x"\n')
        (self.src / "lib.rs").write_text("pub fn f() {}\n")
        self.baseline = self.dir / "baseline.json"
        self.baseline.write_text(json.dumps([[p, c] for p in PHASES for c in CAUSES]))
        self.list = self.dir / "list.txt"
        self.list.write_text(
            "".join("%s%s: test\n" % (MOD, n) for n in TESTS)
            + "livekit::other::tests::unrelated: test\n\n9 tests, 0 benchmarks\n"
        )
        self.sdk = self.dir / "sdk"
        self.write_sdk()
        self.ledger = self.dir / "ledger.json"
        self.receipts = self.dir / "receipts.json"
        self.rows = self.passing_rows()
        self.receipt_list = [self.receipt(MOD + n) for n in TESTS]

    def digest(self):
        return crm.source_digest(self.dir)

    def receipt(self, full, **over):
        base = {
            "test": full,
            "command": ["cargo", "test", "--locked", "--lib", full, "--", "--exact"],
            "exit": 0,
            "discovered": 1,
            "passed": 1,
            "source_digest": self.digest(),
        }
        base.update(over)
        return base

    def write_sdk(self, **result_over):
        run = self.sdk / RUN
        run.mkdir(parents=True, exist_ok=True)
        manifest = b'{"run_id": "run-1"}\n'
        (run / "run-manifest.json").write_bytes(manifest)
        result = {
            "run_id": RUN,
            "manifest_sha256": sha256(manifest),
            "child_exit": 0,
            "status": "complete",
            "budget_failure": None,
            "hard": {
                "scripted_actions": {
                    "granted": 1,
                    "granted_ids": [ACTION],
                    "dispatched_without_permit": [],
                }
            },
            "telemetry": {"events": 3, "malformed": 0, "heartbeat_seen": True},
        }
        result.update(result_over)
        (run / "budget-result.json").write_text(json.dumps(result))

    def write_live(self, run_id, end_mode, granted, end_permit, **result_over):
        """A live-fixture run: complete, reaped, resource samples, no heartbeats."""
        run = self.sdk / run_id
        run.mkdir(parents=True, exist_ok=True)
        manifest = json.dumps({"run_id": run_id, "end_mode": end_mode}).encode()
        (run / "run-manifest.json").write_bytes(manifest)
        result = {
            "run_id": run_id,
            "manifest_sha256": sha256(manifest),
            "child_exit": 0,
            "status": "complete",
            "budget_failure": None,
            "cleanup": {"failures": [], "leaked_children": 0, "reaped": True},
            "resources": {"samples": 2},
            "hard": {
                "scripted_actions": {
                    "granted": len(granted),
                    "granted_ids": list(granted),
                    "dispatched_without_permit": [],
                    "end_permit_used": end_permit,
                }
            },
            "telemetry": {"events": 0, "heartbeat_seen": False, "malformed": 0},
        }
        result.update(result_over)
        (run / "budget-result.json").write_text(json.dumps(result))

    def passing_rows(self):
        rows = []
        for phase in PHASES:
            for cause in CAUSES:
                row = {
                    "phase": phase,
                    "cause": cause,
                    "status": "covered_local_terminal_cleanup",
                    "test": "alpha_cancels" if phase == PHASES[0] else "beta_cancels",
                    "reason": "the cancel path joins the reader and keeps usage",
                    "sdk_report": {"run_id": RUN, "action_id": ACTION},
                }
                if cause == "Finish":
                    row.update(
                        status="N/A",
                        test="pinned_finish",
                        reason="The sole producer in src/agent/events.rs is intercepted as End before the reducer.",
                    )
                    del row["sdk_report"]
                elif cause == "RoomEventsEof":
                    row.update(
                        status="covered_shared",
                        test="shared notice matrix (all notice rows)",
                        tests=["shared_notice", "alpha_cancels"],
                        reason="The reader EOF maps to the same RoomClosed terminal as RoomDisconnected in src/livekit/recovery.rs.",
                    )
                rows.append(row)
        return rows

    def run(self, mode="strict", *extra, rows=None, receipts=True, sdk=True):
        self.ledger.write_text(json.dumps(self.rows if rows is None else rows))
        self.receipts.write_text(
            json.dumps({"schema": 1, "receipts": self.receipt_list})
        )
        command = [
            sys.executable,
            str(TOOL),
            "check",
            str(self.ledger),
            "--mode",
            mode,
            "--json",
            "--source-root",
            str(self.dir),
            "--test-list",
            str(self.list),
            "--synthetic-baseline",
            str(self.baseline),
        ]
        if receipts:
            command += ["--receipts", str(self.receipts)]
        if sdk:
            command += ["--sdk-receipts", str(self.sdk)]
        done = subprocess.run(
            command + list(extra),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            universal_newlines=True,
        )
        try:
            report = json.loads(done.stdout)
        except ValueError:
            report = None
        return done.returncode, report, done.stderr


def kinds(report):
    return sorted({entry[1] for entry in report["integrity"] + report["gaps"]})


class Base(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory(prefix="crm-")
        self.addCleanup(tmp.cleanup)
        self.fx = Fixture(tmp.name)

    def strict(self, **kw):
        return self.fx.run("strict", **kw)

    def assertFails(self, kind, **kw):
        code, report, err = self.strict(**kw)
        self.assertEqual(code, 1, err)
        self.assertIn(kind, kinds(report))
        return report


class PassingLedger(Base):
    def test_strict_passes_a_fully_resolved_ledger(self):
        code, report, err = self.strict()
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(report["rows"], 12)
        self.assertEqual(report["integrity"] + report["gaps"], [])
        self.assertEqual(
            report["by_status"],
            {"covered_local_terminal_cleanup": 8, "covered_shared": 2, "N/A": 2},
        )

    def test_a_fully_qualified_test_name_resolves_exactly(self):
        self.fx.rows[0]["test"] = MOD + "alpha_cancels"
        self.fx.rows[0].pop("tests", None)
        self.assertEqual(self.strict()[0], 0)

    def test_the_default_text_report_names_the_verdict(self):
        self.fx.ledger.write_text(json.dumps(self.fx.rows))
        self.fx.receipts.write_text(
            json.dumps({"schema": 1, "receipts": self.fx.receipt_list})
        )
        done = subprocess.run(
            [
                sys.executable,
                str(TOOL),
                "check",
                str(self.fx.ledger),
                "--mode",
                "strict",
                "--source-root",
                str(self.fx.dir),
                "--test-list",
                str(self.fx.list),
                "--synthetic-baseline",
                str(self.fx.baseline),
                "--receipts",
                str(self.fx.receipts),
                "--sdk-receipts",
                str(self.fx.sdk),
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            universal_newlines=True,
        )
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn("12 rows", done.stdout)
        self.assertTrue(done.stdout.strip().endswith("ok"))


class Incremental(Base):
    def test_uncovered_rows_are_reported_and_exit_zero(self):
        row = self.fx.rows[0]
        row.update(
            status="uncovered",
            test="",
            reason="open: needs a seam",
            sdk_report="uncovered required native SDK run",
        )
        code, report, _ = self.fx.run("incremental")
        self.assertEqual(code, 0)
        self.assertEqual(
            report["gaps"], [["alpha_wait/End", "uncovered", "open: needs a seam"]]
        )

    def test_strict_fails_the_same_uncovered_row(self):
        row = self.fx.rows[0]
        row.update(status="uncovered", test="", reason="open: needs a seam")
        self.assertFails("uncovered")

    def test_pending_sdk_rows_pass_incremental_and_fail_strict(self):
        for row in self.fx.rows:
            if "sdk_report" in row:
                row["sdk_report"] = "uncovered required native SDK run"
        code, report, _ = self.fx.run("incremental")
        self.assertEqual((code, kinds(report)), (0, ["sdk_pending"]))
        self.assertEqual(len(report["gaps"]), 10)
        report = self.assertFails("sdk_pending")
        self.assertEqual(kinds(report), ["sdk_pending"])

    def test_missing_receipts_are_gaps_in_incremental_mode(self):
        code, report, _ = self.fx.run("incremental", receipts=False)
        self.assertEqual((code, kinds(report)), (0, ["no_receipt"]))
        self.assertFails("no_receipt", receipts=False)

    def test_ledger_lies_fail_even_incremental(self):
        self.fx.rows.append(copy.deepcopy(self.fx.rows[0]))
        code, report, _ = self.fx.run("incremental")
        self.assertEqual(code, 1)
        self.assertIn("duplicate", kinds(report))


class FailureFixtures(Base):
    """One whole passing ledger with one thing wrong, per failure class."""

    def fixtures(self):
        fx = self.fx
        out = {}

        def row_edit(name, **change):
            def apply():
                fx.rows[0].update(change)

            out[name] = (apply, {})

        row_edit("missing_test", test="no_such_recovery_test")
        out["zero_test_success"] = (
            lambda: fx.receipt_list.__setitem__(
                0, fx.receipt(MOD + "alpha_cancels", discovered=0, passed=0)
            ),
            {},
        )
        out["duplicate_row"] = (lambda: fx.rows.append(copy.deepcopy(fx.rows[3])), {})
        out["stale_receipt"] = (
            lambda: fx.receipt_list.__setitem__(
                0, fx.receipt(MOD + "alpha_cancels", source_digest="0" * 64)
            ),
            {},
        )
        out["fabricated_sdk"] = (
            lambda: fx.rows[0].update(
                sdk_report={"run_id": "never-ran", "action_id": ACTION}
            ),
            {},
        )
        out["dropped_row"] = (lambda: fx.rows.pop(5), {})
        row_edit("unknown_cause", cause="Sometimes")
        row_edit("na_without_reason", status="N/A", reason="")
        return out

    EXPECT = {
        "missing_test": "missing_test",
        "zero_test_success": "zero_test",
        "duplicate_row": "duplicate",
        "stale_receipt": "stale_receipt",
        "fabricated_sdk": "fabricated_sdk",
        "dropped_row": "dropped_or_extra",
        "unknown_cause": "unknown_cause",
        "na_without_reason": "missing_reason",
    }

    def test_the_fixture_count_is_asserted(self):
        self.assertEqual(len(self.fixtures()), N_FAILURE_FIXTURES)
        self.assertEqual(set(self.fixtures()), set(self.EXPECT))

    def test_each_failure_class_fails_strict_with_its_own_kind(self):
        for name in sorted(self.EXPECT):
            with self.subTest(name):
                self.setUp()
                self.fixtures()[name][0]()
                code, report, err = self.strict()
                self.assertEqual(code, 1, err)
                self.assertIn(self.EXPECT[name], kinds(report))

    def test_each_failure_fixture_is_the_only_thing_wrong(self):
        # The unmodified ledger passes, so a fixture can only fail for its own change.
        self.assertEqual(self.strict()[0], 0)

    def test_a_dropped_row_is_not_hidden_by_a_replacement_duplicate(self):
        self.fx.rows[5] = copy.deepcopy(self.fx.rows[4])
        code, report, _ = self.strict()
        self.assertEqual(code, 1)
        self.assertTrue(
            {"duplicate", "dropped_or_extra", "baseline_mismatch"} <= set(kinds(report))
        )

    def test_an_extra_row_outside_the_baseline_fails(self):
        extra = copy.deepcopy(self.fx.rows[0])
        extra["phase"] = "gamma_extra"
        self.fx.rows.append(extra)
        self.assertFails("dropped_or_extra")


class TestResolution(Base):
    def test_a_descriptive_multi_test_field_needs_an_explicit_list(self):
        self.fx.rows[5].pop(
            "tests"
        )  # RoomEventsEof row names 'shared notice matrix (...)'
        report = self.assertFails("multi_test")
        self.assertTrue(
            any(e[0] == "alpha_wait/RoomEventsEof" for e in report["integrity"])
        )

    def test_a_comma_list_in_the_test_field_must_be_fully_listed(self):
        self.fx.rows[5]["test"] = "shared_notice, alpha_cancels, beta_cancels"
        report = self.assertFails("multi_test")
        self.assertIn("beta_cancels", json.dumps(report["integrity"]))
        self.fx.rows[5]["tests"] = ["shared_notice", "alpha_cancels", "beta_cancels"]
        self.assertEqual(self.strict()[0], 0)

    def test_every_listed_test_must_resolve(self):
        self.fx.rows[5]["tests"] = ["shared_notice", "vanished_test"]
        report = self.assertFails("missing_test")
        self.assertIn("vanished_test", json.dumps(report["gaps"]))

    def test_every_listed_test_needs_its_own_receipt(self):
        self.fx.receipt_list = [
            r for r in self.fx.receipt_list if not r["test"].endswith("shared_notice")
        ]
        report = self.assertFails("no_receipt")
        self.assertIn("shared_notice", json.dumps(report["gaps"]))

    def test_an_ambiguous_short_name_is_not_guessed(self):
        with open(self.fx.list, "a") as handle:
            handle.write("livekit::elsewhere::tests::alpha_cancels: test\n")
        report = self.assertFails("missing_test")
        self.assertIn("ambiguous", json.dumps(report["gaps"]))

    def test_the_test_field_must_appear_in_its_own_list(self):
        self.fx.rows[5]["test"] = "beta_cancels"  # a single name, not in tests
        self.assertFails("multi_test")

    def test_a_pinned_not_applicable_test_must_also_run(self):
        self.fx.receipt_list = [
            r for r in self.fx.receipt_list if not r["test"].endswith("pinned_finish")
        ]
        report = self.assertFails("no_receipt")
        self.assertIn("pinned_finish", json.dumps(report["gaps"]))

    def test_a_failed_run_is_not_a_receipt(self):
        self.fx.receipt_list[0] = self.fx.receipt(MOD + "alpha_cancels", exit=101)
        self.assertFails("failed_receipt")

    def test_a_receipt_must_run_the_test_with_exact(self):
        self.fx.receipt_list[0] = self.fx.receipt(
            MOD + "alpha_cancels", command=["cargo", "test", "alpha_cancels"]
        )
        self.assertFails("failed_receipt")

    def test_either_count_alone_being_zero_is_a_zero_test_run(self):
        # Each count is checked on its own: a receipt claiming one but not
        # the other is as untrustworthy as one claiming neither.
        for counts in ({"discovered": 0, "passed": 1}, {"discovered": 1, "passed": 0}):
            with self.subTest(counts):
                self.fx.receipt_list[0] = self.fx.receipt(
                    MOD + "alpha_cancels", **counts
                )
                self.assertFails("zero_test")

    def test_the_latest_receipt_for_a_test_decides(self):
        stale = self.fx.receipt(MOD + "alpha_cancels", source_digest="1" * 64)
        good = self.fx.receipt(MOD + "alpha_cancels")
        self.fx.receipt_list = [stale, good] + self.fx.receipt_list[1:]
        self.assertEqual(self.strict()[0], 0)
        self.fx.receipt_list = [good, stale] + self.fx.receipt_list[2:]
        self.assertFails("stale_receipt")

    def test_a_run_whose_counts_disagree_is_not_a_receipt(self):
        self.fx.receipt_list[0] = self.fx.receipt(
            MOD + "alpha_cancels", discovered=2, passed=1
        )
        self.assertFails("failed_receipt")

    def test_a_float_or_boolean_exit_is_not_success(self):
        for value in (0.0, False):
            with self.subTest(value):
                self.fx.receipt_list[0] = self.fx.receipt(
                    MOD + "alpha_cancels", exit=value
                )
                self.assertFails("failed_receipt")

    def test_a_command_that_is_not_cargo_test_exact_is_not_a_receipt(self):
        self.fx.receipt_list[0] = self.fx.receipt(
            MOD + "alpha_cancels", command=["true", "--exact", MOD + "alpha_cancels"]
        )
        self.assertFails("failed_receipt")

    def test_changing_an_embedded_web_asset_stales_every_receipt(self):
        (self.fx.dir / "web").mkdir()
        (self.fx.dir / "web" / "app.js").write_text("a")
        self.fx.receipt_list = [self.fx.receipt(MOD + n) for n in TESTS]
        self.assertEqual(self.strict()[0], 0)
        (self.fx.dir / "web" / "app.js").write_text("b")
        self.assertFails("stale_receipt")

    def test_editing_a_source_stales_every_receipt(self):
        self.fx.ledger.write_text("[]")
        (self.fx.src / "lib.rs").write_text("pub fn f() { /* changed */ }\n")
        self.assertFails("stale_receipt")


class SdkReceipts(Base):
    def test_a_run_that_never_granted_the_action_is_fabricated(self):
        self.fx.rows[0]["sdk_report"] = {"run_id": RUN, "action_id": "other"}
        self.assertFails("fabricated_sdk")

    def test_an_unfinished_run_does_not_back_a_row(self):
        self.fx.write_sdk(status="incomplete_budget")
        self.assertFails("fabricated_sdk")

    def test_a_failed_child_does_not_back_a_row(self):
        self.fx.write_sdk(child_exit=1)
        self.assertFails("fabricated_sdk")

    def test_a_run_with_test_overrides_does_not_back_a_row(self):
        self.fx.write_sdk(test_overrides={"min_cleanup_reserve_s": 1})
        self.assertFails("fabricated_sdk")

    def test_a_result_whose_manifest_changed_does_not_back_a_row(self):
        (self.fx.sdk / RUN / "run-manifest.json").write_bytes(
            b'{"run_id": "run-1", "edited": true}\n'
        )
        self.assertFails("fabricated_sdk")

    def test_a_run_without_telemetry_events_does_not_back_a_row(self):
        self.fx.write_sdk(telemetry={"events": 0})
        self.assertFails("fabricated_sdk")

    def test_two_results_for_one_run_are_unusable_not_last_wins(self):
        other = self.fx.sdk / "copy"
        other.mkdir()
        (other / "budget-result.json").write_text(
            (self.fx.sdk / RUN / "budget-result.json").read_text()
        )
        code, report, err = self.strict()
        self.assertEqual((code, report), (2, None))
        self.assertIn("more than one budget-result.json", err)

    def test_a_claim_without_an_sdk_directory_is_fabricated(self):
        self.assertFails("fabricated_sdk", sdk=False)

    def test_free_text_coverage_claims_are_fabricated(self):
        self.fx.rows[0]["sdk_report"] = "covered by the native SDK fixture"
        self.assertFails("fabricated_sdk")

    def test_a_fabricated_claim_fails_incremental_too(self):
        self.fx.rows[0]["sdk_report"] = {"run_id": "never-ran", "action_id": ACTION}
        self.assertEqual(self.fx.run("incremental")[0], 1)


class LedgerShape(Base):
    def test_a_placeholder_reason_does_not_justify_covered_shared(self):
        self.fx.rows[5]["reason"] = "n/a"
        self.assertFails("missing_reason")

    def test_a_na_reason_must_name_a_source(self):
        self.fx.rows[4]["reason"] = (
            "There is simply nothing that can ever happen here at all."
        )
        self.assertFails("missing_reason")

    def test_a_phase_name_with_a_slash_is_malformed(self):
        self.fx.rows[0]["phase"] = "alpha/wait"
        self.assertFails("malformed")

    def test_an_unknown_status_fails(self):
        self.fx.rows[0]["status"] = "mostly_covered"
        self.assertFails("bad_status")

    def test_a_malformed_row_fails_without_crashing(self):
        self.fx.rows[0] = ["not", "a", "row"]
        self.assertFails("malformed")

    def test_the_six_cause_classes_are_the_pinned_set(self):
        self.assertEqual(
            list(CAUSES),
            [
                "End",
                "Deadline",
                "RoomDisconnected",
                "Absent",
                "Finish",
                "RoomEventsEof",
            ],
        )

    def test_the_pinned_baseline_is_29_phases_by_6_causes(self):
        self.assertEqual((crm.BASELINE_ROWS, crm.BASELINE_PHASES), (174, 29))
        self.assertEqual(crm.BASELINE_ROWS, crm.BASELINE_PHASES * len(CAUSES))


class Usage(Base):
    def test_a_missing_ledger_is_unusable_not_a_pass(self):
        done = subprocess.run(
            [
                sys.executable,
                str(TOOL),
                "check",
                str(self.fx.dir / "absent.json"),
                "--test-list",
                str(self.fx.list),
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            universal_newlines=True,
        )
        self.assertEqual(done.returncode, 2)
        self.assertIn("cannot read ledger", done.stderr)

    def test_a_malformed_receipts_file_is_unusable(self):
        self.fx.rows  # ledger written by run(); then corrupt the receipts
        self.fx.ledger.write_text(json.dumps(self.fx.rows))
        self.fx.receipts.write_text('{"schema": 9}')
        done = subprocess.run(
            [
                sys.executable,
                str(TOOL),
                "check",
                str(self.fx.ledger),
                "--test-list",
                str(self.fx.list),
                "--receipts",
                str(self.fx.receipts),
                "--source-root",
                str(self.fx.dir),
                "--synthetic-baseline",
                str(self.fx.baseline),
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            universal_newlines=True,
        )
        self.assertEqual(done.returncode, 2)

    def test_malformed_sdk_results_fail_the_row_without_crashing(self):
        for over in (
            {"hard": []},
            {"hard": {"scripted_actions": {"granted_ids": ACTION}}},
            {"hard": {"scripted_actions": {"granted_ids": 7}}},
            {"telemetry": []},
        ):
            with self.subTest(over):
                self.fx.write_sdk(**over)
                self.assertFails("fabricated_sdk")

    def test_a_non_utf8_ledger_is_unusable_not_a_traceback(self):
        self.fx.ledger.write_bytes(b"\xff\xfe[")
        done = subprocess.run(
            [
                sys.executable,
                str(TOOL),
                "check",
                str(self.fx.ledger),
                "--test-list",
                str(self.fx.list),
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            universal_newlines=True,
        )
        self.assertEqual(done.returncode, 2)
        self.assertNotIn("Traceback", done.stderr)

    def test_a_missing_cargo_is_unusable_not_a_traceback(self):
        self.fx.ledger.write_text(json.dumps(self.fx.rows))
        done = subprocess.run(
            [
                sys.executable,
                str(TOOL),
                "record",
                str(self.fx.ledger),
                "--out",
                str(self.fx.dir / "o.json"),
                "--cargo",
                str(self.fx.dir / "no-cargo"),
                "--test-list",
                str(self.fx.list),
                "--source-root",
                str(self.fx.dir),
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            universal_newlines=True,
        )
        self.assertEqual(done.returncode, 2)
        self.assertNotIn("Traceback", done.stderr)

    def test_the_pinned_baseline_applies_unless_a_synthetic_one_is_named(self):
        # The twelve-row ledger is not the 174-row baseline.
        code, report, _ = self.fx.run("incremental")
        self.assertEqual((code, report["baseline"]), (0, "synthetic"))
        self.fx.ledger.write_text(json.dumps(self.fx.rows))
        done = subprocess.run(
            [
                sys.executable,
                str(TOOL),
                "check",
                str(self.fx.ledger),
                "--json",
                "--source-root",
                str(self.fx.dir),
                "--test-list",
                str(self.fx.list),
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            universal_newlines=True,
        )
        report = json.loads(done.stdout)
        self.assertEqual((done.returncode, report["baseline"]), (1, "pinned"))
        self.assertIn("dropped_or_extra", kinds(report))

    def test_digest_prints_the_current_source_digest(self):
        done = subprocess.run(
            [sys.executable, str(TOOL), "digest", "--source-root", str(self.fx.dir)],
            stdout=subprocess.PIPE,
            universal_newlines=True,
        )
        self.assertEqual(done.stdout.strip(), self.fx.digest())


FAKE_CARGO = """#!/usr/bin/env python3
import sys
args = sys.argv[1:]
if "--list" in args:
    for name in %(names)r:
        print(name + ": test")
    sys.exit(0)
name = args[args.index("--lib") + 1]
if name.endswith("flaky"):
    print("running 1 test\\ntest result: FAILED. 0 passed; 1 failed;")
    sys.exit(101)
if name.endswith("ghost"):
    print("running 0 tests\\ntest result: ok. 0 passed; 0 failed;")
    sys.exit(0)
print("running 1 test\\ntest result: ok. 1 passed; 0 failed;")
"""


class Record(Base):
    def cargo(self, names):
        path = self.fx.dir / "fake-cargo"
        support.write_executable(path, FAKE_CARGO % {"names": names})
        return path

    def record(self, rows, names):
        cargo = self.cargo(names)
        self.fx.ledger.write_text(json.dumps(rows))
        out = self.fx.dir / "out.json"
        done = subprocess.run(
            [
                sys.executable,
                str(TOOL),
                "record",
                str(self.fx.ledger),
                "--out",
                str(out),
                "--cargo",
                str(cargo),
                "--source-root",
                str(self.fx.dir),
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            universal_newlines=True,
        )
        data = json.loads(out.read_text()) if out.exists() else None
        return done, data

    def test_record_lists_runs_and_receipts_each_distinct_test_once(self):
        names = [MOD + n for n in TESTS]
        done, data = self.record(self.fx.rows, names)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertEqual(sorted(r["test"] for r in data["receipts"]), sorted(names))
        for receipt in data["receipts"]:
            self.assertEqual(
                (receipt["exit"], receipt["discovered"], receipt["passed"]), (0, 1, 1)
            )
            self.assertEqual(receipt["source_digest"], self.fx.digest())
            self.assertIn("--exact", receipt["command"])

    def test_recorded_receipts_satisfy_the_check(self):
        names = [MOD + n for n in TESTS]
        done, data = self.record(self.fx.rows, names)
        self.fx.receipt_list = data["receipts"]
        self.assertEqual(self.strict()[0], 0)

    def test_record_keeps_a_failing_run_and_a_zero_test_run_as_failures(self):
        rows = self.fx.rows[:2]
        rows[0]["test"], rows[1]["test"] = "flaky", "ghost"
        names = [MOD + "flaky", MOD + "ghost"]
        done, data = self.record(rows, names)
        self.assertEqual(done.returncode, 1)
        self.assertEqual(data["receipts"], [])
        self.assertIn("FAIL " + MOD + "flaky", done.stdout)
        self.assertIn("FAIL " + MOD + "ghost", done.stdout)
        self.assertIn("run exited 101", done.stdout)
        self.assertIn("discovered", done.stdout)

    def test_record_reports_a_test_that_does_not_exist(self):
        done, data = self.record(self.fx.rows, [MOD + "alpha_cancels"])
        self.assertEqual(done.returncode, 1)
        self.assertIn("skipped", done.stdout)

    def test_the_test_list_comes_from_cargo_when_none_is_supplied(self):
        cargo = self.cargo([MOD + n for n in TESTS])
        self.fx.ledger.write_text(json.dumps(self.fx.rows))
        self.fx.receipts.write_text(
            json.dumps({"schema": 1, "receipts": self.fx.receipt_list})
        )
        done = subprocess.run(
            [
                sys.executable,
                str(TOOL),
                "check",
                str(self.fx.ledger),
                "--mode",
                "strict",
                "--cargo",
                str(cargo),
                "--source-root",
                str(self.fx.dir),
                "--synthetic-baseline",
                str(self.fx.baseline),
                "--receipts",
                str(self.fx.receipts),
                "--sdk-receipts",
                str(self.fx.sdk),
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            universal_newlines=True,
        )
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)


END_D = "d-end-during-pending-recovery-with-thinking-hold"
END_A = "a-manual-end-with-transient-leave"
DEADLINE_B = "b-server-deadline"
ABSENT_C = "c-absence-expiry"
CLOSED_E = "e-room-closed-during-recovery"
SHARE_REASON = (
    "Every recovery phase waits through the single Driver::wait select in "
    "src/livekit/recovery.rs, so the same handler sees the SDK event in every phase."
)
EOF_REASON = (
    "The SDK has no EOF event: the local channel in src/livekit/recovery.rs closes "
    "after Disconnected and maps to the same Terminal::RoomClosed path."
)
N_REPRESENTATIVE_FIXTURES = 30
ROW_END, ROW_DEADLINE, ROW_DISCONNECTED, ROW_ABSENT, ROW_FINISH, ROW_EOF = range(6)


def rep(run_id, action, reason=SHARE_REASON, **over):
    claim = {
        "status": "sdk_representative",
        "run_id": run_id,
        "action_id": action,
        "reason": reason,
    }
    claim.update(over)
    return claim


class RepresentativeSdk(Base):
    """A live run of the same cause at another phase backs a row only as an argument."""

    def setUp(self):
        super().setUp()
        fx = self.fx
        fx.write_live(
            END_D,
            "manual",
            ["thinking_hold", "controlled_recovery_trigger", "end"],
            True,
        )
        fx.write_live(
            END_A,
            "manual",
            ["candidate_join", "controlled_recovery_trigger", "candidate_leave", "end"],
            True,
        )
        fx.write_live(
            DEADLINE_B, "server_deadline", ["controlled_recovery_trigger"], False
        )
        fx.write_live(
            ABSENT_C,
            "absence",
            ["controlled_recovery_trigger", "candidate_leave"],
            False,
        )
        fx.write_live(
            CLOSED_E,
            "room_closed",
            ["controlled_recovery_trigger", "room_delete"],
            False,
        )
        fx.rows[ROW_END]["sdk_report"] = rep(END_D, "end")
        fx.rows[ROW_DEADLINE]["sdk_report"] = rep(
            DEADLINE_B, "controlled_recovery_trigger"
        )
        fx.rows[ROW_ABSENT]["sdk_report"] = rep(ABSENT_C, "candidate_leave")

    def test_representative_receipts_for_the_right_causes_pass_strict(self):
        code, report, err = self.strict()
        self.assertEqual((code, err), (0, ""), report)
        self.assertEqual(report["by_sdk"]["sdk_representative"], 3)

    def test_either_end_scenario_backs_an_end_row(self):
        self.fx.rows[ROW_END]["sdk_report"] = rep(END_A, "end")
        self.assertEqual(self.strict()[0], 0)

    def test_a_receipt_for_a_different_cause_is_fabricated(self):
        for claim in (
            rep(DEADLINE_B, "controlled_recovery_trigger"),
            rep(ABSENT_C, "candidate_leave"),
        ):
            self.setUp()
            self.fx.rows[ROW_END]["sdk_report"] = claim
            self.assertFails("fabricated_sdk")
        self.setUp()
        self.fx.rows[ROW_ABSENT]["sdk_report"] = rep(END_D, "end")
        self.assertFails("fabricated_sdk")

    def test_a_placeholder_or_unanchored_reason_is_rejected(self):
        for reason in (
            "TODO",
            "n/a",
            "",
            "short reason",
            "src/livekit.rs",
            "the phases all behave in the very same way as the live one did",
        ):
            with self.subTest(reason):
                self.setUp()
                self.fx.rows[ROW_END]["sdk_report"] = rep(END_D, "end", reason)
                self.assertFails("missing_reason")

    def test_a_missing_reason_key_is_rejected(self):
        claim = rep(END_D, "end")
        del claim["reason"]
        self.fx.rows[ROW_END]["sdk_report"] = claim
        self.assertFails("fabricated_sdk")

    def test_a_failed_or_unfinished_run_does_not_represent(self):
        for over in (
            {"child_exit": 101, "status": "child_failed"},
            {"status": "incomplete_budget"},
            {"child_exit": 1},
            {"budget_failure": "wall"},
        ):
            with self.subTest(over):
                self.setUp()
                self.fx.write_live(
                    END_D,
                    "manual",
                    ["controlled_recovery_trigger", "end"],
                    True,
                    **over,
                )
                self.assertFails("fabricated_sdk")

    def test_a_run_with_overrides_or_unreaped_children_does_not_represent(self):
        for over in (
            {"test_overrides": {"min_cleanup_reserve_s": 1}},
            {"cleanup": {"failures": [], "leaked_children": 1, "reaped": True}},
            {"cleanup": {"failures": ["x"], "leaked_children": 0, "reaped": True}},
            {"cleanup": {"failures": [], "leaked_children": 0, "reaped": False}},
            {"resources": {"samples": 0}},
            {"telemetry": {"events": 0, "malformed": 2}},
        ):
            with self.subTest(over):
                self.setUp()
                self.fx.write_live(
                    END_D,
                    "manual",
                    ["controlled_recovery_trigger", "end"],
                    True,
                    **over,
                )
                self.assertFails("fabricated_sdk")

    def test_the_wrong_scenario_shape_does_not_represent(self):
        for run_id, mode, granted, permit in (
            (END_D, "absence", ["controlled_recovery_trigger", "end"], True),
            (END_D, "manual", ["controlled_recovery_trigger"], True),
            (END_D, "manual", ["end"], True),
            (END_D, "manual", ["controlled_recovery_trigger", "end"], False),
            (DEADLINE_B, "server_deadline", ["controlled_recovery_trigger"], True),
        ):
            with self.subTest((run_id, mode, granted, permit)):
                self.setUp()
                self.fx.write_live(run_id, mode, granted, permit)
                self.assertFails("fabricated_sdk")

    def test_a_lookalike_run_that_is_not_the_scenario_is_rejected(self):
        self.fx.write_live(
            "x-manual-end-lookalike",
            "manual",
            ["controlled_recovery_trigger", "end"],
            True,
        )
        self.fx.rows[ROW_END]["sdk_report"] = rep("x-manual-end-lookalike", "end")
        self.assertFails("fabricated_sdk")

    def test_a_setup_action_does_not_stand_in_for_the_driving_one(self):
        self.fx.rows[ROW_END]["sdk_report"] = rep(END_D, "controlled_recovery_trigger")
        self.assertFails("fabricated_sdk")
        self.setUp()
        self.fx.rows[ROW_ABSENT]["sdk_report"] = rep(
            ABSENT_C, "controlled_recovery_trigger"
        )
        self.assertFails("fabricated_sdk")

    def test_a_non_boolean_end_permit_flag_is_rejected(self):
        for flag in ("false", "yes", 2, [0], None):
            with self.subTest(flag):
                self.setUp()
                self.fx.write_live(
                    END_D,
                    "manual",
                    ["thinking_hold", "controlled_recovery_trigger", "end"],
                    flag,
                )
                self.assertFails("fabricated_sdk")

    def test_an_action_that_does_not_drive_the_cause_is_rejected(self):
        self.fx.rows[ROW_END]["sdk_report"] = rep(END_D, "thinking_hold")
        self.assertFails("fabricated_sdk")

    def test_a_cause_without_a_live_scenario_cannot_be_represented(self):
        # RoomEventsEof has no SDK event: it shares RoomDisconnected, and no
        # run represents it.
        self.fx.rows[ROW_EOF]["sdk_report"] = rep(CLOSED_E, "room_delete")
        report = self.assertFails("fabricated_sdk")
        self.assertIn(
            "no live scenario drives RoomEventsEof",
            "".join(entry[2] for entry in report["integrity"]),
        )

    def test_the_room_closed_scenario_backs_a_disconnect_row(self):
        self.fx.rows[ROW_DISCONNECTED]["sdk_report"] = rep(CLOSED_E, "room_delete")
        code, report, err = self.strict()
        self.assertEqual((code, err), (0, ""), report)
        self.assertEqual(report["by_sdk"]["sdk_representative"], 4)

    def test_the_end_of_events_row_stands_on_a_representative_disconnect_claim(self):
        fx = self.fx
        fx.rows[ROW_DISCONNECTED]["sdk_report"] = rep(CLOSED_E, "room_delete")
        fx.rows[ROW_EOF]["sdk_report"] = {
            "status": "N/A",
            "shares": "RoomDisconnected",
            "reason": EOF_REASON,
        }
        code, report, err = self.strict()
        self.assertEqual((code, err), (0, ""), report)
        self.assertEqual(report["by_sdk"]["N/A"], 1)
        # The same row is left uncovered once the disconnect claim is not the
        # room-closed scenario's.
        fx.rows[ROW_DISCONNECTED]["sdk_report"] = rep(ABSENT_C, "candidate_leave")
        _, report, _ = self.strict()
        self.assertIn("alpha_wait/RoomEventsEof", [g[0] for g in report["gaps"]])

    def test_only_the_room_closed_scenario_represents_a_disconnect(self):
        for run_id, action in (
            (ABSENT_C, "candidate_leave"),
            (DEADLINE_B, "controlled_recovery_trigger"),
            (END_A, "end"),
            (END_D, "end"),
        ):
            with self.subTest(run_id):
                self.setUp()
                self.fx.rows[ROW_DISCONNECTED]["sdk_report"] = rep(run_id, action)
                report = self.assertFails("fabricated_sdk")
                self.assertIn(
                    "is not a RoomDisconnected scenario",
                    "".join(entry[2] for entry in report["integrity"]),
                )

    def test_the_room_closed_scenario_backs_no_other_cause(self):
        for row, action in (
            (ROW_END, "end"),
            (ROW_DEADLINE, "controlled_recovery_trigger"),
            (ROW_ABSENT, "candidate_leave"),
        ):
            with self.subTest(row):
                self.setUp()
                self.fx.rows[row]["sdk_report"] = rep(CLOSED_E, action)
                self.assertFails("fabricated_sdk")
        self.setUp()
        self.fx.rows[ROW_END]["sdk_report"] = {"run_id": CLOSED_E, "action_id": "end"}
        report = self.assertFails("fabricated_sdk")
        self.assertIn(
            "is the RoomDisconnected scenario",
            "".join(entry[2] for entry in report["integrity"]),
        )

    def test_the_room_closed_run_must_have_the_scenarios_shape(self):
        for mode, granted, permit in (
            ("absence", ["controlled_recovery_trigger", "room_delete"], False),
            ("manual", ["controlled_recovery_trigger", "room_delete"], False),
            ("room_closed", ["controlled_recovery_trigger"], False),
            ("room_closed", ["room_delete"], False),
            ("room_closed", ["controlled_recovery_trigger", "room_delete"], True),
        ):
            with self.subTest((mode, granted, permit)):
                self.setUp()
                self.fx.write_live(CLOSED_E, mode, granted, permit)
                self.fx.rows[ROW_DISCONNECTED]["sdk_report"] = rep(
                    CLOSED_E, "room_delete"
                )
                self.assertFails("fabricated_sdk")

    def test_a_setup_action_does_not_stand_in_for_the_room_delete(self):
        self.fx.rows[ROW_DISCONNECTED]["sdk_report"] = rep(
            CLOSED_E, "controlled_recovery_trigger"
        )
        self.assertFails("fabricated_sdk")

    def test_a_failed_or_lookalike_room_closed_run_does_not_represent(self):
        self.fx.write_live(
            CLOSED_E,
            "room_closed",
            ["controlled_recovery_trigger", "room_delete"],
            False,
            child_exit=101,
            status="child_failed",
        )
        self.fx.rows[ROW_DISCONNECTED]["sdk_report"] = rep(CLOSED_E, "room_delete")
        self.assertFails("fabricated_sdk")
        self.setUp()
        self.fx.write_live(
            "x-room-closed-lookalike",
            "room_closed",
            ["controlled_recovery_trigger", "room_delete"],
            False,
        )
        self.fx.rows[ROW_DISCONNECTED]["sdk_report"] = rep(
            "x-room-closed-lookalike", "room_delete"
        )
        self.assertFails("fabricated_sdk")

    def test_a_plain_claim_cannot_cite_another_causes_scenario(self):
        self.fx.rows[ROW_DISCONNECTED]["sdk_report"] = {
            "run_id": ABSENT_C,
            "action_id": "candidate_leave",
        }
        self.fx.rows[ROW_EOF]["sdk_report"] = {
            "status": "N/A",
            "shares": "RoomDisconnected",
            "reason": EOF_REASON,
        }
        report = self.assertFails("fabricated_sdk")
        self.assertIn(
            "is the Absent scenario",
            "".join(entry[2] for entry in report["integrity"]),
        )
        self.assertIn("alpha_wait/RoomEventsEof", [gap[0] for gap in report["gaps"]])

    def test_manifest_and_liveness_values_must_be_exact(self):
        for body in (
            b'{"run_id": "other", "end_mode": "manual"}',
            b'["end_mode", "manual"]',
        ):
            with self.subTest(body):
                self.setUp()
                path = self.fx.sdk / END_D / "run-manifest.json"
                path.write_bytes(body)
                result = json.loads((path.parent / "budget-result.json").read_text())
                result["manifest_sha256"] = sha256(body)
                (path.parent / "budget-result.json").write_text(json.dumps(result))
                self.assertFails("fabricated_sdk")
        for over in (
            {"cleanup": {"failures": [], "leaked_children": False, "reaped": True}},
            {"telemetry": {"events": 0, "malformed": False}},
        ):
            with self.subTest(over):
                self.setUp()
                self.fx.write_live(
                    END_D,
                    "manual",
                    ["thinking_hold", "controlled_recovery_trigger", "end"],
                    True,
                    **over,
                )
                self.assertFails("fabricated_sdk")

    def test_an_sdk_na_claim_with_extra_keys_is_rejected(self):
        self.fx.rows[ROW_EOF]["sdk_report"] = {
            "status": "N/A",
            "shares": "RoomDisconnected",
            "reason": EOF_REASON,
            "extra": 1,
        }
        self.assertFails("fabricated_sdk")

    def test_a_reason_without_a_source_file_path_is_rejected(self):
        self.fx.rows[ROW_END]["sdk_report"] = rep(
            END_D,
            "end",
            "Every recovery phase waits through the single Driver::wait select, "
            "so the same handler sees the SDK event in every phase.",
        )
        self.assertFails("missing_reason")

    def test_a_representative_claim_with_extra_or_foreign_keys_is_rejected(self):
        self.fx.rows[ROW_END]["sdk_report"] = rep(END_D, "end", note="x")
        self.assertFails("fabricated_sdk")

    def test_a_representative_claim_without_an_sdk_directory_is_fabricated(self):
        self.assertFails("fabricated_sdk", sdk=False)

    def test_the_end_of_events_row_stands_only_on_a_valid_disconnect_claim(self):
        fx = self.fx
        fx.rows[ROW_EOF]["sdk_report"] = {
            "status": "N/A",
            "shares": "RoomDisconnected",
            "reason": EOF_REASON,
        }
        # RoomDisconnected still has its plain claim from the base ledger.
        code, report, err = self.strict()
        self.assertEqual((code, err), (0, ""), report)
        self.assertEqual(report["by_sdk"]["N/A"], 1)

    def test_the_end_of_events_row_is_uncovered_while_disconnect_is_uncovered(self):
        fx = self.fx
        fx.rows[ROW_EOF]["sdk_report"] = {
            "status": "N/A",
            "shares": "RoomDisconnected",
            "reason": EOF_REASON,
        }
        fx.rows[ROW_DISCONNECTED]["sdk_report"] = "uncovered required native SDK run"
        code, report, _ = self.strict()
        self.assertEqual(code, 1)
        self.assertEqual(
            sorted((g[0], g[1]) for g in report["gaps"]),
            [
                ("alpha_wait/RoomDisconnected", "sdk_pending"),
                ("alpha_wait/RoomEventsEof", "sdk_pending"),
            ],
        )
        self.assertEqual(self.fx.run("incremental")[0], 0)
        # A failed disconnect claim is a ledger lie that leaves EOF uncovered too.
        fx.rows[ROW_DISCONNECTED]["sdk_report"] = rep(ABSENT_C, "candidate_leave")
        _, report, _ = self.strict()
        self.assertIn("alpha_wait/RoomEventsEof", [g[0] for g in report["gaps"]])

    def test_an_sdk_na_column_needs_a_reason_and_only_fits_end_of_events(self):
        base = {"status": "N/A", "shares": "RoomDisconnected", "reason": EOF_REASON}
        self.fx.rows[ROW_EOF]["sdk_report"] = dict(base, reason="n/a")
        self.assertFails("missing_reason")
        self.setUp()
        self.fx.rows[ROW_EOF]["sdk_report"] = dict(base, shares="End")
        self.assertFails("fabricated_sdk")
        self.setUp()
        self.fx.rows[ROW_END]["sdk_report"] = dict(base)
        self.assertFails("fabricated_sdk")

    def test_the_fixture_count_is_asserted(self):
        self.assertEqual(
            len(
                [
                    n
                    for n in dir(self)
                    if n.startswith("test_")
                    and n != "test_the_fixture_count_is_asserted"
                ]
            ),
            N_REPRESENTATIVE_FIXTURES,
        )

    def test_plain_claims_still_demand_telemetry_events(self):
        self.fx.write_sdk(telemetry={"events": 0})
        self.assertFails("fabricated_sdk")


if __name__ == "__main__":
    unittest.main()

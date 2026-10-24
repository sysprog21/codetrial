"""Exercise audits with real base commits and a deterministic cargo boundary."""

import importlib.util
import json
import os
import re
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

CARGO = """#!/usr/bin/env python3
import json, os, subprocess, sys
from pathlib import Path
args = sys.argv
db = Path(args[args.index("--db") + 1])
with open(os.environ["AUDIT_CALLS"], "a") as log:
    log.write(json.dumps(args[1:]) + "\\n")
if "--no-fetch" not in args:
    db.mkdir()
    subprocess.run(["git", "init", "--quiet", str(db)], check=True)
    subprocess.run(["git", "-C", str(db), "-c", "user.name=Test",
                    "-c", "user.email=test@example.org", "-c", "core.hooksPath=/dev/null",
                    "commit", "--quiet", "--allow-empty", "-m", "snapshot"], check=True)
data = json.loads(Path(args[args.index("--file") + 1]).read_text())
if "raw" in data:
    print(data["raw"])
    sys.exit(data.get("exit", 1))
items = [{"advisory": {"id": a}, "package": {"name": n, "version": v}}
         for a, n, v in data["findings"]]
print(json.dumps({"vulnerabilities": {"found": bool(items), "count": len(items),
                                    "list": items}, "warnings": data.get("warnings", {})}))
sys.exit(data.get("exit", int(bool(items))))
"""

GH = """#!/usr/bin/env python3
import json, os, subprocess, sys
args = sys.argv[1:]
action = "api" if args[0] == "api" else args[1]
with open(os.environ["GH_CALLS"], "a") as log:
    log.write(json.dumps(args) + "\\n")
if action == os.environ["FAIL_ACTION"]:
    sys.exit(1)
if "--slurp" in args and ("--jq" in args or "--template" in args):
    sys.exit("the `--slurp` option is not supported with `--jq` or `--template`")
if action == "api":
    comments = any(arg.endswith("/comments") for arg in args)
    print(os.environ["COMMENT_PAGES" if comments else "ISSUE_PAGES"])
if action == "view":
    print(os.environ["OLD_BODY"])
"""

A = ["RUSTSEC-2026-0385", "ctutils", "0.4.3"]
B = ["RUSTSEC-2026-9999", "other", "1.0.0"]
ISSUE = {
    "number": 42,
    "title": "Dependency security audit",
    "state": "open",
    "user": {"login": "github-actions[bot]"},
}


class AuditTests(unittest.TestCase):
    def test_empty_base_cannot_fall_back_to_a_full_scan(self):
        result = subprocess.run(
            [sys.executable, str(support.ROOT / "scripts/audit.py"), "--base", ""],
            capture_output=True,
            text=True,
            timeout=60,
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("--base must name", result.stderr)

    def run_tracking(
        self, pages, *, findings="true", old="", current="", fail="", comments=()
    ):
        workflow = (support.ROOT / ".github/workflows/audit.yml").read_text()
        tracking = support.step(support.job("audit", workflow), "Track remediation")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "audit.md").write_text(current)
            support.write_executable(root / "gh", GH)
            result = subprocess.run(
                # Actions runs bash steps with errexit and pipefail.
                ["bash", "-euo", "pipefail", "-c", support.run_block(tracking)],
                env={
                    **os.environ,
                    "PATH": str(root) + os.pathsep + os.environ["PATH"],
                    "GH_CALLS": str(root / "calls"),
                    "RUNNER_TEMP": str(root),
                    "ISSUE_PAGES": json.dumps(pages),
                    "COMMENT_PAGES": json.dumps([list(comments)]),
                    "OLD_BODY": old,
                    "FAIL_ACTION": fail,
                    "FINDINGS": findings,
                    "GH_REPO": "sysprog21/codetrial",
                },
                capture_output=True,
                text=True,
                timeout=60,
            )
            calls = [
                json.loads(line) for line in (root / "calls").read_text().splitlines()
            ]
            self.assertIn("creator=github-actions[bot]", calls[0])
            self.assertIn("--paginate", calls[0])
            self.assertIn("--slurp", calls[0])
            self.assertIn("state=all", calls[0])
            for call in calls[1:]:
                if "--body-file" in call:
                    self.assertEqual(
                        Path(call[call.index("--body-file") + 1]).read_text(), current
                    )
            return result, calls

    def test_only_trusted_main_runs_with_issue_write_access(self):
        workflow = (support.ROOT / ".github/workflows/audit.yml").read_text()
        job = support.job("audit", workflow)
        tracking = support.step(job, "Track remediation")
        self.assertIn("steps.audit.outputs.findings != ''", tracking)
        self.assertIn("if: github.ref == 'refs/heads/main'", job)
        self.assertIn(
            "    permissions:\n      contents: read\n      issues: write\n", job
        )
        self.assertIn("\npermissions:\n  contents: read\n\n", workflow)
        triggers = support.section(
            workflow, "\non:\n", "\npermissions:", "audit triggers"
        )
        self.assertNotIn("pull_request", triggers)

    @unittest.skipUnless(shutil.which("jq"), "jq is not installed")
    def test_tracking_filters_real_json_including_other_authors_prs_and_pages(self):
        bot = {key: value for key, value in ISSUE.items() if key != "number"}
        decoys = [
            {**bot, "number": 1, "user": {"login": "outsider"}},
            {**bot, "number": 2, "pull_request": {}},
            {**bot, "number": 3, "title": "Other issue"},
        ]
        for pages, expected in (
            ([[]], None),
            ([decoys], None),
            ([decoys, [{**bot, "number": 42}, {**bot, "number": 43}]], "42"),
        ):
            with self.subTest(pages=pages):
                result, calls = self.run_tracking(pages, findings="false")
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(
                    [call[:3] for call in calls[1:]],
                    [["issue", "edit", expected], ["issue", "close", expected]]
                    if expected
                    else [],
                )

    @unittest.skipUnless(shutil.which("jq"), "jq is not installed")
    def test_tracking_notifies_only_when_findings_change_and_preserves_retry(self):
        issue = ISSUE
        closed = {**issue, "state": "closed", "closed_at": "2026-01-03T00:00:00Z"}
        unchanged = "Findings fingerprint: `same`\n"
        changed = "Findings fingerprint: `new`\n"
        bot = {"login": "github-actions[bot]"}
        before = "2026-01-02T00:00:00Z"
        after = "2026-01-04T00:00:00Z"
        posted = [{"user": bot, "created_at": before, "body": "Report\n" + changed}]
        reposted = [{**posted[0], "created_at": after}]
        stale = [
            {"user": bot, "created_at": before, "body": "Report\n" + changed},
            {"user": bot, "created_at": before, "body": "Report\n" + unchanged},
            {"user": {"login": "outsider"}, "created_at": after, "body": changed},
        ]
        notify = ["view", "comments", "comment", "edit"]
        for pages, findings, old, current, fail, comments, actions in (
            ([[]], "true", "", unchanged, "", (), ["create"]),
            (
                [[issue]],
                "true",
                unchanged,
                unchanged + "New database revision\n",
                "",
                (),
                ["view", "edit"],
            ),
            ([[issue]], "true", unchanged, changed, "", (), notify),
            ([[issue]], "true", "Legacy report", changed, "", (), notify),
            ([[issue]], "true", unchanged, changed, "", stale, notify),
            # A comment whose follow-up edit failed is not posted twice.
            (
                [[issue]],
                "true",
                unchanged,
                changed,
                "",
                posted,
                ["view", "comments", "edit"],
            ),
            # Recurring findings reopen the issue its subscribers follow.
            (
                [[closed]],
                "true",
                unchanged,
                unchanged,
                "",
                (),
                ["view", "edit", "reopen"],
            ),
            # Findings that return after a clean close notify again, even when
            # they match the comment from before the close.
            (
                [[closed]],
                "true",
                "Clean report",
                changed,
                "",
                posted,
                notify + ["reopen"],
            ),
            # A comment posted after the close, whose edit then failed, is not
            # posted again.
            (
                [[closed]],
                "true",
                "Clean report",
                changed,
                "",
                reposted,
                ["view", "comments", "edit", "reopen"],
            ),
            ([[issue]], "false", unchanged, "Clean report", "", (), ["edit", "close"]),
            ([[closed]], "false", unchanged, "Clean report", "", (), []),
            ([[]], "false", "", "Clean report", "", (), []),
            ([[issue]], "true", unchanged, changed, "api", (), []),
            ([[issue]], "true", unchanged, changed, "comment", (), notify[:3]),
            ([[issue]], "true", unchanged, changed, "edit", (), notify),
        ):
            with self.subTest(
                pages=pages, findings=findings, old=old, fail=fail, actions=actions
            ):
                result, calls = self.run_tracking(
                    pages,
                    findings=findings,
                    old=old,
                    current=current,
                    fail=fail,
                    comments=comments,
                )
                self.assertEqual(result.returncode, int(bool(fail)), result.stderr)
                names = [
                    "comments" if call[0] == "api" else call[1] for call in calls[1:]
                ]
                self.assertEqual(names, actions)
                for call in calls[1:]:
                    if call[0] == "api":
                        self.assertIn(
                            "repos/sysprog21/codetrial/issues/42/comments", call
                        )
                    elif call[1] != "create":
                        self.assertEqual(call[2], "42")

    def run_audit(
        self,
        base,
        head,
        *,
        full=False,
        invalid_base=False,
        desktop_base=None,
        desktop_head=None,
    ):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "scripts").mkdir()
            script = root / "scripts/audit.py"
            script.write_bytes((support.ROOT / "scripts/audit.py").read_bytes())
            support.write_executable(root / "cargo", CARGO)

            def git(*args):
                return subprocess.check_output(["git", "-C", str(root), *args])

            git("init", "--quiet")
            git("config", "user.name", "Test")
            git("config", "user.email", "test@example.org")
            git("config", "core.hooksPath", "/dev/null")
            lockfile = root / "Cargo.lock"
            lockfile.write_text(json.dumps(base))
            git("add", "Cargo.lock")
            desktop = root / "desktop/Cargo.lock"
            if desktop_base is not None:
                desktop.parent.mkdir()
                desktop.write_text(json.dumps(desktop_base))
                git("add", "desktop/Cargo.lock")
            git("commit", "--quiet", "-m", "base")
            sha = git("rev-parse", "HEAD").decode().strip()
            lockfile.write_text(json.dumps(head))
            if desktop_head is not None:
                desktop.parent.mkdir(exist_ok=True)
                desktop.write_text(json.dumps(desktop_head))
            command = [sys.executable, str(script), "--report", str(root / "report.md")]
            if not full:
                command += ["--base", "missing" if invalid_base else sha]
            env = {
                **os.environ,
                "PATH": str(root) + os.pathsep + os.environ["PATH"],
                "AUDIT_CALLS": str(root / "calls"),
                "GITHUB_OUTPUT": str(root / "outputs"),
                "GITHUB_STEP_SUMMARY": str(root / "summary"),
            }
            result = subprocess.run(
                command, env=env, capture_output=True, text=True, timeout=30
            )
            report = (root / "report.md").read_text()
            summary = (root / "summary").read_text()
            if len(summary) <= 60000:
                self.assertEqual(report, summary)
            else:
                self.assertTrue(report.startswith(summary[:60000]))
                self.assertIn("Report truncated", report)
            outputs = (
                (root / "outputs").read_text() if (root / "outputs").exists() else ""
            )
            calls = (
                [json.loads(line) for line in (root / "calls").read_text().splitlines()]
                if (root / "calls").exists()
                else []
            )
            return result, report, outputs, calls

    def test_unchanged_lockfile_reports_inherited_findings_with_one_scan(self):
        result, report, outputs, calls = self.run_audit(
            {"findings": [A]}, {"findings": [A]}
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("ctutils 0.4.3` (inherited)", report)
        self.assertIn("blocking: 0", report)
        self.assertNotIn("::error::", result.stderr)
        self.assertEqual(outputs, "findings=true\n")
        self.assertEqual(len(calls), 1)

    def test_changed_lockfile_uses_one_snapshot_and_allows_inherited_findings(self):
        result, report, _, calls = self.run_audit(
            {"findings": [A]}, {"findings": [A], "change": True}
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("(inherited)", report)
        self.assertEqual(len(calls), 2)
        self.assertNotIn("--no-fetch", calls[0])
        self.assertIn("--no-fetch", calls[1])
        self.assertEqual(
            calls[0][calls[0].index("--db") + 1], calls[1][calls[1].index("--db") + 1]
        )

    def test_equal_counts_do_not_hide_replacement_or_version_changes(self):
        for finding in (B, [*A[:2], "0.4.2"]):
            with self.subTest(finding=finding):
                result, report, _, _ = self.run_audit(
                    {"findings": [A]}, {"findings": [finding]}
                )
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertIn("blocking: 1", report)

    def test_fixed_current_base_blocks_a_vulnerability_from_an_old_branch(self):
        result, report, _, _ = self.run_audit({"findings": []}, {"findings": [A]})
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("(blocking)", report)
        self.assertIn("::error::RUSTSEC-2026-0385 ctutils 0.4.3", result.stderr)

    def test_annotations_escape_workflow_command_control_characters(self):
        finding = [A[0], "crate%\r\n::warning::injected", "1.0.0"]
        result, _, _, _ = self.run_audit(
            {"findings": []}, {"findings": [finding]}, full=True
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn(
            "::error::RUSTSEC-2026-0385 crate%25%0D%0A::warning::injected 1.0.0",
            result.stderr,
        )
        self.assertNotIn("\n::warning::", result.stderr)

    def test_fingerprint_tracks_findings_not_order_or_database_and_warning_changes(
        self,
    ):
        fingerprints = []
        for findings, warnings in (
            ([A, B], {}),
            ([B, A], {"unmaintained": ["example"]}),
            ([A], {}),
        ):
            result, report, _, _ = self.run_audit(
                {"findings": []},
                {"findings": findings, "warnings": warnings},
                full=True,
            )
            self.assertEqual(result.returncode, 1, result.stderr)
            match = re.search(r"^Findings fingerprint: `([0-9a-f]{64})`$", report, re.M)
            self.assertIsNotNone(match)
            fingerprints.append(match[1])
        self.assertEqual(fingerprints[0], fingerprints[1])
        self.assertNotEqual(fingerprints[0], fingerprints[2])

    def test_full_scan_blocks_findings_but_accepts_a_patch(self):
        for findings, status in (([A], 1), ([], 0)):
            with self.subTest(findings=findings):
                result, _, outputs, calls = self.run_audit(
                    {"findings": []}, {"findings": findings}, full=True
                )
                self.assertEqual(result.returncode, status, result.stderr)
                self.assertEqual(outputs, f"findings={str(bool(findings)).lower()}\n")
                self.assertEqual(len(calls), 1)

    def test_incomplete_scans_never_claim_a_clean_result(self):
        # Every malformed report fails the same schema check, so one per mode
        # suffices beyond the diff mode that tries them all.
        cases = [
            {"raw": "not JSON"},
            {"raw": "{}"},
            {
                "raw": json.dumps(
                    {"vulnerabilities": {"found": False, "count": 0, "list": {}}}
                ),
                "exit": 0,
            },
            {"findings": [], "warnings": []},
            {"findings": [], "exit": 2},
            {"findings": [A], "exit": 0},
        ]
        runs = [(broken, "diff") for broken in cases]
        runs += [(cases[0], "full"), (cases[-1], "informational")]
        for broken, mode in runs:
            with self.subTest(broken=broken, mode=mode):
                result, report, outputs, _ = self.run_audit(
                    broken if mode == "informational" else {"findings": []},
                    broken,
                    full=mode == "full",
                )
                self.assertEqual(result.returncode, 0 if mode == "informational" else 2)
                self.assertIn("Scan incomplete", report)
                self.assertEqual(outputs, "")

    def test_base_scan_errors_and_missing_bases_block(self):
        for base, invalid in (({"raw": "broken"}, False), ({"findings": []}, True)):
            with self.subTest(base=base, invalid=invalid):
                result, report, outputs, _ = self.run_audit(
                    base, {"findings": []}, invalid_base=invalid
                )
                self.assertEqual(result.returncode, 2)
                self.assertIn("Scan incomplete", report)
                self.assertEqual(outputs, "")

    def test_the_desktop_lockfile_blocks_what_it_adds_on_one_snapshot(self):
        result, report, _, calls = self.run_audit(
            {"findings": []},
            {"findings": []},
            desktop_base={"findings": [A]},
            desktop_head={"findings": [A, B]},
        )
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("ctutils 0.4.3` (inherited)", report)
        self.assertIn("other 1.0.0` (blocking)", report)
        # The root once, unchanged, then the desktop head and its base, all
        # against the database the first audit fetched.
        self.assertEqual(len(calls), 3)
        self.assertNotIn("--no-fetch", calls[0])
        self.assertTrue(all("--no-fetch" in call for call in calls[1:]))
        databases = {call[call.index("--db") + 1] for call in calls}
        self.assertEqual(len(databases), 1)

    def test_a_desktop_lockfile_the_merge_adds_inherits_nothing(self):
        result, report, _, _ = self.run_audit(
            {"findings": []}, {"findings": []}, desktop_head={"findings": [A]}
        )
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("ctutils 0.4.3` (blocking)", report)

    def test_an_unchanged_desktop_lockfile_is_informational(self):
        result, report, _, calls = self.run_audit(
            {"findings": []},
            {"findings": []},
            desktop_base={"findings": [A]},
            desktop_head={"findings": [A]},
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("(inherited)", report)
        self.assertEqual(len(calls), 2)

    def test_warnings_remain_visible_without_blocking(self):
        result, report, _, _ = self.run_audit(
            {"findings": []},
            {"findings": [], "warnings": {"unmaintained": ["example"]}},
            full=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("unmaintained", report)

    def test_large_issue_reports_are_bounded_without_losing_the_full_summary(self):
        findings = [
            ["RUSTSEC-2026-9999", f"crate-{index}-" + "x" * 200, "1.0.0"]
            for index in range(300)
        ]
        result, report, outputs, _ = self.run_audit(
            {"findings": []}, {"findings": findings}, full=True
        )
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertLess(len(report), 65536)
        self.assertIn("Report truncated", report)
        self.assertEqual(outputs, "findings=true\n")


if __name__ == "__main__":
    unittest.main()

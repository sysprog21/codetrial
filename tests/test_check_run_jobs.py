#!/usr/bin/env python3
"""Contract tests for scripts/check-run-jobs.sh against a stub `gh`."""

import os
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "check-run-jobs.sh"

# Answers the three reads the script makes from RUNS: id -> (state, job names).
# A job list of None makes the jobs read fail; a state of None, the run read.
STUB = """#!/bin/sh
case "$*" in
*workflows/check.yml/runs*) [ "$LISTING" = fail ] && exit 1; printf '%s\\n' $IDS ;;
*actions/runs/*/jobs*) eval "jobs=\\$JOBS_$(echo "$*" | sed 's,.*runs/\\([0-9]*\\)/jobs.*,\\1,')"
    [ "$jobs" = fail ] && exit 1; printf '%s\\n' $jobs ;;
*actions/runs/*) eval "state=\\$STATE_$(echo "$*" | sed 's,.*runs/\\([0-9]*\\).*,\\1,')"
    [ "$state" = fail ] && exit 1; echo "${state:-completed success}" ;;
esac
"""


def scan(runs, *args, listing="ok"):
    """Run the script over RUNS, {id: (state, jobs)}; return (status, stdout)."""
    with tempfile.TemporaryDirectory() as tmp:
        gh = Path(tmp, "gh")
        gh.write_text(STUB)
        gh.chmod(0o755)
        env = {
            **os.environ,
            "PATH": f"{tmp}{os.pathsep}{os.environ['PATH']}",
            "IDS": " ".join(runs),
            "LISTING": listing,
        }
        for run_id, (state, jobs) in runs.items():
            env[f"STATE_{run_id}"] = "fail" if state is None else state
            env[f"JOBS_{run_id}"] = "fail" if jobs is None else jobs
        result = subprocess.run(
            ["sh", str(SCRIPT), *args],
            env=env,
            capture_output=True,
            text=True,
            check=False,
        )
    return result.returncode, result.stdout


class CheckRunJobs(unittest.TestCase):
    def test_a_run_with_its_check_job_is_clean(self):
        status, out = scan({"1": ("completed success", "build check")}, "x")
        self.assertEqual((status, out), (0, ""))

    def test_a_run_without_a_check_job_is_listed(self):
        status, out = scan({"7": ("completed failure", "build lint")}, "x")
        self.assertEqual(status, 1)
        self.assertEqual(out, "run 7: 2 jobs, check: missing\n")

    def test_cancelled_and_unfinished_runs_are_skipped(self):
        runs = {
            "1": ("completed cancelled", "build"),
            "2": ("in_progress null", "build"),
        }
        self.assertEqual(scan(runs, "x"), (0, ""))

    def test_cancelled_run_without_jobs_is_listed(self):
        self.assertEqual(
            scan({"7": ("completed cancelled", "")}, "x"),
            (1, "run 7: 0 jobs, check: missing\n"),
        )

    def test_an_api_error_is_not_a_finding(self):
        for runs, listing in (
            ({"1": ("completed success", None)}, "ok"),
            ({"1": (None, "check")}, "ok"),
            ({}, "fail"),
        ):
            with self.subTest(runs=runs, listing=listing):
                self.assertEqual(scan(runs, "x", listing=listing)[0], 2)

    def test_an_api_error_wins_over_a_finding(self):
        runs = {
            "1": ("completed success", "build"),
            "2": ("completed success", None),
        }
        self.assertEqual(scan(runs, "x")[0], 2)

    def test_limit_is_bounded(self):
        for limit in ("0", "101", "abc"):
            with self.subTest(limit=limit):
                self.assertEqual(scan({}, "x", limit)[0], 2)

    def test_run_mode_reads_the_named_run(self):
        status, out = scan({"9": ("completed success", "build")}, "--run", "9", "x")
        self.assertEqual((status, out), (1, "run 9: 1 jobs, check: missing\n"))


if __name__ == "__main__":
    unittest.main()

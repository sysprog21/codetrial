"""Exercise mutation planning against real Git diffs and a recorded tool stub."""

import importlib.util
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location(
    "support", Path(__file__).with_name("support.py")
)
support = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(support)


def planner_step(name):
    return support.step(support.job("plan-mutants"), name)


class MutantsPlanTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="mutants plan ")
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        self.output = self.directory / "output"
        self.env = os.environ | {
            "RUNNER_TEMP": str(self.directory),
            "GITHUB_OUTPUT": str(self.output),
            "GITHUB_STEP_SUMMARY": str(self.directory / "summary"),
            "BASE_REF": "main",
            "FAKE_COUNT": "1",
            "FAKE_FAIL": "0",
            "PATH": f"{self.directory}{os.pathsep}{os.environ['PATH']}",
        }
        # A Rust diff, unless a case reads its own from Git.
        (self.directory / "pr.diff").write_text("+fn example() -> bool { false }\n")
        support.write_executable(
            self.directory / "cargo",
            """#!/bin/sh
set -eu
echo "$*" >>"$RUNNER_TEMP/cargo.log"
[ "$FAKE_FAIL" = 0 ] || exit 7
[ "$*" = "mutants --list --in-diff $RUNNER_TEMP/pr.diff" ] || exit 9
i=0
while [ "$i" -lt "$FAKE_COUNT" ]; do
    echo "mutant $i"
    i=$((i + 1))
done
""",
        )

    def run_step(self, name):
        return subprocess.run(
            ["bash", "-e", "-c", support.run_block(planner_step(name))],
            cwd=self.directory,
            env=self.env,
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )

    def outputs(self):
        return dict(line.split("=", 1) for line in self.output.read_text().splitlines())

    def git(self, *args):
        return subprocess.run(
            [
                "git",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                *args,
            ],
            cwd=self.directory,
            capture_output=True,
            text=True,
            check=True,
        )

    def diff_fixture(self, rust=False):
        self.git("init", "-q")
        (self.directory / "example.rs").write_text("fn example() -> bool { true }\n")
        (self.directory / "notes.md").write_text("before\n")
        self.git("add", "example.rs", "notes.md")
        self.git("commit", "-qm", "Baseline")
        self.git("update-ref", "refs/remotes/origin/main", "HEAD")
        changed = self.directory / ("example.rs" if rust else "notes.md")
        changed.write_text("fn example() -> bool { false }\n" if rust else "after\n")
        self.git("commit", "-qam", "Change")

    def test_non_rust_diff_needs_no_cargo(self):
        self.diff_fixture()
        result = self.run_step("Read the Rust diff")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.outputs(), {"changed": "false"})
        self.output.unlink()
        result = self.run_step("Count the mutants this diff is worth")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.outputs()["count"], "0")
        self.assertFalse((self.directory / "cargo.log").exists())

    def test_rust_diff_is_preserved_for_the_mutation_tool(self):
        self.diff_fixture(rust=True)
        result = self.run_step("Read the Rust diff")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.outputs(), {"changed": "true"})
        self.assertIn(
            "+fn example() -> bool { false }", (self.directory / "pr.diff").read_text()
        )

    def test_unfetched_base_fails_instead_of_reporting_zero(self):
        self.diff_fixture()
        self.env["BASE_REF"] = "missing"
        result = self.run_step("Read the Rust diff")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.output.exists())

    def test_mutant_counts_assign_every_shard(self):
        for count in (0, 1, 25, 26, 400, 401, 1200, 1201):
            with self.subTest(count=count):
                self.output.unlink(missing_ok=True)
                self.env["FAKE_COUNT"] = str(count)
                result = self.run_step("Count the mutants this diff is worth")
                self.assertEqual(result.returncode, 0, result.stderr)
                outputs = self.outputs()
                width = min(48, max(1, (count + 24) // 25))
                self.assertEqual(int(outputs["count"]), count)
                self.assertEqual(int(outputs["width"]), width)
                self.assertEqual(
                    json.loads(outputs["shards"]), [str(i) for i in range(width)]
                )

    def test_listing_failure_cannot_become_zero_mutants(self):
        self.env["FAKE_FAIL"] = "1"
        result = self.run_step("Count the mutants this diff is worth")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.output.exists())

    def test_tool_setup_is_guarded_by_the_diff(self):
        for name in ("Install Rust", "Cache cargo-mutants", "Install cargo-mutants"):
            self.assertIn(
                "        if: steps.diff.outputs.changed == 'true'\n", planner_step(name)
            )


if __name__ == "__main__":
    unittest.main()

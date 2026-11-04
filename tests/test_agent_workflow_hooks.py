"""The failure-loop hook, the closeout check and the tree fingerprint."""

import importlib.util
import io
import json
import re
import subprocess
import tempfile
import unittest
from unittest import mock
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


loop = load("agent_failure_loop", "agent-failure-loop.py")
closeout = load("agent_closeout_check", "agent-closeout-check.py")


class FailureLoopTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.state = Path(self.dir.name) / "state.json"

    def run_gate(self, output, failure=True):
        return loop.update(output, self.state, failure)

    def test_second_identical_failure_prompts_and_names_the_gates(self):
        self.assertIsNone(self.run_gate("failed gates: clippy fmt\n"))
        message = self.run_gate("failed gates: fmt clippy\n")
        self.assertIn("2 runs in a row", message)
        self.assertIn("clippy fmt", message)

    def test_a_pass_or_a_different_failure_resets_the_count(self):
        self.run_gate("failed gates: fmt\n")
        self.assertIsNone(self.run_gate("failed gates: clippy\n"))
        self.assertIsNone(self.run_gate("all fine\n", failure=False))
        self.assertIsNone(self.run_gate("failed gates: clippy\n"))

    def test_an_aborted_run_leaves_the_count_alone(self):
        self.run_gate("failed gates: fmt\n")
        self.assertIsNone(self.run_gate("usage: test.sh [all | rust]\n"))
        self.assertIsNotNone(self.run_gate("failed gates: fmt\n"))

    def test_a_timed_out_run_leaves_the_count_alone(self):
        self.run_gate("failed gates: fmt\n")
        self.assertIsNone(self.run_gate("Command timed out after 120000ms\n"))
        self.assertIsNotNone(self.run_gate("failed gates: fmt\n"))

    def test_only_gate_commands_are_watched(self):
        for command in (
            "./scripts/test.sh rust",
            "make check",
            "cd x && scripts/test.sh",
            "timeout 900 ./scripts/test.sh rust",
            "env CI=1 make check",
            "time scripts/test.sh",
        ):
            self.assertTrue(loop.GATE_RUN.search(command), command)
        for command in (
            "ls",
            "make hooks",
            "git status",
            "grep -n x scripts/test.sh",
            "sed -n 1,9p scripts/test.sh",
            "git commit -m 'edit scripts/test.sh'",
            "cat ./scripts/test.sh",
        ):
            self.assertIsNone(loop.GATE_RUN.search(command), command)


GIT_ENV = {"GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_SYSTEM": "/dev/null"}


def make_repo(test):
    tmp = tempfile.TemporaryDirectory()
    test.addCleanup(tmp.cleanup)
    for args in (
        ["init", "-q"],
        [
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "x",
        ],
    ):
        subprocess.run(["git", *args], cwd=tmp.name, check=True, env=GIT_ENV)
    return Path(tmp.name)


def ran(lane, tree="T1", passed=True, **more):
    return {"lane": lane, "passed": passed, "skipped": 3, "tree": tree, **more}


class ReceiptTests(unittest.TestCase):
    def fingerprint(self, repo):
        done = subprocess.run(
            [str(ROOT / "scripts" / "tree-fingerprint.sh"), str(repo)],
            capture_output=True,
            text=True,
            check=True,
        )
        return done.stdout.strip()

    def test_fingerprint_follows_tracked_and_untracked_content(self):
        repo = make_repo(self)
        clean = self.fingerprint(repo)
        self.assertEqual(clean, self.fingerprint(repo))
        (repo / "a.txt").write_text("one")
        first = self.fingerprint(repo)
        (repo / "a.txt").write_text("two")
        second = self.fingerprint(repo)
        # A tracked file is part of the diff against HEAD, not of the untracked list.
        git = ["git", "-c", "user.name=t", "-c", "user.email=t@t"]
        subprocess.run([*git, "add", "a.txt"], cwd=repo, check=True, env=GIT_ENV)
        subprocess.run([*git, "commit", "-qm", "a"], cwd=repo, check=True, env=GIT_ENV)
        committed = self.fingerprint(repo)
        (repo / "a.txt").write_text("three")
        edited = self.fingerprint(repo)
        self.assertEqual(len({clean, first, second, committed, edited}), 5)

    def test_a_directory_that_is_not_a_work_tree_has_no_fingerprint(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        done = subprocess.run(
            [str(ROOT / "scripts" / "tree-fingerprint.sh"), tmp.name],
            capture_output=True,
            text=True,
        )
        self.assertEqual((done.returncode, done.stdout), (1, ""))

    def status(self, receipts):
        with (
            mock.patch.object(closeout, "tree", return_value="T1"),
            mock.patch.object(closeout, "receipts", return_value=receipts),
        ):
            return closeout.status()

    def test_status_requires_every_lane_on_this_exact_tree(self):
        covered, text = self.status([ran("all", tree="T0")])
        self.assertEqual(covered, False)
        self.assertIn("none matches", text)
        covered, text = self.status([ran("all", passed=False, failed="fmt")])
        self.assertEqual(covered, False)
        self.assertIn("failed (all: fmt)", text)
        # One lane is not the gate: the reminder must stay.
        covered, text = self.status([ran("rust")])
        self.assertEqual(covered, False)
        self.assertIn("not yet run here: browser, portable", text)
        self.assertTrue(self.status([ran("rust"), ran("portable"), ran("browser")])[0])
        covered, text = self.status([ran("all")])
        self.assertTrue(covered)
        self.assertIn("all (3 skipped)", text)

    def test_a_later_lane_failure_beats_an_earlier_all_pass(self):
        covered, text = self.status(
            [
                ran("all", time="2026-11-04T10:00:00Z"),
                ran(
                    "portable", passed=False, failed="fmt", time="2026-11-04T10:30:00Z"
                ),
            ]
        )
        self.assertFalse(covered)
        self.assertIn("failed (portable: fmt)", text)
        covered, _ = self.status(
            [
                ran("all", time="2026-11-04T11:00:00Z"),
                ran(
                    "portable", passed=False, failed="fmt", time="2026-11-04T10:30:00Z"
                ),
            ]
        )
        self.assertTrue(covered)

    def test_the_lanes_a_receipt_needs_are_the_lanes_test_sh_accepts(self):
        accepted = re.search(
            r"^    all \| ([a-z |]+)\) ;;", (ROOT / "scripts/test.sh").read_text(), re.M
        )
        self.assertEqual(set(accepted[1].replace("|", " ").split()), closeout.ALL_LANES)

    def test_stop_hook_never_blocks_and_ignores_a_reentrant_stop(self):
        done = subprocess.run(
            [str(ROOT / "scripts" / "agent-closeout-check.py")],
            input=json.dumps({"stop_hook_active": True}),
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual((done.returncode, done.stdout), (0, ""))


class StopReminderTests(unittest.TestCase):
    def test_an_uncovered_tree_is_reminded_once_and_again_after_an_edit(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        current = ["T1"]
        with mock.patch.multiple(
            closeout,
            git=lambda *args: " M file",
            tree=lambda: current[0],
            receipts=lambda: [],
            receipts_folder=lambda: Path(tmp.name) / "gate-receipts",
        ):

            def stop():
                out = io.StringIO()
                with (
                    mock.patch("sys.stdin", io.StringIO("{}")),
                    mock.patch("sys.stdout", out),
                ):
                    self.assertEqual(closeout.main([]), 0)
                return out.getvalue()

            self.assertIn("systemMessage", stop())
            self.assertEqual(stop(), "")
            current[0] = "T2"
            self.assertIn("systemMessage", stop())


if __name__ == "__main__":
    unittest.main()

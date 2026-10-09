"""Starting a task bank from reference tasks and building it for a site."""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from problem_bank import task_authoring  # noqa: E402
from problem_bank.task_authoring import (  # noqa: E402
    EXAMPLES,
    STAGING,
    new_task,
    next_version,
    reference,
    references,
)
from problem_bank.task_package import (  # noqa: E402
    decrypt,
    load_bank,
    validate_package,
    validated_rules,
)

PIN = "583209"
SITE = "https://teacher.github.io/course"
COMMAND = [sys.executable, str(ROOT / "scripts/task-package.py")]


def bank_file(bank, name):
    return json.loads((bank / f"{name}.json").read_text())


def snapshot(bank):
    return {path.name: path.read_text() for path in bank.iterdir() if path.is_file()}


def run(*args, stdin=None):
    return subprocess.run(
        COMMAND + [str(arg) for arg in args],
        input=stdin,
        capture_output=True,
        text=True,
    )


class TaskAuthoringTests(unittest.TestCase):
    def test_every_reference_is_a_valid_task_under_its_own_id_or_a_new_one(self):
        listed = references()
        examples = [row for row in listed if row[2] == "example"]
        # The shipped examples come first, so they are what an instructor sees.
        self.assertEqual(listed[: len(examples)], examples)
        self.assertEqual(
            sorted(row[0] for row in examples),
            sorted(path.stem for path in EXAMPLES.glob("*.json")),
        )
        self.assertGreater(len(listed) - len(examples), 100)
        for reference_id, _, _ in listed:
            for task_id in (reference_id, "renamed-task"):
                with self.subTest(reference=reference_id, task_id=task_id):
                    row = reference(reference_id)
                    row["problem"]["id"] = task_id
                    validate_package(
                        {
                            "packageVersion": 1,
                            "setId": "course",
                            "setVersion": 1,
                            "rules": validated_rules(None),
                            "problems": [row["problem"]],
                            "judges": {task_id: row["judge"]},
                            "variants": {task_id: row["variant"]},
                            "sidecars": {task_id: row["sidecar"]}
                            if row["sidecar"]
                            else {},
                        }
                    )

    def test_a_new_task_is_written_out_whole_and_ready_to_edit(self):
        with tempfile.TemporaryDirectory() as temporary:
            bank = Path(temporary) / "bank"
            new_task(bank, "two-sum", languages=["javascript", "python"])
            sidecar = bank_file(bank, "task-mode")["two-sum"]
            self.assertEqual(sidecar["languages"], ["javascript", "python"])
            # Every field there to change, the defaults included.
            self.assertEqual(
                set(sidecar),
                {
                    "promptVersion",
                    "interactionPrompt",
                    "completionTargets",
                    "understandingChecks",
                    "requiredCases",
                    "maxHintRungs",
                    "rubricProfile",
                    "languages",
                },
            )
            self.assertEqual(
                set(bank_file(bank, "task-set")),
                {
                    "durationMin",
                    "maxHintRungs",
                    "closesAt",
                    "lookAwaySeconds",
                    "rulesNote",
                },
            )
            self.assertFalse((bank / STAGING).exists())
            # A new id keeps the catalog origin, so the checks that keep the
            # source out of what the learner reads still apply to the copy.
            _, notes = new_task(bank, "two-sum", "pair-sum")
            problems = {row["id"]: row for row in bank_file(bank, "problems")}
            self.assertEqual(problems["pair-sum"].get("origin", "leetcode"), "leetcode")
            self.assertIn("title", problems["pair-sum"])
            self.assertTrue(any("catalog" in note for note in notes))
            # An example is already the instructor's kind of task.
            _, notes = new_task(bank, "delimiter-closer", "my-closer")
            self.assertEqual(notes, [])
            self.assertEqual(len(load_bank(bank, "course", 1)["problems"]), 3)

    def test_files_new_does_not_change_are_left_as_they_were(self):
        with tempfile.TemporaryDirectory() as temporary:
            bank = Path(temporary) / "bank"
            new_task(bank, "two-sum")
            rules = '{"lookAwaySeconds": 12}'
            (bank / "task-set.json").write_text(rules)
            new_task(bank, "valid-parentheses")
            self.assertEqual((bank / "task-set.json").read_text(), rules)

    def test_narrowing_the_languages_drops_the_targets_they_marked(self):
        with tempfile.TemporaryDirectory() as temporary:
            bank = Path(temporary) / "bank"
            _, notes = new_task(bank, "delimiter-closer", languages=["javascript"])
            sidecar = bank_file(bank, "task-mode")["delimiter-closer"]
            self.assertEqual(sidecar["languages"], ["javascript"])
            self.assertEqual(
                [target["language"] for target in sidecar["completionTargets"]],
                ["javascript"],
            )
            self.assertTrue(any("dropped 1" in note for note in notes))
            # A language with no target in a task that has them is pointed out.
            _, notes = new_task(bank, "delimiter-closer", "with-cpp", ["python", "cpp"])
            self.assertTrue(any("cpp" in note for note in notes))

    def test_nothing_is_written_for_a_task_the_bank_would_refuse(self):
        with tempfile.TemporaryDirectory() as temporary:
            bank = Path(temporary) / "bank"
            new_task(bank, "two-sum")
            before = snapshot(bank)
            for reference_id, task_id, languages, message in (
                ("two-sum", None, None, "already in"),
                ("missing-problem", None, None, "unknown reference"),
                ("two-sum", "Bad_Id", None, "invalid task id"),
                ("two-sum", "-dash", None, "invalid task id"),
                ("two-sum", "pair-sum", ["python", "rust"], "unknown language"),
                ("two-sum", "pair-sum", [], "names no language"),
                # Known, but this class judge cannot run in C.
                ("min-stack", None, ["c"], "C runs only function judges"),
            ):
                with self.subTest(reference=reference_id, task_id=task_id):
                    with self.assertRaisesRegex(ValueError, message):
                        new_task(bank, reference_id, task_id, languages)
                    self.assertEqual(snapshot(bank), before)
                    self.assertFalse((bank / STAGING).exists())

    def test_a_bank_that_is_already_invalid_is_refused_not_repaired(self):
        with tempfile.TemporaryDirectory() as temporary:
            bank = Path(temporary) / "bank"
            new_task(bank, "two-sum")
            (bank / "judges.json").write_text("{}")
            before = snapshot(bank)
            with self.assertRaisesRegex(ValueError, "already invalid"):
                new_task(bank, "valid-parentheses")
            self.assertEqual(snapshot(bank), before)

    def test_a_move_cut_short_is_completed_by_the_next_new(self):
        # The task's own entry names it, so it goes into place last and a
        # bank cut short never lists a task it does not hold.
        self.assertEqual(task_authoring.BANK_FILES[-1], "problems")
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            bank, finished = directory / "bank", directory / "finished"
            new_task(bank, "two-sum")
            new_task(finished, "two-sum")
            new_task(finished, "valid-parentheses")
            # What a `new` stopped after validating leaves: the staged bank,
            # its mark, and one file already moved. Built by hand rather than
            # by interrupting a run, which tests in parallel would share.
            staged = bank / STAGING
            staged.mkdir()
            for name in ("variants", "task-mode", "problems"):
                shutil.copyfile(finished / f"{name}.json", staged / f"{name}.json")
            shutil.copyfile(finished / "judges.json", bank / "judges.json")
            (staged / task_authoring.READY).write_text("")
            new_task(bank, "merge-sorted-array")
            self.assertFalse(staged.exists())
            ids = {row["id"] for row in load_bank(bank, "course", 1)["problems"]}
            self.assertEqual(
                ids, {"two-sum", "valid-parentheses", "merge-sorted-array"}
            )

    def test_staging_left_before_validation_is_explained_not_reused(self):
        with tempfile.TemporaryDirectory() as temporary:
            bank = Path(temporary) / "bank"
            new_task(bank, "two-sum")
            # Stopped while staging: a file written, the bank not validated.
            (bank / STAGING).mkdir()
            (bank / STAGING / "judges.json").write_text("{}")
            with self.assertRaisesRegex(ValueError, "delete that directory"):
                new_task(bank, "valid-parentheses")
            self.assertEqual(len(bank_file(bank, "problems")), 1)

    def test_staging_emptied_by_a_finished_move_is_cleared(self):
        with tempfile.TemporaryDirectory() as temporary:
            bank = Path(temporary) / "bank"
            new_task(bank, "two-sum")
            # A finished move that stopped before removing its directory.
            (bank / STAGING).mkdir()
            new_task(bank, "valid-parentheses")
            self.assertFalse((bank / STAGING).exists())
            self.assertEqual(len(bank_file(bank, "problems")), 2)

    def test_versions_count_only_numbered_directories(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            self.assertEqual(next_version(output, "course"), (1, True))
            sets = output / "sets/course"
            for name in ("1", "3", "tmp", "²"):
                (sets / name).mkdir(parents=True)
            (sets / "9").write_text("not a version")
            self.assertEqual(next_version(output, "course"), (4, False))

    def test_a_build_names_the_set_after_the_bank_and_takes_the_next_version(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            bank = directory / "course"
            output = directory / "publish"
            new_task(bank, "delimiter-closer")
            new_task(bank, "two-sum")
            for version in (1, 2):
                built = run(
                    "build",
                    "--bank",
                    bank,
                    "--site",
                    SITE,
                    "--output",
                    output,
                    stdin=PIN + "\n",
                )
                self.assertEqual(built.returncode, 0, built.stderr)
                self.assertIn(f"set course, version {version}", built.stdout)
                self.assertEqual("holds no version" in built.stdout, version == 1)
                self.assertIn(f"{SITE}/sets/course/{version}/", built.stdout)
                for task_id in ("delimiter-closer", "two-sum"):
                    self.assertIn(
                        f"http://localhost:3000/t/course/{task_id}?site="
                        f"https%3A%2F%2Fteacher.github.io%2Fcourse&version={version}",
                        built.stdout,
                    )
                self.assertNotIn(PIN, built.stdout + built.stderr)
            manifest = json.loads((output / "sets/course/2/manifest.json").read_text())
            source = decrypt(
                (output / "sets/course/2/tasks.enc").read_bytes(), manifest, PIN
            )
            self.assertEqual((source["setId"], source["setVersion"]), ("course", 2))

    def test_a_build_refuses_bad_names_and_versions_before_the_pin(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            for name in ("Course", "2026-fall", "cs101_tasks"):
                bank = directory / name
                new_task(bank, "two-sum")
                # No PIN is given: a build that asked for one would fail on
                # reading it, not on the name.
                refused = run(
                    "build",
                    "--bank",
                    bank,
                    "--site",
                    SITE,
                    "--output",
                    directory / "publish",
                    stdin="",
                )
                self.assertNotEqual(refused.returncode, 0)
                self.assertIn("pass --set-id", refused.stderr, name)
            bank = directory / "course"
            new_task(bank, "two-sum")
            for args in (["--version", "0"], ["--set-id", ""]):
                refused = run(
                    "build",
                    "--bank",
                    bank,
                    "--site",
                    SITE,
                    "--output",
                    directory / "publish",
                    *args,
                    stdin="",
                )
                self.assertNotEqual(refused.returncode, 0, args)
                # Refused on the value itself, before the build began.
                self.assertIn("invalid", refused.stderr, args)
                self.assertNotIn("Building", refused.stdout, args)
            self.assertFalse((directory / "publish").exists())

    def test_the_tool_finds_an_interpreter_that_can_build(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            # An interpreter with nothing installed: no `cryptography`.
            subprocess.run(
                [sys.executable, "-m", "venv", "--without-pip", directory / "bare"],
                check=True,
            )
            bare = (
                directory
                / "bare"
                / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
            )
            script = ROOT / "scripts/task-package.py"
            bank = directory / "course"
            # Authoring needs no packages, so it runs where it was started.
            added = subprocess.run(
                [bare, script, "new", "--bank", bank, "--from", "two-sum"],
                capture_output=True,
                text=True,
            )
            self.assertEqual(added.returncode, 0, added.stderr)
            # A build moves to the tool's own environment, here the one these
            # tests run in, which has the package.
            built = subprocess.run(
                [bare, script, "build", "--bank", bank, "--site", SITE]
                + ["--output", directory / "publish"],
                input=PIN + "\n",
                capture_output=True,
                text=True,
                env={**os.environ, "CODETRIAL_TASK_TOOLS": sys.prefix},
            )
            self.assertEqual(built.returncode, 0, built.stderr)
            self.assertIn("Built", built.stdout)
            # An environment that still lacks it is reported, not rerun.
            refused = subprocess.run(
                [bare, script, "build", "--bank", bank, "--site", SITE]
                + ["--output", directory / "publish"],
                input=PIN + "\n",
                capture_output=True,
                text=True,
                env={
                    **os.environ,
                    "CODETRIAL_TASK_TOOLS": str(directory / "bare"),
                    "CODETRIAL_TASK_RUNTIME": "1",
                },
            )
            self.assertNotEqual(refused.returncode, 0)
            self.assertIn("cannot run this tool", refused.stderr)

    def test_the_cli_lists_references_and_adds_a_task(self):
        with tempfile.TemporaryDirectory() as temporary:
            bank = Path(temporary) / "bank"
            listed = run("references")
            self.assertEqual(listed.returncode, 0, listed.stderr)
            self.assertTrue(listed.stdout.startswith("delimiter-closer"))
            added = run(
                "new",
                "--bank",
                bank,
                "--from",
                "two-sum",
                "--languages",
                "python, cpp",
            )
            self.assertEqual(added.returncode, 0, added.stderr)
            self.assertEqual(
                bank_file(bank, "task-mode")["two-sum"]["languages"],
                ["python", "cpp"],
            )
            # The command it suggests runs as printed.
            suggested = added.stdout.splitlines()[1].strip()
            shown = subprocess.run(
                suggested, shell=True, capture_output=True, text=True
            )
            self.assertEqual(shown.returncode, 0, shown.stderr)
            again = run("new", "--bank", bank, "--from", "two-sum")
            self.assertNotEqual(again.returncode, 0)
            self.assertIn("already in", again.stderr)


if __name__ == "__main__":
    unittest.main()

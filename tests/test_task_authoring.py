"""Starting a task bank from reference tasks and building it for a site."""

import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import types
import unittest
from unittest import mock
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from problem_bank import runtime, task_authoring  # noqa: E402
from problem_bank.bank import read_json  # noqa: E402
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


# Without it, a `build` would set up the tool's environment, which is the
# gate's job, not one for tests running in parallel.
needs_cryptography = unittest.skipUnless(
    importlib.util.find_spec("cryptography"),
    "the interpreter running the tests has no `cryptography`",
)


def bank_file(bank, name):
    return read_json(bank / f"{name}.json")


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
                # A file beside the examples, not a reference task.
                ("../task-defaults", None, None, "unknown reference"),
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

    def test_a_bank_the_variant_checks_refuse_is_explained_too(self):
        with tempfile.TemporaryDirectory() as temporary:
            bank = Path(temporary) / "bank"
            new_task(bank, "two-sum")
            # Well formed, but no new name: refused by the variant checks,
            # which raise their own error rather than ValueError.
            variants = bank_file(bank, "variants")
            variants["two-sum"]["entry"] = "pair_sum"
            (bank / "variants.json").write_text(json.dumps(variants))
            before = snapshot(bank)
            with self.assertRaisesRegex(ValueError, "already invalid.*fix it"):
                new_task(bank, "valid-parentheses")
            self.assertEqual(snapshot(bank), before)
            self.assertFalse((bank / STAGING).exists())

    def test_a_sidecar_file_alone_must_still_be_an_object(self):
        with tempfile.TemporaryDirectory() as temporary:
            bank = Path(temporary) / "bank"
            bank.mkdir()
            (bank / "task-mode.json").write_text("[]")
            with self.assertRaisesRegex(ValueError, "already invalid"):
                new_task(bank, "two-sum")
            self.assertEqual(snapshot(bank), {"task-mode.json": "[]"})
            self.assertFalse((bank / STAGING).exists())

    def test_a_refused_task_leaves_no_bank_where_there_was_none(self):
        with tempfile.TemporaryDirectory() as temporary:
            parent = Path(temporary) / "courses"
            # A class judge, so C is refused once the bank is staged.
            with self.assertRaisesRegex(ValueError, "C runs only function judges"):
                new_task(
                    parent / "bank", "insert-delete-getrandom-o1", None, ["c", "python"]
                )
            self.assertFalse(parent.exists())

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

    @needs_cryptography
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

    @needs_cryptography
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
            # Not beside "Course": on a case-insensitive filesystem the two
            # are one directory, which already holds two-sum.
            bank = directory / "valid" / "course"
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

    @needs_cryptography
    # The tool is pointed at this interpreter's own environment, which must be
    # a venv: elsewhere a prefix need not hold bin/python. Handing it a real
    # prefix is safe only because setup never writes into an existing path.
    @unittest.skipIf(sys.prefix == sys.base_prefix, "the tests run outside a venv")
    def test_the_tool_finds_an_interpreter_that_can_build(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            # An interpreter with nothing installed: no `cryptography`.
            subprocess.run(
                [sys.executable, "-m", "venv", "--without-pip", directory / "bare"],
                check=True,
            )
            bare = runtime._python(directory / "bare")
            script = ROOT / "scripts/task-package.py"
            bank = directory / "course"
            # Authoring needs no packages, so it runs where it was started.
            added = subprocess.run(
                [bare, script, "new", "--bank", bank, "--from", "two-sum"],
                capture_output=True,
                text=True,
            )
            self.assertEqual(added.returncode, 0, added.stderr)
            # A build moves to the tool's own environment, which has it.
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
            # Asking a build for its help encrypts nothing, so it runs here.
            helped = subprocess.run(
                [bare, script, "build", "--help"],
                capture_output=True,
                text=True,
                env={
                    **os.environ,
                    "CODETRIAL_TASK_TOOLS": str(directory / "absent"),
                    "CODETRIAL_TASK_RUNTIME": "1",
                },
            )
            self.assertEqual(helped.returncode, 0, helped.stderr)
            self.assertIn("--site", helped.stdout)

    @unittest.skipIf(
        os.environ.get(runtime.RERUN), "the tests run under a rerun of the tool"
    )
    def test_a_build_reruns_under_a_python_on_path_that_has_cryptography(self):
        class Rerun(Exception):
            pass

        def rerun(python):
            raise Rerun(python)

        # Other cases run beside this one in this process and call
        # `_set_up` themselves; only this thread's call is the failure.
        this_thread, real_set_up = threading.get_ident(), runtime._set_up

        def set_up(base, environment):
            if threading.get_ident() == this_thread:
                self.fail(f"set up {environment} with {sys.executable} on PATH")
            return real_set_up(base, environment)

        on_path = {"python3": "/usr/bin/python3", "python3.12": "/opt/python3.12"}
        with tempfile.TemporaryDirectory() as temporary:
            with (
                mock.patch.object(runtime, "tools", lambda: Path(temporary) / "tools"),
                # Neither this interpreter nor `python3` has `cryptography`.
                mock.patch.object(
                    runtime,
                    "importlib",
                    types.SimpleNamespace(
                        util=types.SimpleNamespace(find_spec=lambda name: None)
                    ),
                ),
                mock.patch.object(
                    runtime,
                    "shutil",
                    types.SimpleNamespace(which=on_path.get, rmtree=shutil.rmtree),
                ),
                mock.patch.object(runtime, "_new_enough", lambda python: True),
                mock.patch.object(
                    runtime,
                    "_has_cryptography",
                    lambda python: python == "/opt/python3.12",
                ),
                mock.patch.object(runtime, "_rerun", rerun),
                mock.patch.object(runtime, "_set_up", set_up),
            ):
                with self.assertRaises(Rerun) as reran:
                    runtime.ensure(needs_cryptography=True)
        self.assertEqual(reran.exception.args, ("/opt/python3.12",))

    def test_setting_up_refuses_a_directory_that_is_already_there(self):
        with tempfile.TemporaryDirectory() as temporary:
            existing = Path(temporary) / "prefix"
            existing.mkdir()
            (existing / "keep").write_text("kept")
            with mock.patch.object(runtime.subprocess, "check_call") as call:
                with self.assertRaises(SystemExit) as refused:
                    runtime._set_up(Path(sys.executable), existing)
            call.assert_not_called()
            self.assertIn("exists", str(refused.exception))
            self.assertEqual((existing / "keep").read_text(), "kept")

    def test_a_failed_set_up_leaves_nothing_behind(self):
        with tempfile.TemporaryDirectory() as temporary:
            environment = Path(temporary) / "tools"

            def fail(argv):
                environment.mkdir()
                raise subprocess.CalledProcessError(1, argv)

            with mock.patch.object(runtime.subprocess, "check_call", fail):
                with self.assertRaises(subprocess.CalledProcessError):
                    runtime._set_up(Path(sys.executable), environment)
            self.assertFalse(environment.exists())

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

"""The shape of the mutation job and the aggregate status in check.yml."""

import importlib.util
from pathlib import Path
import re
import shlex
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "check_workflow_support", ROOT / "tests/support.py"
)
support = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(support)


def header(body, key):
    """The value of a job-level key, with a folded scalar joined to one line."""
    found = re.search(rf"^    {key}: (.*(?:\n      .*)*)", body, re.M)
    if found is None:
        raise AssertionError(f"job no longer sets {key}")
    return " ".join(found[1].replace(">-", "").split())


class MutantsJobTests(unittest.TestCase):
    def setUp(self):
        self.body = support.job("mutants")

    def test_waits_for_the_gates_and_the_plan(self):
        needs = set(re.findall(r"[a-z][a-z-]*", header(self.body, "needs")))
        self.assertLessEqual({"gates", "plan-mutants"}, needs)

    def test_starts_only_after_both_succeed_with_work_to_do(self):
        condition = header(self.body, "if").strip("${} ")
        # Exactly these terms, all required. `!cancelled()` replaces the
        # implicit success() that would otherwise skip the job. plan-mutants
        # is skipped on push and merge_group, where its count output is empty
        # and would pass the != '0' guard on its own.
        self.assertEqual(
            [
                "!cancelled()",
                "needs.plan-mutants.result == 'success'",
                "needs.gates.result == 'success'",
                "needs.plan-mutants.outputs.count != '0'",
            ],
            [term.strip() for term in condition.split("&&")],
        )


def mutants_invocations(body):
    """The argument words of every `cargo mutants` command.

    Continuation lines are joined, comments dropped, and the words end at the
    first redirection, pipe or `||`, so a flag only counts when the shell would
    pass it.
    """
    found = []
    for command in re.findall(
        r"^[^#\n]*?cargo mutants\b((?:[^\n]*\\\n)*[^\n]*)", body, re.M
    ):
        words = []
        for word in shlex.split(command.replace("\\\n", " "), comments=True):
            if word[0] in "|>":
                break
            words.append(word)
        found.append(words)
    return found


class MutantsLockfileTests(unittest.TestCase):
    def setUp(self):
        self.plan = mutants_invocations(support.job("plan-mutants"))
        self.shard = mutants_invocations(support.job("mutants"))

    def test_every_selecting_or_running_command_holds_the_lockfile(self):
        # cargo-mutants forwards the flag to every cargo it runs; without it
        # a mutant can be judged against a lockfile cargo quietly updated.
        commands = self.plan + self.shard
        self.assertEqual(3, len(commands))
        for words in commands:
            self.assertIn("--cargo-arg=--locked", words, words)

    def test_plan_and_shard_select_the_same_mutants(self):
        # The shard count comes from the plan's list, so what is listed has to
        # match on both sides: the same flags with the same values.
        def selection(words):
            kept, skip = [], False
            for word in words:
                if skip:
                    skip = False
                elif word == "--shard":
                    skip = True
                elif word not in ("--list", "--in-place"):
                    kept.append(word)
            return sorted(kept)

        listings = [w for w in self.plan + self.shard if "--list" in w]
        self.assertEqual(2, len(listings))
        self.assertEqual(selection(listings[0]), selection(listings[1]))
        run = next(w for w in self.shard if "--list" not in w)
        self.assertEqual(selection(listings[1]), selection(run))


class MutantsLocalRunDocsTests(unittest.TestCase):
    """docs/development.md tells a contributor to run what CI's shard step runs."""

    def setUp(self):
        self.text = (support.ROOT / "docs/development.md").read_text()
        self.block = support.section(
            self.text, "```sh\nSCRATCH=", "```\n", "the local mutation commands"
        )

    def test_both_documented_commands_hold_the_lockfile_like_ci(self):
        documented = mutants_invocations(self.block)
        self.assertEqual(2, len(documented))
        for words in documented:
            self.assertIn("--cargo-arg=--locked", words, words)

    def test_the_run_selects_what_the_shard_step_selects(self):
        # Only the diff's location and the shard differ between a local run and
        # a shard of CI's plan.
        def selection(words):
            kept, skip = [], False
            for word in words:
                if skip:
                    skip = False
                elif word in ("--in-diff", "--shard"):
                    skip = True
                elif word != "--list":
                    kept.append(word)
            return sorted(kept)

        run = next(w for w in mutants_invocations(self.block) if "--list" not in w)
        shard = next(
            w for w in mutants_invocations(support.job("mutants")) if "--list" not in w
        )
        self.assertEqual(selection(shard), selection(run))


class AggregateStatusTests(unittest.TestCase):
    def test_check_accepts_mutants_skipped_after_a_plan(self):
        body = support.job("check")
        self.assertRegex(body, r"success/success \| skipped/success[^)]*\) ;;")
        # A failed gates lane fails the job before the mutants table is read.
        self.assertLess(body.index('"$GATES" != success'), body.index("case "))

    def test_release_does_not_wait_for_mutants(self):
        self.assertNotIn("mutants", header(support.job("release"), "needs"))


if __name__ == "__main__":
    unittest.main()

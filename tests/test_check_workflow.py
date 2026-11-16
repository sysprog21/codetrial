"""The shape of the mutation job and the aggregate status in check.yml."""

import importlib.util
from pathlib import Path
import re
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

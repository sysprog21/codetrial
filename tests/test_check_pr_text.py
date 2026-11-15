"""Check the pull request title and description rules of CONTRIBUTING.md."""

import importlib.util
from pathlib import Path
import subprocess
import sys
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/check-pr-text.py"
FENCE = "`" * 3

_spec = importlib.util.spec_from_file_location("check_pr_text", SCRIPT)
check_pr_text = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(check_pr_text)

BODY = """## Problem and behavior

<!-- The concrete problem. -->
The picker changed the devices behind the setup check.

## Validation

Ran the browser lane.

"""
CUBIC = """
<!-- This is an auto-generated description by cubic. -->
---
## Summary by cubic
Rechecks setup. Fixes #12 and refs #9.
"""


def title(text, head=None):
    return check_pr_text.title_problems(text, head)


def body(text):
    return check_pr_text.body_problems(text)


class Title(unittest.TestCase):
    def test_a_subject_shaped_title_passes(self):
        self.assertEqual(title("Recheck setup after the picker closes"), [])

    def test_the_subject_rules_apply(self):
        for text in (
            "[Bug] Lobby broken",
            "fix(web): recheck setup",
            "Explain site data when the peer lobby cannot store an invite",
            "Update web/app.js",
            "Fix bug",
        ):
            with self.subTest(text=text):
                problems = title(text)
                self.assertTrue(problems)
                self.assertTrue(all(p.startswith("Title: ") for p in problems))

    def test_one_commit_means_its_subject(self):
        self.assertEqual(title("Recheck setup", "Recheck setup"), [])
        problems = title("Recheck the setup", "Recheck setup")
        self.assertEqual(len(problems), 1)
        self.assertIn("reuses its commit subject: Recheck setup", problems[0])


class Command(unittest.TestCase):
    def run_check(self, *args):
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--title", "[Bug] Broken", *args],
            capture_output=True,
            text=True,
            check=False,
        )

    def test_dependabot_is_decided_where_the_commit_log_decides_it(self):
        self.assertEqual(
            self.run_check(
                "--author", "dependabot[bot]", "--author-type", "Bot"
            ).returncode,
            0,
        )
        # Another bot, or a person named like one, is held to the text.
        for author, kind in (("renovate[bot]", "Bot"), ("dependabot[bot]", "User")):
            with self.subTest(author=author, kind=kind):
                result = self.run_check("--author", author, "--author-type", kind)
                self.assertEqual(result.returncode, 1)


class Workflow(unittest.TestCase):
    def test_the_base_branch_s_checker_judges_the_text(self):
        # A pull request that edits the checker must not be judged by its edit.
        text = (SCRIPT.parents[1] / ".github/workflows/pr-text.yml").read_text()
        self.assertIn("python3 base/scripts/check-pr-text.py", text)
        self.assertIn("ref: ${{ github.event.pull_request.base.sha }}", text)
        self.assertNotIn("python3 scripts/check-pr-text.py", text)


class Description(unittest.TestCase):
    def test_close_alone_on_the_last_line_passes(self):
        self.assertEqual(body(BODY + "Close #12\n"), [])
        self.assertEqual(body(BODY + "Closes #12\r\n\r\n---\r\n"), [])
        # GitHub closes the issue whatever the case, as the commit hook reads it.
        self.assertEqual(body(BODY + "close #12\n"), [])

    def test_no_close_at_all_passes(self):
        self.assertEqual(body(BODY), [])

    def test_what_a_bot_appends_is_not_the_author_s(self):
        self.assertEqual(body(BODY + "Close #12\n" + CUBIC), [])

    def test_author_text_after_the_bot_s_closing_comment_is_read(self):
        block = CUBIC + "<!-- End of auto-generated description by cubic. -->\n"
        self.assertEqual(body(BODY + block + "Close #12\n"), [])
        problems = body(BODY + block + "Refs #9\n")
        self.assertEqual(len(problems), 1)
        self.assertIn("no Refs #N", problems[0])

    def test_a_close_before_more_text_is_refused(self):
        problems = body(BODY + "Close #12\n\n## Screenshots\n<img src=x>\n")
        self.assertEqual(len(problems), 1)
        self.assertIn("alone on the last line", problems[0])

    def test_two_closing_lines_are_refused(self):
        self.assertEqual(len(body(BODY + "Close #12\nClose #13\n")), 1)

    def test_a_closing_keyword_in_a_sentence_on_the_last_line(self):
        problems = body(BODY + "This fixes #12 for good.\n")
        self.assertEqual(len(problems), 1)
        self.assertIn("end with Close #N alone", problems[0])

    def test_the_template_s_empty_close_is_refused(self):
        problems = body(BODY + "Close #\n")
        self.assertEqual(len(problems), 1)
        self.assertIn("empty Close #", problems[0])

    def test_refs_is_refused_anywhere(self):
        problems = body("Refs #9 for the history.\n\n" + BODY + "Close #12\n")
        self.assertEqual(len(problems), 1)
        self.assertIn("no Refs #N", problems[0])

    def test_quoted_and_commented_text_is_not_read(self):
        text = (
            BODY
            + FENCE
            + "\nFixes #3\nRefs #4\n"
            + FENCE
            + "\n<!-- Close #5 -->\nClose #12\n"
        )
        self.assertEqual(body(text), [])

    def test_an_ai_attribution_is_refused(self):
        problems = body(
            BODY + "Generated with [Claude Code](https://claude.com/claude-code)\n"
        )
        self.assertEqual(len(problems), 1)
        self.assertIn("AI attribution", problems[0])


if __name__ == "__main__":
    unittest.main()

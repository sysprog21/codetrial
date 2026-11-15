"""Check that commit-reflow.py joins ragged prose, keeps layout, and converges.

The cases name inputs somebody thought of; the sweep at the end writes its own,
from words that sit on the seams (comment characters, list markers, trailer
keys, closing keywords, rules, fences), because the bug a reflow ships with is
the break nobody thought to put next to one of them. Fixed seed, so a failure
reproduces from its case number.
"""

import importlib.util
from pathlib import Path
import random
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/commit-reflow.py"
FENCE = "`" * 3

_spec = importlib.util.spec_from_file_location("commit_reflow", SCRIPT)
commit_reflow = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(commit_reflow)


def body(text, comment_char="#"):
    return commit_reflow.reflow("Recheck setup\n\n" + text, comment_char).split("\n")[
        2:
    ]


class Joins(unittest.TestCase):
    def test_ragged_prose_becomes_one_paragraph_at_72(self):
        ragged = (
            "The picker can change\nthe devices while it is open, so the\n"
            "setup it confirmed before opening\nno longer describes what the "
            "call will use, which is why the check runs again once it closes."
        )
        lines = body(ragged)
        self.assertTrue(all(len(line.encode()) <= 72 for line in lines))
        self.assertEqual(" ".join(lines), " ".join(ragged.split()))
        self.assertEqual(len(lines), 3)
        self.assertGreater(len(lines[0]), 60)

    def test_one_long_line_is_wrapped(self):
        lines = body(" ".join(["word"] * 40))
        self.assertTrue(all(len(line) <= 72 for line in lines))
        self.assertEqual(len(lines), 3)

    def test_the_subject_is_never_touched(self):
        subject = "Recheck setup after the capture picker closes and more words"
        message = subject + "\n\nWhy."
        self.assertEqual(commit_reflow.reflow(message).split("\n")[0], subject)

    def test_a_long_link_stays_whole(self):
        link = "https://example.org/" + "a" * 80
        self.assertIn(link, body("See " + link + " for the reason it fails."))

    def test_a_sample_indented_under_an_item_stays_as_written(self):
        sample = "- Document the example:\n    call_one()\n    call_two()"
        self.assertEqual(body(sample), sample.split("\n"))

    def test_list_items_keep_their_marker_and_hang(self):
        lines = body(
            "- the picker can change the devices while it is open, so the setup "
            "it confirmed\nno longer holds\n- the call\n  uses them\n"
            "10. a numbered item long enough that it has to wrap onto a second "
            "line here"
        )
        self.assertEqual(lines[0][:2], "- ")
        self.assertTrue(lines[1].startswith("  ") and not lines[1].startswith("   "))
        self.assertIn("- the call uses them", lines)
        ten = lines.index(next(line for line in lines if line.startswith("10. ")))
        self.assertTrue(lines[ten + 1].startswith("    "))


class Guards(unittest.TestCase):
    """A break must not forge a line the next run, git or the hook reads as
    something other than prose."""

    def opener(self, text, forbidden):
        for line in body(text)[1:]:
            self.assertFalse(line.startswith(forbidden), line)

    def test_no_line_opens_with_a_comment_character(self):
        self.opener(
            "The whiteboard sheet and its toolbar were spelled as literal "
            "colors #ffffff, #d6d9df, #252a32 and #686868 across both files.",
            "#",
        )

    def test_a_custom_comment_character_is_guarded_too(self):
        lines = body(
            "The run takes the value the old build set as its default, ; here "
            "we go ; and ; then ; some more ; tokens ; to push it past the limit",
            ";",
        )
        self.assertTrue(all(not line.startswith(";") for line in lines[1:]))

    def test_no_line_opens_as_a_list_item(self):
        self.opener(
            "A long sentence here that keeps on going for a while so that the "
            "hyphen - lands at the start of a line, and 2. would too.",
            ("- ", "2. "),
        )

    def test_no_line_opens_as_a_trailer_or_a_heading(self):
        self.opener(
            "The check now reads the value of the header that names the "
            "Signed-off-by: field when a commit has one, and the How: label.",
            ("Signed-off-by:", "How:"),
        )

    def test_no_line_opens_as_the_closing_line(self):
        lines = body(
            "The check had been wrong for a long time and this is the change "
            "that fixes #12 for the lobby."
        )
        self.assertFalse(any(line.startswith("fixes #12") for line in lines))

    def test_a_sentence_wrapped_after_a_number_is_not_a_list(self):
        lines = body(
            "cards inside hidden entries measured as one row of\n57. The test "
            "now hides and restores whole entries."
        )
        self.assertFalse(any(line.startswith("57.") for line in lines))


class PassesThrough(unittest.TestCase):
    def kept(self, block, before="Why this matters.", after=""):
        text = before + "\n\n" + block + ("\n\n" + after if after else "")
        self.assertIn(block, "\n".join(body(text)))

    def test_layout_is_left_as_written(self):
        long = " this line runs on past the limit on purpose" * 2
        for block in (
            FENCE + "\n$ make hooks" + long + "\n" + FENCE,
            "    $ make hooks" + long,
            "> quoted" + long,
            "| a | b |\n|---|---|",
            "Before: 40 ms\nAfter: 12 ms",
            "+--------+\n| box    |\n+--------+",
        ):
            with self.subTest(block=block[:20]):
                self.kept(block)

    def test_the_closing_line_and_trailers_are_kept(self):
        lines = body(
            "The picker can change the devices.\nClose #12\n"
            "Co-authored-by: Ada Lovelace <ada@example.org>"
        )
        self.assertEqual(
            lines,
            [
                "The picker can change the devices.",
                "Close #12",
                "Co-authored-by: Ada Lovelace <ada@example.org>",
            ],
        )

    def test_a_trailer_directly_under_prose_is_kept_in_any_paragraph(self):
        lines = body(
            "The picker can change the devices.\n"
            "Signed-off-by: Ada Lovelace <ada@example.org>\n\nThe rest of the reason."
        )
        self.assertEqual(
            lines[:2],
            [
                "The picker can change the devices.",
                "Signed-off-by: Ada Lovelace <ada@example.org>",
            ],
        )

    def test_a_reference_by_link_is_kept_like_one_by_number(self):
        link = "Refs https://github.com/sysprog21/codetrial/issues/12"
        self.assertEqual(body("The lobby lost the invite.\n" + link)[-1], link)

    def test_trailing_whitespace_lines_settle_in_one_run(self):
        message = "Recheck setup\n\nThe picker changed the devices.\n \n\t\n"
        once = commit_reflow.reflow(message)
        self.assertEqual(commit_reflow.reflow(once), once)
        self.assertEqual(once, "Recheck setup\n\nThe picker changed the devices.\n")

    def test_comments_and_the_scissors_cut_are_kept(self):
        diff = "-------- >8 --------\n+ a long diff line" + " x" * 40
        self.kept("# Commit rules, enforced by scripts/git-commit-msg.sh:")
        self.kept(diff)

    def test_the_cut_under_another_comment_character_is_kept(self):
        # core.commentChar set, or chosen by git under "auto": the diff below
        # its scissors line is still git's, not prose to join.
        cut = "; ------------------------ >8 ------------------------"
        diff = cut + "\ndiff --git a/x b/x\n+" + "word " * 20 + "\n+short"
        text = "Why this matters.\n\n" + diff
        self.assertIn(diff, "\n".join(body(text, ";")))

    def test_an_ambiguous_paragraph_goes_out_as_it_came(self):
        # Joined, "Fixes" and "#12" would read as the closing line, and "1."
        # with what follows as a list item.
        for text in ("1.\nthe rest of it", "Fixes\n#12"):
            with self.subTest(text=text):
                self.assertEqual(body(text), text.split("\n"))


class Converges(unittest.TestCase):
    VOCAB = "the lobby setup call picker devices token because so it was".split() + [
        "#12",
        "#",
        "-",
        "*",
        "+",
        "1.",
        "12.",
        "2)",
        "Fixes",
        "close",
        "Refs",
        "Signed-off-by:",
        "How:",
        "Note:",
        "Before:",
        ">",
        "|",
        "---",
        "===",
        FENCE,
        "~~~",
        "-------- >8 --------",
        "https://example.org/" + "a" * 80,
        "web/app.js",
        "a::b",
        "x" * 75,
    ]

    def test_no_word_is_lost_and_a_second_run_changes_nothing(self):
        rng = random.Random(7)
        for case in range(1500):
            paragraphs = []
            for _ in range(rng.randint(1, 3)):
                words = [rng.choice(self.VOCAB) for _ in range(rng.randint(3, 40))]
                limit = rng.choice([15, 30, 50, 80, 10**6])
                lines, line = [], ""
                for word in words:
                    if line and len(line) + 1 + len(word) <= limit:
                        line += " " + word
                    else:
                        if line:
                            lines.append(line)
                        line = word
                lines.append(line)
                paragraphs.append("\n".join(lines))
            message = "Recheck setup\n\n" + "\n\n".join(paragraphs)
            once = commit_reflow.reflow(message)
            with self.subTest(case=case):
                self.assertEqual(once.split(), message.split())
                self.assertEqual(commit_reflow.reflow(once), once)


class Command(unittest.TestCase):
    RAGGED = "Recheck setup\n\nThe picker can change\nthe devices.\n"

    def run_script(self, *args, stdin=None):
        return subprocess.run(
            [sys.executable, str(SCRIPT), *args],
            input=stdin,
            capture_output=True,
            text=True,
            check=False,
        )

    def test_a_file_is_rewritten_in_place_and_check_writes_nothing(self):
        with tempfile.TemporaryDirectory() as scratch:
            path = Path(scratch) / "MSG"
            path.write_text(self.RAGGED)
            self.assertEqual(self.run_script("--check", str(path)).returncode, 1)
            self.assertEqual(path.read_text(), self.RAGGED)
            self.assertEqual(self.run_script(str(path)).returncode, 0)
            fixed = "Recheck setup\n\nThe picker can change the devices.\n"
            self.assertEqual(path.read_text(), fixed)
            self.assertEqual(self.run_script("--check", str(path)).returncode, 0)

    def test_standard_input_is_filtered(self):
        result = self.run_script("-", stdin=self.RAGGED)
        self.assertEqual(
            result.stdout, "Recheck setup\n\nThe picker can change the devices.\n"
        )

    def test_a_message_that_is_not_utf8_is_rewrapped_not_a_crash(self):
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "-"],
            input=b"Recheck setup\n\nThe picker \xff can change\nthe devices.\n",
            capture_output=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            result.stdout, b"Recheck setup\n\nThe picker \xff can change the devices.\n"
        )

    def test_the_comment_character_comes_from_git(self):
        with tempfile.TemporaryDirectory() as scratch:
            subprocess.run(["git", "init", "-q", scratch], check=True)
            subprocess.run(
                ["git", "-C", scratch, "config", "core.commentChar", ";"], check=True
            )
            message = "Recheck setup\n\nThe picker can change\n; a comment git drops\n"
            result = subprocess.run(
                [sys.executable, str(SCRIPT), "-"],
                input=message,
                cwd=scratch,
                capture_output=True,
                text=True,
                check=False,
            )
        self.assertEqual(result.stdout, message)


if __name__ == "__main__":
    unittest.main()

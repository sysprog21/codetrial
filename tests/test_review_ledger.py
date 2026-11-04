#!/usr/bin/env python3
"""Contract tests for .claude/skills/codetrial-review/review-ledger.py.

Cases that only need the placement call `run` directly. A child process is kept
for what a process owns: exit codes, the printed text and a relative root. It
is never an in-process stdout redirect, since the gate runs cases on threads.
"""

import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / ".claude" / "skills" / "codetrial-review" / "review-ledger.py"
SPEC = importlib.util.spec_from_file_location("review_ledger", SCRIPT)
ledger = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ledger)

OLD, NEW = "1111111", "2222222"

LEDGER = """# Pending
- [P1] a.rs on line 3-4: refused admission still mints - "if !allowed {" (at 1111111)
- [P2] a.rs on line 9: handler added twice - "listen(topic)" (at 1111111)
- [P2] a.rs on line 1: gone - "removed_call()" (at 1111111)
- [P3] missing.rs on line 2: file left - "x" (at 1111111)
- [P3] a.rs: whole file
- prose bullet that is not a finding

# Completed
- [P2] a.rs on line 99: not placed - "nothing" (at 1111111)
"""

MOVED = LEDGER.replace("on line 3-4", "on line 5-6", 1).replace(
    "on line 9", "on line 6", 1
)
WRITTEN = MOVED.replace(
    '"if !allowed {" (at 1111111)', '"if !allowed {" (at 2222222)'
).replace('"listen(topic)" (at 1111111)', '"listen(topic)" (at 2222222)')


def pending(line, quote=None, text="d", at=None, heading="# Pending", span=None):
    """A one-bullet ledger on a.rs."""
    where = "%s" % line if span is None else "%s-%s" % (line, span)
    bullet = "- [P1] a.rs on line %s: %s" % (where, text)
    if quote is not None:
        bullet += ' - "%s"' % quote
    if at:
        bullet += " (at %s)" % at
    return "%s\n%s\n" % (heading, bullet)


class Case(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        (self.root / "a.rs").write_text(
            "one\ntwo\nthree\nfour\nif !allowed {\n  listen(topic)\n"
        )
        self.ledger = self.root / "ledger.md"
        self.ledger.write_text(LEDGER)

    def run_ledger(self, text=None, write=False, head=None):
        if text is not None:
            self.ledger.write_bytes(text.encode())
        return ledger.run(str(self.ledger), str(self.root), write, head)

    def states(self, text):
        return [(r["state"], r["from"], r["to"]) for r in self.run_ledger(text)]

    def rewrite(self, text, head=NEW):
        self.run_ledger(text, write=True, head=head)
        return self.ledger.read_bytes().decode()

    def cli(self, *args, cwd=None, root=None):
        p = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                str(self.ledger),
                root or str(self.root),
                *args,
            ],
            cwd=cwd,
            capture_output=True,
            text=True,
        )
        return p.returncode, p.stdout, p.stderr

    # placement

    def test_states(self):
        self.assertEqual(
            self.states(LEDGER),
            [
                ("moved", 3, 5),
                ("moved", 9, 6),
                ("stale", 1, 1),
                ("stale", 2, 2),
                ("exact", None, None),
                ("unparsed", None, None),
            ],
        )

    def test_place(self):
        dup = ["x", "dup", "x", "x", "dup"]
        for lines, start, want in (
            (dup, 4, ("moved", 5)),
            (dup, 3, ("moved", 2)),
            (dup, 2, ("exact", 2)),
            (["dup", "x", "dup"], 2, ("moved", 1)),
            (["x", "y", "dup"], 1, ("moved", 3)),
        ):
            b = {"start": start, "end": start, "quote": "dup"}
            self.assertEqual(ledger.place(b, lines), want, (lines, start))
        b = {"start": 1, "end": 1, "quote": "dup"}
        self.assertEqual(ledger.place(b, None), ("stale", 1))

    def test_start_beyond_the_file_finds_a_quote_near_the_top(self):
        self.assertEqual(self.states(pending(99, "two")), [("moved", 99, 2)])

    def test_a_trailing_space_does_not_hide_a_bullet(self):
        text = pending(1, "nothing", at=OLD).rstrip("\n") + " \n"
        self.assertEqual(self.states(text), [("stale", 1, 1)])

    def test_a_line_without_a_quote_is_unchecked_and_not_restamped(self):
        text = pending(99, at=OLD)
        self.assertEqual(self.states(text), [("unchecked", 99, 99)])
        self.assertEqual(self.rewrite(text), text)

    def test_list_items_that_do_not_parse_are_counted_and_kept(self):
        text = (
            "# Pending\n"
            '- [P1] a.rs on line 1: ok - "two"\n'
            '- [P2] d/g h.txt on line 1: space in the path - "x"\n'
            '  - [P2] a.rs on line 1: indented - "two"\n'
            '- a.rs on line 1: no level - "two"\n'
            "plain prose is not an item\n"
        )
        self.assertEqual(
            [r["state"] for r in self.run_ledger(text)],
            ["moved", "unparsed", "unparsed", "unparsed"],
        )
        self.assertEqual(
            self.rewrite(text),
            text.replace("line 1: ok", "line 2: ok", 1).replace(
                '"two"\n- [P2] d/g', '"two" (at 2222222)\n- [P2] d/g', 1
            ),
        )

    def test_a_bullet_without_a_line_is_stale_only_when_its_path_is_gone(self):
        (self.root / "dir").mkdir()
        text = "# Pending\n- [P2] a.rs: file\n- [P2] dir: directory\n- [P2] gone.rs: file\n- [P2] ../x: out\n"
        self.assertEqual(
            [r["state"] for r in self.run_ledger(text)],
            ["exact", "exact", "stale", "stale"],
        )

    def test_fenced_text_is_never_a_heading_or_a_bullet(self):
        text = (
            "# Notes\n```\n## Pending\n"
            + '- [P1] a.rs on line 1: example - "two"\n```\n'
            + pending(1, "two")
        )
        self.assertEqual(self.states(text), [("moved", 1, 2)])
        fence = text[: text.rindex("# Pending\n")]
        self.assertEqual(self.rewrite(text, head=None), fence + pending(2, "two"))

    def test_a_fence_inside_pending_hides_its_bullets_and_pending_resumes(self):
        text = (
            "# Pending\n```\n"
            + '- [P1] a.rs on line 1: example - "two"\n```\n'
            + '- [P1] a.rs on line 1: real - "two"\n'
        )
        self.assertEqual(self.states(text), [("moved", 1, 2)])

    def test_a_fence_closes_only_on_its_own_marker_and_length(self):
        fence3, fence4 = "`" * 3, "`" * 4
        bullet = '- [P1] a.rs on line 1: %s - "two"\n'
        real = bullet % "real"
        for name, head, middle in (
            ("a shorter fence inside a longer one", fence4, fence3),
            ("tildes inside backticks", fence3, "~~~"),
            ("a closer with an info string", fence3, fence3 + "rust"),
        ):
            text = "## Pending\n" + head + "\n" + middle + "\n"
            text += bullet % "fenced" + head + "\n" + real
            self.assertEqual(
                [(r["state"], r["from"], r["to"]) for r in self.run_ledger(text)],
                [("moved", 1, 2)],
                name,
            )

    def test_an_unclosed_fence_reports_itself_and_the_items_it_hides(self):
        text = (
            "# Pending\n"
            + '- [P1] a.rs on line 1: ok - "two"\n```\n- [P1] a.rs on line 9: y - "x"\n'
        )
        results = self.run_ledger(text)
        self.assertEqual(
            [r["state"] for r in results], ["moved", "unparsed", "unparsed"]
        )
        self.assertIn("unclosed code fence", results[1]["line"])
        self.assertIn("a.rs on line 9", results[2]["line"])
        self.assertEqual(
            self.rewrite(text, head=None), text.replace("line 1: ok", "line 2: ok", 1)
        )

    def test_only_the_open_fence_reports_its_items(self):
        fence = "```"
        text = (
            "# Pending\n"
            + fence
            + "\n- one\n"
            + fence
            + "\n"
            + '- [P1] a.rs on line 1: real - "two"\n'
            + fence
            + "\n- two\n"
        )
        lines = [r["line"] for r in self.run_ledger(text) if r["state"] == "unparsed"]
        self.assertEqual(len(lines), 2)
        self.assertIn("unclosed code fence", lines[0])
        self.assertEqual(lines[1], "- two")

    def test_an_unclosed_fence_outside_pending_is_still_reported(self):
        text = (
            "# Notes\n```\n- a note\n# Pending\n" + '- [P1] a.rs on line 1: x - "two"\n'
        )
        results = self.run_ledger(text)
        self.assertEqual([r["state"] for r in results], ["unparsed"])
        self.assertIn("unclosed code fence", results[0]["line"])

    def test_an_inline_code_span_or_a_backtick_in_the_info_does_not_open_a_fence(self):
        for opener in ("```code``` is the idiom", "``` a`b", "```` x`"):
            text = "# Notes\n" + opener + "\n" + pending(1, "two")
            self.assertEqual(self.states(text), [("moved", 1, 2)], opener)

    def test_a_tilde_opener_may_hold_a_backtick(self):
        text = "# Pending\n~~~ a`b\n" + '- [P1] a.rs on line 1: hidden - "two"\n~~~\n'
        self.assertEqual(self.states(text), [])

    def test_a_tilde_fence_hides_a_heading_and_a_bullet(self):
        text = (
            "# Pending\n~~~~\n## Completed\n"
            + '- [P1] a.rs on line 1: hidden - "two"\n~~~~~\n'
            + '- [P1] a.rs on line 1: real - "two"\n'
        )
        self.assertEqual(self.states(text), [("moved", 1, 2)])

    def test_numbered_and_tab_separated_items_are_unparsed(self):
        text = "# Pending\n1. [P1] a.rs: numbered\n2) [P1] a.rs: paren\n-\t[P1] a.rs: tab\n"
        self.assertEqual([r["state"] for r in self.run_ledger(text)], ["unparsed"] * 3)

    def test_a_heading_must_be_the_word_pending(self):
        for heading in ("# Pendingness", "# Pend", "# Not pending"):
            self.assertEqual(
                self.states(pending(1, "two", heading=heading)), [], heading
            )

    def test_an_inverted_range_is_written_as_one_line(self):
        self.assertEqual(
            self.rewrite(pending(1, "three", span=0), head=None), pending(3, "three")
        )

    def test_pending_heading_at_any_level(self):
        for heading in ("## Pending", "# Pending (1)", "# pending"):
            self.assertEqual(
                self.states(pending(1, "two", heading=heading)),
                [("moved", 1, 2)],
                heading,
            )
        self.assertEqual(self.states(pending(1, "two", heading="# Findings")), [])

    def test_the_quote_is_everything_after_the_first_separator(self):
        b = ledger.parse_bullet('- [P1] a.rs on line 1: d - "x - "y" - "z"')
        self.assertEqual(b["quote"], 'x - "y" - "z')
        for line in (
            '- [P1] a.rs on line 1: d - "unclosed',
            '- [P1] a.rs on line 1: d - ""',
        ):
            self.assertIsNone(ledger.parse_bullet(line)["quote"], line)

    # writing

    def test_write_changes_only_placed_bullets(self):
        self.assertEqual(self.rewrite(LEDGER), WRITTEN)
        before = self.ledger.stat().st_mtime_ns
        self.run_ledger(write=True, head=NEW)
        self.assertEqual(self.ledger.read_text(), WRITTEN)
        self.assertEqual(self.ledger.stat().st_mtime_ns, before)
        self.assertFalse(Path(str(self.ledger) + ".tmp").exists())

    def test_write_without_a_head_moves_numbers_and_leaves_commits(self):
        self.assertEqual(self.rewrite(LEDGER, head=None), MOVED)

    def test_write_adds_a_commit_to_a_bullet_without_one(self):
        self.assertEqual(self.rewrite(pending(2, "two")), pending(2, "two", at=NEW))

    def test_only_the_final_commit_is_replaced(self):
        text = pending(1, "two", text="see (at abcdef1) here", at=OLD)
        self.assertEqual(
            self.rewrite(text), pending(2, "two", text="see (at abcdef1) here", at=NEW)
        )

    def test_a_range_moves_whole_and_a_message_may_mention_a_line(self):
        text = pending(1, "if !allowed {", text="see on line 7", span=2)
        self.assertEqual(self.states(text), [("moved", 1, 5)])
        self.assertEqual(
            self.rewrite(text, head=None),
            pending(5, "if !allowed {", text="see on line 7", span=6),
        )

    def test_crlf_and_trailing_spaces_survive_a_write(self):
        text = (
            pending(1, "two", at=OLD).replace("\n", "  \r\n")
            + "\r\n# Notes\r\nkeep \r\n"
        )
        want = text.replace("on line 1", "on line 2").replace(OLD, NEW)
        self.assertEqual(self.rewrite(text), want)

    # paths

    def test_paths_that_leave_the_root_read_as_missing(self):
        outside = self.root / "outside.rs"
        outside.write_text("secret\n")
        inner = self.root / "inner"
        inner.mkdir()
        (inner / "link.rs").symlink_to(outside)
        sibling = self.root / "inner2"
        sibling.mkdir()
        (sibling / "x.rs").write_text("x\n")
        for path in ("../a.rs", "link.rs", "missing.rs", "../inner2/x.rs", "a\0b.rs"):
            self.assertIsNone(ledger.read_lines(str(inner), path), path)
        self.assertEqual(ledger.read_lines("/", str(outside).lstrip("/"))[0], "secret")

    def test_the_cli_writes_and_stamps_with_write_and_head(self):
        self.ledger.write_text(LEDGER)
        code, out, err = self.cli("--write", "--head", NEW)
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(self.ledger.read_text(), WRITTEN)

    def test_write_goes_through_a_symlinked_ledger(self):
        real = self.root / "real.md"
        real.write_text(LEDGER)
        self.ledger.unlink()
        self.ledger.symlink_to(real)
        self.run_ledger(write=True, head=NEW)
        self.assertTrue(self.ledger.is_symlink())
        self.assertEqual(real.read_text(), WRITTEN)

    def test_a_quote_keeps_its_indentation(self):
        text = pending(6, "    listen(topic)", at=OLD)
        self.assertEqual(self.states(text), [("stale", 6, 6)])
        self.assertEqual(self.states(pending(6, " ", at=OLD)), [("unchecked", 6, 6)])

    def test_a_symlinked_root_is_followed(self):
        link = self.root / "rootlink"
        os.symlink(self.root, link)
        self.assertEqual(ledger.read_lines(str(link), "a.rs")[0], "one")

    # the command line

    def test_printed_summary(self):
        code, out, _ = self.cli()
        self.assertEqual(code, 0)
        self.assertIn("moved a.rs 3 -> 5\n", out)
        self.assertEqual(
            out.splitlines()[-1], "exact 1, moved 2, stale 2, unchecked 0, unparsed 1"
        )
        self.assertEqual(self.ledger.read_text(), LEDGER)

    def test_json_output(self):
        code, out, err = self.cli("--json")
        self.assertEqual(code, 0, err)
        self.assertEqual(json.loads(out)[0]["state"], "moved")

    def test_bad_arguments_exit_2_and_change_nothing(self):
        for args in (
            ["--head", "nothex1"],
            ["--write", "--head", "abc12"],
            ["--head", "a" * 41],
        ):
            self.assertEqual(self.cli(*args)[0], 2, args)
        self.assertEqual(self.cli(root=str(self.root / "nope"))[0], 2)
        self.assertEqual(self.ledger.read_text(), LEDGER)

    def test_unreadable_ledger_exits_2(self):
        self.ledger.write_bytes(b"# Pending\n\xff\n")
        self.assertEqual(self.cli()[0], 2)
        self.ledger.unlink()
        self.assertEqual(self.cli()[0], 2)

    def test_relative_root(self):
        code, out, err = self.cli("--json", cwd=self.root, root=".")
        self.assertEqual(code, 0, err)
        self.assertEqual(json.loads(out)[0]["state"], "moved")


if __name__ == "__main__":
    unittest.main()

"""The explainer draft of a reference note: what it accepts and what it refuses.

A note's diagrams, tables and code are checked against the pinned diff when the
reviewer writes it, so what the page draws is something the diff holds.
"""

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

SKILL = Path(__file__).resolve().parents[1] / ".claude/skills/codetrial-review"


def load(name, file):
    spec = importlib.util.spec_from_file_location(name, SKILL / file)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


explainer = load("explainer", "explainer.py")
workbench = load("review_workbench_for_explainer", "workbench.py")
diffmodel = load("diffmodel_for_explainer", "diffmodel.py")

DIFF = """diff --git a/src/cache.rs b/src/cache.rs
--- a/src/cache.rs
+++ b/src/cache.rs
@@ -10,5 +10,6 @@ fn get
 fn get(key: &str) -> Option<Value> {
-    db.load(key)
+    let hit = cache.lookup(key);
+    hit.or_else(|| db.load(key))
 }

 fn put() {}
"""


def files():
    return diffmodel.reading(DIFF)["files"]


def read(text, trusted=True):
    return explainer.read(text, files(), trusted)


class ReaderTest(unittest.TestCase):
    def test_prose_needs_no_syntax(self):
        got = read("The cache is read first.\nThen the database.\n\n- one\n- two")
        self.assertEqual(got["problems"], [])
        self.assertEqual([b["t"] for b in got["blocks"]], ["p", "ul"])
        self.assertEqual(
            got["blocks"][0]["parts"], ["The cache is read first. Then the database."]
        )

    def test_a_citation_names_a_line_of_either_side(self):
        got = read("New @src/cache.rs:11-12 and old @src/cache.rs:o11 lines.")
        self.assertEqual(got["problems"], [])
        cites = [p for p in got["blocks"][0]["parts"] if isinstance(p, dict)]
        self.assertEqual(
            cites,
            [
                {"path": "src/cache.rs", "side": "new", "start": 11, "end": 12},
                {"path": "src/cache.rs", "side": "old", "start": 11, "end": 11},
            ],
        )

    def test_a_line_outside_the_hunks_is_refused(self):
        for text, why in (
            ("see @src/cache.rs:40", "line 40 is not in the pinned diff"),
            ("see @src/cache.rs:9-11", "line 9 is not in the pinned diff"),
            ("see @src/cache.rs:o20", "line 20 is not in the pinned diff"),
            ("see @src/cache.rs:12-11", "range of 1 to"),
        ):
            with self.subTest(text):
                problems = read(text)["problems"]
                self.assertEqual(len(problems), 1, problems)
                self.assertIn(why, problems[0])

    def test_current_path_takes_precedence_over_a_renamed_old_path(self):
        def row(n):
            return {"o": n, "n": None, "k": "d", "t": str(n)}

        entries = [
            {"path": "new", "old_path": "old", "hunks": [{"lines": [row(1)]}]},
            {"path": "old", "hunks": [{"lines": [row(2)]}]},
        ]
        self.assertTrue(explainer.read("@old:o1", entries)["problems"])
        self.assertEqual(explainer.read("@old:o2", entries)["problems"], [])

    def test_nothing_is_cited_when_the_diff_could_not_be_read_whole(self):
        self.assertEqual(read("plain text", trusted=False)["problems"], [])
        problems = read("see @src/cache.rs:11", trusted=False)["problems"]
        self.assertIn("could not be read line by line", problems[0])

    def test_a_solid_edge_needs_a_citation_and_a_dashed_one_is_inferred(self):
        got = read(
            "```flow LR\n"
            "get -> cache: lookup first @src/cache.rs:11\n"
            "cache --> db: on a miss\n"
            "```"
        )
        self.assertEqual(got["problems"], [])
        flow = got["blocks"][0]
        self.assertEqual([n["text"] for n in flow["nodes"]], ["get", "cache", "db"])
        solid, dashed = flow["edges"]
        self.assertFalse(solid["inferred"])
        self.assertEqual(solid["ev"]["start"], 11)
        self.assertTrue(dashed["inferred"])
        self.assertIsNone(dashed["ev"])
        problems = read("```flow\na -> b\n```")["problems"]
        self.assertIn("a solid edge needs a diff line", problems[0])
        self.assertTrue(problems[0].startswith("line 2:"))

    def test_flow_limits_and_shape(self):
        edges = "\n".join(f"n{i} --> n{i + 1}" for i in range(13))
        for text, why in (
            ("```flow\na --> a\n```", "two different nodes"),
            ("```flow UP\na --> b\n```", "LR or TB"),
            ("```flow\n```", "at least one edge"),
            (f"```flow\n{edges}\n```", "at most 12 nodes"),
            ("```flow\na --> b @src/cache.rs:11 @src/cache.rs:12\n```", "one citation"),
            ("```flow\njust words\n```", "From -> To"),
        ):
            with self.subTest(text[:20]):
                got = read(text)
                self.assertTrue(any(why in p for p in got["problems"]), got["problems"])

    def test_code_comes_from_the_diff_and_cannot_be_typed(self):
        got = read("```code src=src/cache.rs lines=11-12\n```")
        self.assertEqual(got["problems"], [])
        block = got["blocks"][0]
        self.assertEqual(
            [(r["k"], r["n"], r["t"]) for r in block["rows"]],
            [
                ("a", 11, "    let hit = cache.lookup(key);"),
                ("a", 12, "    hit.or_else(|| db.load(key))"),
            ],
        )
        old = read("```code src=src/cache.rs lines=11 side=old\n```")["blocks"][0]
        self.assertEqual(
            [(r["k"], r["o"], r["t"]) for r in old["rows"]],
            [("d", 11, "    db.load(key)")],
        )
        for text, why in (
            (
                "```code src=src/cache.rs lines=11\nlet x = 1;\n```",
                "leave a code block empty",
            ),
            ("```code lines=11\n```", "code src=path"),
            ("```code src=src/cache.rs lines=11 side=left\n```", "code src=path"),
            ("```code src=src/cache.rs lines=99\n```", "not in the pinned diff"),
            ("```flow\na --> b", "never closed"),
        ):
            with self.subTest(text[:30]):
                problems = read(text)["problems"]
                self.assertTrue(any(why in p for p in problems), problems)

    def test_table_cells_carry_citations(self):
        got = read(
            "| Option | Cost |\n| --- | --- |\n"
            "| Read through | one extra lookup @src/cache.rs:11 |\n"
            "| Write behind | a flush queue |\n"
        )
        self.assertEqual(got["problems"], [])
        table = got["blocks"][0]
        self.assertEqual(table["head"], [["Option"], ["Cost"]])
        self.assertEqual(len(table["rows"]), 2)
        self.assertEqual(table["rows"][0][1][1]["start"], 11)
        for text, why in (
            ("| a | b |\n|---|---|\n| c |", "as many cells"),
            ("| a | b |\n|---|---|", "1 to 8 rows"),
            ("| a |\n|---|\n| c @src/cache.rs:50 |", "not in the pinned diff"),
        ):
            with self.subTest(text[:12]):
                problems = read(text)["problems"]
                self.assertTrue(any(why in p for p in problems), problems)


class LimitsTest(unittest.TestCase):
    """Each documented limit holds at the limit and refuses one past it."""

    def refused(self, text):
        return read(text)["problems"]

    def test_a_flow_holds_16_edges_and_48_characters_a_name_and_a_label(self):
        edges = lambda n: "\n".join(f"n{i} --> n{i + 1}" for i in range(n))  # noqa: E731
        self.assertEqual(self.refused(f"```flow\n{edges(11)}\n```"), [])  # 12 nodes
        self.assertTrue(self.refused(f"```flow\n{edges(12)}\n```"))  # 13 nodes
        same = "\n".join(["a --> b"] * 16)
        self.assertEqual(self.refused(f"```flow\n{same}\n```"), [])
        self.assertTrue(self.refused(f"```flow\n{same}\na --> b\n```"))
        self.assertEqual(self.refused(f"```flow\n{'x' * 48} --> b\n```"), [])
        self.assertTrue(self.refused(f"```flow\n{'x' * 49} --> b\n```"))
        self.assertEqual(self.refused(f"```flow\na --> b: {'x' * 48}\n```"), [])
        self.assertTrue(self.refused(f"```flow\na --> b: {'x' * 49}\n```"))

    def test_a_table_holds_5_columns_and_8_rows(self):
        table = lambda cols, rows: (  # noqa: E731
            "|"
            + "a|" * cols
            + "\n|"
            + "---|" * cols
            + "\n"
            + ("|" + "b|" * cols + "\n") * rows
        )
        self.assertEqual(self.refused(table(5, 8)), [])
        self.assertTrue(self.refused(table(6, 1)))
        self.assertTrue(self.refused(table(1, 9)))

    def test_code_holds_30_lines_and_a_note_3_code_blocks_and_24_blocks(self):
        rows = [{"o": None, "n": n, "k": "a", "t": str(n)} for n in range(1, 61)]
        entries = [{"path": "big.rs", "hunks": [{"lines": rows}]}]
        code = lambda span: f"```code src=big.rs lines=1-{span}\n```"  # noqa: E731
        self.assertEqual(explainer.read(code(30), entries)["problems"], [])
        self.assertIn(
            "at most 30 lines", explainer.read(code(31), entries)["problems"][0]
        )
        three = "\n\n".join([code(2)] * 3)
        self.assertEqual(explainer.read(three, entries)["problems"], [])
        self.assertIn(
            "at most 3 blocks",
            explainer.read(three + "\n\n" + code(2), entries)["problems"][0],
        )
        paragraphs = lambda n: "\n\n".join(f"p{i}" for i in range(n))  # noqa: E731
        self.assertEqual(self.refused(paragraphs(24)), [])
        self.assertIn("at most 24 blocks", self.refused(paragraphs(25))[0])


class TableCellTest(unittest.TestCase):
    def test_a_pipe_inside_a_code_span_is_part_of_the_cell(self):
        got = read("| Form | Meaning |\n|---|---|\n| `a | b` | a union |")
        self.assertEqual(got["problems"], [])
        self.assertEqual(got["blocks"][0]["rows"][0][0], ["`a | b`"])
        self.assertEqual(len(got["blocks"][0]["head"]), 2)


class RenameTest(unittest.TestCase):
    def entries(self):
        row = {"o": 1, "n": None, "k": "d", "t": "gone"}
        return [{"path": "new.rs", "old_path": "old.rs", "hunks": [{"lines": [row]}]}]

    def test_a_renamed_file_is_cited_by_its_old_name_for_the_old_side(self):
        self.assertEqual(explainer.read("@old.rs:o1", self.entries())["problems"], [])
        self.assertEqual(explainer.read("@new.rs:o1", self.entries())["problems"], [])

    def test_the_old_name_has_no_new_side(self):
        self.assertTrue(explainer.read("@old.rs:1", self.entries())["problems"])


class LineMapTest(unittest.TestCase):
    def test_a_file_cited_many_times_has_its_lines_read_once(self):
        rows = [{"k": "a", "t": "x", "o": None, "n": n} for n in range(1, 501)]
        reader = explainer.Reader([{"path": "a.py", "hunks": [{"lines": rows}]}])
        first = reader.lines_of("a.py", "new")
        for _ in range(50):
            self.assertIs(reader.lines_of("a.py", "new"), first)
        self.assertIsNot(reader.lines_of("a.py", "old"), first)


class ReviewFindingsTest(unittest.TestCase):
    """Defects three independent reviewers reproduced; each stays fixed."""

    def test_a_note_written_before_the_format_existed_is_still_prose(self):
        # The managed worker writes free text; refusing it fails a paid call.
        for text in (
            "Use:\n```rust\nfoo()\n```\nafter",
            "```\nbare fence\n```",
            "A | B is a union, not a table\n| still not a table",
            "Mail a@b.com:80 or clone git@github.com:22/x.git",
            "```flowchart\nnot ours\n```",
        ):
            with self.subTest(text[:20]):
                got = read(text)
                self.assertEqual(got["problems"], [])
                self.assertTrue(all(b["t"] == "p" for b in got["blocks"]))

    def test_an_unknown_fence_does_not_split_the_prose_around_it(self):
        got = read("Use:\n```rust\nfoo()\n```\nafter")
        self.assertEqual([b["t"] for b in got["blocks"]], ["p"])

    def test_a_citation_spans_at_most_forty_lines(self):
        rows = [{"o": None, "n": n, "k": "a", "t": str(n)} for n in range(1, 101)]
        entries = [{"path": "big.rs", "hunks": [{"lines": rows}]}]
        self.assertEqual(explainer.read("@big.rs:1-40", entries)["problems"], [])
        problems = explainer.read("@big.rs:1-41", entries)["problems"]
        self.assertIn("range of 1 to 40", problems[0])

    def test_prose_may_mention_things_that_look_like_citations(self):
        for text in (
            "Run it on @localhost:8080 first.",
            "See `@src/cache.rs:99` for the form.",
            "Ask @alice:3 about it.",
            "Name it @other.rs:11 if you like.",
        ):
            with self.subTest(text):
                got = read(text)
                self.assertEqual(got["problems"], [])
                self.assertEqual(got["blocks"][0]["parts"], [text])

    def test_prose_naming_a_real_file_at_a_missing_line_is_still_refused(self):
        problems = read("See @src/cache.rs:99.")["problems"]
        self.assertIn("not in the pinned diff", problems[0])

    def test_a_table_cell_is_strict_about_unknown_files(self):
        problems = read("| a | b |\n|---|---|\n| x @src/other.rs:11 | y |")["problems"]
        self.assertIn("src/other.rs is not in the pinned diff", problems[0])

    def test_a_dashed_edge_is_an_inference_and_never_cites_a_line(self):
        problems = read("```flow\na --> b @src/cache.rs:11\n```")["problems"]
        self.assertIn("marks an inference and cites no line", problems[0])
        edge = read("```flow\na -> b @src/cache.rs:11\n```")["blocks"][0]["edges"][0]
        self.assertFalse(edge["inferred"])
        edge = read("```flow\na --> b\n```")["blocks"][0]["edges"][0]
        self.assertTrue(edge["inferred"])

    def test_a_table_header_cannot_smuggle_an_unchecked_citation(self):
        problems = read("| @src/cache.rs:999 | x |\n|---|---|\n| a | b |")["problems"]
        self.assertIn("not in the pinned diff", problems[0])
        head = read("| @src/cache.rs:11 | x |\n|---|---|\n| a | b |")["blocks"][0][
            "head"
        ]
        self.assertEqual(head[0][0]["start"], 11)

    def test_a_two_way_arrow_is_not_a_node_name(self):
        problems = read("```flow\na <-> b @src/cache.rs:11\n```")["problems"]
        self.assertIn("one direction", problems[0])

    def test_a_list_after_a_paragraph_is_a_list(self):
        got = read("Notes:\n- one\n- two\n## Next\ntext")
        self.assertEqual([b["t"] for b in got["blocks"]], ["p", "ul", "h", "p"])


class WorkbenchTest(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        (root / "manifest.json").write_text(
            json.dumps(
                {
                    "head": "a" * 40,
                    "against": "b" * 40 + "...HEAD",
                    "pr": None,
                    "committed": True,
                    "files": [{"path": "src/cache.rs", "additions": 2, "deletions": 1}],
                }
            )
        )
        (root / "pr.diff").write_text(DIFF)
        questions = root / "questions.json"
        questions.write_text("[]")
        self.app = workbench.Workbench(root, questions)

    def send(self, answers):
        state = self.app.state()
        state["answers"] = answers
        self.app.write(state)

    def feedback(self, text):
        snapshot = self.app.snapshot()
        return {
            "identity": snapshot["identity"],
            "answer_digest": snapshot["answer_digest"],
            "notes": [{"field": "motivation", "text": text}],
        }

    def request(self):
        self.send({"motivation": "Reads the cache before the database on every get."})
        state = self.app.state()
        state["requested_digest"] = self.app.snapshot()["answer_digest"]
        state["request_id"] = "r1"
        self.app.write(state)

    def test_a_note_citing_a_missing_line_is_refused_before_it_is_stored(self):
        # What the agent CLI runs before it stores a note.
        with self.assertRaisesRegex(ValueError, "Reference note: .*not in the pinned"):
            self.app.check_note(self.feedback("see @src/cache.rs:99"))
        self.assertIsNone(self.app.snapshot()["feedback"])

    def test_the_snapshot_carries_what_the_page_draws(self):
        self.request()
        text = "Read first.\n\n```flow\nget -> cache @src/cache.rs:11\n```"
        self.app.feedback(self.feedback(text))
        view = self.app.page_snapshot()["reference_view"]
        self.assertEqual([b["t"] for b in view[0]], ["p", "flow"])

    RAGGED = "| a | b |\n|---|---|\n| only one |"

    def test_the_managed_worker_stores_the_free_text_it_wrote_and_it_is_shown_as_text(
        self,
    ):
        self.request()
        with self.assertRaisesRegex(ValueError, "Reference note"):
            self.app.check_note(self.feedback(self.RAGGED))  # an agent is told
        self.app.feedback(self.feedback(self.RAGGED))  # the worker is not asked
        snapshot = self.app.page_snapshot()
        self.assertEqual(snapshot["feedback"]["notes"][0]["text"], self.RAGGED)
        self.assertEqual(snapshot["reference_view"], [None])

    def test_a_drawn_note_completes_the_review_like_any_other(self):
        s = self.app.snapshot()
        self.app.save(
            {
                "identity": s["identity"],
                "revision": s["revision"],
                "answers": {
                    "motivation": "The lookup has to come first, and only a miss "
                    "may fall through to the database."
                },
            }
        )
        s = self.app.snapshot()
        self.app.request_review({"identity": s["identity"], "revision": s["revision"]})
        self.app.feedback(
            self.feedback("Read first.\n\n```flow\nget -> cache @src/cache.rs:11\n```")
        )
        s = self.app.snapshot()
        self.assertEqual(s["status"], "reflection_needed")
        self.app.reflect(
            {
                "identity": s["identity"],
                "revision": s["revision"],
                "feedback_digest": s["feedback_digest"],
                "grade": "got",
                "correction": "",
            }
        )
        self.assertEqual(self.app.snapshot()["status"], "explanation_complete")

    def test_handler_threads_reading_many_notes_at_once_do_not_corrupt_the_cache(self):
        import threading

        failures = []

        def work(n):
            try:
                for i in range(120):
                    self.app.read_note(f"note {n} {i} @src/cache.rs:11")
            except Exception as error:  # noqa: BLE001 - the point is that none escapes
                failures.append(error)

        threads = [threading.Thread(target=work, args=(n,)) for n in range(8)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        self.assertEqual(failures, [])
        self.assertLessEqual(len(self.app._notes), 32)

    def test_a_note_is_read_once_however_often_the_snapshot_is_taken(self):
        self.request()
        self.app.feedback(self.feedback("See @src/cache.rs:11."))
        calls = []
        used = workbench.load_explainer()  # the module read_note() itself calls
        real = used.Reader.cite

        def counting(reader, *args):
            calls.append(args)
            return real(reader, *args)

        used.Reader.cite = counting
        try:
            for _ in range(5):
                self.app.page_snapshot()
        finally:
            used.Reader.cite = real
        # One citation, read by the first snapshot (so the patch is live) and
        # reused by the other four.
        self.assertEqual(len(calls), 1)

    def test_a_stored_plain_note_is_prose_and_one_that_no_longer_reads_is_text(self):
        self.assertEqual(self.app.page_snapshot()["reference_view"], [])
        legacy = {"notes": [{"field": "motivation", "text": "see @src/cache.rs:99"}]}
        self.assertEqual(self.app.reference_view(legacy), [None])


if __name__ == "__main__":
    unittest.main()

"""The three-debt signals: what is observed, never a score, never written for anyone."""

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SKILL = Path(__file__).resolve().parents[1] / ".claude/skills/codetrial-review"


def load(name, file):
    spec = importlib.util.spec_from_file_location(name, SKILL / file)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


debt = load("intentdebt", "intentdebt.py")
diffmodel = load("diffmodel_for_debt", "diffmodel.py")
workbench = load("review_workbench_for_debt", "workbench.py")

DIFF = """diff --git a/src/cache.rs b/src/cache.rs
--- a/src/cache.rs
+++ b/src/cache.rs
@@ -1,7 +1,4 @@
 fn get(key: &str) -> Option<Value> {
-    if key.is_empty() {
-        return None;
-    }
-    // if this were code it would be a guard
+    // the check moved to the caller
     db.load(key)
 }
@@ -20,2 +17,2 @@ fn put
-    if full() {
+    if full() {
     store()
diff --git a/tests/cache_test.rs b/tests/cache_test.rs
--- a/tests/cache_test.rs
+++ b/tests/cache_test.rs
@@ -1,4 +1,2 @@
 #[test]
-    assert_eq!(get(""), None);
-    assert_eq!(get("a"), None);
 fn t() {}
"""


def found(diff=DIFF):
    return debt.signals(diffmodel.reading(diff)["files"])


class KindTest(unittest.TestCase):
    def test_paths_are_sorted_by_what_they_are(self):
        for path, expected in (
            ("src/cache.rs", "source"),
            ("src/design/button.rs", "source"),  # a folder name is not a document
            ("tests/cache_test.rs", "test"),
            ("web/app.test.js", "test"),
            ("pkg/test_thing.py", "test"),
            ("docs/large-change-explanation.md", "intent"),
            ("README.md", "intent"),
            ("adr/0007-queue.txt", "intent"),
            ("Cargo.lock", "other"),
            ("logo.svg", "other"),
        ):
            with self.subTest(path):
                self.assertEqual(debt.kind(path), expected)


class SignalsTest(unittest.TestCase):
    def test_removed_checks_are_found_at_their_old_lines_and_comments_are_not(self):
        got = found()["removed_checks"]
        self.assertEqual(
            [(r["path"], r["line"], r["text"]) for r in got],
            [
                ("src/cache.rs", 2, "if key.is_empty() {"),
                ("src/cache.rs", 3, "return None;"),
            ],
        )

    def test_a_check_that_comes_back_unchanged_is_a_move_not_a_loss(self):
        texts = [r["text"] for r in found()["removed_checks"]]
        self.assertNotIn("if full() {", texts)

    def test_a_moved_assertion_does_not_hide_a_lost_one(self):
        row = lambda k, t: {"k": k, "t": t, "o": 1, "n": None}  # noqa: E731
        got = debt.signals(
            [
                {
                    "path": "tests/a.rs",
                    "hunks": [
                        {
                            "lines": [
                                row("d", "assert!(a);"),
                                row("d", "assert!(b);"),
                                row("a", "assert!(a);"),
                            ]
                        }
                    ],
                }
            ]
        )
        # "a" moved; "b" is gone and nothing took its place.
        self.assertEqual(
            [r["text"] for r in got["removed_assertions"]], ["assert!(b);"]
        )
        self.assertEqual(got["added_assertions"], 0)
        asks = debt.lens(got)["intent"]["signals"]
        self.assertTrue(any("removed from tests" in s["text"] for s in asks))

    def test_one_removed_line_says_looks(self):
        row = {"k": "d", "t": "if x {", "o": 3, "n": None}
        got = debt.signals([{"path": "src/a.rs", "hunks": [{"lines": [row]}]}])
        text = debt.lens(got)["code"]["signals"][1]["text"]
        self.assertTrue(text.startswith("1 removed line looks like"))

    def test_tests_and_documents_are_kept_apart_from_source(self):
        got = found()
        self.assertEqual(got["source"], ["src/cache.rs"])
        self.assertEqual(got["test"], ["tests/cache_test.rs"])
        self.assertEqual(got["intent"], [])
        self.assertEqual(len(got["removed_assertions"]), 2)
        self.assertEqual(got["added_assertions"], 0)

    def test_a_binary_file_is_not_read(self):
        files = [{"path": "src/a.rs", "binary": True, "hunks": []}]
        self.assertEqual(debt.signals(files)["source"], [])


class LensTest(unittest.TestCase):
    def texts(self, layer, **kw):
        return " ".join(s["text"] for s in debt.lens(found(), **kw)[layer]["signals"])

    def test_it_asks_one_question_for_each_layer_and_scores_nothing(self):
        got = debt.lens(found())
        self.assertEqual(set(got), {"code", "people", "intent"})
        for layer in got.values():
            self.assertTrue(layer["asks"].endswith("?"))
        dumped = json.dumps(got).lower()
        for word in ("score", "rating", "grade", "debt level"):
            self.assertNotIn(word, dumped)

    def test_removed_checks_come_with_citations_into_the_old_file(self):
        signal = debt.lens(found())["code"]["signals"][1]
        self.assertIn("2 removed lines", signal["text"])
        self.assertEqual(
            signal["cites"][0],
            {"path": "src/cache.rs", "side": "old", "start": 2, "end": 2},
        )

    def test_intent_notes_when_nothing_pins_or_records_the_change(self):
        self.assertIn("Where will the why live", self.texts("intent"))
        self.assertIn("2 assertion-like lines removed", self.texts("intent"))
        no_tests = debt.signals(
            diffmodel.reading(DIFF.split("diff --git a/tests")[0])["files"]
        )
        text = " ".join(s["text"] for s in debt.lens(no_tests)["intent"]["signals"])
        # What was observed, said as such: a path heuristic found no changed test.
        self.assertIn("No changed test file was recognized", text)
        self.assertIn("existing or new", text)
        self.assertNotIn("Nothing here pins", text)

    def test_a_change_with_a_test_and_a_document_has_nothing_to_flag(self):
        files = [
            {"path": "src/a.rs", "hunks": [{"lines": []}]},
            {"path": "tests/a_test.rs", "hunks": [{"lines": []}]},
            {"path": "docs/a.md", "hunks": [{"lines": []}]},
        ]
        got = debt.lens(debt.signals(files))["intent"]["signals"]
        self.assertEqual(
            [s["text"] for s in got], ["A test and a document changed with the code."]
        )

    def test_a_change_with_no_source_does_not_claim_a_test_or_a_document_changed(self):
        for files in ([], [{"path": "logo.svg", "hunks": [{"lines": []}]}]):
            got = debt.lens(debt.signals(files))["intent"]["signals"]
            self.assertEqual(len(got), 1)
            self.assertIn("No source file changed", got[0]["text"])
            self.assertNotIn("test and a document", got[0]["text"])

    def test_people_signals_use_only_what_the_contributor_did(self):
        self.assertIn(
            "does not yet name code", self.texts("people", checks={"grounded": False})
        )
        self.assertIn(
            "names code from the change",
            self.texts("people", checks={"grounded": True}),
        )
        self.assertIn("records what you missed", self.texts("people"))
        said = self.texts(
            "people", reflection={"grade": "partly", "correction": "SECRETWORD"}
        )
        self.assertIn("covered part of the reference", said)
        self.assertIn("A gap in understanding is recorded, in your words.", said)
        # The correction is the contributor's; nothing here quotes it back.
        whole = json.dumps(
            debt.lens(
                found(), reflection={"grade": "partly", "correction": "SECRETWORD"}
            )
        )
        self.assertNotIn("SECRETWORD", whole)


class GuardTest(unittest.TestCase):
    def removed(self, *texts, path="src/a.rs"):
        rows = [{"k": "d", "t": t, "o": i + 1, "n": None} for i, t in enumerate(texts)]
        return debt.signals([{"path": path, "hunks": [{"lines": rows}]}])[
            "removed_checks"
        ]

    def test_every_kind_of_check_the_pattern_names_is_found(self):
        for text in (
            "if x {", "} else if y {", "elif z:", "unless done", "guard let x = y else {",
            "assert_eq!(a, b);", "debug_assert!(ok);", "ensure(ok)", "require(ok)",
            "raise ValueError(x)", "throw new Error(x)", "panic!(\"no\");", "unreachable!()",
            "except KeyError:", "catch (e) {", "return Err(e);", "return None", "return false;",
            "return null;", "return nil", "return undefined;",
        ):  # fmt: skip
            with self.subTest(text):
                self.assertEqual(len(self.removed(text)), 1)

    def test_lines_that_only_mention_a_check_are_not_one(self):
        for text in (
            "// if x {", "# raise ValueError", "let iffy = 1;", "notify(x);", "returns = 3",
            "let s = \"if x\";", "assertion_count += 1", "required_items = []", "ifconfig()",
            "catches = 3",
        ):  # fmt: skip
            with self.subTest(text):
                self.assertEqual(self.removed(text), [])

    def test_an_attribute_is_not_an_assertion(self):
        row = lambda t: {"k": "d", "t": t, "o": 1, "n": None}  # noqa: E731
        got = debt.signals(
            [{"path": "tests/a.rs", "hunks": [{"lines": [row("#[must_use]")]}]}]
        )
        self.assertEqual(got["removed_assertions"], [])
        got = debt.signals(
            [{"path": "tests/a.rs", "hunks": [{"lines": [row("assert!(ok);")]}]}]
        )
        self.assertEqual(len(got["removed_assertions"]), 1)
        got = debt.signals(
            [
                {
                    "path": "tests/test_a.py",
                    "hunks": [{"lines": [row("self.assertEqual(a, b)")]}],
                }
            ]
        )
        self.assertEqual(len(got["removed_assertions"]), 1)
        got = debt.signals(
            [
                {
                    "path": "tests/a.rs",
                    "hunks": [{"lines": [row("assertion_count += 1")]}],
                }
            ]
        )
        self.assertEqual(got["removed_assertions"], [])


class WorkbenchTest(unittest.TestCase):
    def make(self, diff=DIFF):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        files = diffmodel.reading(diff)["files"]
        (root / "manifest.json").write_text(
            json.dumps(
                {
                    "head": "a" * 40,
                    "against": "b" * 40 + "...HEAD",
                    "pr": None,
                    "committed": True,
                    "files": [
                        {
                            "path": f["path"],
                            "additions": f["additions"],
                            "deletions": f["deletions"],
                        }
                        for f in files
                    ],
                }
            )
        )
        (root / "pr.diff").write_text(diff)
        (root / "questions.json").write_text("[]")
        return workbench.Workbench(root, root / "questions.json"), root

    def test_the_snapshot_reads_the_change_as_three_debts(self):
        app, _ = self.make()
        got = app.page_snapshot()["debt"]
        self.assertEqual(set(got), {"code", "people", "intent"})
        cites = got["code"]["signals"][1]["cites"]
        self.assertEqual(cites[0]["side"], "old")
        self.assertEqual(cites[0]["path"], "src/cache.rs")

    def test_a_diff_the_page_cannot_show_line_by_line_points_at_no_line(self):
        app, root = self.make()
        app.manifest["files"][0]["additions"] += 5  # the manifest no longer matches
        app._reading = None
        got = app.page_snapshot()["debt"]
        self.assertIn(
            "could not be read line by line", got["code"]["signals"][0]["text"]
        )
        for layer in got.values():
            for signal in layer["signals"]:
                self.assertNotIn("cites", signal)

    def test_the_intent_record_is_kept_as_written_and_changes_nothing_else(self):
        app, root = self.make()
        before = app.page_snapshot()
        self.assertEqual(
            before["intent"], {**{k: "" for k in workbench.INTENT_FIELDS}, "home": ""}
        )
        record = {
            "goal": "A miss must reach the database.",
            "check": "tests/cache_test.rs",
            "home": "pr",
        }
        self.assertEqual(app.save_intent(record)["intent"]["goal"], record["goal"])
        after = app.page_snapshot()
        self.assertEqual(after["intent"]["goal"], record["goal"])
        self.assertEqual(after["intent"]["constraints"], "")
        # Not part of the answers, so the gate's inputs are untouched.
        for key in ("answer_digest", "identity", "revision", "answers", "status"):
            self.assertEqual(after[key], before[key], key)
        self.assertTrue((root / f"intent-record-{app.identity[:16]}.json").exists())

    def test_the_intent_record_is_refused_when_it_is_not_what_it_claims(self):
        app, _ = self.make()
        for payload, why in (
            ({"goal": 5}, "text"),
            ({"goal": "x" * 2001}, "at most 2000"),
            ({"verdict": "approved"}, "has goal"),
            ({"home": "elsewhere"}, "from the list"),
            ("goal", "has goal"),
        ):
            with self.subTest(str(payload)[:20]):
                with self.assertRaisesRegex(ValueError, why):
                    app.save_intent(payload)

    def test_a_new_snapshot_does_not_inherit_the_old_record(self):
        app, root = self.make()
        app.save_intent({"goal": "Old reasons."})
        (root / "pr.diff").write_text(DIFF + " \n")  # the code changed
        newer = workbench.Workbench(root, root / "questions.json")
        self.assertNotEqual(newer.identity, app.identity)
        self.assertEqual(newer.intent()["goal"], "")

    def test_a_window_left_open_on_an_older_snapshot_cannot_replace_the_newer_record(
        self,
    ):
        older, root = self.make()
        (root / "pr.diff").write_text(DIFF + " \n")
        newer = workbench.Workbench(root, root / "questions.json")
        newer.save_intent({"goal": "The newer reasons."})
        older.save_intent({"goal": "Written late, in a stale window."})
        self.assertEqual(newer.intent()["goal"], "The newer reasons.")
        self.assertEqual(older.intent()["goal"], "Written late, in a stale window.")

    def test_the_record_is_not_handed_to_the_reviewing_agent(self):
        app, root = self.make()
        app.save_intent({"goal": "Only for the contributor."})
        self.assertIn("intent", app.page_snapshot())  # the page needs it
        # The agent's snapshot is built without it, not filtered afterwards.
        for key in ("intent", "debt", "reference_view"):
            self.assertNotIn(key, app.snapshot())
        out = subprocess.run(
            [sys.executable, str(SKILL / "workbench.py"), "inspect", "--bundle", str(root),
             "--questions", str(root / "questions.json")],
            capture_output=True, text=True, check=True,
        ).stdout  # fmt: skip
        self.assertNotIn("Only for the contributor.", out)
        for key in ("intent", "debt", "reference_view"):
            self.assertNotIn(key, json.loads(out))


if __name__ == "__main__":
    unittest.main()

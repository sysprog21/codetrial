"""The diff model reads a git diff without dropping a line, and labels honestly."""

import importlib.util
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

MODULE = (
    Path(__file__).resolve().parents[1] / ".claude/skills/codetrial-review/diffmodel.py"
)
SPEC = importlib.util.spec_from_file_location("diffmodel", MODULE)
dm = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(dm)


def hunk(old_start, new_start, old_count, new_count, lines, section=""):
    head = f"@@ -{old_start},{old_count} +{new_start},{new_count} @@"
    return head + (" " + section if section else "") + "\n" + "\n".join(lines) + "\n"


def file_diff(path, body, old=None, extra=""):
    old = old or path
    return f"diff --git a/{old} b/{path}\n{extra}--- a/{old}\n+++ b/{path}\n{body}"


class ParseTests(unittest.TestCase):
    def test_lines_carry_old_and_new_numbers(self):
        text = file_diff(
            "a.py",
            hunk(10, 10, 4, 4, [" keep", "-old", "+new", " tail", " end"], "def f():"),
        )
        # One context line fewer than declared: the model must say so.
        files, problems = dm.parse(text)
        self.assertEqual(files[0]["path"], "a.py")
        lines = files[0]["hunks"][0]["lines"]
        self.assertEqual(
            [(r["k"], r["o"], r["n"]) for r in lines][:3],
            [
                ("c", 10, 10),
                ("d", 11, None),
                ("a", None, 11),
            ],
        )
        self.assertEqual(files[0]["hunks"][0]["section"], "def f():")
        self.assertEqual(problems, [])

    def test_added_deleted_renamed_and_binary_files(self):
        text = (
            "diff --git a/new.txt b/new.txt\nnew file mode 100644\n--- /dev/null\n"
            "+++ b/new.txt\n@@ -0,0 +1,2 @@\n+one\n+two\n"
            "diff --git a/gone.txt b/gone.txt\ndeleted file mode 100644\n"
            "--- a/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\n"
            "diff --git a/old name.txt b/new name.txt\nsimilarity index 80%\n"
            "rename from old name.txt\nrename to new name.txt\n"
            "--- a/old name.txt\n+++ b/new name.txt\n@@ -1 +1 @@\n-a\n+b\n"
            "diff --git a/img.png b/img.png\nBinary files a/img.png and b/img.png differ\n"
        )
        files, problems = dm.parse(text)
        self.assertEqual(problems, [])
        self.assertEqual(
            [(f["path"], f["old_path"], f["status"], f["binary"]) for f in files],
            [
                ("new.txt", "new.txt", "added", False),
                ("gone.txt", "gone.txt", "deleted", False),
                ("new name.txt", "old name.txt", "renamed", False),
                ("img.png", "img.png", "modified", True),
            ],
        )
        self.assertEqual((files[0]["additions"], files[1]["deletions"]), (2, 1))

    def test_quoted_unicode_paths_and_missing_final_newline(self):
        text = (
            'diff --git "a/caf\\303\\251.txt" "b/caf\\303\\251.txt"\n'
            '--- "a/caf\\303\\251.txt"\n+++ "b/caf\\303\\251.txt"\n'
            "@@ -1 +1 @@\n-x\n\\ No newline at end of file\n+y\n"
        )
        files, problems = dm.parse(text)
        self.assertEqual(problems, [])
        self.assertEqual(files[0]["path"], "café.txt")
        self.assertIs(files[0]["hunks"][0]["lines"][0].get("eol"), False)

    def test_a_blank_context_line_that_lost_its_space_still_counts(self):
        text = file_diff("a.txt", "@@ -1,3 +1,3 @@\n a\n\n-b\n+c\n")
        files, problems = dm.parse(text)
        self.assertEqual(problems, [])
        self.assertEqual(len(files[0]["hunks"][0]["lines"]), 4)

    def test_a_hunk_that_ends_early_is_reported_not_dropped(self):
        text = file_diff("a.txt", "@@ -1,4 +1,4 @@\n a\n-b\n+c\n")
        files, problems = dm.parse(text)
        self.assertEqual(len(files[0]["hunks"][0]["lines"]), 3)
        self.assertTrue(problems or dm.check(files, problems))


class AnnotateTests(unittest.TestCase):
    def reading(self, text, manifest=None):
        return dm.reading(text, manifest)

    def test_the_words_that_changed_are_marked_on_both_lines(self):
        body = hunk(1, 1, 1, 1, ["-total = max(a, b)", "+total = max(a, b, c)"])
        item = self.reading(file_diff("a.py", body))["files"][0]
        old, new = item["hunks"][0]["lines"]
        self.assertEqual(new["t"][new["w"][0][0] : new["w"][0][1]], ", c")
        self.assertEqual(old.get("w", []), [])
        self.assertEqual((old["p"], new["p"]), (1, 0))

    def test_unrelated_lines_are_not_paired_and_a_rewrite_is_marked_whole(self):
        body = hunk(1, 1, 1, 1, ["-alpha beta gamma", "+zzz yyy xxx"])
        old, new = self.reading(file_diff("a.py", body))["files"][0]["hunks"][0][
            "lines"
        ]
        self.assertNotIn("p", old)
        body = hunk(1, 1, 1, 1, ["-return value", "+return other"])
        old, new = self.reading(file_diff("a.py", body))["files"][0]["hunks"][0][
            "lines"
        ]
        self.assertIn("w", new)

    def test_whitespace_only_edits_are_labelled(self):
        body = hunk(1, 1, 2, 2, ["-    x = 1", "+        x = 1", "-y  =  2", "+y = 2"])
        item = self.reading(file_diff("a.py", body))["files"][0]
        self.assertTrue(all(r.get("ws") for r in item["hunks"][0]["lines"]))
        self.assertEqual(item["label"], "whitespace only")

    def moved_pair(self, source_lines, target_lines, same_file=False):
        removed = hunk(1, 1, len(source_lines) + 1, 1, [" head", *source_lines])
        added = hunk(1, 1, 1, len(target_lines) + 1, [" head", *target_lines])
        if same_file:
            return file_diff("a.py", removed + added.replace("@@ -1,1", "@@ -9,1"))
        return file_diff("a.py", removed) + file_diff("b.py", added)

    FUNCTION = [
        "def compute(total, counted):",
        "    result = max(total, counted)",
        "    return result + len(str(total))",
        "    # keep the larger reading",
    ]

    def test_a_run_removed_in_one_file_and_added_in_another_is_a_move(self):
        text = self.moved_pair(
            ["-" + x for x in self.FUNCTION], ["+" + x for x in self.FUNCTION]
        )
        files = self.reading(text)["files"]
        gone = files[0]["hunks"][0]["lines"]
        came = files[1]["hunks"][0]["lines"]
        self.assertTrue(all(r.get("m") for r in gone[1:]))
        self.assertTrue(all(r.get("m") for r in came[1:]))
        self.assertEqual(came[1]["mr"], {"path": "a.py", "line": 2, "len": 4})
        self.assertEqual(gone[1]["mr"], {"path": "b.py", "line": 2, "len": 4})
        self.assertEqual(files[0]["label"], "moved only")

    def test_a_move_ignores_indentation_and_keeps_a_changed_word_visible(self):
        indented = ["+    " + x for x in self.FUNCTION]
        text = self.moved_pair(["-" + x for x in self.FUNCTION], indented)
        self.assertTrue(self.reading(text)["files"][1]["hunks"][0]["lines"][1]["m"])
        changed = ["+" + x for x in self.FUNCTION]
        changed[1] = "+    result = min(total, counted)"
        text = self.moved_pair(["-" + x for x in self.FUNCTION], changed)
        lines = self.reading(text)["files"][1]["hunks"][0]["lines"]
        # The unit moved, and the line that differs is still an edit.
        self.assertTrue(lines[1]["m"] and lines[3]["m"] and lines[4]["m"])
        self.assertNotIn("m", lines[2])
        word = lines[2]["w"][0]
        self.assertEqual(lines[2]["t"][word[0] : word[1]], "min")
        note = lines[1]["mr"]
        self.assertEqual((note["how"], note["changed"]), ("renamed", 1))
        self.assertEqual(note["renames"], [["max", "min"]])

    def test_short_runs_and_boilerplate_are_not_moves(self):
        text = self.moved_pair(
            ["-    x = 1", "-    y = 2"], ["+    x = 1", "+    y = 2"]
        )
        self.assertFalse(
            any(r.get("m") for r in self.reading(text)["files"][1]["hunks"][0]["lines"])
        )
        braces = ["-}", "-}", "-}", "-}"]
        text = self.moved_pair(braces, [b.replace("-", "+") for b in braces])
        self.assertFalse(
            any(r.get("m") for r in self.reading(text)["files"][1]["hunks"][0]["lines"])
        )

    def test_text_removed_in_two_places_has_no_single_source(self):
        gone = ["-" + x for x in self.FUNCTION]
        text = (
            file_diff("a.py", hunk(1, 1, 5, 1, [" head", *gone]))
            + file_diff("c.py", hunk(1, 1, 5, 1, [" head", *gone]))
            + file_diff(
                "b.py", hunk(1, 1, 1, 5, [" head", *["+" + x for x in self.FUNCTION]])
            )
        )
        files = self.reading(text)["files"]
        self.assertFalse(
            any(r.get("m") for f in files for h in f["hunks"] for r in h["lines"])
        )

    def test_the_same_text_replaced_in_one_change_block_is_an_edit(self):
        lines = ["-" + x for x in self.FUNCTION] + ["+" + x for x in self.FUNCTION]
        text = file_diff("a.py", hunk(1, 1, 4, 4, lines))
        files = self.reading(text)["files"]
        self.assertFalse(any(r.get("m") for r in files[0]["hunks"][0]["lines"]))

    def test_the_result_is_the_same_every_time(self):
        text = self.moved_pair(
            ["-" + x for x in self.FUNCTION], ["+" + x for x in self.FUNCTION]
        )
        self.assertEqual(
            json.dumps(self.reading(text), sort_keys=True),
            json.dumps(self.reading(text), sort_keys=True),
        )


class CheckTests(unittest.TestCase):
    def test_a_manifest_that_disagrees_is_reported(self):
        text = file_diff("a.py", hunk(1, 1, 1, 2, [" a", "+b"]))
        good = [{"path": "a.py", "additions": 1, "deletions": 0}]
        self.assertEqual(dm.reading(text, good)["problems"], [])
        bad = [{"path": "a.py", "additions": 5, "deletions": 0}]
        self.assertIn("manifest counts 5", dm.reading(text, bad)["problems"][0])
        missing = good + [{"path": "gone.py", "additions": 1, "deletions": 0}]
        self.assertIn("absent from the diff", dm.reading(text, missing)["problems"][0])
        # A partial manifest does not make a file absent.
        self.assertEqual(dm.reading(text, missing, complete=False)["problems"], [])

    def test_a_hunk_header_that_lies_is_reported(self):
        text = file_diff("a.py", hunk(1, 1, 3, 3, [" a", "-b", "+c", " d", " e"]))
        files, problems = dm.parse(text)
        # Two lines of context were written beyond what the header counts.
        self.assertTrue(problems or dm.check(files, problems))


class UnitTests(unittest.TestCase):
    """Matching whole functions, past a change of names or a small edit."""

    OLD = [
        "def compute(total, counted):",
        "    result = max(total, counted)",
        "    if result > limit(total):",
        "        result = limit(total)",
        "    return result + len(str(total))",
    ]

    def test_units_split_at_the_shallowest_definitions(self):
        rows = [
            {"t": t}
            for t in [
                "import os",
                "@cached",
                "@wrapped",
                "def one():",
                "    return 1",
                "",
                "def two():",
                "    def inner():",
                "        pass",
                "    return inner",
                "class Three:",
                "    x = 1",
            ]
        ]
        self.assertEqual(
            dm.segment(rows),
            [[0], [1, 2, 3, 4, 5], [6, 7, 8, 9], [10, 11]],
        )
        rust = [
            {"t": t}
            for t in ["    fn a() {", "        1", "    }", "    fn b() {", "    }"]
        ]
        self.assertEqual(dm.segment(rust), [[0, 1, 2], [3, 4]])

    def test_a_function_whose_names_all_changed_is_still_the_same_function(self):
        came = [
            "def combine(first, second):",
            "    outcome = max(first, second)",
            "    if outcome > limit(first):",
            "        outcome = limit(first)",
            "    return outcome + len(str(first))",
        ]
        text = file_diff(
            "a.py", hunk(1, 1, 6, 1, [" head", *["-" + x for x in self.OLD]])
        ) + file_diff("b.py", hunk(1, 1, 1, 6, [" head", *["+" + x for x in came]]))
        files = dm.reading(text)["files"]
        added = files[1]["hunks"][0]["lines"]
        note = added[1]["mr"]
        self.assertEqual(note["how"], "renamed")
        self.assertEqual(note["len"], 5)
        self.assertIn(["compute", "combine"], note["renames"])
        # Every line differs in a name, so each is an edit with its words marked.
        self.assertFalse(any(r.get("m") for r in added[1:]))
        self.assertTrue(all(r.get("w") for r in added[1:]))
        self.assertEqual(len(added), 6)

    def test_a_moved_function_with_a_small_edit_keeps_its_unchanged_lines_neutral(self):
        edited = list(self.OLD)
        edited[2] = "    if result > limit(total, strict=True):"
        edited[3] = "        result = limit(total, strict=True)"
        text = file_diff(
            "a.py", hunk(1, 1, 6, 1, [" head", *["-" + x for x in self.OLD]])
        ) + file_diff("b.py", hunk(1, 1, 1, 6, [" head", *["+" + x for x in edited]]))
        files = dm.reading(text)["files"]
        added = files[1]["hunks"][0]["lines"]
        kept = [bool(r.get("m")) for r in added[1:]]
        self.assertEqual(kept, [True, True, False, False, True])
        note = added[1]["mr"]
        self.assertEqual((note["how"], note["changed"]), ("edited", 2))
        for r in added[3:5]:
            self.assertIn("w", r)
        self.assertEqual(files[1]["label"], "")

    def test_two_equally_likely_sources_leave_the_unit_an_ordinary_edit(self):
        a_copy = ["    " + x if i else x for i, x in enumerate(self.OLD)]
        text = (
            file_diff("a.py", hunk(1, 1, 6, 1, [" h", *["-" + x for x in self.OLD]]))
            + file_diff("c.py", hunk(1, 1, 6, 1, [" h", *["-" + x for x in a_copy]]))
            + file_diff(
                "b.py",
                hunk(
                    1,
                    1,
                    1,
                    6,
                    [" h", *["+" + x.replace("total", "amount") for x in self.OLD]],
                ),
            )
        )
        lines = dm.reading(text)["files"][2]["hunks"][0]["lines"]
        self.assertFalse(any("mr" in r for r in lines))

    def test_matching_units_never_changes_a_line_or_a_count(self):
        came = [x.replace("total", "amount") for x in self.OLD]
        text = file_diff(
            "a.py", hunk(1, 1, 6, 1, [" head", *["-" + x for x in self.OLD]])
        ) + file_diff("b.py", hunk(1, 1, 1, 6, [" head", *["+" + x for x in came]]))
        before, _ = dm.parse(text)
        after = dm.reading(text)["files"]
        for old, new in zip(before, after):
            self.assertEqual(
                [(r["k"], r["t"], r["o"], r["n"]) for r in old["hunks"][0]["lines"]],
                [(r["k"], r["t"], r["o"], r["n"]) for r in new["hunks"][0]["lines"]],
            )
            self.assertEqual(
                (old["additions"], old["deletions"]),
                (new["additions"], new["deletions"]),
            )
        self.assertEqual(dm.reading(text)["problems"], [])

    def test_the_same_text_replaced_in_one_block_is_not_a_unit_move(self):
        text = file_diff(
            "a.py",
            hunk(
                1,
                1,
                5,
                5,
                [
                    *["-" + x for x in self.OLD],
                    *["+" + x.replace("total", "amount") for x in self.OLD],
                ],
            ),
        )
        lines = dm.reading(text)["files"][0]["hunks"][0]["lines"]
        self.assertFalse(any("mr" in r for r in lines))

    def test_a_function_that_reappears_rewritten_is_not_matched(self):
        other = [
            "def render(page, theme):",
            "    body = theme.wrap(page.title)",
            "    for block in page.blocks:",
            "        body += theme.draw(block)",
            "    return body",
        ]
        text = file_diff(
            "a.py", hunk(1, 1, 6, 1, [" head", *["-" + x for x in self.OLD]])
        ) + file_diff("b.py", hunk(1, 1, 1, 6, [" head", *["+" + x for x in other]]))
        lines = dm.reading(text)["files"][1]["hunks"][0]["lines"]
        self.assertFalse(any("mr" in r for r in lines))

    def test_unit_matching_is_deterministic(self):
        came = [x.replace("total", "amount") for x in self.OLD]
        text = file_diff(
            "a.py", hunk(1, 1, 6, 1, [" head", *["-" + x for x in self.OLD]])
        ) + file_diff("b.py", hunk(1, 1, 1, 6, [" head", *["+" + x for x in came]]))
        self.assertEqual(
            json.dumps(dm.reading(text), sort_keys=True),
            json.dumps(dm.reading(text), sort_keys=True),
        )


class CompactTests(unittest.TestCase):
    def test_moves_and_whitespace_are_folded_and_the_rest_is_kept(self):
        func = AnnotateTests.FUNCTION
        text = file_diff(
            "a.py",
            hunk(
                1,
                1,
                7,
                3,
                [" head", *["-" + x for x in func], "-    x  = 1", "+    x = 1"],
            ),
        ) + file_diff("b.py", hunk(1, 1, 1, 5, [" head", *["+" + x for x in func]]))
        files = dm.reading(text)["files"]
        gone, came = dm.compact(files[0]), dm.compact(files[1])
        self.assertIn("4 lines moved to b.py:2 (unchanged text not repeated)", gone)
        self.assertIn(
            "4 lines moved here from a.py:2 (unchanged text not repeated)", came
        )
        self.assertNotIn("def compute", gone + came)
        self.assertIn("~    x = 1  # whitespace-only change", gone)
        self.assertIn(" head", gone)


class RealGitTests(unittest.TestCase):
    """The model against what git actually writes."""

    def git(self, *args, data=None):
        return subprocess.run(
            ["git", *args],
            cwd=self.repo,
            env=self.env,
            input=data,
            text=True,
            capture_output=True,
            check=True,
        ).stdout

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        self.env = {
            **os.environ,
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_AUTHOR_NAME": "t",
            "GIT_AUTHOR_EMAIL": "t@example.test",
            "GIT_COMMITTER_NAME": "t",
            "GIT_COMMITTER_EMAIL": "t@example.test",
        }

    def test_every_file_kind_counts_the_same_as_numstat(self):
        self.git("init", "-q", "-b", "main")
        files = {
            "a.py": "def f():\n    return 1\n\n\ndef g():\n    return 2\n",
            "gone.txt": "bye\nbye\n",
            "old.txt": "".join(f"line {n}\n" for n in range(40)),
            "café.txt": "un\ndeux\n",
            "noeol.txt": "no newline",
            "spaced name.txt": "x\ny\n",
        }
        for name, body in files.items():
            (self.repo / name).write_text(body)
        (self.repo / "bin.dat").write_bytes(b"\x00\x01\x02\x03")
        self.git("add", ".")
        self.git("commit", "-qm", "base")
        (self.repo / "a.py").write_text(
            "def f():\n    return 11\n\n\ndef g():\n    return 2\n\n\ndef h():\n    pass\n"
        )
        (self.repo / "gone.txt").unlink()
        (self.repo / "old.txt").rename(self.repo / "renamed.txt")
        (self.repo / "renamed.txt").write_text(
            "".join(f"line {n}\n" for n in range(40) if n != 7) + "extra\n"
        )
        (self.repo / "café.txt").write_text("un\ntrois\n")
        (self.repo / "noeol.txt").write_text("no newline changed")
        (self.repo / "spaced name.txt").write_text("x\ny\nz\n")
        (self.repo / "bin.dat").write_bytes(b"\x00\x01\x02\x04\x05")
        (self.repo / "fresh.md").write_text("# new\n\ntext\n")
        self.git("add", "-A")
        diff = self.git("diff", "--cached", "--find-renames=50%", "--unified=3")
        numstat = {}
        for row in self.git(
            "diff", "--cached", "--find-renames=50%", "--numstat"
        ).splitlines():
            added, deleted, path = row.split("\t", 2)
            if "=>" in path:
                path = path.split(" => ")[-1].rstrip("}")
            numstat[path] = (added, deleted)
        result = dm.reading(diff)
        self.assertEqual(result["problems"], [])
        by_path = {f["path"]: f for f in result["files"]}
        self.assertEqual(
            sorted(by_path),
            [
                "a.py",
                "bin.dat",
                "café.txt",
                "fresh.md",
                "gone.txt",
                "noeol.txt",
                "renamed.txt",
                "spaced name.txt",
            ],
        )
        for path, (added, deleted) in numstat.items():
            name = path.split("{")[-1].lstrip(" ") if "{" in path else path
            if name in by_path and added != "-":
                item = by_path[name]
                self.assertEqual(
                    (str(item["additions"]), str(item["deletions"])),
                    (added, deleted),
                    name,
                )
        self.assertTrue(by_path["bin.dat"]["binary"])
        self.assertEqual(by_path["renamed.txt"]["status"], "renamed")
        self.assertEqual(by_path["renamed.txt"]["old_path"], "old.txt")
        # Rebuilding each hunk from the model gives back the diff's own lines.
        rebuilt = []
        for item in result["files"]:
            for part in item["hunks"]:
                for row in part["lines"]:
                    rebuilt.append({"c": " ", "a": "+", "d": "-"}[row["k"]] + row["t"])
        original = [
            line
            for line in diff.split("\n")
            if line[:1] in (" ", "+", "-") and not line.startswith(("--- ", "+++ "))
        ]
        self.assertEqual(rebuilt, original)

    def test_a_function_moved_to_another_file_and_renamed_is_recognised(self):
        self.git("init", "-q", "-b", "main")
        body = (
            "def compute(total, counted):\n"
            "    result = max(total, counted)\n"
            "    if result > limit(total):\n"
            "        result = limit(total)\n"
            "    return result + len(str(total))\n"
        )
        (self.repo / "a.py").write_text("import os\n\n\n" + body + "\n\nVALUE = 1\n")
        (self.repo / "b.py").write_text("import sys\n")
        self.git("add", ".")
        self.git("commit", "-qm", "base")
        (self.repo / "a.py").write_text("import os\n\n\nVALUE = 1\n")
        (self.repo / "b.py").write_text(
            "import sys\n\n\n"
            + body.replace("compute", "combine").replace("total", "amount")
        )
        self.git("add", "-A")
        diff = self.git("diff", "--cached", "--unified=3")
        result = dm.reading(diff)
        self.assertEqual(result["problems"], [])
        notes = [
            r["mr"]
            for f in result["files"]
            for h in f["hunks"]
            for r in h["lines"]
            if r.get("mr")
        ]
        self.assertEqual(len(notes), 2)
        self.assertTrue(all(n["how"] == "renamed" and n["len"] == 5 for n in notes))
        renamed = {tuple(pair) for n in notes for pair in n["renames"]}
        self.assertIn(("compute", "combine"), renamed)

    def test_a_renamed_and_edited_file_does_not_make_the_reading_look_wrong(self):
        self.git("init", "-q", "-b", "main")
        (self.repo / "old.txt").write_text("".join(f"line {n}\n" for n in range(30)))
        self.git("add", ".")
        self.git("commit", "-qm", "base")
        (self.repo / "old.txt").rename(self.repo / "new.txt")
        (self.repo / "new.txt").write_text(
            "".join(f"line {n}\n" for n in range(30) if n != 5) + "extra\n"
        )
        self.git("add", "-A")
        diff = self.git("diff", "--cached", "--find-renames=50%")
        # What a manifest holds when it is read without rename detection.
        manifest = []
        for row in self.git(
            "diff", "--cached", "--no-renames", "--numstat"
        ).splitlines():
            added, deleted, path = row.split("\t", 2)
            manifest.append(
                {"path": path, "additions": int(added), "deletions": int(deleted)}
            )
        self.assertEqual(sorted(m["path"] for m in manifest), ["new.txt", "old.txt"])
        result = dm.reading(diff, manifest)
        self.assertEqual(result["problems"], [])
        self.assertEqual(result["files"][0]["status"], "renamed")


if __name__ == "__main__":
    unittest.main()

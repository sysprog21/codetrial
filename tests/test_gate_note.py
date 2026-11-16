"""Check scripts/gate-note.py, which builds the note a push carries as evidence."""

import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/gate-note.py"
SPEC = importlib.util.spec_from_file_location("gate_note", SCRIPT)
gate_note = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate_note)
TREE = "a" * 40
OTHER = "b" * 40


def receipt(**fields):
    body = {
        "lane": "rust",
        "passed": True,
        "failed": "",
        "skipped": 0,
        "tree": "fingerprint",
        "worktree_tree": TREE,
        "rustc": "rustc 1.100.0 (abcdef123 2026-10-01)",
        "cargo": "cargo 1.100.0 (abcdef123 2026-09-01)",
        "env": {"profile": "local-rust", "ok": True},
        "time": "2026-11-16T12:34:56Z",
    }
    body.update(fields)
    return body


class NoteTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.dir = Path(tmp.name)

    def put(self, name, body, tree=TREE):
        folder = self.dir / tree
        folder.mkdir(exist_ok=True)
        text = body if isinstance(body, str) else json.dumps(body)
        (folder / name).write_text(text)

    def build(self, tree=TREE):
        return subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "build",
                "--receipts",
                str(self.dir),
                "--tree",
                tree,
            ],
            capture_output=True,
            text=True,
            check=False,
        )

    def note(self, tree=TREE):
        done = self.build(tree)
        self.assertEqual(done.returncode, 0, done.stderr)
        return json.loads(done.stdout)

    def test_builds_the_schema(self):
        self.put("rust.json", receipt())
        note = self.note()
        self.assertEqual(
            note,
            {
                "version": 1,
                "worktree_tree": TREE,
                "receipts": [
                    {
                        "lane": "rust",
                        "passed": True,
                        "skipped": 0,
                        "time": "2026-11-16T12:34:56Z",
                        "rustc": "rustc 1.100.0 (abcdef123 2026-10-01)",
                        "cargo": "cargo 1.100.0 (abcdef123 2026-09-01)",
                    }
                ],
            },
        )

    def test_output_is_one_object_without_trailing_data(self):
        self.put("rust.json", receipt())
        out = self.build().stdout
        self.assertEqual(out, out.strip())
        self.assertLessEqual(len(out.encode()), 16384)

    def test_copies_only_the_schema_keys(self):
        self.put("rust.json", receipt(extra="x", env={"big": "y" * 1000}))
        text = self.build().stdout
        self.assertNotIn("extra", text)
        self.assertNotIn("big", text)
        self.assertNotIn("fingerprint", text)

    def test_all_lane_is_evidence(self):
        self.put("all.json", receipt(lane="all"))
        self.assertEqual(self.note()["receipts"][0]["lane"], "all")

    def test_other_lanes_ride_along_with_a_rust_receipt(self):
        self.put("rust.json", receipt())
        self.put("portable.json", receipt(lane="portable"))
        self.put("browser.json", receipt(lane="browser", rustc="", cargo=""))
        lanes = sorted(r["lane"] for r in self.note()["receipts"])
        self.assertEqual(lanes, ["browser", "portable", "rust"])

    def test_the_same_receipts_give_the_same_bytes(self):
        # The hook compares the note with the one already on the remote, so the
        # bytes may depend on the receipts alone, not on when they were written.
        self.put("rust.json", receipt())
        self.put("portable.json", receipt(lane="portable"))
        first = self.build().stdout
        for name in ("portable.json", "rust.json"):
            body = (self.dir / TREE / name).read_text()
            (self.dir / TREE / name).unlink()
            self.put(name, body)
        self.assertEqual(self.build().stdout, first)

    def test_skips_do_not_disqualify(self):
        self.put("rust.json", receipt(skipped=3))
        self.assertEqual(self.note()["receipts"][0]["skipped"], 3)

    def test_no_receipt_for_the_tree_is_refused(self):
        self.put("rust.json", receipt(), tree=OTHER)
        done = self.build()
        self.assertEqual(done.returncode, 1)
        self.assertEqual(done.stdout, "")
        self.assertIn("no passing", done.stderr)

    def test_missing_directory_is_refused(self):
        self.assertEqual(self.build().returncode, 1)

    def test_portable_only_is_refused(self):
        self.put("portable.json", receipt(lane="portable"))
        self.assertEqual(self.build().returncode, 1)

    def test_browser_only_is_refused(self):
        self.put("browser.json", receipt(lane="browser"))
        self.assertEqual(self.build().returncode, 1)

    def test_failed_rust_is_refused_even_beside_a_passing_portable(self):
        self.put("rust.json", receipt(passed=False))
        self.put("portable.json", receipt(lane="portable"))
        self.assertEqual(self.build().returncode, 1)

    def test_receipt_naming_another_tree_is_ignored(self):
        # The folder name is what the hook looked up; the file has to agree.
        self.put("rust.json", receipt(worktree_tree=OTHER))
        self.assertEqual(self.build().returncode, 1)

    def test_malformed_receipts_are_skipped_not_copied(self):
        self.put("rust.json", receipt())
        bad = {
            "unknown-lane.json": receipt(lane="mutants"),
            "passed-string.json": receipt(passed="true"),
            "skipped-negative.json": receipt(skipped=-1),
            "skipped-bool.json": receipt(skipped=True),
            "skipped-float.json": receipt(skipped=1.5),
            "time-long.json": receipt(time="2026-11-16T12:34:56Z" + "0" * 20),
            "time-shape.json": receipt(time="yesterday"),
            "rustc-long.json": receipt(rustc="r" * 201),
            "cargo-number.json": receipt(cargo=1),
            "not-json.json": "{",
            "list.json": "[]",
        }
        for name, body in bad.items():
            self.put(name, body)
        self.assertEqual(len(self.note()["receipts"]), 1)

    def test_a_missing_toolchain_field_is_empty(self):
        body = receipt(lane="browser")
        del body["rustc"], body["cargo"]
        self.put("rust.json", receipt())
        self.put("browser.json", body)
        browser = [r for r in self.note()["receipts"] if r["lane"] == "browser"]
        self.assertEqual((browser[0]["rustc"], browser[0]["cargo"]), ("", ""))

    def test_a_lone_surrogate_in_a_tool_line_is_malformed(self):
        for field in ("rustc", "cargo"):
            self.put("rust.json", receipt(**{field: "rustc \ud800"}))
            done = self.build()
            self.assertEqual(done.returncode, 1, (field, done.stderr))
            self.assertNotIn("Traceback", done.stderr)

    def test_at_most_eight_receipts(self):
        for index in range(12):
            self.put(f"r{index:02}.json", receipt())
        self.assertEqual(len(self.note()["receipts"]), 8)

    def test_a_symlinked_receipt_is_ignored(self):
        self.put("target.json", receipt())
        folder = self.dir / TREE
        # The target is a valid receipt the directory scan does not read, so
        # only the refusal to follow the link keeps the build from passing.
        (folder / "target.json").rename(folder / "target.txt")
        os.symlink(folder / "target.txt", folder / "rust.json")
        self.assertEqual(self.build().returncode, 1)

    def test_an_oversized_receipt_is_ignored(self):
        self.put("rust.json", receipt(env="x" * 600_000))
        self.assertEqual(self.build().returncode, 1)

    def test_a_tree_that_is_not_an_object_id_never_reaches_the_filesystem(self):
        for tree in ("../x", "A" * 40, "a" * 39, "a" * 41, ""):
            done = self.build(tree)
            self.assertEqual(done.returncode, 2, tree)
            self.assertEqual(done.stdout, "")

    def test_sha256_object_ids_are_accepted(self):
        tree = "c" * 64
        self.put("rust.json", receipt(worktree_tree=tree), tree=tree)
        self.assertEqual(self.note(tree)["worktree_tree"], tree)

    def test_the_cap_keeps_the_receipt_that_vouches(self):
        for index in range(10):
            self.put(f"a{index}.json", receipt(lane="portable"))
        self.put("z.json", receipt())
        lanes = [r["lane"] for r in self.note()["receipts"]]
        self.assertEqual(len(lanes), 8)
        self.assertEqual(lanes[0], "rust")

    def test_full_length_strings_fit_the_size_limit(self):
        full = receipt(rustc="r" * 200, cargo="c" * 200)
        for index in range(8):
            self.put(f"r{index}.json", full)
        self.assertLessEqual(
            len(self.build().stdout.encode()), gate_note.MAX_NOTE_BYTES
        )

    def test_a_lane_of_the_wrong_type_does_not_hide_a_valid_receipt(self):
        self.put("a.json", receipt(lane=[]))
        self.put("b.json", receipt(lane={}))
        self.put("rust.json", receipt())
        self.assertEqual(len(self.note()["receipts"]), 1)

    def test_wide_characters_shrink_the_note_but_keep_the_voucher(self):
        wide = "\U0001f600" * 200
        for index in range(8):
            self.put(f"r{index}.json", receipt(lane="portable", rustc=wide, cargo=wide))
        self.put("z.json", receipt(rustc=wide, cargo=wide))
        done = self.build()
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertLessEqual(len(done.stdout.encode()), gate_note.MAX_NOTE_BYTES)
        self.assertEqual(json.loads(done.stdout)["receipts"][0]["lane"], "rust")

    def test_a_note_over_the_size_limit_is_an_error(self):
        with self.assertRaises(gate_note.NoteError):
            gate_note.encode({"pad": "x" * gate_note.MAX_NOTE_BYTES})

    def test_deeply_nested_receipt_is_ignored(self):
        self.put("rust.json", "[" * 100000)
        self.assertEqual(self.build().returncode, 1)

    def test_unknown_subcommand_is_a_usage_error(self):
        done = subprocess.run(
            [sys.executable, str(SCRIPT), "frobnicate"],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(done.returncode, 2)


if __name__ == "__main__":
    unittest.main()

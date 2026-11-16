"""A gate note built from real receipts is read back by the evidence checker.

scripts/test.sh writes the receipt, scripts/gate-note.py turns it into the note
a push carries, and scripts/check-gate-evidence.py judges that note. Each has
its own suite with hand-made inputs; this one runs the three in a row, so a
change to a field on one side that the next side no longer reads fails here.
The receipt is written by the function scripts/test.sh defines, in a scratch
repository, not by a copy of its shape.
"""

import importlib.util
import json
import re
import shlex
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

try:
    import tomllib  # noqa: F401  (the checker reads the toolchain pin with it)
except ImportError:
    tomllib = None

SPEC = importlib.util.spec_from_file_location(
    "support", Path(__file__).with_name("support.py")
)
support = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(support)

ROOT = support.ROOT
SOURCE = (ROOT / "scripts/test.sh").read_text()
NOTE_SCRIPT = ROOT / "scripts/gate-note.py"
CHECK_SCRIPT = ROOT / "scripts/check-gate-evidence.py"
WRITER = "".join(
    support.function_source(SOURCE, name)
    for name in ("worktree_tree", "prune_by_tree", "json_escape", "write_receipt")
)
PINNED = "1.90.0"
RUSTC = f"rustc {PINNED} (abcdef123 2025-01-01)"
ENV_JSON = '{"profile":"local-rust","ok":true,"required":[],"skipped":[]}'
VALID = {
    "lane": "rust",
    "passed": True,
    "skipped": 0,
    "time": "2025-01-01T00:00:00Z",
    "rustc": RUSTC,
    "cargo": "cargo 1.90.0 (abc 2025-01-01)",
}
# Entries that differ from a valid one in a single field both sides must refuse.
BAD_FIELDS = (
    {"lane": "bogus"},
    {"passed": "true"},
    {"skipped": -1},
    {"skipped": True},
    {"time": 7},
    {"rustc": 7},
    {"rustc": "r" * 201},
)


sh = support.sh
COMMIT = support.COMMIT


def run_script(script, *args):
    return subprocess.run(
        [sys.executable, str(script), *args],
        capture_output=True,
        text=True,
        check=False,
    )


def load(path):
    spec = importlib.util.spec_from_file_location(path.stem.replace("-", "_"), path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class SharedSchema(unittest.TestCase):
    """The note's schema is written in the builder, the checker and the fetcher."""

    def test_the_builder_and_the_checker_agree_on_the_schema(self):
        note, check = load(NOTE_SCRIPT), load(CHECK_SCRIPT)
        self.assertEqual(note.LANES, check.LANES)
        self.assertEqual(note.EVIDENCE_LANES, check.COVERING_LANES)
        self.assertEqual(note.MAX_RECEIPTS, check.MAX_RECEIPTS)
        self.assertEqual(note.MAX_NOTE_BYTES, check.RECEIPT_LIMIT)
        self.assertEqual(note.TREE_ID.pattern, check.TREE_ID.pattern)

    def test_the_fetcher_bounds_a_note_at_the_size_the_builder_allows(self):
        found = re.search(
            r"^LIMIT=(\d+)$", (ROOT / "scripts/fetch-gate-note.sh").read_text(), re.M
        )
        self.assertEqual(int(found.group(1)), load(NOTE_SCRIPT).MAX_NOTE_BYTES)


@unittest.skipIf(tomllib is None, "the checker needs Python 3.11 for tomllib")
class RoundTripTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name) / "repo"
        self.root.mkdir()
        self.work = Path(tmp.name)
        (self.root / "a.txt").write_text("a\n")
        done = sh(f"git init -q -b main . && git add -A && {COMMIT} -m x", self.root)
        self.assertEqual(done.returncode, 0, done.stderr)
        self.pin = self.work / "rust-toolchain.toml"
        self.pin.write_text(f'[toolchain]\nchannel = "{PINNED}"\n')
        self.changed = self.work / "changed"
        self.changed.write_bytes(b"src/main.rs\0")

    def tree(self):
        return sh("git rev-parse 'HEAD^{tree}'", self.root).stdout.strip()

    def receipts(self):
        return self.root / ".git/codetrial-review/gate-receipts/by-tree"

    def write_real_receipt(self, lane="rust", verdict="true", rustc=RUSTC):
        """Run the writer scripts/test.sh defines, as a finished gate run would."""
        assigns = "; ".join(
            f"{key}={shlex.quote(value)}"
            for key, value in {
                "lane": lane,
                "failed": "",
                "skipped": "",
                "receipt_tree": "T1",
                "rustc_version": rustc,
                "cargo_version": "cargo 1.90.0 (abc 2025-01-01)",
                "env_json": ENV_JSON,
            }.items()
        )
        script = (
            f"set -u; ROOT={shlex.quote(str(self.root))}; {assigns}\n{WRITER}\n"
            "worktree_start=$(worktree_tree) || worktree_start=\n"
            f"write_receipt {verdict}\n"
        )
        done = sh(script, self.root)
        self.assertEqual(done.returncode, 0, done.stderr)

    def plant(self, name, body, tree=None):
        folder = self.receipts() / (tree or self.tree())
        folder.mkdir(parents=True, exist_ok=True)
        (folder / name).write_text(body if isinstance(body, str) else json.dumps(body))

    def build(self, tree=None):
        return run_script(
            NOTE_SCRIPT,
            "build",
            "--receipts",
            str(self.receipts()),
            "--tree",
            tree or self.tree(),
        )

    def check(self, note, tree=None):
        """The checker's verdict on `note` (text, or None for no note) for a head tree."""
        args = [
            "--head-tree",
            tree or self.tree(),
            "--changed-files-file",
            str(self.changed),
            "--author-type",
            "User",
            "--base-toolchain-file",
            str(self.pin),
        ]
        if note is not None:
            path = self.work / "note.json"
            path.write_text(note)
            args += ["--receipt-file", str(path)]
        return run_script(CHECK_SCRIPT, *args)

    def accepted(self, note, tree=None):
        done = self.check(note, tree)
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn("gate evidence holds", done.stdout)

    def refused(self, note, tree=None):
        done = self.check(note, tree)
        self.assertEqual(done.returncode, 1, done.stdout + done.stderr)

    def test_a_note_from_a_real_receipt_is_accepted_for_its_tree(self):
        self.write_real_receipt()
        built = self.build()
        self.assertEqual(built.returncode, 0, built.stderr)
        self.accepted(built.stdout)

    def test_the_all_lane_vouches_too(self):
        self.write_real_receipt(lane="all")
        built = self.build()
        self.assertEqual(built.returncode, 0, built.stderr)
        self.accepted(built.stdout)

    def test_the_note_is_refused_for_another_tree(self):
        self.write_real_receipt()
        built = self.build()
        (self.root / "b.txt").write_text("b\n")
        sh(f"git add -A && {COMMIT} -m y", self.root)
        self.assertNotEqual(self.tree(), json.loads(built.stdout)["worktree_tree"])
        self.refused(built.stdout)
        # The builder asked about the new tree has nothing to send either.
        self.assertEqual(self.build().returncode, 1)

    def test_a_portable_lane_alone_builds_no_note_and_a_missing_note_is_refused(self):
        self.write_real_receipt(lane="portable")
        self.assertEqual(self.build().returncode, 1)
        self.refused(None)

    def test_a_note_of_a_portable_receipt_alone_is_refused_whoever_wrote_it(self):
        note = {
            "version": 1,
            "worktree_tree": self.tree(),
            "receipts": [{**VALID, "lane": "portable"}],
        }
        self.refused(json.dumps(note))

    def test_a_failed_run_builds_no_note_and_passed_false_is_refused(self):
        self.write_real_receipt(verdict="false")
        self.assertEqual(self.build().returncode, 1)
        note = {
            "version": 1,
            "worktree_tree": self.tree(),
            "receipts": [{**VALID, "passed": False}],
        }
        self.refused(json.dumps(note))

    def test_a_receipt_from_another_rustc_is_built_and_then_refused(self):
        self.write_real_receipt(rustc="rustc 1.89.0 (abcdef123 2025-01-01)")
        built = self.build()
        self.assertEqual(built.returncode, 0, built.stderr)
        done = self.check(built.stdout)
        self.assertEqual(done.returncode, 1)
        self.assertIn(PINNED, done.stderr)

    def test_a_malformed_receipt_is_skipped_by_the_builder(self):
        self.write_real_receipt()
        bodies = [
            "not json",
            "[]",
            {**VALID, "time": "yesterday"},
            {**VALID, "worktree_tree": "0" * 40},
        ]
        # Named for the tree, so only the field under test can disqualify it.
        bodies += [
            {**VALID, **change, "worktree_tree": self.tree()} for change in BAD_FIELDS
        ]
        for index, body in enumerate(bodies):
            self.plant(f"bad{index}.json", body)
        built = self.build()
        self.assertEqual(built.returncode, 0, built.stderr)
        note = json.loads(built.stdout)
        self.assertEqual(len(note["receipts"]), 1, note)
        self.accepted(built.stdout)

    def test_a_folder_of_only_malformed_receipts_builds_nothing(self):
        self.plant("bad.json", {**VALID, "passed": "yes", "worktree_tree": self.tree()})
        self.assertEqual(self.build().returncode, 1)

    def test_the_checker_refuses_an_entry_with_a_field_of_the_wrong_type_or_size(self):
        for change in (*BAD_FIELDS, {"cargo": None}):
            with self.subTest(change=change):
                note = {
                    "version": 1,
                    "worktree_tree": self.tree(),
                    "receipts": [{**VALID, **change}],
                }
                self.refused(json.dumps(note))


if __name__ == "__main__":
    unittest.main()

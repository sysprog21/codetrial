"""The gate receipt's tree id and toolchain fields, run as scripts/test.sh defines them."""

import importlib.util
import json
import os
import shlex
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location(
    "support", Path(__file__).with_name("support.py")
)
support = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(support)

ROOT = support.ROOT
SOURCE = (ROOT / "scripts/test.sh").read_text()
ENV_JSON = '{"profile":"portable","ok":true,"required":[],"skipped":[]}'


def function_source(name):
    return support.function_source(SOURCE, name)


class ReceiptTests(unittest.TestCase):
    def repo(self, files=None):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = Path(tmp.name)
        for name, body in {"a.txt": "a\n", **(files or {})}.items():
            (root / name).write_text(body)
        done = support.sh(
            f"git init -q -b main . && git add -A && {support.COMMIT} -m x", root
        )
        self.assertEqual(done.returncode, 0, done.stderr)
        return root

    def run_gate(self, root, during="", verdict="true", **setup):
        """Take the start tree, run `during`, then write the receipt, in `set -u` sh."""
        defaults = {
            "lane": "rust",
            "failed": "",
            "skipped": "",
            "receipt_tree": "T1",
            "rustc_version": "rustc 1.0.0",
            "cargo_version": "cargo 1.0.0",
            "env_json": ENV_JSON,
        }
        assigns = "; ".join(
            f"{k}={shlex.quote(v)}" for k, v in {**defaults, **setup}.items()
        )
        functions = "".join(
            function_source(name)
            for name in (
                "worktree_tree",
                "prune_by_tree",
                "json_escape",
                "write_receipt",
            )
        )
        script = (
            f"set -u; ROOT={shlex.quote(str(root))}; {assigns}\n{functions}\n"
            "worktree_start=$(worktree_tree) || worktree_start=\n"
            f"{during}\nwrite_receipt {verdict}\n"
        )
        done = support.sh(script, root)
        self.assertEqual(done.returncode, 0, done.stderr)
        return done

    def folder(self, root):
        return root / ".git/codetrial-review/gate-receipts"

    def receipt(self, root, lane="rust"):
        return json.loads((self.folder(root) / f"{lane}.json").read_text())

    def head_tree(self, root):
        return support.sh("git rev-parse 'HEAD^{tree}'", root).stdout.strip()

    def test_a_clean_tree_is_the_head_tree(self):
        root = self.repo()
        self.run_gate(root)
        self.assertEqual(self.receipt(root)["worktree_tree"], self.head_tree(root))

    def test_a_dirty_tree_committed_unchanged_matches_the_commit(self):
        root = self.repo()
        (root / "a.txt").write_text("changed\n")
        (root / "new.txt").write_text("new\n")
        self.run_gate(root)
        seen = self.receipt(root)["worktree_tree"]
        self.assertNotEqual(seen, self.head_tree(root))
        support.sh(f"git add -A && {support.COMMIT} -m y", root)
        self.assertEqual(seen, self.head_tree(root))

    def test_partial_staging_does_not_change_it(self):
        root = self.repo()
        (root / "a.txt").write_text("changed\n")
        (root / "new.txt").write_text("new\n")
        self.run_gate(root)
        plain = self.receipt(root)["worktree_tree"]
        support.sh("git add new.txt", root)
        self.run_gate(root)
        self.assertEqual(self.receipt(root)["worktree_tree"], plain)

    def test_the_real_index_is_left_alone(self):
        root = self.repo()
        (root / "new.txt").write_text("new\n")
        before = support.sh("git status --porcelain=v1 && git ls-files -s", root).stdout
        self.run_gate(root)
        self.assertEqual(
            support.sh("git status --porcelain=v1 && git ls-files -s", root).stdout,
            before,
        )
        self.assertIn("?? new.txt", before)

    def test_untracked_counts_and_ignored_does_not(self):
        root = self.repo({".gitignore": "skip.txt\n"})
        self.run_gate(root)
        base = self.receipt(root)["worktree_tree"]
        (root / "skip.txt").write_text("ignored\n")
        self.run_gate(root)
        self.assertEqual(self.receipt(root)["worktree_tree"], base)
        (root / "kept.txt").write_text("untracked\n")
        self.run_gate(root)
        self.assertNotEqual(self.receipt(root)["worktree_tree"], base)

    def test_a_change_during_the_run_leaves_it_empty(self):
        root = self.repo()
        self.run_gate(root, during="echo edited >> a.txt")
        receipt = self.receipt(root)
        self.assertEqual(receipt["worktree_tree"], "")
        self.assertEqual(receipt["tree"], "T1")
        self.assertFalse((self.folder(root) / "by-tree").exists())

    def test_the_old_keys_stay_and_the_toolchain_is_carried(self):
        root = self.repo()
        self.run_gate(root, verdict="false", failed=" fmt clippy", skipped="\n  x\n  y")
        text = (self.folder(root) / "rust.json").read_text()
        self.assertEqual(len(text.splitlines()), 1)
        receipt = json.loads(text)
        self.assertEqual(
            {k: receipt[k] for k in ("lane", "passed", "failed", "skipped", "tree")},
            {
                "lane": "rust",
                "passed": False,
                "failed": "fmt clippy",
                "skipped": 2,
                "tree": "T1",
            },
        )
        self.assertRegex(receipt["time"], r"^\d{4}-\d\d-\d\dT[\d:]+Z$")
        self.assertEqual(
            (receipt["rustc"], receipt["cargo"]), ("rustc 1.0.0", "cargo 1.0.0")
        )
        self.assertEqual(receipt["env"], json.loads(ENV_JSON))

    def test_version_strings_are_escaped(self):
        root = self.repo()
        self.run_gate(root, rustc_version='rustc "x" \\ y\tz', env_json="not json")
        receipt = self.receipt(root)
        self.assertEqual(receipt["rustc"], 'rustc "x" \\ yz')
        self.assertIsNone(receipt["env"])

    def test_a_report_that_is_not_utf8_is_not_embedded(self):
        # A byte no JSON reader accepts would make the whole receipt unreadable,
        # so a passing run would leave no evidence at all.
        root = self.repo()
        bad = 'env_json="{\\"a\\":\\"$(printf \'\\377\')\\"}"'
        self.run_gate(root, during=bad)
        self.assertIsNone(self.receipt(root)["env"])

    def test_a_utf8_report_is_embedded(self):
        root = self.repo()
        self.run_gate(root, env_json='{"found":"/home/\u00e9"}')
        self.assertEqual(self.receipt(root)["env"], {"found": "/home/\u00e9"})

    def test_each_receipt_is_kept_by_tree(self):
        root = self.repo()
        self.run_gate(root)
        self.run_gate(root, lane="portable")
        tree = self.receipt(root)["worktree_tree"]
        keep = self.folder(root) / "by-tree" / tree
        self.assertEqual(
            sorted(p.name for p in keep.iterdir()), ["portable.json", "rust.json"]
        )
        self.assertEqual(
            json.loads((keep / "rust.json").read_text()), self.receipt(root)
        )

    def test_only_the_newest_twenty_trees_are_kept(self):
        root = self.repo()
        by_tree = self.folder(root) / "by-tree"
        by_tree.mkdir(parents=True)
        for index in range(25):
            old = by_tree / f"{index:040x}"
            old.mkdir()
            (old / "rust.json").write_text("{}")
            os.utime(old, (1_000_000 + index, 1_000_000 + index))
        # Not a tree id: left alone, and not counted.
        (by_tree / "keep-me").mkdir()
        (by_tree / "keep-me" / "x").write_text("x")
        self.run_gate(root)
        names = sorted(p.name for p in by_tree.iterdir())
        trees = [n for n in names if n != "keep-me"]
        self.assertEqual(len(trees), 20)
        self.assertIn("keep-me", names)
        self.assertNotIn(f"{0:040x}", trees)
        self.assertIn(f"{24:040x}", trees)
        self.assertIn(self.receipt(root)["worktree_tree"], trees)

    def prune(self, ages, current):
        """Run prune_by_tree over one tree folder per entry of `ages` (its mtime,
        or None for now), named by index, and return the indexes that survive."""
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        by_tree = Path(tmp.name)
        for index, age in enumerate(ages):
            folder = by_tree / f"{index:040x}"
            folder.mkdir()
            (folder / "rust.json").write_text("{}")
            if age is not None:
                os.utime(folder, (age, age))
        script = (
            f"set -u\n{function_source('prune_by_tree')}\n"
            f"prune_by_tree {shlex.quote(str(by_tree))} {current:040x}\n"
        )
        done = support.sh(script, by_tree)
        self.assertEqual(done.returncode, 0, done.stderr)
        return {int(p.name, 16) for p in by_tree.iterdir()}

    def test_fresh_trees_and_the_newest_old_ones_survive(self):
        # Three folders written now and 22 old ones, older with the lower index;
        # the current tree is a fresh one.
        kept = self.prune([None] * 3 + [1_000_000 + i for i in range(22)], 0)
        self.assertEqual(kept, {0, 1, 2} | set(range(8, 25)))

    def test_a_folder_written_in_the_last_hour_is_never_pruned(self):
        # Another run may have just written it, so even a 23rd stays.
        self.assertEqual(len(self.prune([None] * 23, 0)), 23)

    def test_the_current_tree_survives_when_every_folder_has_the_same_age(self):
        # The current tree sorts last among equals, so only its own rule saves it.
        kept = self.prune([1_000_000] * 26, 25)
        self.assertIn(25, kept)
        self.assertEqual(len(kept), 20)

    def test_a_rerun_on_an_old_tree_keeps_it_from_the_next_prune(self):
        root = self.repo()
        by_tree = self.folder(root) / "by-tree"
        self.run_gate(root)
        again = by_tree / self.receipt(root)["worktree_tree"]
        # Long ago, with 25 folders newer than it.
        os.utime(again, (1_000_000, 1_000_000))
        for index in range(25):
            newer = by_tree / f"{index:040x}"
            newer.mkdir()
            os.utime(newer, (2_000_000 + index, 2_000_000 + index))
        # Rewriting the receipt files in place leaves a folder's mtime alone, so
        # only the writer's own refresh makes this tree look recent again.
        self.run_gate(root)
        (root / "other.txt").write_text("another tree\n")
        self.run_gate(root)
        self.assertNotEqual(self.receipt(root)["worktree_tree"], again.name)
        self.assertTrue(again.exists())

    def test_a_link_among_the_trees_is_not_followed(self):
        root = self.repo()
        by_tree = self.folder(root) / "by-tree"
        by_tree.mkdir(parents=True)
        outside = Path(tempfile.mkdtemp(dir=root.parent))
        self.addCleanup(
            lambda: support.sh(f"rm -rf {shlex.quote(str(outside))}", root.parent)
        )
        (outside / "example.json").write_text("{}")
        link = by_tree / f"{0:040x}"
        link.symlink_to(outside)
        os.utime(link, (1_000_000, 1_000_000), follow_symlinks=False)
        for index in range(1, 22):
            folder = by_tree / f"{index:040x}"
            folder.mkdir()
            os.utime(folder, (1_000_000 + index, 1_000_000 + index))
        self.run_gate(root)
        self.assertTrue((outside / "example.json").exists())

    def test_a_relative_tmpdir_leaves_nothing_in_the_work_tree(self):
        root = self.repo()
        script = (
            f"set -u; ROOT={shlex.quote(str(root))}; TMPDIR=.\n"
            f"{function_source('worktree_tree')}\nworktree_tree\n"
        )
        done = support.sh(script, root)
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertEqual(support.sh("git status --porcelain", root).stdout, "")
        # An index kept under the work tree would be part of the tree it names.
        self.assertEqual(done.stdout.strip(), self.head_tree(root))

    def test_outside_a_work_tree_there_is_no_receipt_and_no_error(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.run_gate(Path(tmp.name), receipt_tree="")
        self.assertEqual(list(Path(tmp.name).iterdir()), [])

    def test_the_start_tree_is_taken_without_the_executable_bit(self):
        # The closeout check matches a receipt against the fingerprint it takes
        # through sh, so the gate has to take the start tree the same way, or a
        # checkout that lost the bit writes no receipt for it to match.
        root = self.repo()
        (root / "scripts").mkdir()
        script = root / "scripts/tree-fingerprint.sh"
        script.write_bytes((ROOT / "scripts/tree-fingerprint.sh").read_bytes())
        script.chmod(0o644)
        line = next(
            line for line in SOURCE.splitlines() if line.startswith("receipt_tree=")
        )
        done = support.sh(
            f'ROOT={shlex.quote(str(root))}\n{line}\nprintf %s "$receipt_tree"',
            root,
        )
        want = support.sh("sh scripts/tree-fingerprint.sh", root).stdout.strip()
        self.assertNotEqual(want, "")
        self.assertEqual(done.stdout, want)

    def test_a_work_tree_without_a_head_writes_no_tree_id(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = Path(tmp.name)
        support.sh("git init -q -b main .", root)
        self.run_gate(root, receipt_tree="T1")
        self.assertEqual(self.receipt(root)["worktree_tree"], "")


class SupportEnvironmentTests(unittest.TestCase):
    def test_an_inherited_git_variable_never_reaches_a_scratch_repository(self):
        # A hook runs with GIT_DIR and GIT_INDEX_FILE set; a suite started from
        # one must still work on its own scratch repository.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "scratch").mkdir()
            decoy = root / "decoy.git"
            os.mkdir(decoy)
            probe = (
                "import importlib.util, sys;"
                "spec = importlib.util.spec_from_file_location('support', sys.argv[1]);"
                "support = importlib.util.module_from_spec(spec);"
                "spec.loader.exec_module(support);"
                "done = support.sh('git init -q . && git rev-parse --git-dir', sys.argv[2]);"
                "sys.stdout.write(done.stdout)"
            )
            done = subprocess.run(
                [sys.executable, "-c", probe, str(SPEC.origin), str(root / "scratch")],
                env={
                    **os.environ,
                    "GIT_DIR": str(decoy),
                    "GIT_INDEX_FILE": str(root / "decoy-index"),
                },
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(done.stdout.strip(), ".git", done.stderr)


if __name__ == "__main__":
    unittest.main()

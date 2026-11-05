"""The hotspot ranking orders files by commits times lines, inside the window."""

import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/ceiling-hotspots.py"
ROOT = SCRIPT.parents[1]
SPEC = importlib.util.spec_from_file_location("ceiling_hotspots", SCRIPT)

# A commit far outside any window the tests ask for.
LONG_AGO = "2001-01-01T00:00:00 +0000"

# The style checker's default ceiling; only files above it are ranked by default.
OVER = 1501


def run(*args):
    return subprocess.run(
        [sys.executable, "-I", str(SCRIPT), *args],
        capture_output=True,
        text=True,
        timeout=60,
    )


def row_of(output, name):
    """The whitespace-split cells of the table row that starts with name."""
    for line in output.splitlines():
        if line.startswith(name + " "):
            return line.split()
    raise AssertionError(f"no row for {name} in:\n{output}")


class Hotspots(unittest.TestCase):
    def setUp(self):
        self.mod = importlib.util.module_from_spec(SPEC)
        SPEC.loader.exec_module(self.mod)
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        self.git("init", "-q", "-b", "main")

    def git(self, *args, date=None):
        env = dict(os.environ)
        if date:
            env["GIT_AUTHOR_DATE"] = env["GIT_COMMITTER_DATE"] = date
        subprocess.run(
            [
                "git",
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "-C",
                str(self.repo),
                *args,
            ],
            check=True,
            capture_output=True,
            env=env,
        )

    def commit(self, files, date=None):
        """One commit that sets each file to the given number of lines."""
        for name, lines in files.items():
            path = self.repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("x\n" * lines)
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "change", date=date)

    def test_score_is_commits_times_lines_and_orders_the_table(self):
        self.commit({"src/big.rs": 10, "src/busy.rs": 5, "src/idle.rs": 100})
        self.commit({"src/busy.rs": 6})
        self.commit({"src/busy.rs": 5})
        rows = self.mod.rank(
            self.repo,
            {"src/big.rs": 10, "src/busy.rs": 5, "src/idle.rs": 100},
            "30.days",
        )
        self.assertEqual(
            [(r["file"], r["commits"], r["lines"], r["score"]) for r in rows],
            [
                ("src/idle.rs", 1, 100, 100),
                ("src/busy.rs", 3, 5, 15),
                ("src/big.rs", 1, 10, 10),
            ],
        )

    def test_a_branch_that_changed_and_reverted_a_file_still_counts(self):
        self.commit({"src/a.rs": 4})
        self.git("checkout", "-q", "-b", "topic")
        self.commit({"src/a.rs": 9})
        self.commit({"src/a.rs": 4})
        self.git("checkout", "-q", "main")
        self.commit({"src/other.rs": 1})
        self.git("merge", "-q", "--no-ff", "-m", "merge topic", "topic")
        counts = self.mod.commit_counts(self.repo, "30.days", ["src/a.rs"])
        self.assertEqual(
            counts["src/a.rs"], 3, "the first commit, the change, the revert"
        )

    def test_a_commit_outside_the_window_does_not_count(self):
        self.commit({"src/a.rs": 4}, date=LONG_AGO)
        self.commit({"src/a.rs": 5})
        counts = self.mod.commit_counts(self.repo, "30.days", ["src/a.rs"])
        self.assertEqual(counts["src/a.rs"], 1)

    def test_a_merge_commit_adds_nothing_beyond_the_branch_commit(self):
        self.commit({"src/a.rs": 4, "src/other.rs": 1})
        self.git("checkout", "-q", "-b", "topic")
        self.commit({"src/a.rs": 5})
        self.git("checkout", "-q", "main")
        self.commit({"src/other.rs": 2})
        self.git("merge", "-q", "--no-ff", "-m", "merge topic", "topic")
        counts = self.mod.commit_counts(self.repo, "30.days", ["src/a.rs"])
        self.assertEqual(
            counts["src/a.rs"], 2, "the first commit and the branch commit"
        )

    def test_a_name_with_a_space_a_bracket_and_a_non_ascii_byte_is_counted(self):
        name = "src/\u00e9 [a].rs"
        self.commit({name: 4})
        self.commit({name: 5})
        counts = self.mod.commit_counts(self.repo, "30.days", [name])
        self.assertEqual(counts[name], 2)

    def test_under_lists_a_non_ascii_name_unquoted(self):
        name = "web/\u00e9 a.js"
        self.commit({name: 20})
        self.assertEqual(self.mod.under_rows(self.repo, "web", 10), {name: None})

    def test_an_unterminated_last_line_counts_like_the_gate_counts_it(self):
        path = self.repo / "a.rs"
        path.write_text("one\ntwo")
        self.assertEqual(self.mod.line_count(path), 2)

    def test_under_filters_by_size_suffix_and_prefix(self):
        self.commit(
            {
                "web/big.js": 20,
                "web/small.js": 2,
                "web/big.css": 20,
                "web/vendor/lib.js": 20,
            }
        )
        rows = self.mod.under_rows(
            self.repo, "web", 10, suffixes=[".js"], excludes=["web/vendor/"]
        )
        self.assertEqual(rows, {"web/big.js": None})

    def test_cli_prints_one_table_per_directory_with_the_revision(self):
        self.commit({"src/a.rs": OVER, "tests/t.rs": OVER + 1})
        done = run("--repo", str(self.repo))
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn("# history at ", done.stdout)
        self.assertIn("## src/ (1 files)", done.stdout)
        self.assertIn("## tests/ (1 files)", done.stdout)

    def test_a_file_at_the_default_ceiling_is_not_ranked(self):
        self.commit({"src/a.rs": OVER, "src/small.rs": OVER - 1, "web/a.js": OVER})
        done = run("--repo", str(self.repo))
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn("src/a.rs", done.stdout)
        self.assertNotIn("src/small.rs", done.stdout)
        self.assertNotIn("web/a.js", done.stdout)

    def test_the_ceiling_is_the_committed_size_not_the_working_one(self):
        self.commit({"src/a.rs": OVER + 9})
        (self.repo / "src/a.rs").write_text("x\n" * (OVER + 20))
        (self.repo / "src/new.rs").write_text("x\n" * OVER)
        done = run("--repo", str(self.repo))
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertEqual(
            row_of(done.stdout, "src/a.rs")[1:3], [str(OVER + 20), str(OVER + 9)]
        )
        self.assertEqual(row_of(done.stdout, "src/new.rs")[1:3], [str(OVER), "1500"])

    def test_cli_under_shows_a_dash_where_no_ceiling_exists(self):
        self.commit({"web/big.js": 20})
        done = run("--repo", str(self.repo), "--under", "web", "--min-lines", "10")
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertEqual(
            row_of(done.stdout, "web/big.js"), ["web/big.js", "20", "-", "1", "20"]
        )

    def test_a_file_with_no_commit_in_the_window_scores_zero(self):
        self.commit({"src/a.rs": OVER}, date=LONG_AGO)
        done = run("--repo", str(self.repo), "--since", "30.days")
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn("since 30.days", done.stdout)
        self.assertEqual(
            row_of(done.stdout, "src/a.rs"),
            ["src/a.rs", str(OVER), str(OVER), "0", "0"],
        )

    def test_under_with_nothing_to_list_exits_two(self):
        self.commit({"web/a.js": 2})
        done = run("--repo", str(self.repo), "--under", "nodir")
        self.assertEqual(done.returncode, 2)
        self.assertIn("no tracked file under nodir", done.stderr)

    def test_outside_a_repository_exits_two(self):
        with tempfile.TemporaryDirectory() as empty:
            done = run("--repo", empty)
        self.assertEqual(done.returncode, 2)
        self.assertIn("ceiling-hotspots:", done.stderr)

    def test_the_real_tree_lists_every_file_over_the_default_ceiling(self):
        mod = self.mod.STYLE
        over = [
            n for n, lines in mod.sizes(ROOT).items() if lines > mod.DEFAULT_CEILING
        ]
        done = run("--repo", str(ROOT))
        self.assertEqual(done.returncode, 0, done.stderr)
        listed = [
            line.split()[0]
            for line in done.stdout.splitlines()
            if line.split() and line.split()[0] in over
        ]
        self.assertEqual(sorted(listed), sorted(over))


if __name__ == "__main__":
    unittest.main()

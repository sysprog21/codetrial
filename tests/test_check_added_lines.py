"""Each added-line rule fires on a line written to break it, stays quiet on the
forms this tree uses on purpose, and judges only what the change adds."""

import importlib.util
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/check-added-lines.py"
SPEC = importlib.util.spec_from_file_location("added_lines", SCRIPT)
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)
SUPPORT = importlib.util.spec_from_file_location(
    "support", Path(__file__).with_name("support.py")
)
support = importlib.util.module_from_spec(SUPPORT)
SUPPORT.loader.exec_module(support)


def rules(rel, text, added=None):
    """The rule names reported for a file, every line counted as added unless
    `added` names them."""
    lines = added or set(range(1, text.count("\n") + 2))
    return [rule for _, _, rule, _ in CHECK.check_file(rel, text, lines)]


class Narration(unittest.TestCase):
    def test_change_narration_fires_in_every_language(self):
        for rel, text in (
            ("src/a.rs", "// Updated to use the new parser.\nfn f() {}\n"),
            ("web/a.js", "// This function will parse the header.\nf();\n"),
            ("web/a.js", "const x = 1; // now we cache it\n"),
            ("scripts/a.py", "# As per the spec, retry twice.\nx = 1\n"),
            ("scripts/a.sh", "# Step 2: copy the tree\ncp a b\n"),
            ("src/a.rs", "/* The user asked for a retry here. */\nfn f() {}\n"),
            ("web/a.js", "// Added a retry around the fetch.\nfetch(u);\n"),
            ("scripts/a.py", "x = 1  # now returns None on a miss\n"),
        ):
            with self.subTest(text=text):
                self.assertIn("narration", rules(rel, text))

    def test_this_trees_register_is_not_narration(self):
        for rel, text in (
            ("src/a.rs", "/// Moved when the call goes out, as the window is.\n"),
            ("src/a.rs", "/// This function will return an error if it is missing.\n"),
            ("web/a.js", "/** This function will throw on a closed socket. */\n"),
            ("src/a.rs", "/// Changed by enough that an analysis may not hold.\n"),
            ("src/a.rs", "/// The facts after the last request that changed it.\n"),
            ("web/a.js", "// Moved without typing: the caret is not the same.\n"),
            ("scripts/a.py", "LIMIT = 8  # added lines looked at per deleted one\n"),
            (
                "src/a.rs",
                "// The old order set bytes aside first,\n// added to the same "
                "branch, which clears it.\n",
            ),
            ("scripts/a.sh", "# The follow-ups the interviewer may now use.\n"),
            (
                "web/a.js",
                "// The row the probe saw first and\n// added the second time.\n",
            ),
        ):
            with self.subTest(text=text):
                self.assertNotIn("narration", rules(rel, text))

    def test_a_string_is_not_a_comment(self):
        for rel, text in (
            ("src/a.rs", 'const S: &str = "// now we cache it";\n'),
            ("src/a.rs", 'const S: &str = r#"x // now we cache"#;\n'),
            ("src/a.rs", 'const C: char = \'"\'; const S: &str = "// now we";\n'),
            ("web/a.js", "const s = `// now we\n// as per the spec`;\n"),
            ("scripts/a.py", 's = "# now we cache it"\n'),
            ("scripts/a.sh", "echo '# now we cache it'\n"),
            ("scripts/a.sh", "cat <<EOF\n# now we cache it\nEOF\n"),
        ):
            with self.subTest(text=text):
                self.assertEqual(rules(rel, text), [])

    def test_a_url_in_a_string_does_not_open_a_comment(self):
        text = 'let u = "https://x"; // as per the spec\n'
        self.assertEqual(rules("src/a.rs", text), ["narration"])

    def test_nested_rust_block_comments_stay_comments(self):
        text = "/* outer /* inner */ as per the spec */\nfn f() {}\n"
        self.assertEqual(rules("src/a.rs", text), ["narration"])


class Prose(unittest.TestCase):
    def test_markers_outside_rust(self):
        for rel in ("scripts/a.py", "scripts/a.sh", "web/a.js"):
            prefix = "//" if rel.endswith(".js") else "#"
            with self.subTest(rel=rel):
                self.assertEqual(rules(rel, f"{prefix} FIXME: retry\n"), ["marker"])
        self.assertEqual(rules("src/a.rs", "// TODO: left to check-rust-style\n"), [])
        for text in ("# spells it `# TODO` in TAP\n", "# TODO.md is untracked\n"):
            with self.subTest(text=text):
                self.assertEqual(rules("scripts/a.py", text), [])

    def test_emoji_in_a_comment_not_in_a_string(self):
        self.assertEqual(rules("web/a.js", "// done \u2705\n"), ["emoji"])
        self.assertEqual(rules("web/a.js", 'const ok = "\u2705";\n'), [])


class Scanner(unittest.TestCase):
    """The comment and code split each rule reads, on the forms a naive
    scanner gets wrong."""

    def test_a_regex_literal_is_not_a_string(self):
        text = 's.replace(/"/g, "&quot;");\n// as per the spec\n'
        self.assertEqual(rules("web/a.js", text), ["narration"])
        text = 'const m = x.match(/[/"]+/); // as per the spec\n'
        self.assertEqual(rules("web/a.js", text), ["narration"])

    def test_a_division_is_not_a_regex(self):
        text = "const x = a / b / c; // as per the spec\n"
        self.assertEqual(rules("web/a.js", text), ["narration"])
        text = "const x = (a) / 2; // as per the spec\n"
        self.assertEqual(rules("web/a.js", text), ["narration"])

    def test_control_condition_can_be_followed_by_a_regex(self):
        # Read as a division, the quote in each regex would open a string that
        # hides the comment after it.
        for text in (
            'if (ready) /"/.test(s); // as per the spec\n',
            'while ((ready)) /"/.test(s); // as per the spec\n',
        ):
            with self.subTest(text=text):
                self.assertEqual(rules("web/a.js", text), ["narration"])

    def test_template_expressions_are_code(self):
        for text in (
            "const s = `${x /* as per the spec */}`;\n",
            "const s = `${`nested ${x /* as per the spec */}`}`;\n",
            "const s = `${{ a: 1 }.a /* as per the spec */}`;\n",
        ):
            with self.subTest(text=text):
                self.assertEqual(rules("web/a.js", text), ["narration"])
        self.assertEqual(
            rules("web/a.js", "const s = `a ${x} // as per the spec`;\n"), []
        )

    def test_shell_quotes_heredocs_and_arithmetic(self):
        for text, expected in (
            ("echo $((1 << BITS))\n# as per the spec\n", ["narration"]),
            ('msg="first\n# as per the spec\n"\n', []),
            ("cat <<\\EOF\n# now we\nEOF\n", []),
            ("cat <<A <<B\n# now we\nA\n# as per the spec\nB\n", []),
            ("cat <<-EOF\n# now we\n\tEOF\n# as per the spec\n", ["narration"]),
            ('echo "C:\\\\" # as per the spec\n', ["narration"]),
            ("cat <<< '# now we'\n", []),
            # A here-string opens no here-document for the lines after it.
            ('read -r x <<< "$y"\n# as per the spec\n', ["narration"]),
            ("cat <<<word\n# as per the spec\n", ["narration"]),
            # The delimiter is the whole word: `END` alone does not close it.
            (
                "cat <<END-DATA\nEND\n# now we\nEND-DATA\n# as per the spec\n",
                ["narration"],
            ),
            ("cat <<'END.txt'\n# now we\nEND.txt\n# as per the spec\n", ["narration"]),
        ):
            with self.subTest(text=text):
                self.assertEqual(rules("scripts/a.sh", text), expected)

    def test_a_hash_inside_a_word_is_no_shell_comment(self):
        text = 'n=${#items[@]}\necho "$n" # as per the spec\n'
        self.assertEqual(CHECK.check_file("scripts/a.sh", text, {1}), [])
        self.assertEqual(rules("scripts/a.sh", text), ["narration"])

    def test_per_the_spec_alone_is_narration(self):
        self.assertEqual(
            rules("web/a.js", "// Retry twice, per the spec.\n"), ["narration"]
        )

    def test_a_marker_in_a_path_or_url_is_a_name(self):
        self.assertEqual(rules("web/a.js", "// See https://x/TODO for the list\n"), [])


class TestsAndSuppressions(unittest.TestCase):
    def test_a_comment_excuses_a_python_skip(self):
        text = "# Flaky until the fake clock lands.\n@unittest.expectedFailure\ndef test_x():\n    pass\n"
        self.assertEqual(rules("tests/test_a.py", text), [])

    def test_a_python_test_skipped_without_a_reason(self):
        for decorator in (
            "@unittest.expectedFailure",
            '@unittest.skip("")',
            "@pytest.mark.skip",
            "@unittest.skipIf(sys.platform == 'win32')",
            # pytest's xfail and skipif take a condition positionally.
            "@pytest.mark.xfail(True)",
            "@pytest.mark.skipif(sys.platform == 'win32')",
        ):
            text = f"{decorator}\ndef test_x():\n    pass\n"
            with self.subTest(decorator=decorator):
                self.assertEqual(rules("tests/test_a.py", text), ["disabled"])
        for decorator in (
            '@unittest.skip("needs the network")',
            "@unittest.skipIf(sys.platform == 'win32', 'POSIX modes')",
            '@pytest.mark.xfail(reason="the upstream bug")',
            "@pytest.mark.skipif(sys.platform == 'win32', reason='POSIX modes')",
        ):
            text = f"{decorator}\ndef test_x():\n    pass\n"
            with self.subTest(decorator=decorator):
                self.assertEqual(rules("tests/test_a.py", text), [])

    def test_a_suppression_says_what_and_why(self):
        for rel, text in (
            ("web/a.js", "// eslint-disable-next-line no-unused-expressions\nx;\n"),
            ("web/a.js", "/* eslint-disable no-console */\n"),
            ("web/a.js", "/* eslint-disable -- generated adapter */\n"),
            ("web/a.js", """/* eslint-disable '', "" -- generated adapter */"""),
            ("web/a.js", "/*\ufeffeslint-disable -- generated adapter */"),
            ("web/a.js", "/* eslint-disable no-console -- \ufeff */"),
            ("web/a.js", "// eslint-disable-line , -- generated adapter\n"),
            ("web/a.js", "/* eslint-disable no-console */ /* -- unrelated */\n"),
            # The `*/` that closes the comment is not the reason.
            ("web/a.js", "/* eslint-disable no-console -- */\n"),
            # A comment over several rows is one directive, reported once.
            ("web/a.js", "/* eslint-disable\n   no-console\n*/\nx;\n"),
        ):
            with self.subTest(text=text):
                self.assertEqual(rules(rel, text), ["suppression"])
        # A U+2028 ends a JavaScript line comment, so a directive after it on
        # the same row is a comment of its own.
        self.assertEqual(
            rules(
                "web/a.js",
                "// ordinary\u2028debugger; // eslint-disable-line no-debugger\n",
            ),
            ["suppression"],
        )
        # A regular expression literal ends at a U+2028 too, so it does not
        # swallow a comment after one.
        self.assertEqual(rules("web/a.js", "x = /a\u2028// FIXME b\n"), ["marker"])
        # Two directives on one row are two findings.
        self.assertEqual(
            rules("web/a.js", "/* eslint-disable */ /* eslint-disable */\n"),
            ["suppression", "suppression"],
        )
        # A directive whose first added row is blank is still reported there.
        self.assertEqual(
            [
                finding[1:3]
                for finding in CHECK.check_file(
                    "web/a.js", "/* eslint-disable no-console\n\n -- \n*/\n", {2, 3}
                )
            ],
            [(2, "suppression")],
        )
        # Adding only a later row of it still reports it, on that row.
        self.assertEqual(
            [
                finding[1:3]
                for finding in CHECK.check_file(
                    "web/a.js", "/* eslint-disable\n   no-console\n*/\n", {2}
                )
            ],
            [(2, "suppression")],
        )
        for rel, text in (
            (
                "web/a.js",
                "// eslint-disable-next-line no-unused-expressions -- flush\nx;\n",
            ),
            (
                "web/a.js",
                "/* eslint-disable no-console, @scope/plugin/rule -- generated */",
            ),
            ("web/a.js", '/* eslint-disable "no-console"\ufeff-- generated */'),
            ("web/a.js", "/* eslint-disable no-console\n -- generated adapter */"),
            ("web/a.js", "// See eslint-disable in the lint documentation.\n"),
            # ruff's PGH004 and PGH003 hold Python's to a code.
            ("scripts/a.py", "import os  # noqa\n"),
        ):
            with self.subTest(text=text):
                self.assertEqual(rules(rel, text), [])


class OnlyAddedLines(unittest.TestCase):
    def test_a_quoted_or_spaced_name_is_read_from_its_header(self):
        for header, name in (
            ("+++ b/web/a b.js\t", "web/a b.js"),
            ('+++ "b/web/a\\"b.js"', 'web/a"b.js'),
            ('+++ "b/web/\\303\\251.js"', "web/\u00e9.js"),
        ):
            diff = f"diff --git a/x b/x\n--- a/x\n{header}\n@@ -0,0 +1 @@\n+x\n"
            with self.subTest(header=header):
                self.assertEqual(CHECK.added_lines(diff), {name: {1}})

    def test_an_unchanged_line_is_not_judged(self):
        text = "// as per the spec\nfn f() {}\n"
        self.assertEqual(rules("src/a.rs", text, {2}), [])

    def test_the_hunk_header_gives_new_side_line_numbers(self):
        diff = (
            "diff --git a/x.rs b/x.rs\n--- a/x.rs\n+++ b/x.rs\n"
            "@@ -3,0 +4,2 @@\n+a\n+b\n@@ -9 +11 @@\n-c\n+d\n"
            "diff --git a/gone.rs b/gone.rs\n--- a/gone.rs\n+++ /dev/null\n"
            "@@ -1 +0,0 @@\n-e\n"
        )
        self.assertEqual(CHECK.added_lines(diff), {"x.rs": {4, 5, 11}})

    def test_a_context_row_that_joins_two_hunks_is_counted(self):
        # diff.interHunkContext can join two additions around unchanged lines
        # even under -U0; the unchanged rows are lines of the new side too.
        diff = "diff --git a/x.py b/x.py\n--- a/x.py\n+++ b/x.py\n@@ -1,4 +1,4 @@\n-a\n+A\n b\n c\n-d\n+D\n"
        self.assertEqual(CHECK.added_lines(diff), {"x.py": {1, 4}})

    def test_an_added_line_that_starts_with_two_pluses_is_a_line(self):
        diff = "diff --git a/x.sh b/x.sh\n--- a/x.sh\n+++ b/x.sh\n@@ -1,0 +1,2 @@\n+++ x\n+y\n"
        self.assertEqual(CHECK.added_lines(diff), {"x.sh": {1, 2}})

    def test_vendored_and_fixture_trees_are_out_of_scope(self):
        for rel in (
            "web/vendor/x.js",
            "vendor/a/b.rs",
            "tests/fixtures/c.py",
            "README.md",
        ):
            with self.subTest(rel=rel):
                self.assertFalse(CHECK.in_scope(rel))
        self.assertTrue(CHECK.in_scope("web/lib.js"))


class EndToEnd(unittest.TestCase):
    """Run the script against a scratch repository with a base branch."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.git("init", "-q", "-b", "main")
        self.write("scripts/a.py", "x = 1\n")
        self.git("add", ".")
        self.git("commit", "-q", "-m", "base")
        self.git("branch", "base")

    def git(self, *args):
        env = dict(
            support.GIT_ENV,
            GIT_AUTHOR_NAME="t",
            GIT_AUTHOR_EMAIL="t@example.invalid",
            GIT_COMMITTER_NAME="t",
            GIT_COMMITTER_EMAIL="t@example.invalid",
        )
        subprocess.run(["git", *args], cwd=self.root, check=True, env=env)

    def write(self, rel, text):
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def run_script(self, *args, input=None):
        return subprocess.run(
            [sys.executable, "-I", str(SCRIPT), "--root", str(self.root), *args],
            input=input,
            env=support.GIT_ENV,
            capture_output=True,
            text=True,
            timeout=30,
        )

    def test_committed_staged_and_untracked_lines_are_all_judged(self):
        self.write("scripts/a.py", "# as per the spec\nx = 1\n")
        self.git("commit", "-qam", "narrate")
        self.write("web/b.js", "// FIXME: retry\n")
        result = self.run_script("--base", "base")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("scripts/a.py:1: narration:", result.stdout)
        self.assertIn("web/b.js:1: marker:", result.stdout)

    def test_staged_judges_the_index_against_head(self):
        self.write("scripts/a.py", "x = 1\n# now we retry\n")
        self.git("add", "scripts/a.py")
        self.write("scripts/a.py", "x = 1\n")
        result = self.run_script("--staged")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("scripts/a.py:2: narration:", result.stdout)

    def test_a_forced_pathspec_mode_still_finds_the_change(self):
        # A literal pathspec mode inherited from the caller would read the
        # diff's glob pathspecs as file names and pass every change.
        self.write("web/b.js", "// FIXME: retry\n")
        self.git("add", "web/b.js")
        for variable in ("GIT_LITERAL_PATHSPECS", "GIT_NOGLOB_PATHSPECS"):
            with self.subTest(variable=variable):
                result = subprocess.run(
                    [sys.executable, "-I", str(SCRIPT), "--root", str(self.root)]
                    + ["--staged"],
                    env={**support.GIT_ENV, variable: "1"},
                    capture_output=True,
                    text=True,
                    timeout=30,
                )
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertIn("web/b.js:1: marker:", result.stdout)

    def test_staged_filename_with_a_newline(self):
        rel = "scripts/two\nlines.py"
        self.write(rel, "# FIXME: fixture text\n")
        self.git("add", rel)
        result = self.run_script("--staged")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn(f"{rel}:1: marker:", result.stdout)

    def test_worktree_reads_the_link_as_stored_in_git(self):
        self.write("fixture.txt", "# FIXME: fixture text\n")
        (self.root / "scripts/link.py").symlink_to("../fixture.txt")
        result = self.run_script("--base", "base")
        self.assertEqual((result.returncode, result.stdout), (0, ""), result.stderr)

    def test_renames_preserve_only_in_scope_history(self):
        old = "// TODO inherited\n" + "run();\n" * 10
        self.write("web/before.js", old)
        self.write("vendor/import.js", "// FIXME imported\n")
        self.git("add", "-A")
        self.git("commit", "-qm", "seed")
        self.git("branch", "seed")
        self.git("mv", "web/before.js", "web/after.js")
        self.assertEqual(self.run_script("--staged").returncode, 0)
        self.write("web/after.js", old + "// HACK introduced\n")
        self.git("mv", "vendor/import.js", "web/import.js")
        self.git("add", "-A")
        staged = self.run_script("--staged")
        self.git("commit", "-qm", "move")
        for result in (
            staged,
            self.run_script("--base", "seed"),
            self.run_script("--commits", input="HEAD\n"),
        ):
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertEqual(len(result.stdout.splitlines()), 2, result.stdout)
            self.assertIn("web/after.js:12: marker: HACK", result.stdout)
            self.assertIn("web/import.js:1: marker: FIXME", result.stdout)

    def test_a_rename_into_another_language_is_judged_whole(self):
        # A Rust block comment is inert there and an ESLint directive in
        # JavaScript; a rename adds no line, so the move itself is judged.
        self.write("src/example.rs", "/* eslint-disable */\nfn f() {}\n")
        self.git("add", "-A")
        self.git("commit", "-qm", "rust")
        self.git("branch", "rust")
        (self.root / "web").mkdir(exist_ok=True)
        self.git("mv", "src/example.rs", "web/example.js")
        staged = self.run_script("--staged")
        self.git("commit", "-qm", "move")
        for result in (
            staged,
            self.run_script("--base", "rust"),
            self.run_script("--commits", input="HEAD\n"),
        ):
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertIn("web/example.js:1: suppression:", result.stdout)

    def test_a_rename_within_javascript_carries_no_line(self):
        # .js, .mjs and .cjs read under the same rules, so a move among them
        # adds nothing to judge, however its old lines are written.
        self.write("web/old.js", "/* eslint-disable */\nrun();\n")
        self.git("add", "-A")
        self.git("commit", "-qm", "seed")
        self.git("mv", "web/old.js", "web/old.mjs")
        self.assertEqual(self.run_script("--staged").returncode, 0)

    def test_a_clean_change_passes(self):
        self.write("scripts/a.py", "x = 1\n# The retry covers a slow disk.\n")
        result = self.run_script("--base", "base")
        self.assertEqual((result.returncode, result.stdout), (0, ""), result.stderr)

    def test_commits_are_judged_from_their_own_content(self):
        self.write("scripts/a.py", "x = 1\n# now we retry\n")
        self.git("commit", "-qam", "narrate")
        self.write("scripts/a.py", "x = 1\n")
        sha = subprocess.run(
            ["git", "rev-parse", "HEAD"], cwd=self.root, capture_output=True, text=True
        ).stdout
        result = subprocess.run(
            [sys.executable, "-I", str(SCRIPT), "--root", str(self.root), "--commits"],
            input=sha,
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn(":scripts/a.py:2: narration:", result.stdout)
        self.assertTrue(result.stdout.startswith(sha[:7]), result.stdout)

    def test_several_commits_are_each_judged_against_their_parent(self):
        self.write("scripts/a.py", "x = 1\n# now we retry\n")
        self.git("commit", "-qam", "first")
        self.write("web/b.js", "// FIXME: retry\n")
        self.git("add", "-A")
        self.git("commit", "-qm", "second")
        shas = subprocess.run(
            ["git", "rev-list", "base..HEAD"],
            cwd=self.root,
            capture_output=True,
            text=True,
        ).stdout
        result = self.run_script("--commits", input=shas)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(
            sorted(line.split(":", 1)[1] for line in result.stdout.splitlines()),
            [
                "scripts/a.py:2: narration: 'now we' narrates the change; say what the code does and why",
                "web/b.js:1: marker: FIXME; do it or file it",
            ],
        )
        self.assertEqual(
            self.run_script("--commits", input="no-such-commit\n").returncode, 2
        )

    def test_a_merge_is_judged_on_the_lines_neither_parent_has(self):
        self.git("checkout", "-qb", "side")
        self.write("scripts/a.py", "x = 1\n# now we retry\n")
        self.git("commit", "-qam", "side narrates")
        self.git("checkout", "-q", "-")
        self.write("web/b.js", "const b = 1;\n")
        self.git("add", "-A")
        self.git("commit", "-qm", "main moves")
        # An ours merge keeps the first parent whole, and a clean merge brings
        # in only what the side's own commit answers for.
        self.git("merge", "-q", "-s", "ours", "side", "-m", "ours")
        self.git("checkout", "-q", "side")
        self.write("scripts/d.py", "# now we log\n")
        self.git("add", "-A")
        self.git("commit", "-qm", "side narrates again")
        self.git("checkout", "-q", "-")
        self.git("merge", "-q", "--no-ff", "--no-commit", "side")
        self.write("scripts/c.py", "# as per the spec\n")
        self.git("add", "-A")
        self.git("commit", "-qm", "merge with a line of its own")
        merges = subprocess.run(
            ["git", "rev-list", "--merges", "base..HEAD"],
            cwd=self.root,
            capture_output=True,
            text=True,
        ).stdout
        self.assertEqual(len(merges.split()), 2)
        result = self.run_script("--commits", input=merges)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(
            [line.split(":", 1)[1] for line in result.stdout.splitlines()],
            [
                "scripts/c.py:1: narration: 'as per' narrates the change; say what the code does and why"
            ],
        )

    def test_diff_settings_and_odd_files_change_nothing(self):
        # A textconv driver would run on every file the diff touches; a prefix
        # setting would move the `b/` its headers are read by; a file of
        # another encoding is read as best it can be.
        (self.root / ".gitattributes").write_text("*.py diff=loud\n")
        self.git("config", "diff.loud.textconv", "touch converter-ran; cat")
        self.git("config", "diff.noprefix", "true")
        self.write("web/a b.js", "// FIXME: retry\n")
        (self.root / "latin.txt").write_bytes(b"caf\xe9\n")
        self.write("scripts/a.py", "x = 1\n# now we retry\n")
        self.git("add", "-A")
        result = self.run_script("--staged")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("web/a b.js:1: marker:", result.stdout)
        self.assertIn("scripts/a.py:2: narration:", result.stdout)
        self.assertFalse((self.root / "converter-ran").exists())

    def test_inter_hunk_context_cannot_shift_the_lines_judged(self):
        self.write("scripts/a.py", "a = 1\nb = 2\nc = 3\nd = 4\n")
        self.git("commit", "-qam", "four lines")
        self.git("config", "diff.interHunkContext", "2")
        self.write(
            "scripts/a.py", "a = 1  # FIXME one\nb = 2\nc = 3\nd = 4  # FIXME four\n"
        )
        self.git("add", "-A")
        result = self.run_script("--staged")
        self.assertEqual(
            [line.split(": ")[0] for line in result.stdout.splitlines()],
            ["scripts/a.py:1", "scripts/a.py:4"],
        )

    def test_a_colon_in_a_filename_can_be_reported(self):
        self.write("web/a:b.js", "// FIXME: retry\n")
        self.git("add", "-A")
        result = self.run_script("--staged")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("web/a:b.js:1: marker:", result.stdout)

    def test_a_missing_base_cannot_run(self):
        result = self.run_script("--base", "origin/nowhere")
        self.assertEqual(result.returncode, 2)
        self.assertIn("not fetched", result.stderr)


if __name__ == "__main__":
    unittest.main()

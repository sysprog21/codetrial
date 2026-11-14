"""Check the CJK rule line by line, then through real Git objects."""

import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/check-cjk.py"
FENCE = "`" * 3

_spec = importlib.util.spec_from_file_location("check_cjk", SCRIPT)
check_cjk = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(check_cjk)


def refused(text):
    return [number for number, _ in check_cjk.findings(text)]


class Lines(unittest.TestCase):
    def test_comments_are_refused(self):
        self.assertEqual(refused("fn main() {}\n// 這是註解\n"), [2])
        self.assertEqual(refused("// Wait， then retry\n"), [1])
        self.assertEqual(refused("# テスト\n# 테스트\n"), [1, 2])

    def test_one_line_strings_are_data(self):
        text = (
            "assert words('中文') == 2\n"
            'cjk = "這是註解"  # the byte width is the point\n'
            'pattern = r"不(?:知道|清楚)"\n'
            'escaped = "say \\"中\\" twice"\n'
            'label = f"{count} 個"\n'
            "assert.equal(value, `${before}你`);\n"
            "// the IME commits `你` here\n"
            'const char *s = u8"中文";\n'
            'w = L"中文";\n'
            "x = html`<p>中文</p>`;\n"
            'return"中文";\n'
            "c = L'中';\n"
        )
        self.assertEqual(refused(text), [])

    def test_fenced_block_is_data(self):
        text = (
            "// Example input:\n"
            f"// {FENCE}\n"
            "// 這是測試資料，因此可保留中文\n"
            "// 第二行\n"
            f"// {FENCE}\n"
            "int main(void) { return 0; }\n"
        )
        self.assertEqual(refused(text), [])
        self.assertEqual(refused("# ````\n# 資料\n# ````\n"), [])

    def test_fence_lines_themselves_are_judged(self):
        self.assertEqual(refused(f"# {FENCE}\n# 資料\n# {FENCE} 說明\n"), [3])
        self.assertEqual(refused(f"# {FENCE} 中文\nx = 1\n"), [1])

    def test_unclosed_fence_hides_nothing(self):
        self.assertEqual(refused(f"# {FENCE}\n# 資料\n"), [2])

    def test_fence_inside_a_string_opens_nothing(self):
        self.assertEqual(refused(f'x = "{FENCE}"\n# 中文\ny = "{FENCE}"\n'), [2])

    def test_quotes_that_hide_nothing(self):
        self.assertEqual(refused("# don't 改 this, it's fixed\n"), [1])
        self.assertEqual(refused("# Bob's 中文 isn'tx\n"), [1])
        self.assertEqual(refused("fn f<'a>(x: &'a str) {} // 中文\n"), [1])
        self.assertEqual(refused('x = 1  # "中文\n'), [1])
        self.assertEqual(refused('# "start\n# 中文\n# end"\n'), [2])

    def test_docstrings_are_prose(self):
        self.assertEqual(refused('"""\n這個函式回傳結果\n"""\n'), [2])
        self.assertEqual(refused('def f():\n    """回傳結果"""\n'), [2])

    def test_line_numbers_count_newlines_only(self):
        self.assertEqual(refused("x = 1\n\x0c\n# 中文\n"), [3])


class Hook(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.env = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith("GIT_")
        }
        self.env.update(GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1")
        self.git("init", "-q")
        self.git("config", "user.email", "fixture@example.org")
        self.git("config", "user.name", "Fixture")
        self.git("config", "core.hooksPath", os.devnull)
        self.git("config", "commit.gpgsign", "false")

    def git(self, *args, env=None):
        return subprocess.run(
            ["git", *args],
            cwd=self.root,
            env=env or self.env,
            capture_output=True,
            check=True,
            text=True,
        ).stdout.strip()

    def write(self, name, text, stage=True):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        if stage:
            self.git("add", "--", name)

    def scan(self, *args, stdin="", env=None, cwd=None):
        return subprocess.run(
            [sys.executable, str(SCRIPT), *args],
            cwd=cwd or self.root,
            env=env or self.env,
            input=stdin,
            text=True,
            encoding="utf-8",
            capture_output=True,
            check=False,
        )

    def assert_clean(self, result=None):
        result = result or self.scan()
        self.assertEqual(result.returncode, 0, result.stderr)

    def assert_refused(self, where, result=None):
        result = result or self.scan()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn(where, result.stderr)

    def test_refusal_names_the_line_and_the_way_out(self):
        self.write("a.py", "x = 1\n# 中文\n")
        result = self.scan()
        self.assert_refused("a.py:2: # 中文", result)
        self.assertIn("quoted string", result.stderr)

    def test_markdown_vendor_and_binary_are_exempt(self):
        self.write("docs/guide.md", "中文說明\n")
        self.write("web/vendor/lib.js", "// 中文\n")
        self.write("vendor/x/lib.c", "/* 中文 */\n")
        (self.root / "image.bin").write_bytes(b"\x00" + "中文".encode())
        (self.root / "latin1.txt").write_bytes(b"caf\xe9")
        self.git("add", "image.bin", "latin1.txt")
        self.assert_clean()

    def test_only_the_index_is_judged(self):
        self.write("a.py", "x = 1\n")
        self.write("a.py", "# 中文\n", stage=False)
        self.assert_clean()
        self.write("b.py", "# 中文\n")
        self.write("b.py", "x = 1\n", stage=False)
        self.assert_refused("b.py:1:")

    def test_alternate_index_is_used(self):
        self.write("a.py", "x = 1\n")
        self.git("commit", "-qm", "base")
        env = dict(self.env, GIT_INDEX_FILE=str(self.root / "other-index"))
        self.git("read-tree", "HEAD", env=env)
        self.write("a.py", "# 中文\n", stage=False)
        self.git("add", "a.py", env=env)
        self.assert_clean()
        self.assert_refused("a.py:1:", self.scan(env=env))

    def test_unchanged_files_are_not_rescanned_when_staged(self):
        self.write("old.py", "# 中文\n")
        self.git("commit", "-qm", "legacy")
        self.write("new.py", "x = 1\n")
        self.assert_clean()
        self.assert_refused("old.py:1:", self.scan("--tree"))

    def test_tree_from_a_subdirectory_sees_every_path(self):
        self.write("sub/a.py", "x = 1\n")
        self.write("b.py", "# 中文\n")
        self.write("web/vendor/lib.js", "// 中文\n")
        self.assert_refused("b.py:1:", self.scan("--tree", cwd=self.root / "web"))

    def test_commits_mode_checks_outgoing_history(self):
        self.write("a.py", "# 中文\n")
        self.git("commit", "-qm", "first")
        first = self.git("rev-parse", "HEAD")
        self.write("a.py", "x = 1\n")
        self.git("commit", "-qm", "second")
        second = self.git("rev-parse", "HEAD")
        result = self.scan("--commits", stdin=f"{second}\n{first}\n")
        self.assert_refused(f"{first[:12]} a.py:1:", result)

    def test_merge_resolution_is_checked(self):
        self.write("a.py", "x = 1\n")
        self.git("commit", "-qm", "base")
        branch = self.git("symbolic-ref", "--short", "HEAD")
        self.git("checkout", "-qb", "side")
        self.write("side.py", "y = 1\n")
        self.git("commit", "-qm", "side")
        self.git("checkout", "-q", branch)
        self.write("main.py", "z = 1\n")
        self.git("commit", "-qm", "main")
        self.git("merge", "-q", "--no-commit", "side")
        self.write("a.py", "# 中文\n")
        self.git("commit", "-qm", "merge")
        merge = self.git("rev-parse", "HEAD")
        result = self.scan("--commits", stdin=merge + "\n")
        self.assert_refused(f"{merge[:12]} a.py:1:", result)

    def test_invalid_commit_is_an_inspection_failure(self):
        result = self.scan("--commits", stdin="not-a-commit\n")
        self.assertEqual(result.returncode, 2, result.stderr)


if __name__ == "__main__":
    unittest.main()

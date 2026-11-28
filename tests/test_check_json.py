"""Each JSON rule fires on bytes built to break it, and the real tree passes."""

import importlib.util
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/check-json.py"
ROOT = SCRIPT.parents[1]
SPEC = importlib.util.spec_from_file_location("check_json", SCRIPT)
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)
SUPPORT = importlib.util.spec_from_file_location(
    "support", Path(__file__).with_name("support.py")
)
support = importlib.util.module_from_spec(SUPPORT)
SUPPORT.loader.exec_module(support)


def rules(data):
    return [line.split(": ")[1] for line in CHECK.check("x.json", data)]


class Rules(unittest.TestCase):
    def test_valid_json_in_any_layout_passes(self):
        for data in (
            b'{"a": [1, 2], "b": {"c": null}}\n',
            b'{"a":[1,2]}',
            b'[\n  "\xce\xb1",\n  9007199254740991,\n  -9007199254740991,\n  1e300\n]\n',
            b'{"a": {"k": 1}, "b": {"k": 2}}',
            b'{"face": "\\ud83d\\ude00"}',
        ):
            with self.subTest(data=data):
                self.assertEqual(rules(data), [])

    def test_a_duplicate_key_is_named_with_its_path(self):
        found = CHECK.check("x.json", b'{"cases": [{"name": 1, "name": 2}]}')
        self.assertEqual(
            found, ["x.json: duplicate: $.cases[0]: key 'name' is given twice"]
        )

    def test_python_only_constants_are_refused(self):
        for constant in (b"NaN", b"Infinity", b"-Infinity"):
            with self.subTest(constant=constant):
                self.assertEqual(rules(b'{"a": ' + constant + b"}"), ["constant"])

    def test_values_serde_json_cannot_read_are_refused(self):
        for data in (b'{"n": 1e400}', b'{"n": "\\ud800"}', b'{"\\udfff": 1}'):
            with self.subTest(data=data):
                self.assertEqual(rules(data), ["syntax"])

    def test_nesting_past_what_serde_json_reads_is_refused(self):
        def nested(n):
            return b"[" * n + b"0" + b"]" * n

        self.assertEqual(rules(nested(127)), [])
        self.assertEqual(rules(nested(128)), ["depth"])
        self.assertEqual(rules(b'{"a": ' + nested(127) + b"}"), ["depth"])

    def test_an_integer_past_the_double_range_is_refused(self):
        self.assertEqual(rules(b'{"id": 9007199254740992}'), ["precision"])
        self.assertEqual(rules(b'{"id": -12345678901234567890}'), ["precision"])
        self.assertEqual(rules(b'{"id": "12345678901234567890"}'), [])
        self.assertEqual(rules(b'{"id": ' + b"9" * 5000 + b"}"), ["precision"])
        self.assertEqual(rules(b'{"ok": true}'), [])

    def test_bytes_that_are_not_plain_utf8_json(self):
        for data in (b'\xef\xbb\xbf{"a": 1}', b'{"a": "\xff"}', b'{"a": 1,}', b""):
            with self.subTest(data=data):
                self.assertEqual(rules(data), ["syntax"])

    def test_a_parse_error_carries_its_line(self):
        # A missing comma is reported at the value after it in every Python
        # version; a trailing comma moved from the brace to the comma in 3.13.
        self.assertEqual(CHECK.check("x.json", b"[\n1,\n2\n3]")[0][:9], "x.json:4:")


class Tree(unittest.TestCase):
    def test_only_tracked_files_outside_vendor_are_read(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            env = support.GIT_ENV
            subprocess.run(["git", "init", "-q"], cwd=root, check=True, env=env)
            for rel, data in (
                ("bad.json", "{"),
                ("deep/er/nested.json", "{"),
                ("web/vendor/v.json", "{"),
                ("untracked.json", "{"),
            ):
                (root / rel).parent.mkdir(parents=True, exist_ok=True)
                (root / rel).write_text(data)
            subprocess.run(
                ["git", "add", "bad.json", "deep/er/nested.json", "web/vendor/v.json"],
                cwd=root,
                check=True,
                env=env,
            )
            # A pathspec mode the caller forces, which would read `*.json` as a
            # file name and list nothing, changes nothing.
            for forced in ({}, {"GIT_LITERAL_PATHSPECS": "1"}):
                with self.subTest(forced=forced):
                    result = subprocess.run(
                        [sys.executable, "-I", str(SCRIPT), "--root", str(root)],
                        env={**env, **forced},
                        capture_output=True,
                        text=True,
                        timeout=30,
                    )
                    self.assertEqual(result.returncode, 1, result.stderr)
                    self.assertEqual(
                        [line.split(":")[0] for line in result.stdout.splitlines()],
                        ["bad.json", "deep/er/nested.json"],
                    )

    def test_the_real_tree_passes(self):
        self.assertEqual(CHECK.run(ROOT), [])
        # web/problems and web/judges alone hold a JSON file per problem, so a
        # pathspec that stopped at the top level would read a handful.
        self.assertGreater(len(list(CHECK.tracked(ROOT))), 300)


if __name__ == "__main__":
    unittest.main()

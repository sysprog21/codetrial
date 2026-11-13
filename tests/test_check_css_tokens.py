"""Tests for the CSS token checker."""

import importlib.util
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "check-css-tokens.py"
spec = importlib.util.spec_from_file_location("check_css_tokens", str(SCRIPT))
check_css_tokens = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check_css_tokens)


class TestCheckCSSTokens(unittest.TestCase):
    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.test_file = Path(self.temp_dir.name) / "test.css"

    def tearDown(self):
        self.temp_dir.cleanup()

    def test_valid_custom_properties(self):
        self.test_file.write_text(
            """
            :root {
                --brand-violet: oklch(0.5 0.2 290);
                --bg-dark: #1e1e1e;
            }
            .practice-page {
                --theme-color: black;
            }
            .card { color: var(--brand-violet); }
        """,
            encoding="utf-8",
        )
        self.assertEqual(check_css_tokens.check_css_file(self.test_file), 0)

    def test_custom_property_restricted_scope(self):
        self.test_file.write_text(
            """
            .card {
                --local-color: #f00;
                color: var(--local-color);
            }
        """,
            encoding="utf-8",
        )
        # Should flag 1 violation because it's not inside :root or a theme block
        self.assertEqual(check_css_tokens.check_css_file(self.test_file), 1)

    def test_invalid_standard_properties(self):
        self.test_file.write_text(
            """
            .card {
                background: oklch(0.5 0.2 290);
                border: 1px solid #f00;
            }
        """,
            encoding="utf-8",
        )
        self.assertEqual(check_css_tokens.check_css_file(self.test_file), 2)

    def test_comments_and_strings_are_ignored(self):
        self.test_file.write_text(
            """
            .card {
                color: var(--text-main); /* #ff0000 note */
                content: "#123";
                fill: url(#a1b2c3);
            }
        """,
            encoding="utf-8",
        )
        self.assertEqual(check_css_tokens.check_css_file(self.test_file), 0)

    def test_media_queries_and_scopes_are_ignored(self):
        self.test_file.write_text(
            """
            @media (prefers-color-scheme: light) {
                :root { --a: #fff; }
            }
            [data-theme="dark"] .editor-stack {
                --card-color: #000;
            }
        """,
            encoding="utf-8",
        )
        self.assertEqual(check_css_tokens.check_css_file(self.test_file), 0)

    def test_multiline_violations_caught(self):
        self.test_file.write_text(
            """
            .card {
                box-shadow: 0 4px 6px
                            rgba(0, 0, 0, 0.1);
            }
        """,
            encoding="utf-8",
        )
        self.assertEqual(check_css_tokens.check_css_file(self.test_file), 1)

    def test_semicolon_inside_content_string(self):
        self.test_file.write_text(
            """
            .card {
                content: "a; b: #fff";
                color: var(--text-main);
            }
        """,
            encoding="utf-8",
        )
        self.assertEqual(check_css_tokens.check_css_file(self.test_file), 0)

    def test_mask_exempt(self):
        self.test_file.write_text(
            """
            .turn-ring {
                mask: linear-gradient(#000, transparent);
                -webkit-mask: url(#a) #000;
            }
        """,
            encoding="utf-8",
        )
        self.assertEqual(check_css_tokens.check_css_file(self.test_file), 0)

    def test_named_colors_caught(self):
        self.test_file.write_text(
            """
            .btn {
                color: white;
                background-color: transparent;
            }
        """,
            encoding="utf-8",
        )
        # Transparent is exempt, so only 'white' should be flagged
        self.assertEqual(check_css_tokens.check_css_file(self.test_file), 1)
        self.test_file.write_text(
            '.card { filter: drop-shadow(0 0 2px black); }', encoding="utf-8"
        )
        self.assertEqual(check_css_tokens.check_css_file(self.test_file), 1)

        self.test_file.write_text(
            '.card { --local-color: red; }', encoding="utf-8"
        )
        self.assertEqual(check_css_tokens.check_css_file(self.test_file), 1)
        self.test_file.write_text('.x { -webkit-text-stroke: 1px red; }', encoding="utf-8")
        self.assertEqual(check_css_tokens.check_css_file(self.test_file), 1)

    def test_main_exits_on_violation(self):
        web_dir = Path(self.temp_dir.name) / "web" / "sub"
        web_dir.mkdir(parents=True)
        css_file = web_dir / "x.css"
        css_file.write_text(".card { color: #f00; }", encoding="utf-8")

        with self.assertRaises(SystemExit) as cm:
            check_css_tokens.main(Path(self.temp_dir.name))
        self.assertEqual(cm.exception.code, 1)

    def test_main_clean_tree(self):
        web_dir = Path(self.temp_dir.name) / "web" / "sub"
        web_dir.mkdir(parents=True)
        css_file = web_dir / "clean.css"
        css_file.write_text(":root { --ok: #fff; }\n.card { color: var(--ok); }", encoding="utf-8")

        check_css_tokens.main(Path(self.temp_dir.name))

    def test_string_and_comment_lexical_pass(self):
        self.test_file.write_text('.x { content: "/*"; color: red; } .y { content: "*/"; }', encoding="utf-8")
        self.assertEqual(check_css_tokens.check_css_file(self.test_file), 1)

    def test_identifier_properties_ignored(self):
        self.test_file.write_text('a { font-family: Gold; animation: blue 1s; grid-area: tan; }', encoding="utf-8")
        self.assertEqual(check_css_tokens.check_css_file(self.test_file), 0)
        self.test_file.write_text('a { color: blue; }', encoding="utf-8")
        self.assertEqual(check_css_tokens.check_css_file(self.test_file), 1)

    def test_scope_false_positives(self):
        self.test_file.write_text(
            '.practice-page-x { --a: red; }', encoding="utf-8"
        )
        self.assertEqual(check_css_tokens.check_css_file(self.test_file), 1)
        
        self.test_file.write_text(
            ':root .card { --a: red; }', encoding="utf-8"
        )
        self.assertEqual(check_css_tokens.check_css_file(self.test_file), 1)
        
        self.test_file.write_text(
            ':root { @media (min-width: 0) { --a: red; } }', encoding="utf-8"
        )
        self.assertEqual(check_css_tokens.check_css_file(self.test_file), 0)

        self.test_file.write_text(
            '.card .practice-page { --x: #f00; }', encoding="utf-8"
        )
        self.assertEqual(check_css_tokens.check_css_file(self.test_file), 1)

        self.test_file.write_text(
            '.editor-stack[data-theme="light"] { --x: #f00; }', encoding="utf-8"
        )
        self.assertEqual(check_css_tokens.check_css_file(self.test_file), 0)
        
        self.test_file.write_text(
            ':root { color: #f00; --a: #fff; }', encoding="utf-8"
        )
        self.assertEqual(check_css_tokens.check_css_file(self.test_file), 1)
        self.test_file.write_text(':root, .practice-page { --a: red; }', encoding="utf-8")
        self.assertEqual(check_css_tokens.check_css_file(self.test_file), 0)

        self.test_file.write_text(':root, .card { --a: red; }', encoding="utf-8")
        self.assertEqual(check_css_tokens.check_css_file(self.test_file), 1)

if __name__ == "__main__":
    unittest.main()

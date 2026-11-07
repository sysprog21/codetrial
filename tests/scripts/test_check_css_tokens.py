import unittest
import tempfile
import importlib.util
from pathlib import Path
import sys


script_path = Path(__file__).resolve().parent.parent.parent / "scripts" / "check-css-tokens.py"
spec = importlib.util.spec_from_file_location("check_css_tokens", script_path)
check_css_tokens = importlib.util.module_from_spec(spec)
sys.modules["check_css_tokens"] = check_css_tokens
spec.loader.exec_module(check_css_tokens)

class TestCheckCssTokens(unittest.TestCase):
    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.temp_file = Path(self.temp_dir.name) / "test.css"

    def tearDown(self):
        self.temp_dir.cleanup()

    def test_valid_tokens_and_scopes(self):
        css = """
        :root { --bg: #ffffff; }
        [data-theme="light"] { --text: rgba(0,0,0,0.5); }
        .valid { 
            color: var(--bg); 
            background: black; 
        }
        /* #ff0000 inside comment should be ignored */
        """
        self.temp_file.write_text(css, encoding='utf-8')
        errors = check_css_tokens.check_file(self.temp_file)
        self.assertEqual(len(errors), 0)

    def test_hardcoded_hex(self):
        css = ".invalid { color: #ff0000; }"
        self.temp_file.write_text(css, encoding='utf-8')
        errors = check_css_tokens.check_file(self.temp_file)
        self.assertEqual(len(errors), 1)
        self.assertIn("Hardcoded color \"#ff0000\"", errors[0])
        
    def test_hardcoded_rgba(self):
        css = ".invalid { box-shadow: 0 0 10px rgba(0, 0, 0, 0.5); }"
        self.temp_file.write_text(css, encoding='utf-8')
        errors = check_css_tokens.check_file(self.temp_file)
        self.assertEqual(len(errors), 1)
        self.assertIn("rgba(0, 0, 0, 0.5)", errors[0])

if __name__ == '__main__':
    unittest.main()
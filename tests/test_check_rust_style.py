"""Each Rust style rule fires on a tree built to break it, and the real tree passes."""

import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/check-rust-style.py"
ROOT = SCRIPT.parents[1]

# The loosest manifest the lint-table rule accepts, copied from the real one so
# a lint added to REQUIRED_LINTS without Cargo.toml follows is caught below.
MANIFEST = (ROOT / "Cargo.toml").read_text()
HOOK_MANIFEST = (ROOT / "scripts/hook-check/Cargo.toml").read_text()
SPEC = importlib.util.spec_from_file_location("rust_style", SCRIPT)


def run(root, *args):
    return subprocess.run(
        [sys.executable, "-I", str(SCRIPT), "--root", str(root), *args],
        capture_output=True,
        text=True,
        timeout=30,
    )


class RustStyle(unittest.TestCase):
    def setUp(self):
        self.style = importlib.util.module_from_spec(SPEC)
        SPEC.loader.exec_module(self.style)
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.write("Cargo.toml", MANIFEST)
        self.write("scripts/hook-check/Cargo.toml", HOOK_MANIFEST)
        self.fixtures = []

    def write(self, name, text):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def findings(self, name, text):
        """Findings for a tree holding just this file, besides the manifest."""
        for old in self.fixtures:
            old.unlink(missing_ok=True)
        self.write(name, text)
        self.fixtures = [self.root / name]
        return self.style.run(self.root)

    def assertFires(self, rule, name, text):
        lines = self.findings(name, text)
        self.assertTrue(
            any(f": {rule}:" in line and line.startswith(name) for line in lines),
            lines,
        )

    def test_a_clean_file_is_silent(self):
        self.assertEqual(self.findings("src/lib.rs", "pub fn f() {}\n"), [])

    def test_allow_is_refused_in_each_spelling(self):
        for text in (
            "#[allow(dead_code)]\nfn f() {}\n",
            "#![allow(unused)]\n",
            "# /* gap */ [allow(dead_code)]\nfn f() {}\n",
            "#! /* gap */ [allow(dead_code)]\nfn f() {}\n",
            "# /* gap */ [cfg_attr(test, allow(dead_code))]\nfn f() {}\n",
            "#[\nallow(dead_code)\n]\nfn f() {}\n",
            "#[cfg_attr(\n test,\n allow(dead_code)\n)]\nfn f() {}\n",
            "#![deny(warnings)]\n",
            "#[cfg_attr(test, allow(dead_code))]\nfn f() {}\n",
        ):
            with self.subTest(text=text):
                self.assertFires("suppression", "src/lib.rs", text)

    def test_a_method_named_allow_is_not_a_suppression(self):
        self.assertEqual(
            self.findings("src/lib.rs", "fn f(l: L) { l.allow(1); }\n"), []
        )

    def test_cfg_attr_calls_are_not_attributes(self):
        text = "fn f() { cfg_attr(allow()); cfg_attr(ignore()); }\n"
        self.assertEqual(self.findings("src/lib.rs", text), [])

    def test_multiline_cfg_attr_without_a_finding_finishes(self):
        text = "#[cfg_attr(test, expect(dead_code,\n" + "\n" * 100
        text += 'reason = "test-only"))]\nfn f() {}\n'
        self.write("src/lib.rs", text)
        result = run(self.root)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "")

    def test_cfg_conditions_and_attribute_arguments_are_not_attributes(self):
        for text in (
            '#[cfg_attr(ignore, doc = "only documentation")]',
            '#[cfg_attr(any(ignore, all(test, unix)), doc = "documentation")]',
            "#[cfg_attr(test, doc = stringify!(allow(dead_code)))]",
            "#[cfg_attr(test, doc = stringify!(ignore()))]",
        ):
            with self.subTest(text=text):
                self.assertEqual(self.findings("src/lib.rs", text + "\n"), [])

    def test_nested_and_multiple_conditional_attributes_are_checked(self):
        for rule, text in (
            (
                "suppression",
                '#[cfg_attr(any(test, unix), doc = "doc", allow(dead_code))]',
            ),
            ("suppression", "#[cfg_attr(test, cfg_attr(unix, allow(dead_code)))]"),
            ("suppression", "#![deny(dead_code, warnings)]"),
            ("ignore", '#[cfg_attr(any(test, unix), cfg_attr(feature = "x", ignore))]'),
        ):
            with self.subTest(rule=rule, text=text):
                self.assertFires(rule, "src/lib.rs", text + "\n")

    def test_documentation_tokens_do_not_hide_the_next_attribute(self):
        text = "#[doc = stringify!(#[ignore])]\n#[allow(dead_code)]\nfn f() {}\n"
        lines = self.findings("src/lib.rs", text)
        self.assertEqual(len(lines), 1, lines)
        self.assertTrue(lines[0].startswith("src/lib.rs:2: suppression:"), lines)

    def test_raw_identifiers_follow_the_same_policy(self):
        for rule, text in (
            ("suppression", "#[r#allow(dead_code)]"),
            ("suppression", "#![r#forbid(r#warnings)]"),
            ("suppression", "#[r#cfg_attr(test, r#allow(dead_code))]"),
            ("ignore", "#[r#ignore]"),
            ("ignore", '#[r#cfg_attr(test, r#ignore = "")]'),
            ("expect", 'fn f() { value.r#expect(""); }'),
        ):
            with self.subTest(rule=rule, text=text):
                self.assertFires(rule, "src/lib.rs", text + "\n")

    def test_strings_and_block_comments_are_not_code(self):
        for text in (
            "/* #[allow(dead_code)] */\nfn f() {}\n",
            'const S: &str = "#[allow(x)]";\n',
            '#[cfg_attr(test, expect(a, reason = "we allow(b)"))]\nfn f() {}\n',
            'const S: &str = r#"x.expect(""); // TODO"#;\n',
            "const C: char = '\\'';\nfn f() {}\n",
        ):
            with self.subTest(text=text):
                self.assertEqual(self.findings("src/lib.rs", text), [])

    def test_a_slash_pair_inside_a_string_hides_nothing(self):
        text = 'fn f() { let s = "a // b"; x.expect(""); }\n'
        self.assertFires("expect", "src/lib.rs", text)

    def test_markers_in_block_and_doc_comments_are_refused(self):
        for text in ("/* TODO */\n", "/// FIXME later\n", "/*\n x\n HACK\n*/\n"):
            with self.subTest(text=text):
                self.assertFires("marker", "src/lib.rs", text)

    def test_the_marker_line_is_the_comment_line(self):
        lines = self.findings("src/lib.rs", "fn f() {}\n/*\n x\n HACK\n*/\n")
        self.assertEqual(lines, ["src/lib.rs:4: marker: HACK"])

    def test_expect_with_a_reason_passes(self):
        text = '#[expect(dead_code, reason = "held")]\nfn f() {}\n'
        self.assertEqual(self.findings("src/lib.rs", text), [])

    def test_bare_ignore_is_refused_and_reasoned_ignore_passes(self):
        self.assertFires("ignore", "tests/a.rs", "#[ignore]\n#[test]\nfn t() {}\n")
        self.assertFires(
            "ignore", "tests/a.rs", "#[cfg_attr(test, ignore)]\nfn t() {}\n"
        )
        self.assertFires("ignore", "tests/a.rs", "# /* gap */ [ignore]\n")
        self.assertFires(
            "ignore", "tests/a.rs", "# /* gap */ [cfg_attr(test, ignore)]\n"
        )
        ok = '#[ignore = "needs a key"]\n#[test]\nfn t() {}\n'
        self.assertEqual(self.findings("tests/a.rs", ok), [])

    def test_empty_ignore_reasons_are_refused(self):
        for text in (
            '#[ignore = ""]',
            '#[ignore = r#""#]',
            '#[cfg_attr(test, ignore = "")]',
        ):
            with self.subTest(text=text):
                self.assertFires("ignore", "tests/a.rs", text + "\nfn f() {}\n")

    def test_an_empty_expect_message_is_refused(self):
        for text in (
            'x.expect("")',
            'x.expect("",)',
            'x.expect(r#""#)',
            'x.expect /* checked */ ("")',
            'x. /* checked */ expect("")',
        ):
            with self.subTest(text=text):
                self.assertFires("expect", "src/lib.rs", f"fn f() {{ {text}; }}\n")

    def test_multiline_rules_report_the_opening_line(self):
        for rule, text in (
            ("suppression", "#[cfg_attr(\n test,\n allow(dead_code)\n)]"),
            ("ignore", "#[\nignore\n]"),
            ("expect", 'value.expect(\n ""\n);'),
        ):
            with self.subTest(rule=rule):
                lines = self.findings("src/lib.rs", "\n" + text + "\n")
                self.assertTrue(
                    any(line.startswith(f"src/lib.rs:2: {rule}:") for line in lines),
                    lines,
                )

    def test_markers_are_refused_in_comments_only(self):
        for text in (
            "// TODO: later\n",
            "// FIXME\n",
            "// ponytail: shortcut\n",
            "// see TODO.md for the plan\n",
            "// history lives in DONE.md\n",
            "// per CLAUDE.md\n",
        ):
            with self.subTest(text=text):
                self.assertFires("marker", "src/lib.rs", text)
        self.assertEqual(self.findings("src/lib.rs", 'const T: &str = "TODO";\n'), [])

    def test_cjk_is_refused_in_rust_and_the_browser_half(self):
        han = chr(0x4E2D) + chr(0x6587)
        self.assertFires("language", "src/lib.rs", f'const S: &str = "{han}";\n')
        self.assertFires("language", "web/app.js", f"// {han}\n")
        self.assertEqual(self.findings("web/vendor/x.js", f"// {han}\n"), [])
        for point in (
            0x1100,
            0x2E80,
            0x3105,
            0x3131,
            0x31A0,
            0x31F0,
            0x3300,
            0xA960,
            0xD7B0,
            0xF900,
            0xFE10,
            0xFE30,
            0x1AFF0,
            0x1B000,
            0x1B100,
            0x20000,
            0x30000,
        ):
            with self.subTest(point=hex(point)):
                self.assertFires(
                    "language", "src/lib.rs", f'const S: &str = "{chr(point)}";\n'
                )
        greek = chr(0x3B1) + chr(0x3B2)
        self.assertEqual(
            self.findings("src/lib.rs", f'const S: &str = "{greek}";\n'), []
        )

    def test_a_new_file_cannot_exceed_the_default_ceiling(self):
        self.assertEqual(self.findings("src/big.rs", "// line\n" * 1500), [])
        self.assertFires("ceiling", "src/big.rs", "// line\n" * 1501)

    def start_history(self):
        self.git("init", "-q")
        self.git("symbolic-ref", "HEAD", "refs/heads/main")
        self.commit_tree("base")
        self.git("tag", "base")

    def commit_tree(self, message):
        self.git("add", "-A")
        self.git("commit", "-qm", message)

    def test_git_history_provides_ceilings_without_a_table(self):
        self.write("src/big.rs", "// line\n" * 1600)
        self.start_history()
        self.assertEqual(self.style.run(self.root, "HEAD"), [])
        self.assertFalse((self.root / "scripts/rust-style-ceilings.json").exists())
        self.assertEqual(self.git("status", "--porcelain").stdout, b"")
        self.write("src/big.rs", "// line\n" * 1601)
        self.assertTrue(any("over 1600" in x for x in self.style.run(self.root)))

    def test_committed_shrink_cannot_grow_back(self):
        self.write("src/big.rs", "// line\n" * 1600)
        self.start_history()
        self.write("src/big.rs", "// line\n" * 1550)
        self.assertEqual(self.style.run(self.root, "base"), [])
        self.commit_tree("shrink")
        self.write("src/big.rs", "// line\n" * 1560)
        self.commit_tree("regrow")
        self.assertEqual(
            self.style.run(self.root), []
        )  # HEAD checks uncommitted growth.
        self.assertTrue(
            any("over 1550" in x for x in self.style.run(self.root, "base"))
        )

    def test_new_committed_and_renamed_paths_get_no_exemption(self):
        self.write("src/big.rs", "// line\n" * 1600)
        self.start_history()
        self.git("mv", "src/big.rs", "src/renamed.rs")
        self.write("src/new.rs", "// line\n" * 1600)
        self.commit_tree("new paths")
        findings = self.style.run(self.root, "base")
        for name in ["renamed.rs", "new.rs"]:
            self.assertTrue(any(name in x and "over 1500" in x for x in findings))

    def test_deletion_retires_the_allowance(self):
        self.write("src/big.rs", "// line\n" * 1600)
        self.start_history()
        (self.root / "src/big.rs").unlink()
        self.assertEqual(self.style.run(self.root, "base"), [])
        self.commit_tree("delete")
        self.write("src/big.rs", "// line\n" * 1600)
        self.commit_tree("recreate")
        self.assertTrue(
            any("over 1500" in x for x in self.style.run(self.root, "base"))
        )

    def test_shrinking_below_default_keeps_the_general_default(self):
        self.write("src/big.rs", "// line\n" * 1600)
        self.start_history()
        self.write("src/big.rs", "// line\n" * 1400)
        self.commit_tree("shrink")
        self.write("src/big.rs", "// line\n" * 1500)
        self.assertEqual(self.style.run(self.root, "base"), [])
        # One past the default is checked against the default, rather than
        # against the 1600 the base allowed.
        self.write("src/big.rs", "// line\n" * 1501)
        findings = self.style.run(self.root, "base")
        self.assertTrue(any("over 1500" in x for x in findings), findings)

    def test_merge_history_cannot_hide_a_smaller_version(self):
        self.write("src/big.rs", "// line\n" * 1600)
        self.start_history()
        self.git("checkout", "-qb", "side")
        self.write("src/big.rs", "// line\n" * 1550)
        self.commit_tree("shrink on side")
        self.git("checkout", "main")
        self.git("merge", "--no-ff", "-s", "ours", "side", "-m", "merge")
        self.assertTrue(
            any("over 1550" in x for x in self.style.run(self.root, "base"))
        )

    def test_a_merge_deletion_retires_the_allowance(self):
        self.write("src/big.rs", "// line\n" * 1600)
        self.start_history()
        self.git("checkout", "-qb", "side")
        self.write("src/other.rs", "// other\n")
        self.commit_tree("side changes")
        self.git("checkout", "main")
        self.git("merge", "--no-ff", "--no-commit", "side")
        self.git("rm", "src/big.rs")
        self.commit_tree("merge deletes")
        self.write("src/big.rs", "// line\n" * 1600)
        self.commit_tree("recreate")
        self.assertTrue(
            any("over 1500" in x for x in self.style.run(self.root, "base"))
        )

    def test_unrelated_history_cannot_supply_an_allowance(self):
        self.start_history()
        self.git("checkout", "--orphan", "unrelated")
        self.commit_tree("other root")
        self.assertTrue(
            any(
                "cannot read Git size baseline" in x
                for x in self.style.run(self.root, "base")
            )
        )

    def test_diverged_baseline_keeps_branch_history(self):
        self.write("src/big.rs", "// line\n" * 1600)
        self.start_history()
        self.git("checkout", "-qb", "side")
        self.write("src/big.rs", "// line\n" * 1550)
        self.commit_tree("shrink")
        self.git("checkout", "main")
        self.write("src/other.rs", "// other\n")
        self.commit_tree("main advances")
        self.git("checkout", "side")
        self.write("src/big.rs", "// line\n" * 1560)
        self.assertTrue(
            any("over 1550" in x for x in self.style.run(self.root, "main"))
        )

    def test_git_and_worktree_count_newlines_the_same_way(self):
        name = "src/large name " + chr(0x3B1) + ".rs"
        self.write(name, "// line\r\n" * 1500 + "// last")
        self.start_history()
        self.assertEqual(self.style.run(self.root, "HEAD"), [])
        self.write(name, "// line\n" * 1500 + "// last\n")
        self.assertEqual(self.style.run(self.root, "HEAD"), [])
        self.write(name, "// line\n" * 1500 + "// last\n// added")
        self.assertTrue(
            any("over 1501" in x for x in self.style.run(self.root, "HEAD"))
        )

    def test_a_descendant_baseline_cannot_relax_head(self):
        self.write("src/big.rs", "// line\n" * 1600)
        self.start_history()
        self.git("checkout", "-qb", "ahead")
        self.write("src/big.rs", "// line\n" * 2000)
        self.commit_tree("ahead grows")
        self.git("checkout", "main")
        self.write("src/big.rs", "// line\n" * 1900)
        self.assertTrue(
            any("over 1600" in x for x in self.style.run(self.root, "ahead"))
        )

    def test_a_baseline_only_path_is_still_new_in_the_worktree(self):
        self.start_history()
        self.git("checkout", "-qb", "ahead")
        self.write("src/new.rs", "// line\n" * 1600)
        self.commit_tree("ahead adds")
        self.git("checkout", "main")
        self.write("src/new.rs", "// line\n" * 1600)
        self.assertTrue(
            any("over 1500" in x for x in self.style.run(self.root, "ahead"))
        )

    def test_every_required_lint_refuses_weakening_or_removal(self):
        for group, lints in self.style.REQUIRED_LINTS.items():
            for name, level in lints.items():
                original = f'{name} = "{level}"'
                self.assertIn(original, MANIFEST)
                for replacement in ("", f'{name} = "warn"', f'{name} = "allow"'):
                    with self.subTest(group=group, lint=name, replacement=replacement):
                        self.write(
                            "Cargo.toml", MANIFEST.replace(original, replacement)
                        )
                        findings = list(self.style.check_lint_table(self.root))
                        self.assertTrue(
                            any(name in line for line in findings), findings
                        )

    def stub_cargo(self, **kwargs):
        """Make the checker's subprocess module answer or fail as given."""
        self.style.subprocess = SimpleNamespace(
            run=Mock(**kwargs), TimeoutExpired=subprocess.TimeoutExpired
        )

    def workspace_findings(self, extra=()):
        paths = ["Cargo.toml", "scripts/hook-check/Cargo.toml", *extra]
        metadata = {
            "workspace_members": paths,
            "packages": [{"id": p, "manifest_path": str(self.root / p)} for p in paths],
        }
        result = subprocess.CompletedProcess([], 0, json.dumps(metadata), "")
        self.stub_cargo(return_value=result)
        return list(self.style.check_workspace(self.root))

    def test_workspace_members_cannot_opt_out_of_lint_policy(self):
        member = "scripts/hook-check/Cargo.toml"
        self.write(member, '[package]\nname = "hook-check"\nversion = "0.0.0"\n')
        findings = self.workspace_findings()
        self.assertTrue(any(member in finding for finding in findings), findings)

    def test_root_cannot_opt_out_of_workspace_lint_policy(self):
        self.write(
            "Cargo.toml", MANIFEST.replace("workspace = true", "workspace = false")
        )
        findings = list(self.style.check_lint_table(self.root))
        self.assertTrue(any("inherit" in finding for finding in findings), findings)

    def test_a_new_workspace_member_must_inherit_the_policy(self):
        member = "scripts/helper/Cargo.toml"
        helper = '[package]\nname = "helper"\nversion = "0.0.0"\n'
        self.write(member, helper)
        findings = self.workspace_findings([member])
        self.assertTrue(any(member in f for f in findings), findings)
        self.write(member, helper + "[lints]\nworkspace = true\n")
        self.assertEqual(self.workspace_findings([member]), [])

    def test_workspace_metadata_failure_is_not_a_pass(self):
        for result in (
            subprocess.CompletedProcess([], 1, "", "workspace resolution failed"),
            subprocess.CompletedProcess(
                [], 0, '{"workspace_members":[],"packages":[]}', ""
            ),
            subprocess.CompletedProcess(
                [], 0, '{"workspace_members":["missing"],"packages":[]}', ""
            ),
        ):
            with self.subTest(result=result):
                self.stub_cargo(return_value=result)
                self.assertTrue(list(self.style.check_workspace(self.root)))
        for error in (
            FileNotFoundError("cargo"),
            subprocess.TimeoutExpired("cargo metadata", 60),
        ):
            self.stub_cargo(side_effect=error)
            self.assertTrue(list(self.style.check_workspace(self.root)))

    def test_dependencies_outside_the_workspace_are_not_audited(self):
        paths = ["Cargo.toml", "scripts/hook-check/Cargo.toml"]
        metadata = {
            "workspace_members": paths,
            "packages": [{"id": p, "manifest_path": str(self.root / p)} for p in paths]
            + [
                {
                    "id": "dependency",
                    "manifest_path": "/not/a/workspace/member/Cargo.toml",
                }
            ],
        }
        result = subprocess.CompletedProcess([], 0, json.dumps(metadata), "")
        self.stub_cargo(return_value=result)
        self.assertEqual(list(self.style.check_workspace(self.root)), [])

    def test_portable_check_needs_no_cargo_and_workspace_check_fails_without_it(self):
        for args, status in (([], 0), (["--workspace"], 1)):
            with self.subTest(args=args):
                result = subprocess.run(
                    [
                        sys.executable,
                        "-I",
                        str(SCRIPT),
                        "--root",
                        str(self.root),
                        *args,
                    ],
                    env=dict(os.environ, PATH=""),
                    capture_output=True,
                    text=True,
                    timeout=30,
                )
                self.assertEqual(result.returncode, status, result.stderr)
                if status:
                    self.assertIn("cannot inspect workspace membership", result.stdout)

    def test_table_form_lint_levels_are_checked(self):
        for level in ("deny", "warn"):
            with self.subTest(level=level):
                self.write(
                    "Cargo.toml",
                    MANIFEST.replace(
                        'unwrap_used = "deny"',
                        f'unwrap_used = {{ level = "{level}", priority = 1 }}',
                    ),
                )
                findings = list(self.style.check_lint_table(self.root))
                self.assertEqual(bool(findings), level != "deny", findings)

    def test_a_group_allowed_above_a_required_lint_is_refused(self):
        weakened = MANIFEST.replace(
            'unwrap_used = "deny"',
            'unwrap_used = { level = "deny", priority = -1 }\n'
            'restriction = { level = "allow", priority = 1 }',
        )
        self.assertNotEqual(weakened, MANIFEST)
        self.write("Cargo.toml", weakened)
        findings = list(self.style.check_lint_table(self.root))
        self.assertTrue(
            any("restriction" in line and "overrides" in line for line in findings),
            findings,
        )

    def git(self, *args):
        identity = [
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.org",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
        ]
        return subprocess.run(
            ["git", "-C", str(self.root), *identity, *args],
            capture_output=True,
            check=True,
        )

    def test_explicit_missing_baseline_fails(self):
        # The checker reads no CI variable, so there is one answer. The test
        # does not set one: the runner executes cases in threads, and a changed
        # os.environ under a thread that is spawning git makes the spawn fail
        # with EFAULT. A repository with history, so the refusal is of the
        # absent ref and not of the directory.
        self.write("src/big.rs", "// line\n" * 1600)
        self.start_history()
        self.assertTrue(
            any("not fetched" in x for x in self.style.run(self.root, "no-such-ref"))
        )

    def test_shallow_history_cannot_claim_a_ratchet(self):
        self.write("src/big.rs", "// line\n" * 1600)
        self.start_history()
        with tempfile.TemporaryDirectory() as clone:
            self.git("clone", "-q", "--depth", "1", self.root.as_uri(), clone)
            findings = self.style.run(Path(clone), "HEAD")
        self.assertTrue(any("complete history" in x for x in findings))

    def test_legacy_ratchet_command_is_removed(self):
        self.assertEqual(run(self.root, "--ratchet").returncode, 2)


if __name__ == "__main__":
    unittest.main()

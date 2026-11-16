"""Check the Rust-scope helper of scripts/check-gate-evidence.py."""

import importlib.util
import json
from pathlib import Path
import posixpath
import re
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/check-gate-evidence.py"

_spec = importlib.util.spec_from_file_location("check_gate_evidence", SCRIPT)
gate_evidence = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(gate_evidence)


def run(args, data):
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        input=data,
        capture_output=True,
        check=False,
    )


def scope(*paths):
    return run(["--scope-only"], ("\n".join(paths) + "\n").encode())


# One pass over Rust text, left to right, so a quote inside a comment or a
# character literal cannot pair with a later one; only string literals are kept.
RUST_TOKEN = re.compile(
    r"""//[^\n]*
    |/\*.*?\*/
    |(?<!\w)b?r(?P<hashes>\#*)"(?P<raw>.*?)"(?P=hashes)
    |b?"(?P<plain>(?:[^"\\]|\\.)*)"
    |'(?:\\[^']+|[^\\'])'""",
    re.DOTALL | re.VERBOSE,
)
JOIN_CHAIN = r"((?:\s*\.\s*join\(\s*\"[^\"]*\"\s*\))+)"
ENV_CALL = re.compile(r'env!\(\s*"[^"]*"\s*\)')
INCLUDE = re.compile(r"\binclude(?:_str|_bytes)?!\s*\(")
FOLDER = re.compile(r'#\[folder\s*=\s*"([^"]*)"')
MANIFEST_BINDING = re.compile(
    r"let\s+(?:mut\s+)?(\w+)\s*=[^;]*CARGO_MANIFEST_DIR[^;]*;"
)
MANIFEST_CHAIN = re.compile(r'CARGO_MANIFEST_DIR\s*"?\)*' + JOIN_CHAIN)
REPO_LITERAL = re.compile(
    r'(?:Path::new|read_to_string|fs::read|File::open|read_dir)\(\s*"([^"]*)"'
)
RUST_SOURCE_PREFIXES = ("src/", "tests/", "desktop/", "scripts/hook-check/", "vendor/")


def string_literals(text):
    return [
        m.group("raw") if m.group("raw") is not None else m.group("plain")
        for m in RUST_TOKEN.finditer(text)
        if m.group("raw") is not None or m.group("plain") is not None
    ]


def tracked_files():
    """The tracked paths, or [] only where there is no checkout to ask.

    A failing git inside a checkout raises, so the coverage tests cannot be
    skipped into passing by a broken repository.
    """
    if not (ROOT / ".git").exists():
        return []
    listing = subprocess.run(
        ["git", "-C", str(ROOT), "ls-files", "-z"], capture_output=True, check=True
    )
    return [p.decode() for p in listing.stdout.split(b"\0") if p]


def balanced(text, open_at):
    """The text between the parenthesis at `open_at` and its partner."""
    depth = 0
    for at in range(open_at, len(text)):
        depth += {"(": 1, ")": -1}.get(text[at], 0)
        if depth == 0:
            return text[open_at + 1 : at]
    return None


def under(base, relative):
    """`relative` joined to `base` as a repository path, None when it leaves the tree."""
    path = posixpath.normpath(posixpath.join(base, relative))
    return None if path.startswith("..") else path


def referenced_paths(source, text, crate_root):
    """Repository paths a Rust source names by literal, and what it names otherwise.

    Resolved statically from include_str!, include_bytes! and include! (relative
    to the file, or to the crate root through CARGO_MANIFEST_DIR), #[folder],
    `.join("...")` chains on CARGO_MANIFEST_DIR or on a variable bound to it, and
    path literals handed to Path::new or a file-reading call, which count only
    where they resolve against the crate root. The second result lists the
    references a regular expression cannot resolve, so they are reported rather
    than guessed.
    """
    found, unresolved = set(), []
    for match in INCLUDE.finditer(text):
        args = balanced(text, match.end() - 1) or ""
        literals = string_literals(ENV_CALL.sub("", args))
        if "OUT_DIR" in args:
            continue
        if "CARGO_MANIFEST_DIR" in args:
            found.add(under(crate_root, "".join(literals).lstrip("/")))
        elif len(literals) == 1 and args.strip().startswith('"'):
            found.add(under(posixpath.dirname(source), literals[0]))
        else:
            unresolved.append(f"{source}: include of {args.strip()[:60]!r}")
    for match in FOLDER.finditer(text):
        found.add(under(crate_root, match.group(1)))
    chains = [m.group(1) for m in MANIFEST_CHAIN.finditer(text)]
    for name in {m.group(1) for m in MANIFEST_BINDING.finditer(text)}:
        chains += re.findall(r"\b" + name + JOIN_CHAIN, text)
    for chain in chains:
        found.add(under(crate_root, "/".join(string_literals(chain))))
    if "CARGO_MANIFEST_DIR" in text and not chains:
        unresolved.append(f"{source}: CARGO_MANIFEST_DIR without a literal join")
    for match in REPO_LITERAL.finditer(text):
        found.add(under(crate_root, match.group(1)))
    return found, unresolved


def mentioned_paths(text, crate_root):
    """Every string literal, read against the crate root and the repository root.

    Broader than referenced_paths and used only where a literal is exactly a
    tracked file: a file handed to a command or joined onto a variable names no
    function the scan could follow, and a message that merely mentions a file
    costs the same toolchain requirement as a read would.
    """
    bases = {crate_root, ""}
    return {under(base, lit) for lit in string_literals(text) for base in bases}


class ScopeFunction(unittest.TestCase):
    def test_rust_paths_are_in(self):
        for path in (
            "src/main.rs",
            "src/web/mod.rs",
            "desktop/src/main.rs",
            "desktop/Cargo.toml",
            "scripts/hook-check/lib.rs",
            "build.rs",
            "Cargo.toml",
            "Cargo.lock",
            ".cargo/config.toml",
            "rust-toolchain.toml",
            "clippy.toml",
            "rustfmt.toml",
            "deny.toml",
            "vendor/livekit-net/src/lib.rs",
            "problem-bank/task-defaults.json",
            "tests/cli.rs",
            "tests/fixtures/sample.json",
            "tests/unit/web/tasks.rs",
        ):
            with self.subTest(path=path):
                self.assertTrue(gate_evidence.in_rust_scope(path))

    def test_non_rust_paths_are_out(self):
        for path in (
            "README.md",
            "docs/contributing-notes.md",
            "scripts/test.sh",
            "scripts/check-pr-text.py",
            "tests/test_check_pr_text.py",
            "tests/support.py",
            "tests/browser/app.spec.js",
            "tests/stubs/codex",
            ".github/workflows/check.yml",
            "package.json",
            "Makefile",
            "srcs/x.rs",
            "build.rs.orig",
            "Cargo.toml.bak",
        ):
            with self.subTest(path=path):
                self.assertFalse(gate_evidence.in_rust_scope(path))

    def test_web_follows_the_one_named_constant(self):
        self.assertFalse(gate_evidence.WEB_IN_RUST_SCOPE)
        self.assertFalse(gate_evidence.in_rust_scope("web/app.js"))
        with mock.patch.object(gate_evidence, "WEB_IN_RUST_SCOPE", True):
            self.assertTrue(gate_evidence.in_rust_scope("web/app.js"))

    def test_files_rust_reads_are_in_even_under_an_out_directory(self):
        for path in ("docs/task-mode.md", "scripts/soak-judge.cjs", "web/lib.js"):
            with self.subTest(path=path):
                self.assertTrue(gate_evidence.in_rust_scope(path))

    def test_github_never_counts_as_in_scope(self):
        self.assertFalse(gate_evidence.in_rust_scope(".github/workflows/x.yml"))

    def test_every_tracked_rust_file_is_in(self):
        tracked = tracked_files()
        if not tracked:
            self.skipTest("not a git checkout")
        rust = [
            p
            for p in tracked
            if p.endswith(".rs") or Path(p).name in ("Cargo.toml", "Cargo.lock")
        ]
        self.assertTrue(rust)
        missed = [p for p in rust if not gate_evidence.in_rust_scope(p)]
        self.assertEqual(missed, [], "Rust files outside the scope list")


class RustReferences(unittest.TestCase):
    maxDiff = None

    def test_the_scan_resolves_each_reference_form(self):
        text = """
        const A: &str = include_str!("../docs/a.md");
        const B: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/b.md"));
        #[folder = "web/"]
        struct Assets;
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let c = root.join("scripts").join("c.sh");
        let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/d.md");
        let e = std::fs::read_to_string("docs/e.md");
        const F: &str = include_str!(concat!(env!("OUT_DIR"), "/gen.rs"));
        const G: &str = include_str!(PATH);
        """
        found, unresolved = referenced_paths("tests/unit/x.rs", text, "")
        self.assertEqual(
            found,
            {
                "tests/docs/a.md",
                "docs/b.md",
                "web",
                "scripts/c.sh",
                "docs/d.md",
                "docs/e.md",
            },
        )
        self.assertEqual(len(unresolved), 1)
        self.assertIn("PATH", unresolved[0])

    def test_quotes_in_comments_and_char_literals_do_not_shift_literals(self):
        text = """
        // it's "quoted" here
        let quote = '"';
        /* "block" */
        let raw = r#"docs/raw.md"#;
        let path = Path::new("docs/after.md");
        """
        self.assertEqual(string_literals(text), ["docs/raw.md", "docs/after.md"])

    def test_a_literal_is_read_against_both_roots(self):
        self.assertEqual(
            mentioned_paths('run("docs/f.md")', "desktop"),
            {"desktop/docs/f.md", "docs/f.md"},
        )

    def test_every_path_a_rust_source_names_is_in_scope(self):
        tracked = tracked_files()
        if not tracked:
            self.skipTest("not a git checkout")
        files = set(tracked)
        crate_roots = sorted(
            (
                posixpath.dirname(p)
                for p in tracked
                if posixpath.basename(p) == "Cargo.toml"
            ),
            key=len,
            reverse=True,
        )
        missed = {}
        members_of = {}
        for source in tracked:
            if not source.endswith(".rs") or not (
                source.startswith(RUST_SOURCE_PREFIXES) or source == "build.rs"
            ):
                continue
            crate_root = next(
                r for r in crate_roots if r == "" or source.startswith(r + "/")
            )
            text = (ROOT / source).read_text(encoding="utf-8")
            named = referenced_paths(source, text, crate_root)[0]
            for path in (named | mentioned_paths(text, crate_root)) - {None, "."}:
                if path in files:
                    members = [path]
                elif path in named:
                    # A directory the Rust code walks or embeds. web/ is decided
                    # by WEB_IN_RUST_SCOPE alone; single web/ files named by a
                    # literal include are in scope whatever it says.
                    if path not in members_of:
                        members_of[path] = [
                            f
                            for f in tracked
                            if f.startswith(path + "/") and not f.startswith("web/")
                        ]
                    members = members_of[path]
                else:
                    continue
                for member in members:
                    if not gate_evidence.in_rust_scope(member):
                        missed.setdefault(member, source)
        self.assertEqual(missed, {}, "paths Rust names that the scope omits")


class ScopeOnly(unittest.TestCase):
    def test_in_when_any_path_is_in(self):
        result = scope("docs/a.md", "src/lib.rs")
        self.assertEqual((result.returncode, result.stdout), (0, b"in\n"))

    def test_out_when_none_is(self):
        result = scope("docs/a.md", "web/app.js", ".github/workflows/x.yml")
        self.assertEqual((result.returncode, result.stdout), (0, b"out\n"))

    def test_an_empty_list_is_out(self):
        result = run(["--scope-only"], b"")
        self.assertEqual((result.returncode, result.stdout), (0, b"out\n"))

    def test_nul_separated_paths_may_hold_newlines(self):
        result = run(["--scope-only", "-z"], b"docs/a\nsrc/b.md\0")
        self.assertEqual(result.stdout, b"out\n")
        result = run(["--scope-only", "-z"], b"docs/a.md\0src/x\ny.rs\0")
        self.assertEqual(result.stdout, b"in\n")

    def test_invalid_input_exits_2(self):
        for data in (
            b"\n",
            b"Cargo.toml\r\n",
            b"/etc/passwd\n",
            b"src/../docs/a.md\n",
            b"..\n",
            b"./src/a.rs\n",
            b"docs//a.md\n",
            b"docs/a.md\n\nsrc/a.rs\n",
            b"\xff\xfe\n",
            b"docs/a\0src/main.rs\n",
        ):
            with self.subTest(data=data):
                result = run(["--scope-only"], data)
                self.assertEqual((result.returncode, result.stdout), (2, b""))
                self.assertTrue(result.stderr)

    def test_bad_usage_exits_2(self):
        for args in ([], ["-z"], ["--needs-maintainer"]):
            with self.subTest(args=args):
                self.assertEqual(run(args, b"").returncode, 2)


TREE = "a" * 40
OTHER_TREE = "b" * 40
PINNED = '[toolchain]\nchannel = "1.100.0"\n'


def receipt(**over):
    one = {
        "lane": "rust",
        "passed": True,
        "skipped": 0,
        "time": "2026-11-16T12:34:56Z",
        "rustc": "rustc 1.100.0 (abcdef123 2026-10-01)",
        "cargo": "cargo 1.100.0 (abcdef123 2026-09-01)",
    }
    one.update(over)
    return one


def note(*receipts, **over):
    body = {
        "version": 1,
        "worktree_tree": TREE,
        "receipts": list(receipts) or [receipt()],
    }
    body.update(over)
    return body


class EvidenceCheck(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.dir = Path(tmp.name)

    def put(self, name, data):
        path = self.dir / name
        if isinstance(data, (dict, list)):
            data = json.dumps(data)
        path.write_bytes(data if isinstance(data, bytes) else data.encode())
        return str(path)

    def args(self, paths=("src/main.rs",), tree=TREE, author="User", base=PINNED):
        args = [
            "--head-tree", tree,
            "--changed-files-file", self.put("changed", "".join(p + "\0" for p in paths)),
            "--author-type", author,
        ]  # fmt: skip
        if base is not None:
            args += ["--base-toolchain-file", self.put("base.toml", base)]
        return args

    def check(self, body=None, head=None, **kwargs):
        args = self.args(**kwargs)
        if body is not None:
            args += ["--receipt-file", self.put("receipt", body)]
        if head is not None:
            args += ["--head-toolchain-file", self.put("head.toml", head)]
        return run(args, b"")

    def fails(self, result, reason=None):
        self.assertEqual(result.returncode, 1, result.stderr + result.stdout)
        lines = result.stderr.decode().splitlines()
        self.assertTrue(lines[0].startswith("check-gate-evidence: "), lines)
        if reason:
            self.assertIn(reason, lines[0])
        text = "\n".join(lines[1:])
        self.assertIn("./scripts/env-check.sh", text)
        self.assertIn("./scripts/test.sh", text)
        self.assertIn("partial commit", text)
        return lines[0]

    def test_receipt_for_the_head_tree_passes(self):
        result = self.check(note())
        self.assertEqual((result.returncode, result.stderr), (0, b""))

    def test_skips_do_not_disqualify(self):
        self.assertEqual(self.check(note(receipt(skipped=3))).returncode, 0)

    def test_all_lane_satisfies(self):
        self.assertEqual(self.check(note(receipt(lane="all"))).returncode, 0)

    def test_one_good_receipt_among_others_passes(self):
        body = note(receipt(lane="portable"), receipt(passed=False), receipt())
        self.assertEqual(self.check(body).returncode, 0)

    def test_unknown_keys_are_ignored(self):
        body = note(receipt(extra=[1]), extra="x")
        self.assertEqual(self.check(body).returncode, 0)

    def test_missing_receipt_fails(self):
        self.fails(self.check(None), "no receipt")

    def test_missing_receipt_file_fails(self):
        args = self.args() + ["--receipt-file", str(self.dir / "absent")]
        self.fails(run(args, b""), "no receipt")

    def test_unreadable_receipt_is_indeterminate(self):
        args = self.args() + ["--receipt-file", str(self.dir)]
        self.assertEqual(run(args, b"").returncode, 2)

    def test_malformed_receipts_fail_with_a_one_line_reason(self):
        valid = json.dumps(note()).encode()
        cases = {
            "not json": b"not json",
            "empty": b"",
            "not utf-8": b"\xff\xfe",
            "not an object": b"[]",
            "trailing data": valid + b" {}",
            "over 16 KiB": valid + b" " * (16385 - len(valid)),
            "duplicate key": b'{"version":1,"version":1}',
            "nan": b'{"version": NaN}',
            "deep": b"[" * 16000,
            "wrong version": note(version=2),
            "version bool": note(version=True),
            "version missing": {"worktree_tree": TREE, "receipts": [receipt()]},
            "bad tree": note(worktree_tree="A" * 40),
            "short tree": note(worktree_tree="a" * 39),
            "tree not str": note(worktree_tree=7),
            "no receipts": note(receipts=[]),
            "receipts not list": note(receipts={}),
            "nine receipts": note(*[receipt()] * 9),
            "receipt not object": note(receipts=[1]),
            "unknown lane": note(receipt(lane="mutants")),
            "lane list": note(receipt(lane=[])),
            "lane object": note(receipt(lane={})),
            "version float": note(version=1.0),
            "version exponent": json.dumps(note()).replace('"version": 1,', '"version": 1e0,').encode(),
            "passed not bool": note(receipt(passed="yes")),
            "passed int": note(receipt(passed=1)),
            "skipped bool": note(receipt(skipped=True)),
            "skipped negative": note(receipt(skipped=-1)),
            "skipped float": note(receipt(skipped=0.0)),
            "time long": note(receipt(time="x" * 33)),
            "time not str": note(receipt(time=5)),
            "rustc long": note(receipt(rustc="r" * 201)),
            "cargo long": note(receipt(cargo="c" * 201)),
            "cargo not str": note(receipt(cargo=None)),
            "key missing": note({"lane": "rust", "passed": True}),
        }  # fmt: skip
        for name, body in cases.items():
            with self.subTest(name):
                reason = self.fails(self.check(body))
                self.assertIn("receipt", reason)

    def test_eight_receipts_are_allowed(self):
        self.assertEqual(self.check(note(*[receipt()] * 8)).returncode, 0)

    def test_empty_strings_are_allowed(self):
        body = note(receipt(rustc="", cargo=""), receipt())
        self.assertEqual(self.check(body).returncode, 0)

    def test_receipt_for_another_tree_fails(self):
        self.fails(self.check(note(worktree_tree=OTHER_TREE)), "another tree")

    def test_sha256_tree_ids_compare_exactly(self):
        tree = "c" * 64
        self.assertEqual(self.check(note(worktree_tree=tree), tree=tree).returncode, 0)

    def test_bad_head_tree_is_indeterminate(self):
        for tree in ("", "A" * 40, "a" * 41, "../x"):
            with self.subTest(tree=tree):
                self.assertEqual(self.check(note(), tree=tree).returncode, 2)

    def test_wrong_lane_fails(self):
        for lane in ("portable", "browser"):
            with self.subTest(lane=lane):
                self.fails(self.check(note(receipt(lane=lane))), "rust or all")

    def test_failed_gate_fails(self):
        self.fails(self.check(note(receipt(passed=False))), "did not pass")

    def test_a_toolchain_mismatch_says_to_rebase_onto_the_base(self):
        # A branch cut before the pin moved produces the same old rustc on every
        # rerun, so running the gate again cannot clear the refusal.
        old = receipt(rustc="rustc 1.99.0 (abcdef123 2026-10-01)")
        self.fails(self.check(note(old)), "rebase")

    def test_rustc_must_match_the_base_channel(self):
        old = receipt(rustc="rustc 1.99.0 (abcdef123 2026-10-01)")
        self.fails(self.check(note(old)), "1.100.0")
        for bad in (
            "",
            "cargo 1.100.0",
            "rustc 1.100.01 (x)",
            "rustc 1.100.0-nightly (x)",
            "rustc 1.100.0",
            "rustc 1.100.0 (",
            "rustc 1.100.0 ()",
            "rustc 1.100.\u0660 (x)",
        ):
            with self.subTest(rustc=bad):
                self.fails(self.check(note(receipt(rustc=bad))), "1.100.0")

    def test_receipt_text_is_never_echoed(self):
        result = self.check(note(receipt(rustc="rustc 9.9.9 EVIL")))
        self.fails(result)
        self.assertNotIn(b"EVIL", result.stderr + result.stdout)

    def test_toolchain_change_compares_to_the_head(self):
        paths = ("rust-toolchain.toml", "src/a.rs")
        new = receipt(rustc="rustc 1.101.0 (abcdef123 2026-11-01)")
        head = '[toolchain]\nchannel = "1.101.0"\n'
        self.assertEqual(self.check(note(new), paths=paths, head=head).returncode, 0)
        self.fails(self.check(note(receipt()), paths=paths, head=head), "1.101.0")

    def test_head_toolchain_is_ignored_when_the_diff_leaves_it_alone(self):
        new = receipt(rustc="rustc 1.101.0 (abcdef123 2026-11-01)")
        head = '[toolchain]\nchannel = "1.101.0"\n'
        self.fails(self.check(note(new), head=head), "1.100.0")

    def test_toolchain_that_cannot_be_read_is_indeterminate(self):
        bad = (
            "",
            "channel = ",
            'channel = "1.100.0"\n',
            "[toolchain]\nchannel = 7\n",
            '[toolchain]\nchannel = "1.100"\n',
            '[toolchain]\nchannel = "stable"\n',
            '[toolchain]\nchannel = "1.100.٠"\n',
            PINNED + "#" * 70000,
        )
        for text in bad:
            with self.subTest(text=text[:30]):
                self.assertEqual(self.check(note(), base=text).returncode, 2)
        self.assertEqual(self.check(note(), base=None).returncode, 2)
        removed = self.check(note(), paths=("rust-toolchain.toml",))
        self.assertEqual(removed.returncode, 2)

    def test_without_tomllib_the_check_is_indeterminate(self):
        with mock.patch.object(gate_evidence, "tomllib", None):
            with self.assertRaises(gate_evidence.Indeterminate):
                gate_evidence.pinned_channel(b'[toolchain]\nchannel = "1.100.0"')

    def test_out_of_scope_paths_pass_without_a_receipt(self):
        paths = ("docs/a.md", "web/app.js", "tests/test_x.py")
        result = self.check(None, paths=paths)
        self.assertEqual((result.returncode, result.stderr), (0, b""))
        self.assertIn(b"no Rust-scope", result.stdout)

    def test_no_changed_files_pass(self):
        self.assertEqual(self.check(None, paths=()).returncode, 0)

    def test_a_github_path_owes_no_receipt_by_itself(self):
        workflow = (".github/workflows/check.yml",)
        self.assertEqual(self.check(None, paths=workflow).returncode, 0)
        self.assertEqual(
            self.check(note(), paths=workflow + ("src/a.rs",)).returncode, 0
        )
        self.assertEqual(self.check(None, paths=workflow + ("src/a.rs",)).returncode, 1)

    def test_bot_is_exempt_and_named(self):
        result = self.check(None, author="Bot")
        self.assertEqual(result.returncode, 0)
        self.assertIn(b"exempt", result.stdout)
        self.assertIn(b"Bot", result.stdout)

    def test_other_authors_are_not_exempt(self):
        for author in ("User", "Organization", "bot", "Mannequin", "x"):
            with self.subTest(author=author):
                self.assertEqual(self.check(None, author=author).returncode, 1)

    def test_bad_changed_path_is_indeterminate(self):
        for paths in (("../x",), ("/etc/passwd",), ("a//b",)):
            with self.subTest(paths=paths):
                self.assertEqual(self.check(note(), paths=paths).returncode, 2)

    def test_changed_list_without_its_final_nul_is_indeterminate(self):
        args = self.args()
        args[args.index("--changed-files-file") + 1] = self.put("cut", "docs/a.md")
        result = run(args, b"")
        self.assertEqual(result.returncode, 2)
        self.assertIn(b"changed-file list", result.stderr)

    def test_a_refused_path_is_not_echoed_in_full(self):
        result = self.check(note(), paths=("../" + "x" * 5000,))
        self.assertEqual(result.returncode, 2)
        self.assertLess(len(result.stderr), 500)

    def test_changed_paths_may_hold_a_newline(self):
        result = self.check(None, paths=("docs/a\nsrc/b.md",))
        self.assertEqual(result.returncode, 0)

    def test_bad_usage_is_indeterminate(self):
        self.assertEqual(run([], b"").returncode, 2)
        self.assertEqual(run(["--head-tree", TREE], b"").returncode, 2)
        for flag in ("--head-tree", "--changed-files-file", "--author-type"):
            args = self.args()
            at = args.index(flag)
            with self.subTest(flag=flag):
                self.assertEqual(run(args[:at] + args[at + 2 :], b"").returncode, 2)
        mixed = ["--scope-only", "--head-tree", TREE]
        self.assertEqual(run(mixed, b"").returncode, 2)

    def test_missing_changed_files_file_is_indeterminate(self):
        args = self.args()
        args[args.index("--changed-files-file") + 1] = str(self.dir / "absent")
        self.assertEqual(run(args, b"").returncode, 2)


if __name__ == "__main__":
    unittest.main()

"""Check the Rust-scope helper of scripts/check-gate-evidence.py."""

import importlib.util
from pathlib import Path
import posixpath
import re
import subprocess
import sys
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


if __name__ == "__main__":
    unittest.main()

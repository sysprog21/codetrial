#!/usr/bin/env python3
"""Hold the Rust tree to the rules clippy cannot state.

Clippy, at `-D warnings` with the `[workspace.lints]` table in Cargo.toml, owns idiom,
ownership, panics and casts. This owns what a lint has no way to ask:

  lint-table   Cargo.toml still denies every lint in REQUIRED_LINTS, and no
               lint group set to allow or warn at an equal or higher priority
               undoes one, so the policy cannot be loosened quietly.
  suppression  No `#[allow]`, no `#![deny(warnings)]`: a suppression is an
               `expect` with a `reason`, and warnings-as-errors is the gate's
               job, since a new toolchain adds warnings and a crate attribute
               turns that into a broken build.
  ignore       Every `#[ignore]` says what it waits for.
  expect       An `expect` message is not empty.
  marker       No TODO, FIXME, XXX or HACK comment, no `ponytail:` label, and
               no comment naming the untracked working documents.
  language     No CJK in Rust or the browser half; fixtures that need
               multi-byte text use Greek or emoji.
  ceiling      New files stay within DEFAULT_CEILING. Existing larger files
               cannot grow beyond their committed size in HEAD. --baseline REF
               also uses the smallest committed size since REF; deletion retires
               an exemption. Limits come from Git, without a generated table.
               An explicit baseline needs complete history and never skips errors.

`--workspace` audits Cargo's own membership for lint inheritance and needs
Cargo; the default run needs none. Exit status 1 when anything is reported.
"""

import argparse
import functools
import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CEILING = 1500

RUST_ROOTS = ("src", "tests", "scripts/hook-check")
RUST_FILES = ("build.rs",)
WEB_ROOT = "web"
WEB_SKIP = ("web/vendor/",)
WEB_SUFFIXES = {".js", ".mjs", ".html", ".css"}

# Lint groups a table can set wholesale; one left at allow or warn can undo a
# required lint beside it.
LINT_GROUPS = {
    "rust": {
        "warnings",
        "future_incompatible",
        "nonstandard_style",
        "rust_2018_idioms",
        "rust_2021_compatibility",
        "rust_2024_compatibility",
        "unused",
        "let_underscore",
        "refining_impl_trait",
    },
    "clippy": {
        "all",
        "cargo",
        "complexity",
        "correctness",
        "nursery",
        "pedantic",
        "perf",
        "restriction",
        "style",
        "suspicious",
    },
}

REQUIRED_LINTS = {
    "rustdoc": {
        "broken_intra_doc_links": "deny",
        "private_intra_doc_links": "deny",
    },
    "rust": {
        "unsafe_code": "forbid",
        "unreachable_pub": "deny",
        "unused_lifetimes": "deny",
        "unused_must_use": "deny",
        "unfulfilled_lint_expectations": "deny",
    },
    "clippy": {
        "all": "deny",
        "dbg_macro": "deny",
        "panic": "deny",
        "todo": "deny",
        "unwrap_used": "deny",
        "allow_attributes": "deny",
        "allow_attributes_without_reason": "deny",
        "clone_on_ref_ptr": "deny",
        "redundant_clone": "deny",
        "string_add": "deny",
        "fn_params_excessive_bools": "deny",
        "unnecessary_wraps": "deny",
        "panic_in_result_fn": "deny",
        "wildcard_imports": "deny",
        "semicolon_if_nothing_returned": "deny",
        "cast_sign_loss": "deny",
        "await_holding_lock": "deny",
        "let_underscore_future": "deny",
        "mem_forget": "deny",
    },
}

ATTRIBUTE = re.compile(r"#\s*!?\s*\[\s*")
ATTRIBUTE_NAME = re.compile(r"\s*(?:r#)?(\w+)")
OPEN_PAREN = re.compile(r"\s*\(")
BARE_IGNORE = re.compile(r'\s*(?:\]|[,)]|=\s*"")')
EMPTY_EXPECT = re.compile(r"\.\s*(?:r#)?expect\s*\(\s*\"\"\s*,?\s*\)")
MARKER = re.compile(
    r"\b(?:TODO|FIXME|XXX|HACK)\b|ponytail:|\b(?:TODO|DONE|CLAUDE)\.md\b"
)
CJK = re.compile(
    "[\u1100-\u11ff\u2e80-\u33ff\u3400-\u4dbf\u4e00-\u9fff"
    "\ua960-\ua97f\uac00-\ud7ff\uf900-\ufaff\ufe10-\ufe1f\ufe30-\ufe4f"
    "\uff00-\uffef\U0001aff0-\U0001b16f\U00020000-\U0003ffff]"
)
# What can hide a rule's text from a line scan: the first alternative to match at
# a position wins, so a raw string is tried before a plain one.
TOKEN = re.compile(
    r"""(?P<line>//[^\n]*)|(?P<block>/\*)|(?P<raw>b?r\#*")"""
    r"""|(?P<str>b?")|(?P<char>'(?:\\.[^']*?|[^\\'])')"""
)


def blank(chunk):
    return "".join(c if c == "\n" else " " for c in chunk)


def block_end(text, start):
    """Where the (nestable) block comment opening at `start` closes."""
    depth, i = 1, start + 2
    while i < len(text) and depth:
        pair = text[i : i + 2]
        depth += (pair == "/*") - (pair == "*/")
        i += 2 if pair in ("/*", "*/") else 1
    return i


def raw_end(text, start, hashes):
    """(end of the contents, end of the literal) of a raw string body at `start`."""
    closing = '"' + "#" * hashes
    found = text.find(closing, start)
    if found < 0:
        return len(text), len(text)
    return found, found + len(closing)


def string_end(text, start):
    """(end of the contents, end of the literal) of a string body at `start`."""
    i = start
    while i < len(text) and text[i] != '"':
        i += 2 if text[i] == "\\" else 1
    return i, i + 1


def split_source(text):
    """Return (code, comments): the source with strings emptied and comments
    cut out, keeping every newline in place, and each comment as (offset, text).

    A string keeps its quotes and loses its contents, so `expect("")` is still
    visible as empty and `"#[allow]"` or `"a // b"` is no longer taken for code
    or for a comment. A lifetime is not a char literal: it has no closing quote.
    """
    code, comments, pos = [], [], 0
    while match := TOKEN.search(text, pos):
        code.append(text[pos : match.start()])
        kind = match.lastgroup
        if kind in ("line", "block"):
            end = match.end() if kind == "line" else block_end(text, match.start())
            comments.append((match.start(), text[match.start() : end]))
            code.append(blank(text[match.start() : end]))
        elif kind == "char":
            end = match.end()
            code.append("' '")
        else:
            body, end = (
                raw_end(text, match.end(), match.group().count("#"))
                if kind == "raw"
                else string_end(text, match.end())
            )
            code.append('"' + blank(text[match.end() : body]) + '"')
        pos = end
    code.append(text[pos:])
    return "".join(code), comments


def arguments(code, start):
    """Top-level comma-separated ranges after an opening delimiter."""
    depth, first = 0, start
    for index in range(start, len(code)):
        char = code[index]
        if char in "([{":
            depth += 1
        elif char in ")]}":
            if depth == 0:
                yield first, index
                return
            depth -= 1
        elif char == "," and depth == 0:
            yield first, index
            first = index + 1


def attribute_findings(code):
    position = 0
    while opening := ATTRIBUTE.search(code, position):
        position = len(code)
        # Nested attribute-shaped tokens belong to this attribute's arguments.
        for _, end in arguments(code, opening.end()):
            position = end + 1
        pending = [opening.end()]
        while pending:
            name = ATTRIBUTE_NAME.match(code, pending.pop())
            if name is None:
                continue
            kind = name.group(1)
            paren = OPEN_PAREN.match(code, name.end())
            args = list(arguments(code, paren.end())) if paren else []
            if kind == "cfg_attr":
                # The first argument is a condition, not an applied attribute.
                pending.extend(start for start, _ in reversed(args[1:]))
            elif kind == "allow" or (
                kind in ("deny", "forbid")
                and any(
                    code[a:b].strip().removeprefix("r#") == "warnings" for a, b in args
                )
            ):
                yield (
                    opening.start(),
                    'suppression: use #[expect(.., reason = "..")], never allow',
                )
            elif kind == "ignore" and BARE_IGNORE.match(code, name.end()):
                yield opening.start(), 'ignore: say what it waits for, #[ignore = ".."]'


@functools.cache
def read(path):
    """Each file is read once per run, whichever rule asks first."""
    return path.read_text()


def rust_files(root):
    for top in RUST_ROOTS:
        yield from sorted((root / top).rglob("*.rs"))
    for name in RUST_FILES:
        if (root / name).exists():
            yield root / name


def web_files(root):
    for path in sorted((root / WEB_ROOT).rglob("*")):
        rel = path.relative_to(root).as_posix()
        if path.suffix in WEB_SUFFIXES and not rel.startswith(WEB_SKIP):
            yield path


def lint_entry(table, name):
    """(level, priority) of a [lints] entry written as a string or a table."""
    entry = table.get(name)
    if isinstance(entry, dict):
        return entry.get("level"), entry.get("priority", 0)
    return entry, 0


def group_overrides(table, group, priority):
    """Groups set to allow or warn at `priority` or higher.

    Cargo applies them after a lint of lower or equal priority, which undoes it.
    """
    for name in table:
        level, other = lint_entry(table, name)
        if (
            name in LINT_GROUPS.get(group, ())
            and level in ("allow", "warn")
            and other >= priority
        ):
            yield name, level, other


def check_lint_table(root):
    manifest = tomllib.loads((root / "Cargo.toml").read_text())
    for group, wanted in REQUIRED_LINTS.items():
        table = manifest.get("workspace", {}).get("lints", {}).get(group, {})
        for name, level in wanted.items():
            actual, priority = lint_entry(table, name)
            if actual != level:
                yield f'Cargo.toml: [workspace.lints.{group}] {name} must be "{level}", is {actual!r}'
                continue
            for other, level_other, priority_other in group_overrides(
                table, group, priority
            ):
                yield (
                    f"Cargo.toml: [workspace.lints.{group}] {other} = "
                    f'"{level_other}" at priority {priority_other} overrides {name}'
                )

    if manifest.get("lints", {}).get("workspace") is not True:
        yield "Cargo.toml: inherit the workspace lint policy with [lints] workspace = true"


def check_workspace(root):
    """Use Cargo's membership rules, including globs and implicit path members."""
    command = [
        "cargo",
        "metadata",
        "--offline",
        "--locked",
        "--no-deps",
        "--format-version",
        "1",
        "--manifest-path",
        str(root / "Cargo.toml"),
    ]
    try:
        result = subprocess.run(
            command, capture_output=True, text=True, check=False, timeout=60
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        yield f"Cargo.toml: cannot inspect workspace membership: {error}"
        return
    if result.returncode:
        yield f"Cargo.toml: cannot inspect workspace membership: {result.stderr.strip()}"
        return
    metadata = json.loads(result.stdout)
    members = set(metadata["workspace_members"])
    packages = [p for p in metadata["packages"] if p["id"] in members]
    if not members or {p["id"] for p in packages} != members:
        yield "Cargo.toml: Cargo metadata did not identify every workspace package"
        return
    for package in packages:
        path = Path(package["manifest_path"])
        manifest = tomllib.loads(path.read_text())
        if manifest.get("lints", {}).get("workspace") is not True:
            rel = (
                path.relative_to(root.resolve())
                if path.is_relative_to(root.resolve())
                else path
            )
            yield f"{rel}: inherit the workspace lint policy with [lints] workspace = true"


def check_rust(root, path):
    rel = path.relative_to(root).as_posix()
    text = read(path)
    code, comments = split_source(text)
    for start, message in attribute_findings(code):
        number = code.count("\n", 0, start) + 1
        yield f"{rel}:{number}: {message}"
    for match in EMPTY_EXPECT.finditer(code):
        number = code.count("\n", 0, match.start()) + 1
        yield f"{rel}:{number}: expect: the message is empty"
    for start, comment in comments:
        for found in MARKER.finditer(comment):
            number = text.count("\n", 0, start + found.start()) + 1
            yield f"{rel}:{number}: marker: {found.group()}"
    yield from check_cjk(rel, text, "use Greek or emoji for multi-byte fixtures")


def check_web(root, path):
    rel = path.relative_to(root).as_posix()
    yield from check_cjk(rel, read(path), "the browser half stays English")


def check_cjk(rel, text, advice):
    if not CJK.search(text):
        return
    for number, line in enumerate(text.splitlines(), 1):
        if CJK.search(line):
            yield f"{rel}:{number}: language: CJK text; {advice}"


def sizes(root):
    return {
        path.relative_to(root).as_posix(): len(read(path).splitlines())
        for path in rust_files(root)
    }


def check_ceilings(current, recorded):
    for rel, lines in sorted(current.items()):
        ceiling = recorded.get(rel, DEFAULT_CEILING)
        if lines > ceiling:
            yield (
                f"{rel}: ceiling: {lines} lines, over {ceiling}; "
                "split it by responsibility, do not raise the ceiling"
            )


def git_ok(root, *args):
    """The output of a git command, or None when git is absent or it fails."""
    try:
        result = subprocess.run(
            ["git", "-C", str(root), *args],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return None
    return result.stdout if result.returncode == 0 else None


def git_ceilings(root, current, baseline=None):
    """Derive allowances from committed paths, including every branch version."""
    head = git_ok(root, "rev-parse", "--verify", "-q", "HEAD^{commit}")
    if head is None and baseline is None:
        return {}

    def required(*args):
        value = git_ok(root, *args)
        if value is None:
            raise ValueError(
                "cannot read Git size baseline; fetch its complete history"
            )
        return value

    if baseline is not None:
        ref = git_ok(
            root,
            "rev-parse",
            "--verify",
            "-q",
            "--end-of-options",
            f"{baseline}^{{commit}}",
        )
        if ref is None:
            raise ValueError(
                f"baseline {baseline} is not fetched; fetch its complete history"
            )
        if (
            head is None
            or required("rev-parse", "--is-shallow-repository").strip() != "false"
        ):
            raise ValueError(
                "Git size baseline needs complete history; fetch --unshallow"
            )
        required("merge-base", "--all", ref.strip(), head.strip())
        history = required(
            "rev-list", "--parents", "--full-history", f"{ref.strip()}..{head.strip()}"
        )
    else:
        ref, history = head, ""

    paths = sorted(path for path, size in current.items() if size > DEFAULT_CEILING)
    if not paths:
        return {}
    trees, counts = {}, {}

    def tree(commit):
        if commit not in trees:
            rows = required(
                "--literal-pathspecs",
                "ls-tree",
                "-rz",
                "--full-tree",
                commit,
                "--",
                *paths,
            )
            trees[commit] = {
                path: meta.split()[2]
                for row in rows.split("\0")
                if row
                for meta, path in [row.split("\t", 1)]
                if path in current and path.endswith(".rs")
            }
        return trees[commit]

    def lines(oid):
        if oid not in counts:
            counts[oid] = len(required("cat-file", "blob", oid).splitlines())
        return counts[oid]

    base, committed = tree(ref.strip()), tree(head.strip())
    limits = {
        path: max(DEFAULT_CEILING, min(lines(base[path]), lines(committed[path])))
        for path, size in current.items()
        if size > DEFAULT_CEILING and path in base and path in committed
    }
    for row in history.splitlines():
        commit, *parents = row.split()
        version = tree(commit)
        for path in limits:
            if path in version:
                limits[path] = min(
                    limits[path], max(DEFAULT_CEILING, lines(version[path]))
                )
            elif any(path in tree(parent) for parent in parents):
                limits[path] = DEFAULT_CEILING
    return limits


def run(root, baseline=None):
    read.cache_clear()
    findings = list(check_lint_table(root))
    for path in rust_files(root):
        findings.extend(check_rust(root, path))
    for path in web_files(root):
        findings.extend(check_web(root, path))
    current = sizes(root)
    try:
        findings.extend(check_ceilings(current, git_ceilings(root, current, baseline)))
    except ValueError as error:
        findings.append(f"ceiling: {error}")
    return findings


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument(
        "--workspace",
        action="store_true",
        help="audit Cargo workspace lint inheritance (requires Cargo and cached dependencies)",
    )
    parser.add_argument(
        "--baseline",
        metavar="REF",
        help="derive size limits from REF and all committed versions since it (needs complete Git history)",
    )
    parser.add_argument("--root", type=Path, default=ROOT, help=argparse.SUPPRESS)
    args = parser.parse_args()
    findings = (
        list(check_lint_table(args.root)) + list(check_workspace(args.root))
        if args.workspace
        else run(args.root, args.baseline)
    )
    for finding in findings:
        print(finding)
    if findings:
        print(f"{len(findings)} Rust style finding(s)", file=sys.stderr)
    return int(bool(findings))


if __name__ == "__main__":
    sys.exit(main())

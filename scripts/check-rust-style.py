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
               turns that into a broken build. An `expect` names one lint, not
               `warnings` or a group: a group's expectation is met by any lint
               in it, so it waives the rest unseen. No `cfg(clippy)` either,
               which hides code from the linter.
  lint-config  clippy.toml sets only keys CLIPPY_KEYS lists with the reason
               each loosens no denied lint, and no rustflags in
               .cargo/config.toml allow a lint or cap lint levels.
  ignore       Every `#[ignore]` says what it waits for.
  expect       An `expect` message is not empty.
  swallow      No `Err(_) => {}` arm and no empty `if let Err(..)`: an error a
               match receives is handled, or the code says why it may drop it
               (`unused_result_ok` holds the `.ok();` form to the same).
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
import importlib.util
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
        "cast_lossless": "deny",
        "await_holding_lock": "deny",
        "let_underscore_future": "deny",
        "mem_forget": "deny",
        "unimplemented": "deny",
        "match_wildcard_for_single_variants": "deny",
        "match_wild_err_arm": "deny",
        "implicit_clone": "deny",
        "inefficient_to_string": "deny",
        "enum_glob_use": "deny",
        "rc_mutex": "deny",
        "needless_continue": "deny",
        "assigning_clones": "deny",
        "stable_sort_primitive": "deny",
        "unnecessary_semicolon": "deny",
        "unused_result_ok": "deny",
        "match_bool": "deny",
        "bool_to_int_with_if": "deny",
        "manual_is_variant_and": "deny",
        "manual_assert_eq": "deny",
        "unnested_or_patterns": "deny",
        "uninlined_format_args": "deny",
        "unreadable_literal": "deny",
        "duration_suboptimal_units": "deny",
    },
}

# An expectation of any of these is met by any lint it covers, so it silences
# every other one unseen; `allow_attributes` itself would waive the rule above.
SILENCERS = (
    LINT_GROUPS["rust"]
    | {f"clippy::{group}" for group in LINT_GROUPS["clippy"]}
    | {"clippy::allow_attributes", "clippy::allow_attributes_without_reason"}
    | {"rustdoc::all"}
)
# The clippy.toml keys this tree sets, and why each loosens nothing the table
# denies. Almost every key widens what some lint accepts, and `clippy::all`
# denies hundreds, so a key is refused until it is read and listed here: a
# list of loosening keys would be out of date with the next clippy.
CLIPPY_KEYS = {
    "allow-unwrap-in-tests": "a test that fails by panicking works as intended",
    "allow-panic-in-tests": "a test that fails by panicking works as intended",
}
LINT_FLAGS = re.compile(r"^(?:-A|--allow\b|--cap-lints\b|--force-warn\b)")
CLIPPY_CFG = re.compile(r"\bclippy\b")

ATTRIBUTE = re.compile(r"#\s*!?\s*\[\s*")
ATTRIBUTE_NAME = re.compile(r"\s*(?:r#)?(\w+)")
OPEN_PAREN = re.compile(r"\s*\(")
BARE_IGNORE = re.compile(r'\s*(?:\]|[,)]|=\s*"")')
EMPTY_EXPECT = re.compile(r"\.\s*(?:r#)?expect\s*\(\s*\"\"\s*,?\s*\)")
EMPTY_ERR = re.compile(r"\bErr\s*\(\s*_\w*\s*\)\s*=>\s*(?:\{\s*\}|\(\s*\))")
IF_LET_ERR = re.compile(r"\bif\s+let\s+Err\s*\(")
# A higher-ranked lifetime binder in a type argument has no loop body.
RHS_TOKEN = re.compile(r"for\s*<(?=\s*')|(?:r#)?\w+|\S")
# A brace in operand position opens a block expression or a macro argument.
BEFORE_OPERAND = frozenset(
    "if match while in else async move const unsafe loop try mut = ! : < > , & | + - * / % ^".split()
)
EMPTY_BLOCK = re.compile(r"\{\s*\}")
ASSIGN = re.compile(r"\s*=(?![=>])")
MARKER = re.compile(
    r"\b(?:TODO|FIXME|XXX|HACK)\b|ponytail:|\b(?:TODO|DONE|CLAUDE)\.md\b"
)
_spec = importlib.util.spec_from_file_location(
    "check_cjk", Path(__file__).with_name("check-cjk.py")
)
_check_cjk = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_check_cjk)
# The CJK ranges belong to the hook, which allows quoted data elsewhere; Rust
# and the browser half refuse every match.
CJK = _check_cjk.CJK

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


def if_let_body(code, start):
    """Locate the branch, skipping delimiter groups and block operands."""
    depth = pending_blocks = angles = 0
    previous, casting = "=", False
    for token in RHS_TOKEN.finditer(code, start):
        word = token.group()
        # In an expression `<` opens generic arguments only after `::` or in
        # the type of an `as` cast; any other `<` or `>` compares, and a brace
        # after a comparison is its operand, not the branch.
        if not depth and word == "as":
            casting = True
        elif not depth and word in "&|":
            casting = False
        if depth:
            depth += (word in "([{") - (word in ")]}")
        elif word == "<" and (previous == ":" or casting):
            angles += 1
        elif word == ">" and angles:
            angles -= 1
            previous = "generic >"
            continue
        elif word in "([":
            depth = 1
        elif word in ("if", "match", "while", "for"):
            pending_blocks += 1
        elif word == "{":
            if previous not in BEFORE_OPERAND:
                if pending_blocks == 0:
                    return token.start()
                pending_blocks -= 1
            depth = 1
        elif word in ";)]}":
            break
        previous = word
    return None


def empty_err_ranges(code):
    for match in EMPTY_ERR.finditer(code):
        yield match.start(), match.start(), match.end()
    for opening in IF_LET_ERR.finditer(code):
        # The pattern's arguments end at the `)` that closes them, the last
        # range arguments() yields.
        ranges = list(arguments(code, opening.end()))
        if not ranges or code[ranges[-1][1]] != ")":
            continue
        if assignment := ASSIGN.match(code, ranges[-1][1] + 1):
            body = if_let_body(code, assignment.end())
            if body is not None and (empty := EMPTY_BLOCK.match(code, body)):
                yield opening.start(), body, empty.end()


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
            if kind in ("cfg", "cfg_attr") and any(
                CLIPPY_CFG.search(code[a:b]) for a, b in args[:1]
            ):
                yield (
                    opening.start(),
                    "suppression: cfg(clippy) hides code from the linter",
                )
            if kind == "cfg_attr":
                # The first argument is a condition, not an applied attribute.
                pending.extend(start for start, _ in reversed(args[1:]))
            elif kind == "expect" and any(
                re.sub(r"\s+", "", code[a:b]).replace("r#", "") in SILENCERS
                for a, b in args
            ):
                yield (
                    opening.start(),
                    "suppression: expect one lint by name, not warnings or a group",
                )
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


def normalized(table, group):
    """A [lints] table keyed by the underscore spelling Cargo treats a dashed
    name as, and the names given twice that way."""
    out, twice = {}, []
    for name, entry in table.items():
        key = name.replace("-", "_")
        if key in out:
            twice.append(f"Cargo.toml: [workspace.lints.{group}] {key} is set twice")
        out[key] = entry
    return out, twice


def check_lint_table(root):
    manifest = tomllib.loads((root / "Cargo.toml").read_text())
    for group, wanted in REQUIRED_LINTS.items():
        table, twice = normalized(
            manifest.get("workspace", {}).get("lints", {}).get(group, {}), group
        )
        yield from twice
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


def rustflags(value):
    """Every rustflags list in a parsed Cargo config, at any depth."""
    if isinstance(value, dict):
        for key, inner in value.items():
            if key == "rustflags":
                yield inner.split() if isinstance(inner, str) else inner
            else:
                yield from rustflags(inner)


def check_lint_config(root):
    # Clippy reads either name, so a key cannot hide under the other.
    for name in ("clippy.toml", ".clippy.toml"):
        path = root / name
        if not path.exists():
            continue
        config = tomllib.loads(path.read_text())
        for key in sorted(set(config) - set(CLIPPY_KEYS)):
            yield (
                f"{name}: lint-config: {key} is not one the checker has read; "
                "list it in CLIPPY_KEYS with why it loosens no denied lint"
            )
    for name in (".cargo/config.toml", ".cargo/config"):
        path = root / name
        if not path.exists():
            continue
        for flags in rustflags(tomllib.loads(path.read_text())):
            for flag in flags:
                if isinstance(flag, str) and LINT_FLAGS.match(flag):
                    yield f"{name}: lint-config: rustflags {flag!r} overrides the lint table"


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
    # The comments are cut out of `code`, so an arm whose body is only the
    # reason it may drop the error reads as empty there; a comment on the
    # arm's own lines is that reason.
    commented = {text.count("\n", 0, start) + 1 for start, _ in comments}
    for start, body, end in empty_err_ranges(code):
        number = code.count("\n", 0, start) + 1
        first = number + code.count("\n", start, body)
        last = first + code.count("\n", body, end)
        if commented.isdisjoint(range(first, last + 1)):
            yield f"{rel}:{number}: swallow: an empty Err arm drops the error; handle it or say why"
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
    findings.extend(check_lint_config(root))
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

#!/usr/bin/env python3
"""Hold every tracked JSON file to what both of its readers agree on.

The same bytes are read by Python (the generators, the tests), by Rust
(serde_json) and by the browser (`JSON.parse`), and each forgives something
different, so a file one of them accepts can mean something else to another:

  syntax     The file is UTF-8 without a byte-order mark, parses as JSON, and
             has no unpaired surrogate escapes or out-of-range numbers.
  constant   No `NaN`, `Infinity` or `-Infinity`. Python's json writes and
             reads them; `JSON.parse` and serde_json refuse the file.
  duplicate  No object names a key twice. Every reader keeps the last value
             without a word, so the first one is dead text that looks live.
  precision  No integer beyond 2**53 - 1 in magnitude. The browser reads it
             as a double and rounds it, while Python and Rust keep it exact.
             A large identifier belongs in a string.
  depth      No more than 127 arrays and objects nested in one another.
             serde_json refuses the 128th as a recursion limit, while Python
             and the browser read far deeper.

Layout is not judged: the webhook fixtures are compact because they stand for
wire bytes, and problem-bank/judges.json keeps a grid on one line. Vendored and
external trees are skipped. Exit status 1 when anything is reported.
"""

import argparse
import json
import math
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SKIP = ("vendor/", "web/vendor/", "externals/", "node_modules/")
SAFE_INTEGER = 2**53 - 1
# serde_json's recursion limit: the 128th nested container fails to parse.
MAX_DEPTH = 127


class Pairs(list):
    """An object as parsed, before duplicates collapse into a dict."""


class Constant(ValueError):
    pass


def refuse_constant(name):
    raise Constant(name)


# One search instead of a Python loop over every character of every key and
# string; re reads the escapes itself, so the pattern stays ASCII.
SURROGATE = re.compile(r"[\ud800-\udfff]")


def has_surrogate(value):
    return SURROGATE.search(value) is not None


def walk(value, where="$", depth=0):
    """(rule, JSON path, detail) for every duplicate key, unpaired surrogate,
    number past the finite range, unsafe integer and nesting past the depth
    serde_json reads."""
    if isinstance(value, (Pairs, list)):
        depth += 1
        if depth > MAX_DEPTH:
            yield "depth", where, f"nested {depth} deep; serde_json reads {MAX_DEPTH}"
            return
    if isinstance(value, Pairs):
        seen = set()
        for key, inner in value:
            if has_surrogate(key):
                yield "syntax", where, f"key {key!r} contains an unpaired surrogate"
            if key in seen:
                yield "duplicate", where, f"key {key!r} is given twice"
            seen.add(key)
            safe_key = key.encode("utf-8", errors="backslashreplace").decode("utf-8")
            yield from walk(inner, f"{where}.{safe_key}", depth)
    elif isinstance(value, list):
        for index, inner in enumerate(value):
            yield from walk(inner, f"{where}[{index}]", depth)
    elif isinstance(value, str) and has_surrogate(value):
        yield "syntax", where, "string contains an unpaired surrogate"
    elif isinstance(value, float) and not math.isfinite(value):
        yield "syntax", where, "number is outside the finite range"
    elif (
        isinstance(value, int)
        and not isinstance(value, bool)
        and abs(value) > SAFE_INTEGER
    ):
        yield "precision", where, f"{value} is past 2**53 - 1; write it as a string"


def check(rel, data):
    """Findings for one file's bytes, as `path: rule: message` strings."""
    if data.startswith(b"\xef\xbb\xbf"):
        return [f"{rel}: syntax: starts with a byte-order mark"]
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError as error:
        return [f"{rel}: syntax: not UTF-8 ({error.reason} at byte {error.start})"]
    try:
        document = json.loads(
            text, object_pairs_hook=Pairs, parse_constant=refuse_constant
        )
    except Constant as error:
        return [f"{rel}: constant: {error} is not JSON outside Python"]
    except json.JSONDecodeError as error:
        return [f"{rel}:{error.lineno}: syntax: {error.msg}"]
    except ValueError:
        # Python refuses to convert an integer of more digits than its limit
        # (4300 by default), which is far past 2**53 in any case.
        return [f"{rel}: precision: an integer too long to read; write it as a string"]
    except RecursionError:
        # Python's own parser gives up near its recursion limit, far past the
        # depth serde_json reads.
        return [
            f"{rel}: depth: nested past what Python reads; serde_json reads {MAX_DEPTH}"
        ]
    return [
        f"{rel}: {rule}: {where}: {detail}" for rule, where, detail in walk(document)
    ]


# A literal or no-glob pathspec mode forced by the caller's environment would
# read `*.json` as a file name, list nothing and pass every file.
PATHSPEC_MODES = (
    "GIT_LITERAL_PATHSPECS",
    "GIT_GLOB_PATHSPECS",
    "GIT_NOGLOB_PATHSPECS",
    "GIT_ICASE_PATHSPECS",
)


def tracked(root):
    listing = subprocess.run(
        ["git", "-c", "core.quotepath=off", "ls-files", "-z", "--", "*.json"],
        cwd=root,
        env={k: v for k, v in os.environ.items() if k not in PATHSPEC_MODES},
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    for rel in sorted(filter(None, listing.split("\0"))):
        if not rel.startswith(SKIP) and (root / rel).is_file():
            yield rel


def run(root):
    findings = []
    for rel in tracked(root):
        findings.extend(check(rel, (root / rel).read_bytes()))
    return findings


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--root", type=Path, default=ROOT)
    args = parser.parse_args()
    try:
        findings = run(args.root)
    except (OSError, subprocess.CalledProcessError) as error:
        print(f"check-json: cannot list the tracked files: {error}", file=sys.stderr)
        return 2
    for finding in findings:
        print(finding)
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())

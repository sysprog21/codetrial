#!/usr/bin/env python3
"""Refuse CJK prose in code: comments, messages and identifiers stay English.

CJK is allowed where it is visibly data: inside a quoted string on one line
('...', "..." or `...`), or between two ``` fence lines, which is how a
multi-line sample is set off in a comment. Markdown and vendored files are
exempt, and so is anything that is not UTF-8 text.

Without arguments the changed index blobs are checked, as pre-commit does.
--commits reads commit IDs on stdin, as pre-push and CI do, and --tree checks
every file in the index, as the gate does.
"""

import argparse
import importlib.util
from pathlib import Path
import re
import sys

_spec = importlib.util.spec_from_file_location(
    "check_secrets", Path(__file__).with_name("check-secrets.py")
)
check_secrets = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(check_secrets)

# Hangul, kana, bopomofo, the ideographs, and the ideographic and full-width
# punctuation that comes with them. scripts/check-rust-style.py reads it too.
CJK = re.compile(
    "[\u1100-\u11ff\u2e80-\u33ff\u3400-\u4dbf\u4e00-\u9fff"
    "\ua960-\ua97f\uac00-\ud7ff\uf900-\ufaff\ufe10-\ufe1f\ufe30-\ufe4f"
    "\uff00-\uffef\U0001aff0-\U0001b16f\U00020000-\U0003ffff]"
)
# Only a single quote needs a word boundary on both sides, so the apostrophe
# in "don't" and a Rust lifetime are not quotes; a string prefix (r, b, f, u)
# may sit in front of one. A double quote or a backtick may touch a word, as
# in return"x", u8"x" or a tagged template. Every quote closes on its own line,
# so an unmatched one hides nothing.
QUOTED = re.compile(
    r"(?:(?<!\w)(?:[rRbBfFuUL]{1,2})?(?=')|(?=[\"`]))"
    r"(['\"`])(?:\\.|(?!\1)[^\\])*\1(?!(?<=')\w)"
)
# A triple-quoted string on one line is a docstring far more often than data,
# so its body is judged as prose, quotes inside it still marking data.
TRIPLE = re.compile(r"('''|\"\"\")(.*?)\1")
# A fence is a line of its own: optional comment leader, then three or more
# backticks and no other backtick run, as in a Markdown code block. Backticks
# anywhere else, a "```" string included, open nothing.
FENCE = re.compile(r"\s*(?:(?:#+|//+|/\*+|\*+|--|;+|<!--)\s*)?`{3,}[^`]*")
EXEMPT = re.compile(r"\.(?:md|markdown)$|^(?:web/)?vendor/")


def findings(text):
    """Yield (line number, line) for each line with CJK outside data."""
    # Newlines alone: str.splitlines() also breaks on a form feed and U+2028,
    # which would shift every line number after one.
    lines = [line.rstrip("\r") for line in text.split("\n")]
    fences = [n for n, line in enumerate(lines, 1) if FENCE.fullmatch(line)]
    # Pairs only, so a fence that never closes is a stray ``` and hides nothing.
    sample = set()
    for start, end in zip(fences[::2], fences[1::2]):
        sample.update(range(start + 1, end))
    for number, line in enumerate(lines, 1):
        if number not in sample and CJK.search(
            QUOTED.sub("", TRIPLE.sub(r" \2 ", line))
        ):
            yield number, line.strip()


def inspect(label, data):
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        return False
    # Nearly every file holds no CJK at all, and one search of the whole blob
    # costs a fraction of judging it line by line.
    if "\0" in text or not CJK.search(text):
        return False
    hits = list(findings(text))
    for number, line in hits:
        print(f"{label}:{number}: {line}", file=sys.stderr)
    return bool(hits)


def tree():
    """Yield (path, blob ID) for every regular file in the index."""
    # ls-files lists only the current directory, relative to it, so run it from
    # the top: a gate started in a subdirectory still sees every path, and the
    # vendored exemption still matches.
    top = check_secrets.git("rev-parse", "--show-toplevel").rstrip(b"\n")
    for entry in check_secrets.git("-C", top, "ls-files", "-s", "-z").split(b"\0"):
        if not entry:
            continue
        meta, path = entry.split(b"\t", 1)
        mode, oid, _ = meta.split()
        if mode != b"160000":
            yield path, oid.decode()


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--commits", action="store_true", help="read commit IDs on stdin")
    mode.add_argument("--tree", action="store_true", help="check every indexed file")
    args = parser.parse_args()
    if args.tree:
        targets = (("staged", path, oid) for path, oid in tree())
    else:
        commits = sys.stdin.read().split() if args.commits else None
        targets = check_secrets.targets(commits)
    failed = False
    with check_secrets.Blobs() as blobs:
        for context, path, oid in targets:
            name = path.decode("utf-8", "surrogateescape")
            if EXEMPT.search(name):
                continue
            label = name if context == "staged" else f"{context} {name}"
            failed |= inspect(label, blobs.read(oid))
    if failed:
        print(
            "CJK found outside quotes, backticks or a ``` block. Comments,"
            " docstrings, messages and names are English. Test data that needs"
            " CJK stays in a quoted string ('...', \"...\" or `...`) or between"
            " two ``` lines.",
            file=sys.stderr,
        )
    return int(failed)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (check_secrets.InspectionError, OSError):
        print("CJK check failed: cannot inspect Git objects.", file=sys.stderr)
        sys.exit(2)

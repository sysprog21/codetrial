#!/usr/bin/env python3
"""Build the note that carries a gate run's evidence with a push.

    gate-note.py build --receipts DIR --tree TREE

DIR is the folder scripts/test.sh keeps receipts in, one subfolder per git tree
id. The note body goes to stdout, one JSON object of at most 16 KiB: the tree
and, for each receipt of that tree, the few fields the reader needs. Exit 0
when the note was built, 1 when no passing `rust` or `all` receipt names TREE
(the reason goes to stderr), 2 on bad usage or a note that cannot be sized.

The receipt is a file on the contributor's machine, so the note proves nothing
a person could not write by hand. It saves a maintainer the question of whether
a gate was run, and CI and the maintainer's approval stay what binds. Receipts
are data: only the keys below are copied, only if they have the expected shape,
and nothing from one is ever put on a command line.
"""

import argparse
import json
import os
import re
import stat
import sys

NOTE_VERSION = 1
MAX_NOTE_BYTES = 16384
MAX_RECEIPTS = 8
# A receipt embeds the environment report, a few kilobytes; far more than that
# is not something scripts/test.sh wrote.
MAX_RECEIPT_BYTES = 256 * 1024
LANES = frozenset(("rust", "all", "portable", "browser"))
# What scripts/test.sh writes; the note's schema allows any string up to 32.
TIME_SHAPE = re.compile(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ")
MAX_TOOL_CHARS = 200
# The lanes that build the Rust crate, and so vouch for a Rust change.
EVIDENCE_LANES = frozenset(("rust", "all"))
TREE_ID = re.compile(r"(?:[0-9a-f]{40}|[0-9a-f]{64})")


class NoteError(Exception):
    pass


class NoEvidence(NoteError):
    pass


def tool_string(value):
    """The toolchain line, empty when the lane has none; None when malformed."""
    if value is None:
        return ""
    if isinstance(value, str) and len(value) <= MAX_TOOL_CHARS:
        # A lone surrogate escape parses but cannot be written as UTF-8.
        try:
            value.encode("utf-8")
        except UnicodeEncodeError:
            return None
        return value
    return None


def clean_receipt(raw, tree):
    """The note's entry for one receipt, or None when it is not a valid one."""
    if not isinstance(raw, dict) or raw.get("worktree_tree") != tree:
        return None
    lane, passed, skipped = raw.get("lane"), raw.get("passed"), raw.get("skipped")
    stamp = raw.get("time")
    rustc, cargo = tool_string(raw.get("rustc")), tool_string(raw.get("cargo"))
    if (
        not isinstance(lane, str)
        or lane not in LANES
        or not isinstance(passed, bool)
        or isinstance(skipped, bool)
        or not isinstance(skipped, int)
        or skipped < 0
        or not isinstance(stamp, str)
        or not TIME_SHAPE.fullmatch(stamp)
        or rustc is None
        or cargo is None
    ):
        return None
    return {
        "lane": lane,
        "passed": passed,
        "skipped": skipped,
        "time": stamp,
        "rustc": rustc,
        "cargo": cargo,
    }


def read_receipt(path):
    """The parsed JSON of a regular, bounded file, or None."""
    try:
        info = os.lstat(path)
        if not stat.S_ISREG(info.st_mode) or info.st_size > MAX_RECEIPT_BYTES:
            return None
        with open(path, "rb") as handle:
            return json.loads(handle.read(MAX_RECEIPT_BYTES + 1))
    # A deeply nested file raises RecursionError, not ValueError.
    except (OSError, ValueError, RecursionError):
        return None


def dumps(note):
    return json.dumps(note, separators=(",", ":"), ensure_ascii=False)


def encode(note):
    text = dumps(note)
    if len(text.encode("utf-8")) > MAX_NOTE_BYTES:
        raise NoteError(f"the note is larger than {MAX_NOTE_BYTES} bytes")
    return text


def vouches(entry):
    return entry["passed"] and entry["lane"] in EVIDENCE_LANES


def build_note(receipts_dir, tree):
    """The note for TREE; NoEvidence when nothing passing vouches for it."""
    folder = os.path.join(receipts_dir, tree)
    try:
        names = sorted(n for n in os.listdir(folder) if n.endswith(".json"))
    except OSError:
        names = []
    entries = []
    for name in names:
        entry = clean_receipt(read_receipt(os.path.join(folder, name)), tree)
        if entry is not None:
            entries.append(entry)

    # The receipt that vouches goes first, so neither the cap nor the size
    # limit drops it.
    entries.sort(key=vouches, reverse=True)
    if not entries or not vouches(entries[0]):
        raise NoEvidence(f"no passing rust or all receipt for tree {tree}")
    note = {"version": NOTE_VERSION, "worktree_tree": tree, "receipts": entries}
    del entries[MAX_RECEIPTS:]
    while len(entries) > 1 and len(dumps(note).encode("utf-8")) > MAX_NOTE_BYTES:
        entries.pop()
    return note


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    commands = parser.add_subparsers(dest="command", required=True)
    build = commands.add_parser("build", help="print the note for one tree")
    build.add_argument("--receipts", required=True)
    build.add_argument("--tree", required=True)
    try:
        args = parser.parse_args(argv)
    except SystemExit as err:
        return 2 if err.code else 0
    # The tree names a folder, so it is checked before it is joined to a path.
    if not TREE_ID.fullmatch(args.tree):
        print("gate-note: --tree is not a git tree id", file=sys.stderr)
        return 2
    try:
        text = encode(build_note(args.receipts, args.tree))
    except NoteError as err:
        print(f"gate-note: {err}", file=sys.stderr)
        return 1 if isinstance(err, NoEvidence) else 2
    sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

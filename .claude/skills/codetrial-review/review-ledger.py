#!/usr/bin/env python3
"""Place the Pending findings of a review ledger against a tree of files.

    review-ledger.py LEDGER ROOT [--json]
    review-ledger.py LEDGER ROOT --write [--head SHA]

The ledger is the Markdown file interactive.md describes. ROOT is where the
files are read from: the bundle's head/ directory, or a checkout; a path that
leaves it, through `..` or a symlink, reads as a missing file. Pending is the
section whose heading, at any level, starts with the word; text inside a code
fence is never a heading or a bullet. A fence opens on a line that starts with
three or more backticks or tildes (a backtick opener may not have another
backtick after it, so an inline code span is not one) and closes on a line of
the same character, at least as long, with nothing after it. A fence opened on
a list item is not recognised, and an indented opener counts at any depth. A bullet is
`- [P1] path on line N-M: text - "quote" (at SHA)`: the quote is everything
after the first ` - "` and is taken from the first line of the range. Each
Pending bullet is placed again by the text of that line:

  exact      the recorded line still holds the quote, or the bullet names no
             line and its file or directory is there
  moved      another line holds it; the nearest wins, the lower number on a tie,
             and a range keeps its length
  stale      no line holds it, or the file or directory is missing or outside
             ROOT
  unchecked  the bullet names a line but has no quote to look for
  unparsed   a Pending list item that is not a bullet of this form (no
             `[P0]`..`[P3]` level, a path with a space, an indented or
             numbered item), or one inside a code fence that is never closed,
             and that fence itself; it is reported and left as it is, so the
             counts add up

Without --write nothing is changed. With it, moved bullets get their new line
numbers, and with --head the exact and moved ones that name a line take SHA as
the commit their numbers count in; a review of a dirty tree has no such commit
and leaves it out. Stale and unchecked bullets stay for a person to read.
Everything else, line endings and trailing spaces included, is written back as
read, through a temporary file renamed over the ledger (the file a symlinked
ledger points at, so the link stays). Exit 0, or 2 on a bad
argument, a ROOT that is not a directory, or an unreadable ledger.
"""

import argparse
import json
import os
import re
import stat
import sys
from collections import Counter

SHA = r"[0-9a-f]{7,40}"
SECTION = re.compile(r"^#+\s+(.*?)\s*$")
PENDING = re.compile(r"pending\b", re.IGNORECASE)
ITEM = re.compile(r"^\s*(?:[-*+]|\d+[.)])\s")
FENCE = re.compile(r"^\s*(`{3,}|~{3,})(.*)$")
BULLET = re.compile(
    r"^- \[P[0-3]\] (?P<path>\S+?)"
    r"(?: on line (?P<a>\d+)(?:-(?P<b>\d+))?)?: (?P<rest>.*)$"
)
AT = re.compile(r" \(at %s\)$" % SHA)
ON_LINE = re.compile(r" on line \d+(?:-\d+)?")


def parse_bullet(line):
    """Return path, start, end and quote of a Pending bullet, else None."""
    m = BULLET.match(line)
    if not m:
        return None
    rest = AT.sub("", m.group("rest"))
    cut = rest.find(' - "')
    quote = rest[cut + 4 : -1] if cut != -1 and rest.endswith('"') else ""
    if not quote.strip():
        quote = ""
    start = int(m.group("a")) if m.group("a") else None
    end = max(int(m.group("b")), start) if m.group("b") else start
    return {"path": m.group("path"), "start": start, "end": end, "quote": quote or None}


def resolve(root, path):
    """The real path of `path` under `root`, or None when it leaves the root."""
    try:
        base = os.path.realpath(root)
        full = os.path.realpath(os.path.join(base, path))
        return full if os.path.commonpath((base, full)) == base else None
    except ValueError:
        return None


def read_lines(root, path):
    full = resolve(root, path)
    if full is None:
        return None
    try:
        with open(full, encoding="utf-8", errors="replace") as f:
            return f.read().split("\n")
    except (OSError, ValueError):
        return None


def present(root, path):
    full = resolve(root, path)
    return full is not None and os.path.exists(full)


def place(b, lines):
    """Return (state, line) for one parsed bullet."""
    start, quote = b["start"], b["quote"]
    if start is None:
        return "exact", start
    if quote is None:
        return "unchecked", start
    if lines is None:
        return "stale", start

    hits = [n for n, text in enumerate(lines, 1) if quote in text]
    if start in hits:
        return "exact", start
    if not hits:
        return "stale", start
    return "moved", min(hits, key=lambda n: (abs(n - start), n))


def rewrite(line, b, new, head):
    """The bullet with its numbers and commit replaced, the rest untouched."""
    if b["start"] is None:
        return line
    span = str(new)
    if b["end"] != b["start"]:
        span += "-%d" % (new + b["end"] - b["start"])
    line = ON_LINE.sub(" on line " + span, line, count=1)
    if head is None:
        return line
    stamp = " (at %s)" % head
    return AT.sub(stamp, line) if AT.search(line) else line + stamp


def run(ledger_path, root, write, head):
    with open(ledger_path, encoding="utf-8", newline="") as f:
        original = f.read().split("\n")
    out, results, files, pending = [], [], {}, False
    fence, opener, in_fence = None, "", []
    for raw in original:
        line = raw.rstrip()
        tail = raw[len(line) :]
        m = FENCE.match(line)
        if m and m.group(1)[0] == "`" and "`" in m.group(2):
            m = None
        if m and fence is None:
            fence, opener, in_fence = m.group(1), line.strip(), []
        elif (
            m
            and fence
            and m.group(1)[0] == fence[0]
            and len(m.group(1)) >= len(fence)
            and not m.group(2).strip()
        ):
            fence = None
        if fence is not None:
            if pending and ITEM.match(line):
                in_fence.append(line.strip())
            out.append(raw)
            continue
        h = SECTION.match(line)
        if h:
            pending = PENDING.match(h.group(1)) is not None
        b = parse_bullet(line) if pending else None
        if b is None:
            if pending and ITEM.match(line):
                results.append(unparsed(line))
            out.append(raw)
            continue
        if b["start"] is None:
            state, new = ("exact" if present(root, b["path"]) else "stale"), None
        else:
            if b["path"] not in files:
                files[b["path"]] = read_lines(root, b["path"])
            state, new = place(b, files[b["path"]])
        results.append(
            {"path": b["path"], "state": state, "from": b["start"], "to": new}
        )
        if write and state in ("exact", "moved"):
            line = rewrite(line, b, new, head)
        out.append(line + tail)
    if fence is not None:
        results.append(unparsed("unclosed code fence: " + opener))
        results.extend(unparsed(line) for line in in_fence)
    if write and out != original:
        replace_file(ledger_path, "\n".join(out))
    return results


def unparsed(line):
    return {
        "path": None,
        "state": "unparsed",
        "from": None,
        "to": None,
        "line": line.strip(),
    }


def replace_file(path, text):
    """Write beside the ledger, then rename over it, so an interruption leaves one or the other.

    A symlinked ledger is written through: the file it points at is replaced and the link stays."""
    path = os.path.realpath(path)
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8", newline="") as f:
        f.write(text)
        f.flush()
        os.fsync(f.fileno())
    os.chmod(tmp, stat.S_IMODE(os.stat(path).st_mode))
    os.replace(tmp, path)


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("ledger")
    ap.add_argument("root")
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--write", action="store_true")
    ap.add_argument("--head", help="commit the numbers count in; stamped with --write")
    args = ap.parse_args(argv)
    if args.head is not None and not re.fullmatch(SHA, args.head):
        print("review-ledger.py: --head needs 7 to 40 hex digits", file=sys.stderr)
        return 2
    if not os.path.isdir(args.root):
        print("review-ledger.py: %s is not a directory" % args.root, file=sys.stderr)
        return 2
    try:
        results = run(args.ledger, args.root, args.write, args.head)
    except (OSError, ValueError) as e:
        print("review-ledger.py: %s" % e, file=sys.stderr)
        return 2
    if args.json:
        print(json.dumps(results, indent=2))
        return 0
    for r in results:
        if r["state"] == "unparsed":
            print("unparsed: %s" % r["line"][:70])
            continue
        moved = "" if r["from"] == r["to"] else " %s -> %s" % (r["from"], r["to"])
        print("%s %s%s" % (r["state"], r["path"], moved))
    n = Counter(r["state"] for r in results)
    print(
        "exact %d, moved %d, stale %d, unchecked %d, unparsed %d"
        % (n["exact"], n["moved"], n["stale"], n["unchecked"], n["unparsed"])
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

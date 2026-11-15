#!/usr/bin/env python3
"""Rewrap a commit message body at 72 columns, joining ragged prose first.

A message arrives wrapped at whatever width its writer had: a model's single
long line, a paragraph broken at column 40, or the shape of a sentence that was
edited since. Breaking only the long lines, as an editor's fill does, keeps the
rest of that shape, so prose is joined back into one paragraph and packed again.

Only prose moves. Every line this cannot be sure is prose passes through byte
for byte: the subject, a comment, a code fence and what is inside it, an
indented block, a quote, a table, a run of "Key: value" rows, the closing line,
the trailer block, and everything below a scissors line. Corrupting a trailer
or a sample costs more than leaving one paragraph ragged.

The output converges: a second run changes nothing. That holds only if no break
forges a line the next run, git or the commit-msg hook would read as something
else, so a break never puts a comment character, a list marker, a quote, a
table bar, a "Key:" word, a closing keyword with its issue, or a rule at the
start of a line. It breaks one word earlier instead, and when that cannot work
either it lets the line overflow, which the hook then names: an over-long line
is recoverable, a line turned into a trailer is not.

    commit-reflow.py FILE      rewrite FILE in place
    commit-reflow.py -         filter standard input to standard output
    commit-reflow.py --check FILE|-   exit 1 if the text would change; no writes
"""

import argparse
import re
import subprocess
import sys

WIDTH = 72

FENCE = re.compile(r"^\s{0,3}(```|~~~)")
# Any comment character: core.commentChar may be set, and under "auto" git
# picks one this pass cannot know.
SCISSORS = re.compile(r"^(\S{1,8} ?)?-{8,}\s*>8\s*-{8,}")
INDENTED = re.compile(r"^(    |\t)")
QUOTE = re.compile(r"^\s{0,3}>")
TABLE = re.compile(r"^\s*\|.*\|\s*$")
# Box drawing, arrows between words, and runs of rule characters are a
# diagram's, and the ASCII rule refuses the Unicode ones anyway.
ART = re.compile(r"[+|][-=]{2,}|[-=]{2,}[+|]|-->|<--|^\s*[-=*_]{3,}\s*$")
LIST_ITEM = re.compile(r"^(\s{0,3})([-*+]|\d{1,3}[.)])\s+\S")
# A trailer as this log writes them: a hyphenated key, or one of the one-word
# keys git and GitHub use. A one-word key alone is not enough, since
# "Mechanical: ruff format over the Python" is a sentence that happens to open
# with a label.
TRAILER = re.compile(
    r"^([A-Za-z0-9]+(-[A-Za-z0-9]+)+"
    r"|change-id|rechained|fixes|closes|link|bug|cc|refs?):\s",
    re.IGNORECASE,
)
LABEL = re.compile(r"^[A-Z][A-Za-z0-9 ./()-]{0,30}:\s+\S")
# The hook's own reference shapes (scripts/git-commit-msg.awk, "ref"): a
# number, another repository's number, or the link to an issue or pull request.
ISSUE = (
    r"(?:(?:[\w.-]+/[\w.-]+)?#\d+"
    r"|https?://\S+/(?:issues|pull)/\d+(?:#[\w-]*)?)"
)
# The words that close or reference an issue, as the hook reads them.
KEYWORD = r"(close[sd]?|fix(e[sd])?|resolve[sd]?|refs?|references?)"
CLOSING_LINE = re.compile(
    r"^" + KEYWORD + r"\b:?\s+" + ISSUE + r"([\s,;&]+(and\s+)?" + ISSUE + r")*\s*\.?$",
    re.IGNORECASE,
)
CLOSING_WORD = re.compile(r"^" + KEYWORD + r":?$", re.IGNORECASE)


def item_lead(marker):
    """An item's first-line prefix, the marker and one space; its length is the
    hang its continuation lines take, in classify() and in pack() alike."""
    return marker.group(1) + marker.group(2) + " "


def forged(first, second, comment_char):
    """Whether a line starting "first second" reads as anything but prose."""
    if first.startswith(comment_char) or first.startswith((">", "|", "```", "~~~")):
        return True
    if re.fullmatch(r"[-*+]|\d{1,3}[.)]", first):
        return True
    if re.fullmatch(r"[-=*_~]{3,}", first):
        return True
    # "Signed-off-by:", "How:", "Note:": a trailer, a heading the hook refuses,
    # or one row of a label run on the next pass.
    if re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9 ./()-]*:", first):
        return True
    return bool(CLOSING_WORD.match(first) and re.match(ISSUE + r"\b", second or ""))


def width(text):
    """Columns as the hook counts them: bytes, in the C locale."""
    return len(text.encode("utf-8", "surrogateescape"))


def pack(text, comment_char, indent="", hang=""):
    """Greedy packing with guarded breaks, as described in the module text."""
    words = text.split()
    lines = []
    current = []
    used = 0

    def limit():
        return WIDTH - width(indent if not lines else hang)

    for index, word in enumerate(words):
        if not current:
            current, used = [word], width(word)
            continue
        if used + 1 + width(word) <= limit():
            current.append(word)
            used += 1 + width(word)
            continue
        after = words[index + 1] if index + 1 < len(words) else ""
        if not forged(word, after, comment_char):
            lines.append(current)
            current, used = [word], width(word)
            continue
        # Move words down with this one, the fewest that give a safe opener,
        # as long as something is left behind and the moved words still fit:
        # "#ffffff, #d6d9df, #252a32" has no safe break between its members.
        for keep in range(len(current) - 1, 0, -1):
            moved = current[keep:] + [word]
            fits = width(" ".join(moved)) <= WIDTH - width(hang)
            if fits and not forged(moved[0], moved[1], comment_char):
                lines.append(current[:keep])
                current = moved
                used = width(" ".join(moved))
                break
        else:
            current.append(word)
            used += 1 + width(word)
    if current:
        lines.append(current)
    out = []
    for number, line in enumerate(lines):
        out.append((indent if number == 0 else hang) + " ".join(line))
    return out


def line_kind(line, previous, in_item, comment_char):
    """The kind of one line outside a fence or a scissors cut. The order of
    the checks is the decision: each runs only on what the ones above let by."""
    if not line.strip():
        return "blank"
    if line.startswith(comment_char):
        return "keep"
    item = LIST_ITEM.match(line)
    # Inside an item, a line indented to the item's text is its continuation:
    # the hang under "10. " is four columns, and read as a code block it would
    # end the item on the next run. That is also the hang pack() writes. A line
    # indented any other way is the item's sample ("call_one()" under
    # "- Document the example:") and stays as written.
    if in_item and not item and line[:1] in (" ", "\t"):
        lead = len(line) - len(line.lstrip(" "))
        return "prose" if lead == in_item and line[lead] != "\t" else "keep"
    # Outside a list a number opens one in the middle of a paragraph only at 1,
    # as in Markdown: "57. The test now hides" is a sentence that wrapped after
    # a number.
    if item and not (
        not in_item
        and previous == "prose"
        and item.group(2)[0].isdigit()
        and not re.match(r"1[.)]", item.group(2))
    ):
        return "item"
    # A trailer under the prose with no blank line above it is still somebody's
    # credit; joined into the sentence it is lost whatever git makes of it.
    if (
        INDENTED.match(line)
        or QUOTE.match(line)
        or TABLE.match(line)
        or ART.search(line)
        or CLOSING_LINE.match(line.strip())
        or TRAILER.match(line)
    ):
        return "keep"
    return "prose"


def classify(body, comment_char):
    """One kind per line, in source order. The order of the checks matters:
    ranges first, since inside a fence or below the scissors nothing else is
    asked, then the shapes that are layout, and prose last."""
    kinds = []
    fence = None
    cut = False
    in_item = 0
    for line in body:
        if cut:
            kinds.append("keep")
            continue
        if fence:
            kinds.append("keep")
            if line.strip().startswith(fence):
                fence = None
            continue
        if SCISSORS.match(line):
            cut = True
            kinds.append("keep")
            continue
        match = FENCE.match(line)
        if match:
            fence = match.group(1)
            kinds.append("keep")
            continue
        kind = line_kind(line, kinds[-1] if kinds else "blank", in_item, comment_char)
        if kind == "item":
            in_item = len(item_lead(LIST_ITEM.match(line)))
        elif kind != "prose":
            in_item = 0
        kinds.append(kind)

    # Decided over whole paragraphs, after the lines: a label is a row only in
    # a run of rows, and a trailer only in the block that ends the message.
    start = 0
    while start < len(body):
        if kinds[start] == "blank":
            start += 1
            continue
        end = start
        while end < len(body) and kinds[end] != "blank":
            end += 1
        rows = [i for i in range(start, end) if kinds[i] != "keep"]
        labels = [i for i in rows if LABEL.match(body[i])]
        if len(labels) >= 2 and len(labels) == len(rows):
            for i in rows:
                kinds[i] = "keep"
        start = end
    last = len(body)
    while last > 0 and kinds[last - 1] == "blank":
        last -= 1
    first = last
    while first > 0 and kinds[first - 1] != "blank":
        first -= 1
    block = range(first, last)
    if block and all(
        TRAILER.match(body[i])
        or body[i][:1] in (" ", "\t")
        or CLOSING_LINE.match(body[i].strip())
        or kinds[i] == "keep"
        for i in block
    ):
        for i in block:
            kinds[i] = "keep"
    return kinds


def reflow(message, comment_char="#"):
    """The message with its prose and list items rewrapped; see the module
    text for what passes through. A trailing newline is kept as it came."""
    ending = "\n" if message.endswith("\n") else ""
    # Trailing blank lines go whatever whitespace they hold, as git's cleanup
    # drops them; keeping one as an empty line would change on the next run.
    lines = re.sub(r"\s+\Z", "", message.replace("\r\n", "\n")).split("\n")
    if len(lines) < 2:
        return message
    subject, body = lines[0], lines[1:]
    kinds = classify(body, comment_char)
    runs = units(body, kinds, comment_char)

    # The guards in pack() keep a break from forging a line, but a join can
    # still change what a run reads as: a lone "1." joined to the next line is
    # an item, "Fixes" and "#12" join into the closing line, "Signed-off-by:"
    # and its value join into a trailer. So the result is read back, and a run
    # that no longer reads as what it was goes out as it came. Freezing a run
    # can change how its neighbors read in turn, so this repeats until
    # nothing more freezes, which takes at most one round per run.
    frozen = set()
    while True:
        out = []
        spans = []
        for number, (kind, start, end, wrapped) in enumerate(runs):
            spans.append((len(out), kind))
            out += body[start:end] if number in frozen else wrapped
        read = classify(out, comment_char)
        bad = {
            number
            for number, (first, kind) in enumerate(spans)
            if kind in ("prose", "item")
            and number not in frozen
            and not reads_as(read, first, len(runs[number][3]), kind)
        }
        if not bad:
            break
        frozen |= bad
    return "\n".join([subject] + out) + ending


def reads_as(read, first, count, kind):
    """Whether the lines of one rewrapped run classify as that run did."""
    expected = [kind] + ["prose"] * (count - 1)
    return read[first : first + count] == expected


def units(body, kinds, comment_char):
    """The body as runs of (kind, start, end, rewrapped lines): a layout or
    blank line on its own, a prose paragraph, or a list item with its
    continuation lines."""
    runs = []
    i = 0
    while i < len(body):
        kind = kinds[i]
        start = i
        if kind in ("keep", "blank"):
            line = body[i].rstrip() if kind == "blank" else body[i]
            runs.append((kind, i, i + 1, [line]))
            i += 1
            continue
        # An item's marker is its first-line prefix rather than a word, so a
        # first word too long to share its line cannot strand it: a bare "+"
        # is not an item on the next run. Its continuation, like a paragraph's,
        # is the prose that follows, up to a blank, an item or a layout line.
        lead = ""
        text = []
        if kind == "item":
            lead = item_lead(LIST_ITEM.match(body[i]))
            text.append(body[i].strip()[len(lead.strip()) :])
            i += 1
        while i < len(body) and kinds[i] == "prose":
            text.append(body[i].strip())
            i += 1
        packed = pack(" ".join(text), comment_char, lead, " " * len(lead))
        runs.append((kind, start, i, packed))
    return runs


def main():
    parser = argparse.ArgumentParser(
        description=__doc__.split("\n\n")[0],
        epilog=__doc__.split("\n\n", 1)[1],
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("--check", action="store_true")
    parser.add_argument("file")
    options = parser.parse_args()
    # Bytes in and out, so a message that is not UTF-8 (the hook names this
    # command after refusing one for that) is rewrapped rather than a crash.
    if options.file == "-":
        raw = sys.stdin.buffer.read()
    else:
        with open(options.file, "rb") as handle:
            raw = handle.read()
    text = raw.decode("utf-8", "surrogateescape")
    result = reflow(text, comment_char())
    if options.check:
        return int(result != text)
    out = result.encode("utf-8", "surrogateescape")
    if options.file == "-":
        sys.stdout.buffer.write(out)
    elif result != text:
        with open(options.file, "wb") as handle:
            handle.write(out)
    return 0


def comment_char():
    """git's core.commentChar, whose lines pass through; "#" outside a
    repository, as git defaults it."""
    result = subprocess.run(
        ["git", "config", "--get", "core.commentChar"],
        capture_output=True,
        text=True,
        check=False,
    )
    char = result.stdout.strip()
    return "#" if char in ("", "auto") else char


if __name__ == "__main__":
    sys.exit(main())

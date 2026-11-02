"""Read a unified git diff as files, hunks and numbered lines, and say what each
changed line is: an edit, a move, or only whitespace.

One reading serves the review page and the evidence a reviewer is given, so
the two cannot disagree about what changed. It is plain line and token
matching over the diff text; it knows no language and builds no syntax tree,
and a change it cannot classify stays an ordinary edit.

What it promises:

- Nothing is dropped. Every line of every hunk is kept, in order, with its old
  and new line number, and `check()` compares the result with the hunk headers
  and with the counts the manifest carries. A mismatch is reported, never
  smoothed over, so the page can say it cannot vouch for what it shows.
- Classification only adds labels. A line marked moved or whitespace-only is
  still there; a view may dim it, and the evidence for a reviewer may fold it,
  but the label is the whole of the claim.
- It is deterministic. The same diff gives the same result: ties break by
  position, never by hash order.

The thresholds are this tool's defaults, not findings.
"""

import difflib
import re
from collections import defaultdict

MOVE_MIN_LINES = 3  # consecutive lines that must reappear elsewhere to be a move
MOVE_MIN_CHARS = 40  # ...and carry at least this much text between them
STRONG = 3  # alphanumeric characters that make a line worth matching on its own
MOVE_CANDIDATES = 40  # a line that appears more often than this is boilerplate
PAIR_MIN = 0.5  # similarity at which a deleted and an added line are one edit
PAIR_WINDOW = 8  # added lines looked at for each deleted line
SIMILAR_MIN = 0.6  # share of lines two units must keep to be one unit, moved and edited
SIMILAR_MARGIN = (
    0.1  # by which the best candidate must beat the next, or it is ambiguous
)
UNIT_CANDIDATES = 5  # removed units compared with each added one
UNIT_MAX = 800  # lines; a larger unit is matched by shape or not at all
LINE_MAX = 2000  # characters; past this a line is marked whole, not by word
WHOLE_LINE = 0.8  # share changed above which marking words says nothing

TOKEN = re.compile(r"\w+|\s+|[^\w\s]")
HUNK = re.compile(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@ ?(.*)$")
ESCAPE = re.compile(r"\\(?:([0-7]{3})|(.))")
SIMPLE = {"a": 7, "b": 8, "f": 12, "n": 10, "r": 13, "t": 9, "v": 11}


def unquote(path):
    """A path git wrote in C style: "a\\303\\251b" is the bytes of "aéb"."""
    if not (len(path) >= 2 and path[0] == path[-1] == '"'):
        return path
    out = bytearray()
    pos = 0
    body = path[1:-1]
    while pos < len(body):
        found = ESCAPE.match(body, pos)
        if not found:
            out += body[pos].encode()
            pos += 1
        elif found.group(1):
            out.append(int(found.group(1), 8))
            pos = found.end()
        else:
            letter = found.group(2)
            out += bytes([SIMPLE[letter]]) if letter in SIMPLE else letter.encode()
            pos = found.end()
    return out.decode(errors="replace")


def header_paths(rest):
    """The two paths of `diff --git a/X b/Y`; equal ones are split by length."""
    if rest.startswith('"'):
        match = re.match(r'("(?:[^"\\]|\\.)*") (.*)$', rest)
        if match:
            old, new = unquote(match.group(1)), unquote(match.group(2))
            return old[2:] if old.startswith("a/") else old, (
                new[2:] if new.startswith("b/") else new
            )
    if not rest.startswith("a/"):
        return rest, rest
    for found in re.finditer(" b/", rest):
        old, new = rest[2 : found.start()], rest[found.end() :]
        if old == new:
            return old, new
    first = rest.find(" b/")
    return (rest[2:first], rest[first + 3 :]) if first > 0 else (rest, rest)


def parse(text):
    """The files of a unified diff, each with its hunks and numbered lines.

    Returns (files, problems). A hunk that does not end where its header said
    it would is a problem, kept in the result as far as it was read.
    """
    files, problems = [], []
    current = hunk = None
    old_left = new_left = 0
    rows = text.split("\n")
    if rows and rows[-1] == "":
        rows.pop()
    for number, line in enumerate(rows, 1):
        if hunk is not None and (old_left > 0 or new_left > 0):
            kind = line[:1]
            if kind in (" ", "+", "-") or (line == "" and old_left and new_left):
                body = line[1:] if line else ""
                row = {"k": {" ": "c", "+": "a", "-": "d"}.get(kind, "c"), "t": body}
                row["o"] = row["n"] = None
                if row["k"] != "a":
                    row["o"] = hunk["old_start"] + hunk["old_seen"]
                    hunk["old_seen"] += 1
                    old_left -= 1
                if row["k"] != "d":
                    row["n"] = hunk["new_start"] + hunk["new_seen"]
                    hunk["new_seen"] += 1
                    new_left -= 1
                hunk["lines"].append(row)
                continue
            if kind == "\\":
                if hunk["lines"]:
                    hunk["lines"][-1]["eol"] = False
                continue
            problems.append(
                f"{current['path']}: hunk at line {number} ended early "
                f"({old_left} old and {new_left} new lines were still expected)"
            )
            old_left = new_left = 0
        if line.startswith("\\") and hunk is not None and hunk["lines"]:
            hunk["lines"][-1]["eol"] = False
            continue
        if line.startswith("diff --git "):
            old, new = header_paths(line[len("diff --git ") :])
            current = {
                "path": new,
                "old_path": old,
                "status": "modified",
                "binary": False,
                "hunks": [],
                "additions": 0,
                "deletions": 0,
            }
            files.append(current)
            hunk = None
        elif current is None:
            continue
        elif hunk is None or (old_left <= 0 and new_left <= 0):
            if hunk is not None and line[:1] in (" ", "+", "-"):
                problems.append(
                    f"{current['path']}: line {number} follows a hunk that was "
                    "already complete by its header"
                )
            elif line.startswith("new file mode"):
                current["status"] = "added"
            elif line.startswith("deleted file mode"):
                current["status"] = "deleted"
            elif line.startswith("rename from "):
                current["old_path"] = unquote(line[len("rename from ") :])
                current["status"] = "renamed"
            elif line.startswith("rename to "):
                current["path"] = unquote(line[len("rename to ") :])
                current["status"] = "renamed"
            elif line.startswith(("Binary files ", "GIT binary patch")):
                current["binary"] = True
            else:
                found = HUNK.match(line)
                if found:
                    old_n = int(found.group(2) if found.group(2) is not None else 1)
                    new_n = int(found.group(4) if found.group(4) is not None else 1)
                    hunk = {
                        "old_start": int(found.group(1)),
                        "old_count": old_n,
                        "new_start": int(found.group(3)),
                        "new_count": new_n,
                        "section": found.group(5),
                        "old_seen": 0,
                        "new_seen": 0,
                        "lines": [],
                    }
                    old_left, new_left = old_n, new_n
                    current["hunks"].append(hunk)
    if hunk is not None and (old_left > 0 or new_left > 0):
        problems.append(
            f"{current['path']}: the diff ends inside a hunk "
            f"({old_left} old and {new_left} new lines were still expected)"
        )
    for item in files:
        for part in item["hunks"]:
            part.pop("old_seen")
            part.pop("new_seen")
            for row in part["lines"]:
                if row["k"] == "a":
                    item["additions"] += 1
                elif row["k"] == "d":
                    item["deletions"] += 1
    return files, problems


def normal(text):
    return " ".join(text.split())


def tokens(text):
    return TOKEN.findall(text)


def spans(old, new):
    """Where two lines differ, as character ranges on each, or None for 'all'."""
    if len(old) > LINE_MAX or len(new) > LINE_MAX:
        return None, None
    a, b = tokens(old), tokens(new)
    start_a, start_b = [0], [0]
    for piece in a:
        start_a.append(start_a[-1] + len(piece))
    for piece in b:
        start_b.append(start_b[-1] + len(piece))
    left, right = [], []
    matcher = difflib.SequenceMatcher(None, a, b, autojunk=False)
    for op, a1, a2, b1, b2 in matcher.get_opcodes():
        if op == "equal":
            continue
        if a2 > a1:
            left.append([start_a[a1], start_a[a2]])
        if b2 > b1:
            right.append([start_b[b1], start_b[b2]])

    def share(found, size):
        return sum(e - s for s, e in found) / size if size else 0

    if share(left, len(old)) > WHOLE_LINE and share(right, len(new)) > WHOLE_LINE:
        return None, None
    return left, right


def mark_edits(hunk):
    """Pair each deleted line with the added line it became, and mark the words.

    Lines in a change block are paired in order, each deleted line with the most
    similar added line at or after the last one used, so the pairs never cross.
    """
    lines = hunk["lines"]
    index = 0
    while index < len(lines):
        if lines[index]["k"] == "c":
            index += 1
            continue
        start = index
        while index < len(lines) and lines[index]["k"] != "c":
            index += 1
        deleted = [(i, lines[i]) for i in range(start, index) if lines[i]["k"] == "d"]
        added = [(i, lines[i]) for i in range(start, index) if lines[i]["k"] == "a"]
        floor = 0
        for d_position, old in deleted:
            best, best_ratio = None, PAIR_MIN
            for slot in range(floor, min(len(added), floor + PAIR_WINDOW)):
                new = added[slot][1]
                matcher = difflib.SequenceMatcher(
                    None, old["t"], new["t"], autojunk=False
                )
                if (
                    matcher.real_quick_ratio() <= best_ratio
                    or matcher.quick_ratio() <= best_ratio
                ):
                    continue
                ratio = matcher.ratio()
                if ratio > best_ratio:
                    best, best_ratio = slot, ratio
            if best is None:
                continue
            floor = best + 1
            a_position, new = added[best]
            old["p"], new["p"] = a_position, d_position
            if normal(old["t"]) == normal(new["t"]):
                old["ws"] = new["ws"] = True
                continue
            left, right = spans(old["t"], new["t"])
            if left is not None:
                old["w"], new["w"] = left, right


def strong(text):
    return sum(1 for ch in text if ch.isalnum()) >= STRONG


def mark_moves(files):
    """Mark runs of lines that were removed in one place and added in another.

    A run must be at least MOVE_MIN_LINES lines and MOVE_MIN_CHARS characters
    of text, must match whole lines after whitespace is ignored, and must have
    exactly one source: text that appears in several removed places could have
    come from any, so it stays an ordinary edit. Runs in the same change block
    are edits in place, not moves.
    """
    added, removed = [], []
    for fi, item in enumerate(files):
        for hi, hunk in enumerate(item["hunks"]):
            block = run = None
            for li, row in enumerate(hunk["lines"]):
                if row["k"] == "c":
                    block = run = None
                    continue
                if block is None:
                    block = (fi, hi, li)
                if run is None or run[0] != row["k"]:
                    run = (row["k"], len(added) + len(removed))
                entry = {
                    "loc": (fi, hi, li),
                    "norm": normal(row["t"]),
                    "run": (fi, hi, run),
                    "block": block,
                }
                (added if row["k"] == "a" else removed).append(entry)
    by_text = defaultdict(list)
    for position, entry in enumerate(removed):
        if strong(entry["norm"]):
            by_text[entry["norm"]].append(position)
    found = []
    for i, entry in enumerate(added):
        if not strong(entry["norm"]):
            continue
        sources = by_text.get(entry["norm"], [])
        if len(sources) > MOVE_CANDIDATES:
            continue
        for j in sources:
            if removed[j]["block"] == entry["block"]:
                continue
            before = i > 0 and j > 0 and added[i - 1]["run"] == entry["run"]
            if (
                before
                and removed[j - 1]["run"] == removed[j]["run"]
                and added[i - 1]["norm"] == removed[j - 1]["norm"]
            ):
                continue  # inside a longer run that started earlier
            length = 1
            while (
                i + length < len(added)
                and j + length < len(removed)
                and added[i + length]["run"] == entry["run"]
                and removed[j + length]["run"] == removed[j]["run"]
                and added[i + length]["norm"] == removed[j + length]["norm"]
            ):
                length += 1
            chars = sum(len(added[i + n]["norm"]) for n in range(length))
            if length >= MOVE_MIN_LINES and chars >= MOVE_MIN_CHARS:
                found.append((length, i, j))
    # Several sources for one run, of the same length, is ambiguity: refuse it.
    starts = defaultdict(list)
    for length, i, j in found:
        starts[(i, length)].append(j)
    claimed_added, claimed_removed = set(), set()
    for length, i, j in sorted(found, key=lambda f: (-f[0], f[1], f[2])):
        if len(starts[(i, length)]) != 1:
            continue
        rows_a = range(i, i + length)
        rows_d = range(j, j + length)
        if any(r in claimed_added for r in rows_a) or any(
            r in claimed_removed for r in rows_d
        ):
            continue
        claimed_added.update(rows_a)
        claimed_removed.update(rows_d)
        first_a, first_d = added[i]["loc"], removed[j]["loc"]
        row_a = files[first_a[0]]["hunks"][first_a[1]]["lines"][first_a[2]]
        row_d = files[first_d[0]]["hunks"][first_d[1]]["lines"][first_d[2]]
        for n in range(length):
            fi, hi, li = added[i + n]["loc"]
            files[fi]["hunks"][hi]["lines"][li]["m"] = 1
            fi, hi, li = removed[j + n]["loc"]
            files[fi]["hunks"][hi]["lines"][li]["m"] = 1
        row_a["mr"] = {
            "path": files[first_d[0]]["old_path"],
            "line": row_d["o"],
            "len": length,
        }
        row_d["mr"] = {
            "path": files[first_a[0]]["path"],
            "line": row_a["n"],
            "len": length,
        }


# A unit is a run of changed lines that stands on its own: a function, a class,
# a block. The diff holds no file, so a unit is found from the lines alone, by
# where one starts at the shallowest indentation. That is a guess at syntax, not
# a parse, and a unit it gets wrong only fails to match.
UNIT_START = re.compile(
    r"\s*(?:@|(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?"
    r"(?:def|fn|func|function|class|struct|enum|trait|impl|mod|interface|type)\b"
    r"|(?:export|public|private|protected|static)\b)"
)
LEXEME = re.compile(
    r"""[A-Za-z_]\w*|\d[\w.]*|"(?:\\.|[^"\\\n])*"|'(?:\\.|[^'\\\n])*'|\S"""
)
COMMENT = re.compile(r"//.*$|#(?![\[!]).*$")
KEYWORDS = frozenset(
    """
    def class fn func function let mut const var pub impl struct enum trait mod
    interface type return if else elif for while in is not and or import from use
    as with try except catch finally raise throw new true false none null self
    this async await yield match case switch break continue pass static public
    private protected export unsafe where lambda
    """.split()
)


def segment(rows):
    """Split a run of changed lines into units; each is a list of row indexes."""
    texts = [row["t"] for row in rows]
    depth = [len(t) - len(t.lstrip()) for t in texts if t.strip()]
    if not depth:
        return [list(range(len(rows)))]
    base = min(depth)
    starts, decorated = [], False
    for index, text in enumerate(texts):
        if not text.strip() or len(text) - len(text.lstrip()) != base:
            continue
        if not UNIT_START.match(text):
            continue
        if text.lstrip().startswith("@"):
            if not decorated:
                starts.append(index)
            decorated = True
        else:
            if not decorated:
                starts.append(index)
            decorated = False
    if not starts or starts[0] != 0:
        starts.insert(0, 0)
    bounds = starts + [len(rows)]
    return [list(range(a, b)) for a, b in zip(bounds, bounds[1:]) if b > a]


def lexemes(texts):
    out = []
    for text in texts:
        out.extend(LEXEME.findall(COMMENT.sub("", text)))
    return out


def is_name(token):
    return (token[0].isalpha() or token[0] == "_") and token not in KEYWORDS


def shape(tokens):
    """The tokens with every name replaced by its order of first appearance.

    Two units with the same shape are the same code with its names changed, and
    the same code written out differently: that is all it says.
    """
    names, out = {}, []
    for token in tokens:
        out.append(
            "$%d" % names.setdefault(token, len(names)) if is_name(token) else token
        )
    return tuple(out)


def renames(old, new):
    """The names that changed between two token lists of one shape, or None."""
    forward, backward = {}, {}
    for a, b in zip(old, new):
        if a != b and is_name(a) and is_name(b):
            if forward.setdefault(a, b) != b or backward.setdefault(b, a) != a:
                return None
    return sorted(forward.items())


def runs_of(lines):
    """Maximal runs of changed lines of one kind that were not already moved."""
    run, kind = [], None
    for index, row in enumerate(lines + [{"k": "c"}]):
        if row["k"] == kind and not row.get("m") and kind != "c":
            run.append(index)
            continue
        if run:
            yield kind, run
        run, kind = [], None
        if row["k"] != "c" and not row.get("m"):
            kind, run = row["k"], [index]


def collect_units(files):
    removed, added = [], []
    for fi, item in enumerate(files):
        for hi, hunk in enumerate(item["hunks"]):
            lines = hunk["lines"]
            block = None
            for index, row in enumerate(lines):
                if row["k"] == "c":
                    block = None
                elif block is None:
                    block = (fi, hi, index)
                row["_block"] = block
            for kind, run in runs_of(lines):
                for part in segment([lines[i] for i in run]):
                    rows = [run[i] for i in part]
                    # Blank lines at either end separate units; they are not one.
                    while rows and not lines[rows[0]]["t"].strip():
                        rows.pop(0)
                    while rows and not lines[rows[-1]]["t"].strip():
                        rows.pop()
                    norm = [normal(lines[i]["t"]) for i in rows]
                    if (
                        len(rows) < MOVE_MIN_LINES
                        or sum(map(len, norm)) < MOVE_MIN_CHARS
                    ):
                        continue
                    texts = [lines[i]["t"] for i in rows]
                    tokens = lexemes(texts)
                    (removed if kind == "d" else added).append(
                        {
                            "loc": [(fi, hi, i) for i in rows],
                            "texts": texts,
                            "norm": norm,
                            "tokens": tokens,
                            "shape": shape(tokens),
                            "block": lines[rows[0]]["_block"],
                        }
                    )
    for item in files:
        for hunk in item["hunks"]:
            for row in hunk["lines"]:
                row.pop("_block", None)
    return removed, added


def pair_units(removed, added):
    """Choose the removed unit each added unit came from, where there is one.

    First by shape (same code, names changed), then by how many lines they keep.
    A unit with more than one equally good source is left alone.
    """
    chosen, taken = [], set()
    by_shape = defaultdict(list)
    for j, unit in enumerate(removed):
        by_shape[unit["shape"]].append(j)
    added_shapes = defaultdict(int)
    for unit in added:
        added_shapes[unit["shape"]] += 1
    for i, unit in enumerate(added):
        sources = [
            j
            for j in by_shape.get(unit["shape"], [])
            if removed[j]["block"] != unit["block"]
        ]
        if len(sources) == 1 and added_shapes[unit["shape"]] == 1:
            chosen.append((i, sources[0], "shape"))
            taken.add(sources[0])
    done = {i for i, _, _ in chosen}
    index = defaultdict(set)
    for j, unit in enumerate(removed):
        if j not in taken and len(unit["norm"]) <= UNIT_MAX:
            for line in set(unit["norm"]):
                if strong(line):
                    index[line].add(j)
    for i, unit in enumerate(added):
        if i in done or len(unit["norm"]) > UNIT_MAX:
            continue
        shared = defaultdict(int)
        for line in set(unit["norm"]):
            for j in index.get(line, ()):
                shared[j] += 1
        close = sorted(
            (-count, j)
            for j, count in shared.items()
            if count >= 2 and j not in taken and removed[j]["block"] != unit["block"]
        )[:UNIT_CANDIDATES]
        scored = sorted(
            (
                (
                    -difflib.SequenceMatcher(
                        None, unit["norm"], removed[j]["norm"], autojunk=False
                    ).ratio(),
                    j,
                )
                for _, j in close
            )
        )
        if not scored or -scored[0][0] < SIMILAR_MIN:
            continue
        if len(scored) > 1 and scored[1][0] - scored[0][0] < SIMILAR_MARGIN:
            continue
        chosen.append((i, scored[0][1], "similar"))
        taken.add(scored[0][1])
    return chosen


def mark_unit(files, old, new, how):
    """Mark what an added unit kept from a removed one, and what it changed.

    Lines that are the same once whitespace is ignored are the move. Lines that
    differ stay edits, with their changed words marked, so a unit that moved and
    was edited on the way does not hide the edit.
    """
    rows_d = [files[fi]["hunks"][hi]["lines"][li] for fi, hi, li in old["loc"]]
    rows_a = [files[fi]["hunks"][hi]["lines"][li] for fi, hi, li in new["loc"]]
    if how == "shape" and len(rows_a) == len(rows_d):
        pairs = list(zip(range(len(rows_d)), range(len(rows_a))))
    else:
        pairs = []
        matcher = difflib.SequenceMatcher(
            None, old["norm"], new["norm"], autojunk=False
        )
        for op, d1, d2, a1, a2 in matcher.get_opcodes():
            if op == "equal" or (op == "replace" and d2 - d1 == a2 - a1):
                pairs.extend(zip(range(d1, d2), range(a1, a2)))
    changed = 0
    for d, a in pairs:
        if old["norm"][d] == new["norm"][a]:
            rows_d[d]["m"] = rows_a[a]["m"] = 1
            continue
        changed += 1
        left, right = spans(rows_d[d]["t"], rows_a[a]["t"])
        if left is not None:
            rows_d[d]["w"], rows_a[a]["w"] = left, right
    found = (
        renames(old["tokens"], new["tokens"]) if old["shape"] == new["shape"] else None
    )
    first_d, first_a = old["loc"][0], new["loc"][0]
    notes = {
        "how": "renamed" if found else "edited" if changed else "same",
        "changed": changed,
        "renames": [list(pair) for pair in (found or [])[:4]],
    }
    row_d, row_a = rows_d[0], rows_a[0]
    row_a["mr"] = {
        "path": files[first_d[0]]["old_path"],
        "line": row_d["o"],
        "len": len(rows_a),
        **notes,
    }
    row_d["mr"] = {
        "path": files[first_a[0]]["path"],
        "line": row_a["n"],
        "len": len(rows_d),
        **notes,
    }


def mark_unit_moves(files):
    removed, added = collect_units(files)
    for i, j, how in pair_units(removed, added):
        mark_unit(files, removed[j], added[i], how)


def annotate(files):
    for item in files:
        for hunk in item["hunks"]:
            mark_edits(hunk)
    mark_moves(files)
    mark_unit_moves(files)
    for item in files:
        changed = moved = blank = 0
        for hunk in item["hunks"]:
            for row in hunk["lines"]:
                if row["k"] == "c":
                    continue
                changed += 1
                moved += 1 if row.get("m") else 0
                blank += 1 if row.get("ws") else 0
        item["moved"], item["whitespace"] = moved, blank
        item["label"] = (
            "moved only"
            if changed and moved == changed
            else "whitespace only"
            if changed and blank == changed
            else ""
        )
    return files


def check(files, problems, manifest_files=None, complete=True):
    """Everything about the reading that does not add up, as plain sentences.

    The hunk headers say how many lines each hunk holds, and the manifest says
    how many each file gained and lost; both are counted independently of the
    parse. If they disagree the page must not present the result as the diff.
    """
    out = list(problems)
    for item in files:
        for hunk in item["hunks"]:
            old = sum(1 for r in hunk["lines"] if r["k"] != "a")
            new = sum(1 for r in hunk["lines"] if r["k"] != "d")
            if (old, new) != (hunk["old_count"], hunk["new_count"]):
                out.append(
                    f"{item['path']}: a hunk holds {old} old and {new} new lines, "
                    f"its header says {hunk['old_count']} and {hunk['new_count']}"
                )
    if manifest_files:
        known = {f["path"]: f for f in manifest_files if isinstance(f, dict)}
        for item in files:
            entry = known.get(item["path"])
            if not entry or item["binary"]:
                continue
            for key in ("additions", "deletions"):
                if isinstance(entry.get(key), int) and entry[key] != item[key]:
                    out.append(
                        f"{item['path']}: the manifest counts {entry[key]} {key}, "
                        f"the diff has {item[key]}"
                    )
        if complete:
            shown = {item["path"] for item in files}
            for path in sorted(known):
                if path not in shown:
                    out.append(f"{path}: listed in the manifest, absent from the diff")
    return out


def reading(text, manifest_files=None, complete=True):
    """Parse, annotate and check a diff; the one call the callers make."""
    files, problems = parse(text)
    annotate(files)
    return {
        "files": files,
        "problems": check(files, problems, manifest_files, complete),
        "totals": {
            "files": len(files),
            "additions": sum(f["additions"] for f in files),
            "deletions": sum(f["deletions"] for f in files),
            "moved": sum(f["moved"] for f in files),
            "whitespace": sum(f["whitespace"] for f in files),
        },
    }


def compact(item):
    """One file as text for a reviewer: moves and whitespace folded, the rest whole.

    A moved run is named once at each end and its text is not repeated; a
    whitespace-only edit is one line. Everything else is the diff as written,
    so what is folded is stated and nothing is left for the reviewer to guess.
    """
    out = [f"diff --git a/{item['old_path']} b/{item['path']}"]
    if item["status"] != "modified":
        out.append(f"# {item['status']}")
    if item["binary"]:
        out.append("# binary file")
    for hunk in item["hunks"]:
        out.append(
            f"@@ -{hunk['old_start']},{hunk['old_count']} "
            f"+{hunk['new_start']},{hunk['new_count']} @@ {hunk['section']}".rstrip()
        )
        for row in hunk["lines"]:
            where = row.get("mr")
            if where:
                verb = "moved here from" if row["k"] == "a" else "moved to"
                how = {
                    "renamed": ", names changed: "
                    + ", ".join(f"{a} -> {b}" for a, b in where.get("renames", [])),
                    "edited": f", {where.get('changed', 0)} lines changed (shown below)",
                }.get(where.get("how"), "")
                out.append(
                    f"# {where['len']} lines {verb} {where['path']}:{where['line']}{how}"
                    " (unchanged text not repeated)"
                )
            if row.get("m"):
                continue
            if row.get("ws"):
                if row["k"] == "d":
                    continue
                out.append(f"~{row['t']}  # whitespace-only change")
                continue
            out.append({"c": " ", "a": "+", "d": "-"}[row["k"]] + row["t"])
    return "\n".join(out) + "\n"

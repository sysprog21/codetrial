"""Read a reference note written in the explainer draft format.

The reviewer's reference note may carry a flow diagram, a comparison table and
quoted code besides prose. The draft format follows answer-me-with-html
(`A -> B: label`, `-->` for a dashed edge, pipe tables, `src=` and `lines=` on
a code block), cut to what a review needs and checked against the pinned diff.
The browser only draws what this module returns; it parses nothing itself.

What it promises:

- Every citation is a line the pinned diff holds. `@path:42` is a line of the
  new file, `@path:o42` a line of the old one, `@path:42-47` a range. A line
  outside the diff's hunks is a problem, because the page can only show what
  the diff does.
- Code is never typed. A `code` block names `src=` and `lines=`, its body stays
  empty, and the rows come from the diff, so a snippet cannot differ from it.
- A solid edge (`->`) needs a citation. An edge the reviewer inferred is
  written `-->` and drawn dashed and labeled as inferred, so a picture does
  not present a guess as a fact.
- A note with no special syntax is prose and stays valid.

It is plain text matching and knows no language. The limits keep one note
readable on one screen; they are this tool's defaults, not findings.
"""

import re

MAX_BLOCKS = 24
MAX_NODES = 12
MAX_EDGES = 16
MAX_LABEL = 48
MAX_ROWS = 8
MAX_COLS = 5
MAX_CODE_LINES = 30
MAX_CODE_BLOCKS = 3
MAX_RANGE = 40

REF = re.compile(r"(?<!\w)@(?P<path>[^\s:@]+):(?P<old>o)?(?P<a>\d+)(?:-(?P<b>\d+))?")
FENCE = re.compile(r"^```\s*(?P<kind>\w*)\s*(?P<args>.*?)\s*$")
ARG = re.compile(r"(\w+)=(\S+)")
SEPARATOR = re.compile(r"^\|?\s*:?-{3,}:?\s*(\|\s*:?-{3,}:?\s*)*\|?$")
# A pipe that is not inside a backtick span: `a|b` is one cell.
CELL_SPLIT = re.compile(r"\|(?=(?:[^`]*`[^`]*`)*[^`]*$)")
ARROW = re.compile(r"\s*(-->|->)\s*")
DIRECTIONS = ("LR", "TB")


class Reader:
    def __init__(self, files, trusted=True):
        self.files = files
        self.trusted = trusted
        self.problems = []
        self.number = 0
        self.held = {}  # (path, side) -> the lines of that side the diff holds

    def problem(self, text):
        self.problems.append(f"line {self.number}: {text}")

    def lines_of(self, path, side):
        """The numbered lines of one side of a file that the diff holds.

        Built once for each file and side: a note may cite a file hundreds of times.
        """
        if (path, side) in self.held:
            return self.held[(path, side)]
        out = self.held[(path, side)] = {}
        items = [item for item in self.files if item["path"] == path]
        if not items and side == "old":
            items = [item for item in self.files if item.get("old_path") == path]
        for item in items:
            for hunk in item["hunks"]:
                for row in hunk["lines"]:
                    at = row["o"] if side == "old" else row["n"]
                    if at is not None:
                        out[at] = row
        return out

    def cite(self, path, side, first, last):
        """A citation as data, or None after saying why it does not stand."""
        where = f"{path}:{'o' if side == 'old' else ''}{first}"
        if not self.trusted:
            self.problem(
                f"{where}: the diff could not be read line by line, "
                "so a citation cannot be checked"
            )
            return None
        if last < first or last - first >= MAX_RANGE:
            self.problem(f"{where}: use a range of 1 to {MAX_RANGE} lines")
            return None
        held = self.lines_of(path, side)
        missing = [at for at in range(first, last + 1) if at not in held]
        if not any(item["path"] == path for item in self.files) and not held:
            self.problem(f"{where}: {path} is not in the pinned diff")
            return None
        if missing:
            self.problem(
                f"{where}: line {missing[0]} is not in the pinned diff's hunks; "
                "cite lines the diff shows"
            )
            return None
        return {"path": path, "side": side, "start": first, "end": last}

    def evidence(self, match):
        side = "old" if match.group("old") else "new"
        first = int(match.group("a"))
        last = int(match.group("b") or first)
        return self.cite(match.group("path"), side, first, last)

    def names(self, path):
        """Whether the diff holds a file by this name, new or old."""
        return any(
            item["path"] == path or item.get("old_path") == path for item in self.files
        )

    def parts(self, text, strict=True):
        """Prose as strings and citations, in order.

        A backtick span is code and is kept as written. With `strict` false (a
        paragraph, a heading, a list) a token such as `@localhost:8080` that
        names no file of the diff stays text, so free prose is never refused for
        looking like a citation; a table cell is strict, since it carries a claim.
        """
        out = []

        def text_out(piece):
            if piece and out and isinstance(out[-1], str):
                out[-1] += piece
            elif piece:
                out.append(piece)

        for chunk in re.split(r"(`[^`\n]*`)", text):
            if chunk.startswith("`") and chunk.endswith("`") and len(chunk) > 1:
                text_out(chunk)
                continue
            at = 0
            for match in REF.finditer(chunk):
                text_out(chunk[at : match.start()])
                at = match.end()
                if not strict and not self.names(match.group("path")):
                    text_out(match.group(0))
                    continue
                found = self.evidence(match)
                if found:
                    out.append(found)
            text_out(chunk[at:])
        return out

    def edge(self, line):
        """One `From -> To: label @path:line` as (source, target, label, citation).

        None, after saying what is wrong, when the line is not an edge the page
        can draw as the arrow it is written with.
        """
        cites = list(REF.finditer(line))
        if len(cites) > 1:
            self.problem("one citation for each edge; split it into two edges")
            return None
        ev = self.evidence(cites[0]) if cites else None
        if cites and not ev:
            return None
        if cites:
            line = (line[: cites[0].start()] + line[cites[0].end() :]).strip()
        label = ""
        if ":" in line:
            line, label = (s.strip() for s in line.split(":", 1))
        if len(label) > MAX_LABEL:
            self.problem(f"an edge label is at most {MAX_LABEL} characters")
            return None
        ends = ARROW.split(line)
        if len(ends) != 3:
            self.problem("write one edge as `From -> To: label @path:line`")
            return None
        source, arrow, target = ends
        if arrow == "->" and not ev:
            self.problem(
                "a solid edge needs a diff line (@path:line); "
                "write --> for a link you inferred"
            )
            return None
        if arrow == "-->" and ev:
            self.problem(
                "--> marks an inference and cites no line; "
                "drop the citation or write ->"
            )
            return None
        if source.endswith("<") or target.startswith(">"):
            self.problem("an edge has one direction; write two edges")
            return None
        return source, target, label, ev

    def flow(self, args, body):
        words = args.split()
        direction = words[0] if words else "LR"
        if direction not in DIRECTIONS or len(words) > 1:
            self.problem("flow takes LR or TB")
            return None
        nodes, edges = {}, []

        def node(text):
            text = text.strip()
            if not text or len(text) > MAX_LABEL:
                self.problem(f"a node name is 1 to {MAX_LABEL} characters")
                return None
            return nodes.setdefault(text, f"n{len(nodes)}")

        for offset, raw in body:
            self.number = offset
            line = raw.strip()
            parsed = self.edge(line) if line else None
            if parsed is None:
                continue
            source, target, label, ev = parsed
            a, b = node(source), node(target)
            if a and a == b:
                self.problem("an edge joins two different nodes")
                continue
            if a and b:
                edges.append(
                    {"from": a, "to": b, "label": label, "ev": ev, "inferred": not ev}
                )
        if len(nodes) > MAX_NODES or len(edges) > MAX_EDGES:
            self.problem(
                f"a diagram holds at most {MAX_NODES} nodes and {MAX_EDGES} edges; "
                "split it"
            )
            return None
        if not edges:
            self.problem("a flow needs at least one edge")
            return None
        return {
            "t": "flow",
            "dir": direction,
            "nodes": [{"id": i, "text": t} for t, i in nodes.items()],
            "edges": edges,
        }

    def code(self, args, body):
        if any(raw.strip() for _, raw in body):
            self.problem(
                "leave a code block empty; its lines come from the pinned diff"
            )
            return None
        given = dict(ARG.findall(args))
        stray = set(given) - {"src", "lines", "side"}
        side = given.get("side", "new")
        span = re.fullmatch(r"(\d+)(?:-(\d+))?", given.get("lines", ""))
        if stray or side not in ("new", "old") or "src" not in given or not span:
            self.problem("write `code src=path lines=A-B` with side=old for old lines")
            return None
        first = int(span.group(1))
        ev = self.cite(given["src"], side, first, int(span.group(2) or first))
        if not ev:
            return None
        if ev["end"] - ev["start"] + 1 > MAX_CODE_LINES:
            self.problem(f"a code block shows at most {MAX_CODE_LINES} lines")
            return None
        held = self.lines_of(ev["path"], side)
        rows = [
            {k: held[at][k] for k in ("k", "t", "o", "n")}
            for at in range(ev["start"], ev["end"] + 1)
        ]
        return {"t": "code", "ev": ev, "rows": rows}

    def table(self, rows):
        cells = [
            [c.strip() for c in CELL_SPLIT.split(r.strip().strip("|"))] for _, r in rows
        ]
        self.number = rows[0][0]
        head, body = [self.parts(c) for c in cells[0]], cells[2:]
        if len(head) > MAX_COLS or len(body) > MAX_ROWS or not body:
            self.problem(
                f"a table has 1 to {MAX_ROWS} rows and up to {MAX_COLS} columns"
            )
            return None
        out = []
        for (offset, _), row in zip(rows[2:], body):
            self.number = offset
            if len(row) != len(head):
                self.problem("every row needs as many cells as the header")
                return None
            out.append([self.parts(c) for c in row])
        return {"t": "table", "head": head, "rows": out}

    def fenced(self, kind, rows, at):
        """A ```flow or ```code block at `at`: (block, the line after it).

        None when it is never closed, which ends the note.
        """
        end = next(
            (i for i in range(at + 1, len(rows)) if rows[i].strip() == "```"), None
        )
        if end is None:
            self.problem(f"a ```{kind} block is never closed")
            return None
        args = FENCE.match(rows[at].strip()).group("args")
        body = [(i + 1, rows[i]) for i in range(at + 1, end)]
        block = self.flow(args, body) if kind == "flow" else self.code(args, body)
        return block, end + 1

    def tabled(self, rows, at):
        end = at
        while end < len(rows) and rows[end].strip().startswith("|"):
            end += 1
        return self.table([(i + 1, rows[i]) for i in range(at, end)]), end

    def listed(self, rows, at):
        items = []
        while at < len(rows) and re.match(r"^\s*[-*] \S", rows[at]):
            self.number = at + 1
            items.append(self.parts(rows[at].strip()[2:], strict=False))
            at += 1
        return {"t": "ul", "items": items}, at

    def paragraph(self, rows, at):
        para = []
        while at < len(rows) and rows[at].strip():
            line = rows[at].strip()
            if para and (special(rows, at) or re.match(r"^(#{1,3}|[-*]) \S", line)):
                break
            para.append(line)
            at += 1
        return {"t": "p", "parts": self.parts(" ".join(para), strict=False)}, at


def special(rows, at):
    """What a line opens that is not prose: "flow", "code", "table" or None.

    Any other fence and a `|` line with no separator row below it are prose,
    so a note written before this format existed is still valid.
    """
    line = rows[at].strip()
    fence = FENCE.match(line)
    if fence:
        return fence.group("kind") if fence.group("kind") in ("flow", "code") else None
    if line.startswith("|") and at + 1 < len(rows):
        return "table" if SEPARATOR.match(rows[at + 1].strip()) else None
    return None


def read(text, files, trusted=True):
    """The blocks of a note and what is wrong with it.

    `files` is the `files` of a diffmodel reading. With `trusted` false (the
    diff did not account for every line) a note may still be prose, but
    citing a line is a problem.
    """
    reader = Reader(files, trusted)
    rows = text.split("\n")
    blocks, at = [], 0
    while at < len(rows):
        reader.number = at + 1
        stripped = rows[at].strip()
        if not stripped:
            at += 1
            continue
        kind = special(rows, at)
        if kind in ("flow", "code"):
            done = reader.fenced(kind, rows, at)
            if done is None:
                break
            block, at = done
        elif kind == "table":
            block, at = reader.tabled(rows, at)
        elif re.match(r"^#{1,3} \S", stripped):
            parts = reader.parts(stripped.lstrip("#").strip(), strict=False)
            block, at = {"t": "h", "parts": parts}, at + 1
        elif re.match(r"^[-*] \S", stripped):
            block, at = reader.listed(rows, at)
        else:
            block, at = reader.paragraph(rows, at)
        if block:
            blocks.append(block)
    if len(blocks) > MAX_BLOCKS:
        reader.problem(f"a note holds at most {MAX_BLOCKS} blocks; shorten it")
    if sum(b["t"] == "code" for b in blocks) > MAX_CODE_BLOCKS:
        reader.problem(f"a note quotes code in at most {MAX_CODE_BLOCKS} blocks")
    return {"blocks": blocks, "problems": reader.problems}

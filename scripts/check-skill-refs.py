#!/usr/bin/env python3
"""Report every reference in the skills that no longer resolves.

The skills under `.claude/skills/` name relative links and their anchors, repo
paths and root files in backticks or code blocks, make targets, script flags and
CI lane names, and each one can move. This prints one line per dangling
reference, plus every check script outside the gate that codetrial-verify does
not name, and exits 1 when there is any. It also checks that each SKILL.md
has frontmatter whose `name` is lowercase-hyphen and equals its directory and
whose `description` is not empty. It catches a rename, not a change of meaning.

Anchors follow GitHub's rule rather than a guess at it, because a link whose
anchor is wrong still opens the file and an agent reading it never notices.
"""

import fnmatch
import functools
import glob
import os
import re
import subprocess
import sys
import unicodedata

ROOTS = r"(?:scripts|docs|src|web|tests|problem-bank|\.github|\.claude)"
# A root file named bare in prose; these extensions are the ones the skills
# name, so a bare `lib.js` is not taken for a root file.
ROOT_FILE = re.compile(
    r"[A-Za-z0-9_.-]+\.(?:md|toml|json|mjs|cjs|yml|yaml|lock)|Makefile"
)
# Untracked and per-developer by design, so a clone may lack them.
UNTRACKED = {"TODO.md", "DONE.md", "CLAUDE.md"}
FENCE = re.compile(r"^\s*(`{3,}|~{3,})(.*)$")
TICKS = re.compile(r"`+")


def blocks(text):
    """Yield (in_fence, line) with front matter dropped; a fence closes only on
    its own character, at least as long, with nothing after it."""
    lines = text.split("\n")
    if lines and lines[0] == "---":
        end = next((i for i in range(1, len(lines)) if lines[i] == "---"), 0)
        lines = lines[end + 1 :]
    fence = None
    for line in lines:
        m = FENCE.match(line)
        if fence is None and m and not (m.group(1)[0] == "`" and "`" in m.group(2)):
            fence = m.group(1)
            yield True, None
            continue
        if fence is not None:
            if (
                m
                and m.group(1)[0] == fence[0]
                and len(m.group(1)) >= len(fence)
                and not m.group(2).strip()
            ):
                fence = None
            else:
                yield True, line
            continue
        yield False, line


def code_spans(line):
    """Split a line into prose and inline code: a run of n backticks closes
    only at the next run of exactly n."""
    prose, code, i = [], [], 0
    while i < len(line):
        m = TICKS.search(line, i)
        if not m:
            prose.append(line[i:])
            break
        prose.append(line[i : m.start()])
        close = re.compile(r"(?<!`)" + re.escape(m.group()) + r"(?!`)").search(
            line, m.end()
        )
        if not close:
            prose.append(line[m.start() :])
            break
        code.append(line[m.end() : close.start()].strip())
        i = close.end()
    return "".join(prose), code


def plain(text):
    """A heading as GitHub renders it to text: markup outside code spans is
    dropped, and a code span keeps its text, `<id>` and `__init__` included."""

    def markup(prose):
        prose = re.sub(r"!?\[([^\]]*)\]\([^)]*\)", r"\1", prose)
        prose = re.sub(r"<[^>]+>", "", prose)
        return re.sub(r"(\*+|(?<!\w)_+|_+(?!\w))", "", prose)

    out, i = [], 0
    for m in re.finditer(r"(?<!`)(`+)(?!`)(.+?)(?<!`)\1(?!`)", text):
        out += [markup(text[i : m.start()]), m.group(2).strip()]
        i = m.end()
    return "".join(out + [markup(text[i:])])


def slug(text):
    """GitHub's anchor: lowercase, every character that is not a letter, a
    number, a mark, a space, a hyphen or an underscore dropped, then each space
    a hyphen."""
    out = []
    for ch in plain(text).strip().lower():
        if ch in " -_" or unicodedata.category(ch)[0] in "LNM":
            out.append("-" if ch == " " else ch)
    return "".join(out)


@functools.lru_cache(maxsize=None)
def anchors(path):
    seen, found, prev = {}, set(), None
    with open(path, encoding="utf-8") as f:
        text = f.read()
    for fenced, line in blocks(text):
        heading = None
        if fenced:
            prev = None
            continue
        m = re.match(r"^ {0,3}(#{1,6})(?:[ \t]+(.*?))?(?:[ \t]+#+)?[ \t]*$", line)
        if m:
            heading = m.group(2) or ""
        elif prev is not None and re.match(r"^ {0,3}(=+|-+)[ \t]*$", line):
            heading = prev
        prev = (
            line
            if line.strip()
            and not re.match(r"^ {0,3}([-*+>|]|\d+[.)]|#)", line)
            and heading is None
            else None
        )
        if heading is None:
            continue
        base = slug(heading)
        n = seen.get(base, 0)
        seen[base] = n + 1
        found.add(base if n == 0 else f"{base}-{n}")
    return found


def make_targets():
    targets = set()
    with open("Makefile", encoding="utf-8") as f:
        for line in f:
            m = re.match(r"^([^\t#:=][^#:=]*?)\s*::?(?!=)", line)
            if m and "=" not in m.group(1):
                targets.update(m.group(1).split())
    return targets


def lanes():
    with open(".github/workflows/check.yml", encoding="utf-8") as f:
        return set(re.findall(r"^\s*- lane: ([a-z0-9-]+)\s*$", f.read(), re.M))


def repo_path(token):
    token = re.sub(r"^\./", "", token).rstrip(",;.")
    token = re.sub(r"#L\d+(-L?\d+)?$", "", token)
    return re.sub(r":\d+(:\d+)?$", "", token)


os.chdir(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
TARGETS, LANES = make_targets(), lanes()
problems = []


def report(md, kind, what):
    problems.append(f"{md}: {kind} {what}")


def check(md):
    here = os.path.dirname(md)
    with open(md, encoding="utf-8") as f:
        text = f.read()
    prose_lines, code = [], []
    for fenced, line in blocks(text):
        if line is None:
            continue
        if fenced:
            code.append(("block", line))
            continue
        prose, spans = code_spans(line)
        prose_lines.append(prose)
        code.extend(("span", s) for s in spans)
    prose = "\n".join(prose_lines)

    links = re.findall(
        r"\]\(\s*<?([^)\s>]+)>?(?:\s+(?:\"[^\"]*\"|'[^']*'))?\s*\)", prose
    )
    links += re.findall(r"^ {0,3}\[[^\]]+\]:\s*<?(\S+?)>?(?:\s|$)", prose, re.M)
    for link in links:
        if re.match(r"^[a-z][a-z0-9+.-]*:", link):
            continue
        file, _, anchor = link.partition("#")
        target = os.path.normpath(os.path.join(here, file)) if file else md
        if not os.path.exists(target):
            report(md, "link", link)
        elif anchor and target.endswith(".md") and anchor not in anchors(target):
            report(md, "anchor", link)

    paths, roots, makes, flags = set(), set(), set(), set()
    for kind, s in code:
        for m in re.finditer(
            r"(?:^|(?<=[\s\"'=(]))((?:\./)?" + ROOTS + r"/[^\s\"'`)]*)", s
        ):
            if not re.search(r"[*<>{}$\[\]]|\.\.\.", m.group(1)):
                paths.add(repo_path(m.group(1)))
        if kind == "span" and ROOT_FILE.fullmatch(s):
            roots.add(s)
        # Options and variable assignments before the target are skipped.
        for m in re.finditer(
            r"(?:^|(?<=[\s;&|(]))make(?: +(?:-\S+|[A-Za-z_][A-Za-z0-9_]*=\S*))*"
            r" +([A-Za-z0-9_.][A-Za-z0-9_.-]*)(?![A-Za-z0-9_.=-])",
            s,
        ):
            makes.add(m.group(1))
        for m in re.finditer(
            r"(?:^|(?<=\s))(?:\./)?(scripts/[A-Za-z0-9_.-]+) (--[a-z0-9-]+)", s
        ):
            flags.add((m.group(1), m.group(2)))
    gates = set(re.findall(r"gates \(([a-z0-9-]+)\)", text))

    for path in sorted(paths):
        if not os.path.exists(path):
            report(md, "path", path)
    for name in sorted(roots - UNTRACKED):
        if not os.path.exists(name) and not os.path.exists(os.path.join(here, name)):
            report(md, "file", name)
    for target in sorted(makes - TARGETS):
        report(md, "make", target)
    for script, flag in sorted(flags):
        try:
            with open(script, encoding="utf-8") as f:
                body = f.read()
        except OSError:
            continue
        if not re.search(
            r"(?<![A-Za-z0-9-])" + re.escape(flag) + r"(?![A-Za-z0-9-])", body
        ):
            report(md, "flag", f"{script} {flag}")
    for lane in sorted(gates - LANES):
        report(md, "CI lane", lane)


for md in sorted(glob.glob(".claude/skills/*/**/*.md", recursive=True)):
    check(md)

# Routing keys on the frontmatter: a name that drifts from its directory, or an
# empty description, leaves the skill unreachable until the paid routing run.
NAME = re.compile(r"[a-z0-9]+(-[a-z0-9]+)*")


def scalar(value):
    """A one-line YAML value as text: a quoted value loses its quotes, and an
    unquoted one its trailing comment."""
    value = value.strip()
    close = value.find(value[:1], 1) if value[:1] in ("'", '"') else -1
    if close > 0:
        return value[1:close].strip()
    return re.sub(r"(^|\s)#.*$", "", value).strip()


def frontmatter(text):
    """The top-level `key: value` pairs of the leading --- block, or None. A
    folded or literal block scalar (`>`, `|`) takes its indented lines."""
    lines = text.splitlines()
    if not lines or lines[0] != "---":
        return None
    end = next((i for i in range(1, len(lines)) if lines[i] == "---"), None)
    if end is None:
        return None
    fields, key = {}, None
    for line in lines[1:end]:
        m = re.match(r"([A-Za-z_-]+):[ \t]*(.*)$", line)
        if m:
            key, value = m.groups()
            fields[key] = "" if value[:1] in (">", "|") else scalar(value)
        elif key and line.startswith((" ", "\t")):
            fields[key] = (fields[key] + " " + line.strip()).strip()
    return fields


for md in sorted(glob.glob(".claude/skills/*/SKILL.md")):
    with open(md, encoding="utf-8") as f:
        text = f.read()
    fields = frontmatter(text)
    if fields is None:
        report(md, "frontmatter", "missing")
        continue
    name = fields.get("name", "")
    if not NAME.fullmatch(name):
        report(md, "name", f"{name!r} is not lowercase-hyphen")
    elif name != os.path.basename(os.path.dirname(md)):
        report(md, "name", f"{name} does not match its directory")
    if not fields.get("description"):
        report(md, "description", "empty")
    else:
        # The description is all the router reads, and the skill loader cuts an
        # unquoted one at " #" (a YAML comment): codetrial-review reached it as
        # "... re-review PR" with `PR #N` in the text, losing every trigger after.
        # A colon and a space is a YAML error in the same position, and so is a
        # colon that ends the value.
        # A plain scalar may wrap onto indented lines, and the same two hazards
        # hold on each, so the check reads the joined text, not the first line.
        raw = re.search(r"^description:[ \t]*(.*)$", text, re.M).group(1)
        if raw[:1] not in ("'", '"', ">", "|"):
            rows = text.split("\n")
            at = next(i for i, row in enumerate(rows) if row.startswith("description:"))
            tail = []
            for row in rows[at + 1 :]:
                if not row.startswith((" ", "\t")):
                    break
                tail.append(row.strip())
            joined = " ".join([raw, *tail])
            if ": " in joined:
                report(md, "description", "has a colon and a space; quote it or reword")
            elif joined.rstrip().endswith(":"):
                report(md, "description", "ends with a colon; quote it or reword")
            if re.search(r"(^|\s)#", joined):
                report(
                    md,
                    "description",
                    "has a space before #, a YAML comment; use backticks",
                )

# A skill's rules/paths routes changed files to rule files beside it: each rule
# it names has to exist, and each pattern has to match a tracked file, or it
# routes nothing and reads as coverage.
# review-bundle.sh reads each line with the shell's read, so leading blanks are
# dropped before a # is seen, and a third field would be glued onto the rule
# name it routes to: such a line is reported rather than guessed at.
rules_maps = sorted(glob.glob(".claude/skills/*/rules/paths"))
tracked = (
    subprocess.run(
        ["git", "ls-files", "-z"], capture_output=True, text=True, check=True
    ).stdout.split("\0")
    if rules_maps
    else []
)
for paths in rules_maps:
    with open(paths, encoding="utf-8") as f:
        for line in f:
            fields = line.split()
            if not fields or fields[0].startswith("#"):
                continue
            if len(fields) != 2:
                report(paths, "line", line.strip())
                continue
            pattern, rule = fields
            if not os.path.exists(os.path.join(os.path.dirname(paths), rule)):
                report(paths, "rule", rule)
            if not any(fnmatch.fnmatchcase(t, pattern) for t in tracked):
                report(paths, "pattern", pattern)

# The other direction: a check script outside the gate that no skill names.
with open("scripts/test.sh", encoding="utf-8") as f:
    gate = f.read()
with open(".claude/skills/codetrial-verify/SKILL.md", encoding="utf-8") as f:
    verify = f.read()
for script in sorted(os.listdir("scripts")):
    if script.endswith("-check.sh"):
        script = f"scripts/{script}"
        if f"/{script}" not in gate and script not in verify:
            report("codetrial-verify/SKILL.md", "does not name", script)

print("\n".join(problems), end="\n" if problems else "")
sys.exit(1 if problems else 0)

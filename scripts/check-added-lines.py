#!/usr/bin/env python3
"""Hold the comments a change adds to rules no linter can state.

Code that drops an error, a debugging leftover, a test run alone or skipped,
and a blanket suppression are refused across the whole tree by the linters
that read the code: ruff (S110, S112, T100, PGH003, PGH004), ESLint (`no-empty`,
`no-debugger`, `no-console` in web/, restricted `.only` and `.skip`) and clippy
(`unused_result_ok`) with scripts/check-rust-style.py (an empty `Err` arm).
What is left is prose, which no linter judges, and which this tree has written
its own way for years, so only the lines a change adds are held to it:

  narration  A comment that narrates the change or the plan behind it ("now
             uses", "updated to", "as per the spec", "Step 3:") instead of
             saying what the code does and why. It reads as a commit message
             that will outlive its commit.
  marker     A TODO, FIXME, XXX or HACK comment in Python, JavaScript or shell
             (scripts/check-rust-style.py already refuses one in Rust).
  emoji      An emoji in a comment.
  suppression  An `eslint-disable` must name a rule and give a `-- reason`,
             as a Rust `expect` carries its `reason`.
  disabled   A Python test skipped with no reason: `expectedFailure`, a bare
             `pytest.mark.skip` or `xfail`, a skip whose reason is empty. A
             comment on the line, or alone on the line above, gives it.

Comments are told apart from strings and regular expression literals by a
scanner for Rust and JavaScript, Python's tokenizer and a quote- and
here-document-aware reader for shell, so a string holding the words is not a
comment.

The lines judged are those the work tree (or with --staged, the index) adds
relative to the merge base of --base and HEAD, plus every line of an untracked
file or of a file renamed into another language; with --commits, those each
commit named on stdin adds relative to all its parents, read from the commit,
which is how the pre-push hook replays the check for commits that never met
the pre-commit hook. A rename within a language carries no line with it.
Vendored, external and fixture trees are skipped. Exit status 1 when anything is reported, 2 when the diff
cannot be read.
"""

import argparse
import ast
import bisect
import codecs
import collections
import io
import os
import re
import subprocess
import sys
import tokenize
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

C_LIKE = {".rs", ".js", ".mjs", ".cjs"}
JAVASCRIPT = {".js", ".mjs", ".cjs"}
SUFFIXES = C_LIKE | {".py", ".sh"}
SKIP = (
    "vendor/",
    "web/vendor/",
    "externals/",
    "node_modules/",
    "tests/fixtures/",
    "tests/golden/",
)

NARRATION = re.compile(
    r"""
    \b(?:now\ we|we\ now)\b
    | \bnow\ (?:uses|returns|handles|calls|supports)\b
    | \b(?:updated|modified|refactored|switched\ over)
      \ to\ (?:use|call|return|handle|support)\b
    | \bas\ (?:per|requested\ (?:in|by))\b
    | \bper\ the\ (?:spec|task|plan|prompt|instructions?|request|ticket)\b
    | ^(?:step|phase)\ \d+\s*[:.)-]
    | \b(?:the\ )?(?:user|maintainer|reviewer)\ (?:asked|requested|wants)\b
    """,
    re.IGNORECASE | re.VERBOSE,
)
# Outside documentation this introduces a narration of what is about to be
# written; a rustdoc or JSDoc comment uses it the way std does ("This function
# will return an error if ..."), so doc comments are not judged on it.
PREAMBLE = re.compile(
    r"\bthis (?:function|method|helper|class|module) "
    r"(?:will|is used to|is responsible for)\b",
    re.IGNORECASE,
)
DOC_COMMENT = re.compile(r"^(?:///|//!|/\*\*)")
# A past tense that opens a comment block reads as a changelog entry when an
# object follows it ("Added a retry"); this tree also opens wrapped lines and
# descriptions with participles ("Moved when the call goes out"), so only the
# first line of a block is judged, and only with an object after the verb.
CHANGELOG = re.compile(
    r"^(?:added|fixed|removed|updated|replaced|refactored|implemented)"
    r"\s+(?:a|an|the|new|support|handling|logic|code|this|some)\b",
    re.IGNORECASE,
)
# A marker quoted in backticks, part of a path or URL, or the working document
# TODO.md is the name of a thing, not a marker.
MARKER = re.compile(r"(?<![`\w/])(?:TODO|FIXME|XXX|HACK)\b(?!\.md\b)(?![`/])")
EMOJI = re.compile(
    "[\U0001f000-\U0001faff\u2600-\u27bf\u2b00-\u2bff\u2300-\u23ff\ufe0f]"
)
# ECMAScript includes the byte-order mark in whitespace and excludes U+0085.
JS_WHITESPACE = re.compile(
    r"[\t-\r \xa0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000\ufeff]"
)
ESLINT_DISABLE = re.compile(r"^ *eslint-disable(?:-next-line|-line)?(?= |$)")
ESLINT_REASON = re.compile(r" -{2,} ")
# After one of these, a `/` opens a regular expression rather than dividing.
REGEX_AFTER = set("(,=:[!&|?{};+-*%<>~^")
# A JavaScript line, and so a line comment or a regular expression literal,
# ends at any of its line terminators; a Rust line comment ends at LF.
JS_LINE_ENDS = "\n\r\u2028\u2029"
JS_LINE_END = re.compile(f"[{JS_LINE_ENDS}]")
BLANKS = re.compile(r"\s+")
NAME = re.compile(r"[\w$]+")
RAW_STRING = re.compile(r'(?:b|c)?r(#*)"')
CHAR_LITERAL = re.compile(r"'(?:\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]+\}|.)|[^\\'])'")
REGEX_KEYWORDS = {
    "return",
    "typeof",
    "instanceof",
    "in",
    "of",
    "new",
    "delete",
    "void",
    "throw",
    "case",
    "do",
    "else",
    "yield",
    "await",
}


def comment_body(text):
    """A comment without its opening punctuation, so a rule anchored at the
    start of the body sees the first word."""
    return re.sub(r"^(?:/{2,}!?|/\*+!?|\*+/?|#+!?)\s*", "", text.strip()).strip()


def invalid_eslint_disable(token):
    if "eslint-disable" not in token:
        return False
    body = token[2:]
    if token.startswith("/*") and body.endswith("*/"):
        body = body[:-2]
    body = JS_WHITESPACE.sub(" ", body)
    directive = ESLINT_DISABLE.match(body)
    if not directive:
        return False
    parts = ESLINT_REASON.split(body[directive.end() :], maxsplit=1)
    return (
        len(parts) != 2
        or not any(
            rule.strip(" ") not in {"", '""', "''"} for rule in parts[0].split(",")
        )
        or not parts[1].strip(" ")
    )


class Source:
    """One file as the rules read it: its rows, the comment on each row, and
    each row's code with comments, strings and regular expressions blanked, so
    a comment rule never reads a string as one."""

    def __init__(self, text, comments, code, tokens=()):
        self.rows = text.split("\n")
        self.comments = comments
        self.code = code
        # Each whole comment with the row it starts on, for a rule that reads a
        # comment over several rows as one: an eslint directive.
        self.tokens = tokens

    def comment_only(self, number):
        return number in self.comments and not self.code[number - 1].strip()

    def excused(self, number):
        """Whether a comment on this line, or on the nearest non-blank line
        above it when that line is only a comment, gives the reason for it."""
        if number in self.comments:
            return True
        above = number - 1
        while above >= 1 and not self.rows[above - 1].strip():
            above -= 1
        return above >= 1 and self.comment_only(above)


CONTROL_KEYWORDS = {"if", "while", "for", "with", "switch"}


def scan_c_like(text, rust):
    """Comments and masked code for Rust or JavaScript.

    A character scanner, not a parser. It knows strings (Rust raw and byte
    strings, JavaScript template literals and the code inside their `${}`),
    Rust char literals and lifetimes, nested Rust block comments, and
    JavaScript regular expression literals, which it tells from a division by
    what precedes the slash.
    """
    scanner = CLikeScanner(text, rust)
    scanner.run()
    return Source(
        text,
        scanner.comments,
        "".join(scanner.masked).split("\n"),
        scanner.tokens,
    )


class CLikeScanner:
    """The state one scan carries: the text, its masked copy, the comments by
    line, the position, and what the last token was, which decides whether a
    slash divides. Each reader takes the token at the position or declines."""

    def __init__(self, text, rust):
        self.text, self.rust, self.n = text, rust, len(text)
        self.masked = list(text)
        self.starts = [0] + [m.end() for m in re.finditer("\n", text)]
        self.comments = {}
        self.tokens = []
        self.i, self.last, self.word = 0, None, None
        # The brace depth inside each open `${`, innermost last, and for each
        # open parenthesis whether it holds a control keyword's condition.
        self.templates, self.parentheses = [], []

    def run(self):
        readers = [self.blanks, self.comment, self.raw_string]
        if self.rust:
            readers += [self.quoted, self.char_literal]
        else:
            readers += [self.template, self.quoted, self.regex]
        readers.append(self.name)
        if not self.rust:
            readers += [self.template_brace, self.parenthesis]
        # Each reader declines any character it cannot start on, so trying only
        # those that can start on this one, in the same order, reads the same
        # tokens, and most characters have one candidate or none.
        starts = {
            self.blanks: str.isspace,
            self.comment: "/".__contains__,
            self.raw_string: "brc".__contains__,
            self.template: "`".__contains__,
            self.quoted: "\"'".__contains__,
            self.char_literal: "'".__contains__,
            self.regex: "/".__contains__,
            self.name: lambda ch: ch.isalnum() or ch in "_$",
            self.template_brace: "{}".__contains__,
            self.parenthesis: "()".__contains__,
        }
        table = {}
        while self.i < self.n:
            ch = self.text[self.i]
            candidates = table.get(ch)
            if candidates is None:
                candidates = table[ch] = [r for r in readers if starts[r](ch)]
            if not any(read() for read in candidates):
                self.last, self.word = (")" if ch in ")]" else ch), None
                self.i += 1

    def blank(self, start, stop):
        for k in range(start, stop):
            if self.masked[k] != "\n":
                self.masked[k] = " "

    def note(self, start, stop):
        first = bisect.bisect_right(self.starts, start)
        token = self.text[start:stop]
        pieces = token.split("\n")
        self.tokens.append((first, token))
        for offset, piece in enumerate(pieces):
            number = first + offset
            self.comments[number] = (
                self.comments.get(number, "") + " " + piece
            ).strip()
        self.blank(start, stop)

    def operand(self, end):
        """What precedes a slash after a string, a regex or a name: a value,
        so the next slash divides."""
        self.i, self.last, self.word = end, "a", None
        return True

    def blanks(self):
        # Runs of blanks and of name characters are most of a file, and nothing
        # inside one can open a comment, a string or a regex: skip each whole.
        if not self.text[self.i].isspace():
            return False
        self.i = BLANKS.match(self.text, self.i).end()
        return True

    def comment(self):
        text, i = self.text, self.i
        if text.startswith("//", i):
            if self.rust:
                end = text.find("\n", i)
                end = self.n if end == -1 else end
            else:
                found = JS_LINE_END.search(text, i)
                end = found.start() if found else self.n
        elif text.startswith("/*", i):
            depth, end = 1, i + 2
            while end < self.n and depth:
                if text.startswith("*/", end):
                    depth, end = depth - 1, end + 2
                elif self.rust and text.startswith("/*", end):
                    depth, end = depth + 1, end + 2
                else:
                    end += 1
        else:
            return False
        self.note(i, end)
        self.i = end
        return True

    def raw_string(self):
        # Matched in place: a slice of the rest of the file at every character
        # made each Rust file quadratic.
        text, i = self.text, self.i
        raw = self.rust and RAW_STRING.match(text, i)
        if not raw or (i > 0 and (text[i - 1].isalnum() or text[i - 1] == "_")):
            return False
        close = '"' + raw.group(1)
        end = text.find(close, raw.end())
        end = self.n if end == -1 else end + len(close)
        self.blank(i + 1, end - 1)
        return self.operand(end)

    def quoted(self):
        quote = self.text[self.i]
        if quote != '"' and (self.rust or quote != "'"):
            return False
        end = self.i + 1
        while end < self.n and self.text[end] != quote:
            end += 2 if self.text[end] == "\\" else 1
        end = min(end + 1, self.n)
        self.blank(self.i + 1, end - 1)
        return self.operand(end)

    def char_literal(self):
        if self.text[self.i] != "'":
            return False
        literal = CHAR_LITERAL.match(self.text, self.i)
        if not literal:
            self.i += 1
            return True
        self.blank(self.i + 1, literal.end() - 1)
        return self.operand(literal.end())

    def template_part(self, start):
        """Blank a template literal's text from `start`, and return where it
        stopped and whether it stopped at a `${` rather than its end."""
        text, j = self.text, start
        while j < self.n:
            if text[j] == "\\":
                j += 2
            elif text[j] == "`":
                self.blank(start, j)
                return j + 1, False
            elif text.startswith("${", j):
                self.blank(start, j + 2)
                return j + 2, True
            else:
                j += 1
        self.blank(start, self.n)
        return self.n, False

    def template(self):
        if self.text[self.i] != "`":
            return False
        self.i, expression = self.template_part(self.i + 1)
        if expression:
            self.templates.append(0)
        self.last, self.word = ("(" if expression else "a"), None
        return True

    def template_brace(self):
        if not self.templates or self.text[self.i] not in "{}":
            return False
        if self.text[self.i] == "{":
            self.templates[-1] += 1
            return False
        if self.templates[-1]:
            self.templates[-1] -= 1
            return False
        self.masked[self.i] = " "
        self.i, expression = self.template_part(self.i + 1)
        if not expression:
            self.templates.pop()
        self.last, self.word = ("(" if expression else "a"), None
        return True

    def regex(self):
        if self.text[self.i] != "/" or not (
            self.last is None or self.last in REGEX_AFTER or self.word in REGEX_KEYWORDS
        ):
            return False
        text, j, in_class = self.text, self.i + 1, False
        while j < self.n and text[j] not in JS_LINE_ENDS:
            ch = text[j]
            if ch == "\\":
                j += 2
                continue
            if ch == "[":
                in_class = True
            elif ch == "]":
                in_class = False
            elif ch == "/" and not in_class:
                j += 1
                while j < self.n and (text[j].isalnum() or text[j] == "_"):
                    j += 1
                self.blank(self.i + 1, j)
                return self.operand(j)
            j += 1
        # A slash that closes nothing on its line divides.
        return False

    def name(self):
        ch = self.text[self.i]
        if not (ch.isalnum() or ch in "_$"):
            return False
        end = NAME.match(self.text, self.i).end()
        self.last, self.word, self.i = "a", self.text[self.i : end], end
        return True

    def parenthesis(self):
        ch = self.text[self.i]
        if ch == "(":
            self.parentheses.append(self.word in CONTROL_KEYWORDS)
            return False
        if ch != ")" or not self.parentheses:
            return False
        # After an `if (...)` a slash opens a regex; after a call it divides.
        control = self.parentheses.pop()
        self.last, self.word, self.i = (";" if control else ")"), None, self.i + 1
        return True


def python_comments(text):
    comments = {}
    try:
        for token in tokenize.generate_tokens(io.StringIO(text).readline):
            if token.type == tokenize.COMMENT:
                comments[token.start[0]] = token.string
    except (tokenize.TokenError, SyntaxError, IndentationError):
        pass
    return comments


def shell_comments(text):
    """A `#` that starts a word outside quotes, outside a here-document.

    Quotes may span lines, a backslash escapes the next character outside
    single quotes, and `<<` inside `$((...))` or `((...))` shifts instead of
    opening a here-document. Several here-documents on one line are read in
    order, `<<\\EOF` and `<<-EOF` included.
    """
    comments, quote, pending, ending = {}, None, [], None
    for number, row in enumerate(text.split("\n"), 1):
        if ending is not None:
            strip, word = ending
            if (row.lstrip("\t") if strip else row) == word:
                ending = pending.pop(0) if pending else None
            continue
        arithmetic, index = 0, 0
        while index < len(row):
            ch = row[index]
            if quote:
                if ch == "\\" and quote == '"':
                    index += 2
                    continue
                if ch == quote:
                    quote = None
            elif ch == "\\":
                index += 2
                continue
            elif ch in "'\"":
                quote = ch
            elif row.startswith("((", index):
                arithmetic += 1
                index += 2
                continue
            elif row.startswith("))", index) and arithmetic:
                arithmetic -= 1
                index += 2
                continue
            elif ch == "#" and (index == 0 or row[index - 1] in " \t;|&("):
                comments[number] = row[index:]
                break
            elif row.startswith("<<<", index):
                # A here-string has no body; stepping past all three keeps
                # the `<<` inside it from opening a here-document.
                index += 3
                continue
            elif row.startswith("<<", index) and not arithmetic:
                # The delimiter is the whole shell word, `END-DATA` included,
                # with its quotes removed.
                heredoc = re.match(
                    r"<<(-?)\s*\\?(['\"]?)([^\s;|&<>()'\"`\\]+)\2", row[index:]
                )
                if heredoc:
                    pending.append((heredoc.group(1) == "-", heredoc.group(3)))
                    index += heredoc.end()
                    continue
            index += 1
        if pending and ending is None:
            ending = pending.pop(0)
    return comments


def source_of(suffix, text):
    if suffix in C_LIKE:
        return scan_c_like(text, rust=suffix == ".rs")
    comments = python_comments(text) if suffix == ".py" else shell_comments(text)
    code = []
    for number, row in enumerate(text.split("\n"), 1):
        comment = comments.get(number)
        code.append(row[: row.rindex(comment)] if comment and comment in row else row)
    return Source(text, comments, code)


def dotted(node):
    """`a.b.c` for a name or an attribute chain, otherwise None."""
    parts = []
    while isinstance(node, ast.Attribute):
        parts.append(node.attr)
        node = node.value
    if isinstance(node, ast.Name):
        return ".".join([node.id, *reversed(parts)])
    return None


def unexplained_skips(text):
    """Line numbers of the decorators that skip a Python test with no reason:
    `expectedFailure`, a bare `pytest.mark.skip` or `xfail`, and a skip whose
    reason is empty. No ruff rule asks for the reason."""
    try:
        tree = ast.parse(text)
    except SyntaxError:
        return set()
    found = set()
    for node in ast.walk(tree):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            found.update(d.lineno for d in node.decorator_list if unexplained(d))
    return found


def unexplained(decorator):
    call = decorator if isinstance(decorator, ast.Call) else None
    last = (dotted(call.func if call else decorator) or "").rsplit(".", 1)[-1]
    if last == "expectedFailure":
        return True
    if last not in ("skip", "skipIf", "skipUnless", "skipif", "xfail"):
        return False
    if call is None:
        return True
    # unittest's skipIf and skipUnless take their condition first and the
    # reason second; pytest's xfail and skipif take only conditions
    # positionally, so only `reason=` gives one.
    if last in ("xfail", "skipif"):
        reasons = []
    elif last in ("skipIf", "skipUnless"):
        reasons = call.args[1:]
    else:
        reasons = list(call.args)
    reasons += [k.value for k in call.keywords if k.arg == "reason"]
    return not any(
        not (isinstance(r, ast.Constant) and r.value in ("", None)) for r in reasons
    )


def check_file(rel, text, added):
    """(path, line, rule, message) for each finding on the added line numbers
    of one file, or on every line of it when added is None."""
    suffix = Path(rel).suffix
    source = source_of(suffix, text)
    if added is None:
        added = set(range(1, len(source.rows) + 1))
    skips = unexplained_skips(text) if suffix == ".py" else set()
    # Each invalid directive comment is reported once, on the first of its rows
    # the change adds, even a blank one inside it, and two on one row twice.
    suppressions = collections.Counter()
    if suffix in JAVASCRIPT:
        for first, token in source.tokens:
            rows = added.intersection(range(first, first + token.count("\n") + 1))
            if rows and invalid_eslint_disable(token):
                suppressions[min(rows)] += 1
    findings = []
    for number in sorted(added):
        if number > len(source.rows):
            continue
        comment = source.comments.get(number)
        if comment:
            body = comment_body(comment)
            narration = NARRATION.search(body)
            if not narration and not DOC_COMMENT.match(comment.lstrip()):
                narration = PREAMBLE.search(body)
            if not narration and not source.comment_only(number - 1):
                narration = CHANGELOG.search(body)
            if narration:
                findings.append(
                    (
                        rel,
                        number,
                        "narration",
                        f"{narration.group().strip()!r} narrates the change; say what "
                        "the code does and why",
                    )
                )
            marker = MARKER.search(comment) if suffix != ".rs" else None
            if marker:
                findings.append(
                    (rel, number, "marker", f"{marker.group()}; do it or file it")
                )
            if EMOJI.search(comment):
                findings.append((rel, number, "emoji", "emoji in a comment"))
        findings.extend(
            [(rel, number, "suppression", "name the rule and say why after --")]
            * suppressions[number]
        )
        if number in skips and not source.excused(number):
            findings.append(
                (rel, number, "disabled", "a skipped test says why it waits")
            )
    return findings


# The pathspec modes a caller's environment can force: a literal one would
# read DIFF_PATHS' `:(glob)` as a file name, match nothing and pass every
# change, so these are left to their defaults for the checker's own git.
PATHSPEC_MODES = (
    "GIT_LITERAL_PATHSPECS",
    "GIT_GLOB_PATHSPECS",
    "GIT_NOGLOB_PATHSPECS",
    "GIT_ICASE_PATHSPECS",
)
GIT_ENV = {k: v for k, v in os.environ.items() if k not in PATHSPEC_MODES}


def git(root, *args, check=True, data=None):
    # Replaced, not refused: a file of another encoding beside the ones judged
    # must not stop the check, and a judged one is read as best it can be.
    result = subprocess.run(
        ["git", "-c", "core.quotepath=off", *args],
        cwd=root,
        env=GIT_ENV,
        input=data,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if check and result.returncode:
        raise RuntimeError(result.stderr.strip() or f"git {args[0]} failed")
    return result.stdout


def blobs(root, names):
    """The text of each object name (`:path` in the index, `commit:path`), read
    by one git process rather than one per file; None for a name git lacks."""
    if not names:
        return {}
    result = subprocess.run(
        ["git", "cat-file", "--batch", "-z"],
        cwd=root,
        input=("\0".join(names) + "\0").encode(),
        capture_output=True,
    )
    if result.returncode:
        raise RuntimeError(
            result.stderr.decode(errors="replace").strip() or "git cat-file failed"
        )
    out, found, at = result.stdout, {}, 0
    for name in names:
        end = out.index(b"\n", at)
        header = out[at:end].split()
        # A name git lacks comes back as `<name> missing`, and a name holding
        # a space splits into more words, so the size is what tells them apart.
        if len(header) != 3 or not header[2].isdigit():
            found[name], at = None, end + 1
            continue
        size = int(header[2])
        found[name] = out[end + 1 : end + 1 + size].decode("utf-8", errors="replace")
        at = end + 1 + size + 1
    return found


def in_scope(rel):
    return Path(rel).suffix in SUFFIXES and not rel.startswith(SKIP)


def header_path(target):
    """The path of a `+++` header, or None for /dev/null. Git ends a name that
    holds a space with a tab, and C-quotes one that holds a quote, a backslash
    or a control character, octal escapes standing for its bytes."""
    target = target.rstrip("\t")
    if len(target) > 1 and target[0] == target[-1] == '"':
        raw = codecs.escape_decode(target[1:-1].encode("utf-8"))[0]
        target = raw.decode("utf-8", errors="replace")
    return target[2:] if target.startswith("b/") else None


def added_lines(diff):
    """Map each file a unified diff with zero context touches to the line
    numbers it adds on the new side. A file header is read only between a
    `diff --git` line and the first hunk, so an added line that happens to
    start with `++ ` stays a line.

    A file renamed into another language maps to None, every line: a rename
    alone adds none, but its lines now read under that language's rules (a
    Rust comment that is an ESLint directive in JavaScript)."""
    added, path, number, header, source = {}, None, 0, False, None
    for row in diff.split("\n"):
        if row.startswith("diff --git "):
            header, path, source = True, None, None
        elif header and row.startswith("rename from "):
            source = header_path(as_new_side(row[len("rename from ") :]))
        elif header and row.startswith("rename to ") and source is not None:
            target = header_path(as_new_side(row[len("rename to ") :]))
            if target and language(target) != language(source):
                added[target] = None
        elif header and row.startswith("+++ "):
            path = header_path(row[4:])
            if path is not None:
                added.setdefault(path, set())
        elif row.startswith("@@"):
            header = False
            m = re.match(r"@@ -\S+ \+(\d+)(?:,(\d+))? @@", row)
            number = int(m.group(1)) if m else 0
        elif not header and path is not None and row.startswith("+") and number:
            if added[path] is not None:
                added[path].add(number)
            number += 1
        elif not header and row.startswith(" ") and number:
            # A context row, which -U0 asks for none of but diff.interHunkContext
            # can still bring in to join two hunks: it is a line of the new
            # side too.
            number += 1
    return {p: lines for p, lines in added.items() if lines is None or lines}


def language(rel):
    """The rules a path's lines are read under: `.js`, `.mjs` and `.cjs` are
    one language, so a rename among them carries no line with it."""
    suffix = Path(rel).suffix
    return ".js" if suffix in JAVASCRIPT else suffix


def as_new_side(name):
    """A rename header's name, which carries no prefix, as a `+++` one."""
    return '"b/' + name[1:] if name.startswith('"') else "b/" + name


# The prefixes are pinned, since diff.noprefix and diff.mnemonicPrefix would
# move the `b/` the headers are read by; textconv is off, since a driver would
# run on every file and change what the lines say.
DIFF = (
    "-U0",
    "--inter-hunk-context=0",
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--find-renames",
    "--src-prefix=a/",
    "--dst-prefix=b/",
)


# Restrict both sides before Git pairs renames. A move out of a fixture or
# into a checked language introduces the destination to this policy.
DIFF_PATHS = (
    "--",
    *(f":(glob)**/?*{suffix}" for suffix in sorted(SUFFIXES)),
    *(f":(exclude){prefix}*" for prefix in SKIP),
)


def in_scope_lines(diff):
    """The lines a diff adds to each in-scope file, None for every line."""
    return {rel: lines for rel, lines in added_lines(diff).items() if in_scope(rel)}


def collect(root, base, staged):
    """(path, text, added line numbers) for every in-scope file the change
    adds lines to."""
    head = git(root, "rev-parse", "--verify", "--quiet", "HEAD^{commit}", check=False)
    if staged:
        against = (
            head.strip() or git(root, "hash-object", "-t", "tree", "/dev/null").strip()
        )
        diff = git(root, "diff", "--cached", *DIFF, against, *DIFF_PATHS)
    else:
        if not git(
            root,
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            f"{base}^{{commit}}",
            check=False,
        ).strip():
            raise RuntimeError(f"base {base} is not fetched; fetch it or pass --base")
        against = git(root, "merge-base", base, "HEAD").strip()
        diff = git(root, "diff", *DIFF, against, *DIFF_PATHS)
    files = in_scope_lines(diff)
    if staged:
        texts = blobs(root, [f":{rel}" for rel in files])
        for rel in sorted(files):
            if texts[f":{rel}"] is not None:
                yield rel, texts[f":{rel}"], files[rel]
        return
    untracked = git(root, "ls-files", "--others", "--exclude-standard", "-z")
    for rel in filter(in_scope, untracked.split("\0")):
        files[rel] = None
    for rel in sorted(files):
        path = root / rel
        if path.is_symlink():
            text = os.readlink(path)
        elif path.is_file():
            text = path.read_text(encoding="utf-8", errors="replace")
        else:
            continue
        yield rel, text, files[rel]


def commit_lines(root, names):
    """(commit, path, text, added line numbers) for what each commit adds,
    read from the commits rather than the work tree. A commit is diffed
    against each of its parents, and a line counts as added only when every
    diff adds it: for a merge, that is the resolution, not what either side
    brought in, which that side's own commits answer for. One rev-list
    resolves every name (an unknown one stops the check before any is
    judged), one diff-tree makes every diff and one cat-file reads every file."""
    if not names:
        return
    listed = git(
        root,
        "rev-list",
        "--no-walk=unsorted",
        "--parents",
        "--stdin",
        data="".join(f"{n}^{{commit}}\n" for n in names),
    ).splitlines()
    pairs = []
    for line in listed:
        commit, *parents = line.split()
        pairs += [(commit, parent) for parent in parents] or [(commit, None)]
    out = git(
        root,
        "diff-tree",
        "--stdin",
        "--root",
        "--always",
        "-r",
        *DIFF,
        *DIFF_PATHS,
        data="".join(f"{c} {p}\n" if p else f"{c}\n" for c, p in pairs),
    )
    # --always heads every requested diff with its commit's id (SHA-1, or
    # SHA-256 in a repository that uses it), empty diffs included, so the
    # diffs come back one for each pair and in its order.
    parts = re.split(r"(?m)^([0-9a-f]{40}|[0-9a-f]{64})$", out)
    diffs = parts[2::2]
    if parts[1::2] != [c for c, _ in pairs]:
        raise RuntimeError("git diff-tree did not return one diff per parent")
    # A line a merge adds is one every parent's diff adds; None, every line,
    # gives way to the other side's lines.
    adds = {}
    for (commit, _), diff in zip(pairs, diffs):
        found = in_scope_lines(diff)
        if commit not in adds:
            adds[commit] = found
            continue
        kept, adds[commit] = adds[commit], {}
        for rel in kept.keys() & found.keys():
            a, b = kept[rel], found[rel]
            lines = b if a is None else a if b is None else a & b
            if lines is None or lines:
                adds[commit][rel] = lines
    texts = blobs(root, [f"{c}:{rel}" for c in adds for rel in adds[c]])
    for commit, files in adds.items():
        for rel, lines in sorted(files.items()):
            yield commit, rel, texts[f"{commit}:{rel}"] or "", lines


def formatted(finding, prefix=""):
    path, number, rule, message = finding
    return f"{prefix}{path}:{number}: {rule}: {message}"


def run(root, base="origin/main", staged=False):
    return [
        formatted(finding)
        for rel, text, lines in collect(root, base, staged)
        for finding in check_file(rel, text, lines)
    ]


def run_commits(root, names):
    return [
        formatted(finding, f"{commit[:12]}:")
        for commit, rel, text, lines in commit_lines(root, names)
        for finding in check_file(rel, text, lines)
    ]


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument(
        "--base",
        default="origin/main",
        help="judge lines added since the merge base of this and HEAD",
    )
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument(
        "--staged", action="store_true", help="judge the index against HEAD instead"
    )
    mode.add_argument(
        "--commits",
        action="store_true",
        help="judge each commit named on stdin against all its parents",
    )
    args = parser.parse_args()
    try:
        if args.commits:
            findings = run_commits(args.root, sys.stdin.read().split())
        else:
            findings = run(args.root, args.base, args.staged)
    except (RuntimeError, OSError) as error:
        print(f"check-added-lines: {error}", file=sys.stderr)
        return 2
    for finding in findings:
        print(finding)
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())

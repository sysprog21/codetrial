"""Deterministic checks that an explanation is the contributor's own and points
at the code, without calling a model.

Answers that are too short or too repetitive, or that repeat the question or
code verbatim, are refused. Shorter overlaps are hints. Not knowing is honest
and stays acceptable, and the reflection step can start with what the
contributor would read or run first. Whether a full answer names anything from
the changed lines is only reported. A model can write a fluent paragraph about a diff it never read, but
it cannot be forced to name the identifiers it was not shown, so that signal
reaches the contributor as a hint and the reviewer as a fact, and blocks nothing.

The thresholds are this tool's defaults, not research findings.
"""

import functools
import re
from collections import Counter

MIN_WORDS = 12  # words, or CJK characters, in a sendable core answer
MIN_DISTINCT = 8  # different words among them, so filler does not count
SOFT_GRAM = 5  # a shorter run of copied words, counted as a share of the answer
SOFT_SHARE = 0.5  # the share of an answer made of such runs that prompts a hint
GRAM_WORDS = 8  # consecutive words copied from the question or the code
GRAM_CJK = 12  # consecutive CJK characters copied from them
ANCHORS = 5  # identifiers offered as a hint
STEM = 6  # letters of a word's stem compared between answer and question

_CJK = "\u3400-\u9fff\u3040-\u30ff\uac00-\ud7af"
CJK = re.compile(f"[{_CJK}]")
CJK_RUN = re.compile(f"[{_CJK}]+")
WORD = re.compile(r"[^\W_]+(?:'[^\W_]+)?")
IDENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]{3,}")
CODE_SPAN = re.compile(r"`+[^`]*`+")
UNSURE = re.compile(
    r"^\W*(?:"
    r"i\s+(?:do(?:n'?t|\s+not)\s+know|have\s+no\s+idea|am\s+(?:not\s+sure|unsure))|"
    r"i'?m\s+(?:not\s+sure|unsure)|no\s+idea|not\s+sure|unsure|don'?t\s+know|"
    r"我?(?:還|也|實在|真的|目前)?不(?:太)?(?:知道|清楚|確定|确定|了解)|沒概念|没概念"
    r")",
    re.I,
)

# Words that name a construct, not this change, in the languages the tree uses.
STOP = frozenset(
    """
    self this that with from import true false none null else elif then done
    fi for while return def class fn let mut pub use mod struct impl enum match
    case esac when into over also each only just than them they will would
    should could have been were what which where there their about after before
    function const var async await break continue yield lambda try except catch
    finally raise throw new delete type interface export default static public
    private string number bool boolean void echo exit set unset local readonly
    """.split()
)


def words(text):
    """Words in the text, counting each CJK character as one.

    Quoted code is not the contributor's prose: a line pasted between
    backticks adds to what is said about it, not to the count.
    """
    text = CODE_SPAN.sub(" ", text)
    return len(CJK.findall(text)) + len(WORD.findall(CJK.sub(" ", text)))


def _plain(text):
    return text.replace("\u2019", "'").replace("\u2018", "'")


def unsure(text):
    return bool(UNSURE.match(_plain(text)))


def _latin(text):
    return [w.lower() for w in WORD.findall(CJK.sub(" ", text))]


def distinct(text):
    """Different words in the text, quoted code left out, a CJK character as one."""
    prose = CODE_SPAN.sub(" ", text)
    return len(set(_latin(prose))) + len(set(CJK.findall(prose)))


def _cjk_grams(text):
    """Runs of twelve consecutive CJK characters, each run taken on its own.

    Characters that merely recur around Latin text are not a passage that was
    copied, so a run is never joined to the next one.
    """
    found = set()
    for run in CJK_RUN.findall(text):
        found |= _grams(run, GRAM_CJK)
    return found


def _grams(items, size):
    return {tuple(items[i : i + size]) for i in range(len(items) - size + 1)}


def _latin_runs(text, size):
    """The runs of `size` words a text says, each stretch between CJK on its own:
    the end of one and the start of the next are not neighbours."""
    found = set()
    for part in CJK.split(text):
        found |= _grams(_latin(part), size)
    return found


@functools.lru_cache(maxsize=16)
def _latin_grams(source, size):
    """`_latin_runs` of a source, fixed for a session and cached. The answer's
    own fragments go through `_latin_runs`, so typing cannot evict a source."""
    return _latin_runs(source, size)


@functools.lru_cache(maxsize=8)
def _cjk_there(source):
    return _cjk_grams(source)


def _there(sources, size):
    found = set()
    for source in sources:
        found |= _latin_grams(source, size)
    return found


def _mostly(answer, there):
    """Whether most of the answer is runs of SOFT_GRAM words found in `there`.

    A comment in the patch pasted with a few words of padding passes the
    eight-word test; here the share of the answer that sits in such runs counts.
    """
    # Latin words only: a copy in CJK text is for `copied` to find.
    total = covered = 0
    for part in CODE_SPAN.split(answer):
        for run in CJK.split(part):
            tokens = _latin(run)
            total += len(tokens)
            marked = set()
            for i in range(len(tokens) - SOFT_GRAM + 1):
                if tuple(tokens[i : i + SOFT_GRAM]) in there:
                    marked.update(range(i, i + SOFT_GRAM))
            covered += len(marked)
    return total >= SOFT_GRAM and covered / total >= SOFT_SHARE


def copied(answer, sources):
    """Whether the answer repeats a run of the question or the code verbatim.

    Backticked spans are skipped: quoting an identifier or a line is what a
    grounded answer does, and a run of eight plain words is not quoting.
    """
    latin_there = _there(sources, GRAM_WORDS)
    cjk_there = set().union(*(_cjk_there(source) for source in sources))
    # Each stretch between quoted spans on its own: words either side of a
    # span are not neighbours, and joining them could invent a match.
    return any(
        _latin_runs(part, GRAM_WORDS) & latin_there or _cjk_grams(part) & cjk_there
        for part in CODE_SPAN.split(answer)
    )


@functools.lru_cache(maxsize=8)
def identifiers(patch, quote=""):
    """Every identifier from the changed lines, those in the quote first.

    Common keywords are left out. The question's own words are not: an answer
    that names the function it asks about has still named the code.
    """
    count = Counter()
    for line in patch.splitlines():
        if line[:1] in "+-" and not line.startswith(("+++ ", "--- ")):
            count.update(IDENT.findall(line[1:]))
    quoted = set(IDENT.findall(quote))
    return tuple(
        sorted(
            (word for word in count if word.lower() not in STOP),
            key=lambda word: (word not in quoted, -count[word], word),
        )
    )


def anchors(patch, prompt="", quote=""):
    """The few identifiers offered as a hint: not ones the question already says."""
    asked = {w.lower() for w in IDENT.findall(prompt)}
    return tuple(
        word for word in identifiers(patch, quote) if word.lower() not in asked
    )[:ANCHORS]


# Words every question is made of; sharing one says nothing about the topic.
GENERIC = frozenset(
    "code does change changed this that what why how which would should could "
    "happen happens answer explain mean means about there when then they them".split()
)


def _stems(text):
    """Words of four letters or more, cut to a crude stem, so send, sends and
    sent-ish forms of one word meet. Not language knowledge: a plain cut."""
    return {
        re.sub(r"(?:ing|ed|es|s)$", "", w)[:STEM]
        for w in _latin(CODE_SPAN.sub(" ", text))
        if len(w) >= 4 and w not in STOP and w not in GENERIC
    }


def _codey(name):
    return "_" in name or bool(re.search(r"[a-z][A-Z]", name))


def related(answer, question, every):
    """Whether the answer is about the question: it reuses a word of the
    question, the quote or the file's name, or names code from the change that
    is code and not just a word (an underscore, a camelCase, a path). None when
    nothing was asked or the answer is mostly CJK, which this cannot judge."""
    question = question or {}
    asked = " ".join(
        question.get(k, "") or "" for k in ("prompt", "quote", "path")
    ).strip()
    if not asked or len(CJK.findall(answer)) > len(WORD.findall(CJK.sub(" ", answer))):
        return None
    path = question.get("path") or ""
    if path and (path in answer or path.rsplit("/", 1)[-1] in answer):
        return True
    if _stems(answer) & _stems(asked.replace("/", " ").replace("_", " ")):
        return True
    named = set(IDENT.findall(answer))
    return any(_codey(name) and name in named for name in every)


def mentions(answer, names):
    found = set(IDENT.findall(answer))
    return any(name in found for name in names)


def check(answer, question, patch):
    """What can be said about the core answer without a model."""
    question = question or {}
    sources = [question.get("prompt", ""), question.get("quote", ""), patch]
    every = identifiers(patch, question.get("quote", ""))
    names = anchors(patch, question.get("prompt", ""), question.get("quote", ""))
    return {
        "related": related(answer, question, every),
        "words": words(answer),
        "distinct": distinct(answer),
        "unsure": unsure(answer),
        "copied": copied(answer, sources),
        "copy_hint": _mostly(answer, _there(sources, SOFT_GRAM)),
        "anchors": names,
        # Nothing to name (a prose-only change) means nothing to hold against it.
        "grounded": mentions(answer, every) if every else None,
    }


def state(checks):
    """One word for where the answer stands, in the order a person fixes it."""
    if checks["unsure"]:
        # Not knowing is fine; pasting the question after saying so is not.
        if checks["copied"]:
            return "copied"
        return "unsure"
    if checks["words"] == 0:
        return "empty"
    if checks["words"] < MIN_WORDS or checks["distinct"] < MIN_DISTINCT:
        return "short"
    if checks["copied"]:
        return "copied"
    if checks["copy_hint"]:
        return "copy_hint"
    if checks["related"] is False:
        return "unrelated"
    if checks["grounded"] is False:
        return "ungrounded"
    return "good"


MESSAGES = {
    "empty": "Say how the quoted code works and why it is needed, in your own words. Naming a function, variable or line from the change helps.",
    "short": f"A little more, please: at least {MIN_WORDS} words (or {MIN_WORDS} Chinese characters), including {MIN_DISTINCT} different words or characters, in your own words. Say what the code does and why it matters. If you are unsure, say so and name the first thing you would read or run.",
    "copied": "That repeats the question or the code word for word. Say it in your own words; put a line you want to quote in `backticks`.",
    "copy_hint": "Parts of this answer resemble the question or change. Check that it explains the code in your own words.",
    "unrelated": "This may not be about the question. Reuse a word from it or name the code your answer depends on. The check cannot judge every language; the reviewer will read your explanation.",
    "ungrounded": "Nothing from the changed lines is mentioned yet. Which of these does your answer depend on?",
    "good": "Specific enough. Send to reviewer when you are ready.",
    "unsure": "That is fine. Add what you would check first, and the review starts there.",
}


def message(checks):
    return MESSAGES[state(checks)]


def problem(checks):
    """Why the answer cannot be sent yet, or None. Hints never block."""
    return message(checks) if state(checks) in ("empty", "short", "copied") else None

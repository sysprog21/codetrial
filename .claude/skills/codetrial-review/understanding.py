"""Deterministic checks that an explanation is the contributor's own and points
at the code, without calling a model.

Two things are refused, because both are cheap to see and cheap to fix: an
answer too short to say anything, and an answer that repeats the question or the
code word for word. A third is only reported: whether the answer names anything
from the changed lines. A model can write a fluent paragraph about a diff it
never read, but it cannot be forced to name the identifiers it was not shown, so
that signal reaches the contributor as a hint and the reviewer as a fact, and
blocks nothing. "I don't know" is always an acceptable answer; the reflection
step then asks what to check, which is the useful outcome.

The thresholds are this tool's defaults, not research findings.
"""

import functools
import re
from collections import Counter

MIN_WORDS = 12  # words, or CJK characters, in a sendable core answer
GRAM_WORDS = 8  # consecutive words copied from the question or the code
GRAM_CJK = 12  # consecutive CJK characters copied from them
ANCHORS = 5  # identifiers offered as a hint

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


def unsure(text):
    return bool(UNSURE.match(text.replace("\u2019", "'").replace("\u2018", "'")))


def _latin(text):
    return [w.lower() for w in WORD.findall(CJK.sub(" ", text))]


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


@functools.lru_cache(maxsize=8)
def _source_grams(source):
    """What one source says, as grams; fixed for a session.

    Each source on its own: the end of the question and the start of the patch
    are not neighbours, and a string cached by its own identity hashes once.
    """
    return _grams(_latin(source), GRAM_WORDS), _cjk_grams(source)


def copied(answer, sources):
    """Whether the answer repeats a run of the question or the code verbatim.

    Backticked spans are skipped: quoting an identifier or a line is what a
    grounded answer does, and a run of eight plain words is not quoting.
    """
    latin_there, cjk_there = set(), set()
    for source in sources:
        latin, cjk = _source_grams(source)
        latin_there |= latin
        cjk_there |= cjk
    # Each stretch between quoted spans on its own: words either side of a
    # span are not neighbours, and joining them could invent a match.
    return any(
        _grams(_latin(part), GRAM_WORDS) & latin_there or _cjk_grams(part) & cjk_there
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
        "words": words(answer),
        "unsure": unsure(answer),
        "copied": copied(answer, sources),
        "anchors": names,
        # Nothing to name (a prose-only change) means nothing to hold against it.
        "grounded": mentions(answer, every) if every else None,
    }


def state(checks):
    """One word for where the answer stands, in the order a person fixes it."""
    if checks["unsure"]:
        # Not knowing is fine; pasting the question after saying so is not.
        return "copied" if checks["copied"] else "unsure"
    if checks["words"] == 0:
        return "empty"
    if checks["words"] < MIN_WORDS:
        return "short"
    if checks["copied"]:
        return "copied"
    if checks["grounded"] is False:
        return "ungrounded"
    return "good"


MESSAGES = {
    "empty": "Say how the quoted code works and why it is needed, in your own words. Naming a function, variable or line from the change helps.",
    "short": f'A little more, please: at least {MIN_WORDS} words (or {MIN_WORDS} Chinese characters) in your own words, saying what the code does and why it matters. If you are unsure, say "I don\'t know" and what you would check.',
    "copied": "That repeats the question or the code word for word. Say it in your own words; put a line you want to quote in `backticks`.",
    "ungrounded": "Nothing from the changed lines is mentioned yet. Which of these does your answer depend on?",
    "good": "Specific enough. Send to reviewer when you are ready.",
    "unsure": "That is fine. Add what you would check first, and the review starts there.",
}


def message(checks):
    return MESSAGES[state(checks)]


def problem(checks):
    """Why the answer cannot be sent yet, or None. Hints never block."""
    return message(checks) if state(checks) in ("empty", "short", "copied") else None

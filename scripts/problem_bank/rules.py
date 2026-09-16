"""The rules a scenario variant must pass before anything is generated from it."""

from __future__ import annotations

import functools
import json
import re
from pathlib import Path

from .bank import (
    GUIDE_SOURCE,
    VARIANT_KEYS,
    compact,
    named,
    read_json,
    repeated,
)


def case_input(judge: dict, case: dict) -> str:
    """A judge case written the way an example on the page writes its input."""
    if judge["kind"] == "class":
        operations, arguments = case["input"]
        return f"{compact(operations)}\n{compact(arguments)}"
    return ", ".join(
        f"{name} = {compact(value)}"
        for name, value in zip(judge["paramNames"], case["input"])
    )


def input_values(text: str) -> tuple:
    """The numbers and quoted strings an example input holds, in order.

    Names are dropped, parameters and operations alike, and numbers compare by
    value, so two spellings of the same input agree.
    """
    values = []
    # `null` is a value too: a tree written level by level with its gaps, and
    # dropping them makes two different trees read the same.
    for quoted, number, literal in re.findall(
        r'"((?:[^"\\]|\\.)*)"|(-?\d+(?:\.\d+)?)|\b(null|true|false)\b', text
    ):
        values.append(float(number) if number else literal or quoted)
    return tuple(values)


def camel_words(text: str) -> str:
    """Letters and digits only, lowercased: `coinChange` and "Coin Change" agree."""
    return "".join(character for character in text.lower() if character.isalnum())


def spelled_words(text: str) -> list[str]:
    """Lowercase words, with identifiers split where their case changes, so
    `minStackCreate` reads as min, stack, create and `LRUCache` as lru, cache."""
    text = re.sub(r"([A-Z]+)([A-Z][a-z])", r"\1 \2", text)
    text = re.sub(r"([a-z0-9])([A-Z])", r"\1 \2", text)
    return re.findall(r"[a-z0-9]+", text.lower())


def names_source(title: str, text: str) -> bool:
    """Whether text gives away the published problem it was written from.

    One rule, and `tests/common/words.rs` applies the same one to the prompts
    and to what the interviewer says. A title that is a single ordinary word,
    "Candy" or "Triangle", is also a word a scenario uses, so it is not held
    against anything. Any other title counts when a run of consecutive words
    spells it with the spaces gone, identifiers split at their case changes:
    "LRUCache", "minStackCreate", "lru cache" and "3 Sum" all name their
    problems, and "those 3 sums" does not. Parameter names are not exempt: a
    brief that says "merge intervals" names the problem whatever the parameter
    is called.
    """
    return not title.isalpha() and spells(title, text)


def spells(name: str, text: str) -> bool:
    """Whether a run of consecutive words in text joins into name."""
    target = camel_words(name)
    words = spelled_words(text)
    for start in range(len(words)):
        joined = ""
        for word in words[start:]:
            joined += word
            if joined == target:
                return True
            if len(joined) >= len(target):
                break
    return False


# How much of a published example is recognisable on its own: this many values,
# or one string this long. Both the published-case check and the prose check
# read these, so the two agree on what counts. Tuned against the bank rather
# than derived: three values collide with ordinary illustrations such as "2, 3
# and 4", and four-letter words such as "must" are prose. The tests pin each
# boundary, so moving one is a decision the diff shows.
RECOGNISABLE_VALUES = 4
RECOGNISABLE_CHARS = 5
# Prose may reorder a quoted input only once it holds this many distinct values,
# and with fewer it must run this long in the published order: a board of 0s
# and 1s is spelled by any sentence about 0s and 1s.
REORDERED_DISTINCT_VALUES = 3
ORDERED_RUN_VALUES = 5


@functools.cache
def spoken_numbers(text: str) -> tuple:
    return tuple(value for value in input_values(text) if isinstance(value, float))


def example_arguments(text: str) -> list[tuple]:
    """The values of each argument a published example names, one tuple apiece.

    `head = [1,2,3,4,5], k = 2` gives (1, 2, 3, 4, 5) and (2,). Only an argument
    long enough to be recognised on its own counts: four values or more, or one
    string of five characters or more. The published tree with another k, or the
    published sentence with another word list, is still the published example.
    """
    parts, depth, quoted, start = [], 0, False, 0
    for at, character in enumerate(text):
        # A quote is escaped only behind an odd run of backslashes.
        if character == '"' and (at - len(text[:at].rstrip("\\"))) % 2 == 0:
            quoted = not quoted
        elif quoted:
            continue
        elif character in "[{(":
            depth += 1
        elif character in "]})":
            depth -= 1
        elif character == "," and depth == 0:
            parts.append(text[start:at])
            start = at + 1
    parts.append(text[start:])
    arguments = [input_values(part.split("=", 1)[-1]) for part in parts]
    return [
        values
        for values in arguments
        if len(values) >= RECOGNISABLE_VALUES
        or (
            len(values) == 1
            and isinstance(values[0], str)
            and len(values[0]) >= RECOGNISABLE_CHARS
        )
    ]


def quotes_example(example: tuple, text: str) -> bool:
    """Whether prose repeats a published example's input.

    A hint that walks "3, 0, 6, 1, 5" or lines up "paper and title" hands the
    candidate the published example the page no longer shows. It takes four
    numbers or more holding three distinct values, in any order, since "[1, 6],
    [2, 8], [7, 12] and [10, 16]" is the same input sorted; with fewer distinct
    values only five or more in the published order count, so an output of 0s
    and 1s is not taken for a published board. A string counts from five
    characters, and only as a whole word.
    """
    numbers = tuple(value for value in example if isinstance(value, float))
    if len(numbers) >= RECOGNISABLE_VALUES:
        spoken = spoken_numbers(text)
        distinct = len(set(numbers)) >= REORDERED_DISTINCT_VALUES
        for at in range(len(spoken) - len(numbers) + 1):
            window = spoken[at : at + len(numbers)]
            if (
                window == numbers and (distinct or len(numbers) >= ORDERED_RUN_VALUES)
            ) or (distinct and sorted(window) == sorted(numbers)):
                return True
    return any(
        isinstance(value, str)
        and len(value) >= RECOGNISABLE_CHARS
        and re.search(rf"(?<![\w-]){re.escape(value)}(?![\w-])", text)
        for value in example
    )


def sized(problem_id: str, where: str, value: object, low: int, high: int) -> list:
    if not isinstance(value, list) or not low <= len(value) <= high:
        raise RuntimeError(f"{problem_id}: {where} must hold {low}..{high} entries")
    return value


def spoken_text(problem_id: str, where: str, value: object) -> str:
    """One string of variant prose, held to what both of its readers accept.

    ASCII because the same text is compiled into Rust, where the source stays
    ASCII, and spoken by the interviewer, where a backtick is read aloud.
    """
    if not named(value) or value != value.strip():
        raise RuntimeError(f"{problem_id}: {where} must be non-empty, unpadded text")
    if not value.isascii() or not value.isprintable() or "`" in value:
        raise RuntimeError(
            f"{problem_id}: {where} must be printable ASCII with no backticks"
        )
    return value


def text_list(
    problem_id: str, where: str, value: object, low: int, high: int
) -> list[str]:
    return [
        spoken_text(problem_id, f"{where}[{at}]", item)
        for at, item in enumerate(sized(problem_id, where, value, low, high))
    ]


# Printed beside OK or FAIL in the results panel, so a label saying where a
# case came from, or which approach it defeats, is read by the candidate the
# moment they press Run.
REVEALING_LABEL = re.compile(
    r"leetcode|sample|\bexample|greedy|dynamic|\bheap|pointer|binary search|prefix sum|\bxor\b",
    re.IGNORECASE,
)


def check_labels(problem_id: str, judge: dict) -> None:
    """The labels as they ship, against the judge's published names.

    Checked after renames, so a rename cannot make two labels the same or turn
    one into a word that gives the case away.
    """
    # Two cases under one label are one row twice in the results panel, and a
    # report cannot say which of them failed.
    labels = [case["label"] for case in judge["cases"]]
    if duplicates := repeated(labels):
        raise RuntimeError(f"{problem_id}: case labels repeat: {duplicates}")
    # The published entry point is refused rather than renamed: it is often a
    # plain verb, "rotate by three", and the new name would not read as one.
    published = [name for name in (judge.get("entry"), judge.get("className")) if name]
    for label in labels:
        if REVEALING_LABEL.search(label) or any(
            spells(name, label) for name in published
        ):
            raise RuntimeError(f"{problem_id}: case label {label!r} gives it away")


def boundary_value(value: object, type_name: str) -> bool:
    """Whether a value is small at the domain its declared type admits.

    An empty list or zero element is a boundary for a numeric array; zero and
    one cover inclusive and positive numeric lower bounds. The types are the
    judge contract, so this predicate reads them instead of treating one JSON
    shape as a boundary everywhere.
    """
    if type_name == "character[][]":
        return isinstance(value, list) and (
            len(value) <= 2
            or all(
                isinstance(row, list) and all(cell == "." for cell in row)
                for row in value
            )
        )
    if type_name.endswith("[][]"):
        return isinstance(value, list) and (
            len(value) <= 2
            or any(isinstance(row, list) and len(row) <= 2 for row in value)
        )
    if type_name in {"integer[]", "number[]", "double[]"}:
        return isinstance(value, list) and (len(value) <= 1 or 0 in value)
    if type_name.endswith("[]") or type_name.startswith("list<"):
        return isinstance(value, list) and len(value) <= 1
    if type_name in {"integer", "number", "double"}:
        return value in {0, 1}
    if type_name in {"string", "character"}:
        return isinstance(value, str) and len(value) <= 1
    if type_name.lower() in {
        "linkedlist",
        "tree",
        "listnode",
        "treenode",
        "node",
    }:
        return value is None or (isinstance(value, list) and len(value) <= 2)
    return False


def has_boundary_case(judge: dict) -> bool:
    """Whether one case reaches a boundary valid for this judge's domain."""
    if judge["kind"] == "class":
        return any(len(case["input"][0]) <= 2 for case in judge["cases"])
    return any(
        any(
            boundary_value(value, type_name)
            for value, type_name in zip(case["input"], judge["paramTypes"])
        )
        for case in judge["cases"]
    )


def check_judge_case_coverage(problem_id: str, judge: dict) -> None:
    """Every judge needs five cases and one domain-valid boundary case."""
    if len(judge["cases"]) < 5:
        raise RuntimeError(f"{problem_id}: judge needs at least five cases")
    if not has_boundary_case(judge):
        raise RuntimeError(f"{problem_id}: judge needs a boundary case")


JUDGE_CASE_GAPS_SOURCE = (
    Path(__file__).resolve().parents[2] / "problem-bank" / "judge-case-gaps.txt"
)


def judge_case_gaps() -> set[str]:
    """The temporary, counted list of judges still awaiting authored cases."""
    lines = JUDGE_CASE_GAPS_SOURCE.read_text().splitlines()
    header = next((line for line in lines if line.startswith("# GAPS: ")), None)
    if header is None or not header[8:].isdigit():
        raise RuntimeError("judge-case-gaps.txt starts with '# GAPS: N'")
    gaps = {line for line in lines if line and not line.startswith("#")}
    if len(gaps) != int(header[8:]):
        raise RuntimeError(
            f"judge-case-gaps.txt declares {header[8:]} gaps and lists {len(gaps)}"
        )
    return gaps


def check_judge_case_gaps(judges: dict) -> None:
    """Every exception is still needed, and every missing case is listed."""
    gaps = judge_case_gaps()
    unknown = gaps - set(judges)
    if unknown:
        raise RuntimeError(
            f"judge-case-gaps.txt names unknown judges: {sorted(unknown)}"
        )
    for problem_id, judge in judges.items():
        try:
            check_judge_case_coverage(problem_id, judge)
        except RuntimeError:
            if problem_id not in gaps:
                raise
        else:
            if problem_id in gaps:
                raise RuntimeError(
                    f"{problem_id}: judge now passes; remove it from judge-case-gaps.txt"
                )


def posed(problem: dict, judge: dict, variant: dict) -> tuple[dict, dict]:
    """The bank entry and judge with the variant's names in place of the published ones.

    The bank keeps the names LeetCode publishes, so a scaffolded or refreshed
    entry needs no hand edits; the rename is declared once in the variant and
    applied here to everything that ships or reaches the interviewer.
    """
    # Printed beside OK or FAIL, so a label written against a published
    # parameter, "magazine too short", says it to the candidate on every run. The
    # entry point is not renamed there; check_labels refuses it instead.
    worded = {**variant.get("terms", {}), **variant.get("parameters", {})}
    renames = dict(worded)
    if "entry" in variant:
        renames[judge["entry"]] = variant["entry"]
    # C passes an array with its length beside it, `pointsSize` and
    # `pointsColSize`, so a renamed parameter is a prefix there as well.
    prefixes = dict(variant.get("parameters", {}))
    if "className" in variant:
        old, new = judge["className"], variant["className"]
        renames[old] = new
        # C has no classes, so its starters spell the class as a lower-camel
        # prefix on every function: `minStackCreate`, `lRUCacheGet`.
        prefixes[old[0].lower() + old[1:]] = new[0].lower() + new[1:]

    def renamed(text: str) -> str:
        for old, new in renames.items():
            text = re.sub(rf"\b{re.escape(old)}\b", new, text)
        for old, new in prefixes.items():
            text = re.sub(rf"\b{re.escape(old)}(?=[A-Z])", new, text)
        return text

    judge = {
        **judge,
        "cases": [
            {
                **case,
                "label": re.sub(
                    r"\b\w+\b",
                    lambda word: worded.get(word.group(), word.group()),
                    case["label"],
                ),
            }
            for case in judge["cases"]
        ],
    }
    if "entry" in variant:
        judge["entry"] = variant["entry"]
    if "className" in variant:
        # A class judge calls the class by name and lists the constructor as the
        # first operation of every case, and a leftover `entry` on one is the
        # published camel-case name that nothing reads.
        judge.pop("entry", None)
        judge["className"] = variant["className"]
        judge["cases"] = [
            {
                **case,
                "input": [
                    [
                        renames.get(operation, operation)
                        for operation in case["input"][0]
                    ],
                    *case["input"][1:],
                ],
            }
            for case in judge["cases"]
        ]
        # Class methods do not carry a return type in the source bank. The
        # harness must still know which calls are void when a candidate adds a
        # case without an expected result, so publish that stable property once
        # in the judge rather than infer it from the candidate's case.
        returns = {}
        for case in judge["cases"]:
            for operation, expected in zip(case["input"][0], case["expected"]):
                if operation != judge["className"]:
                    returns[operation] = (
                        "value"
                        if expected is not None
                        else returns.get(operation, "void")
                    )
        judge["methodReturnTypes"] = returns
    if "paramNames" in judge:
        judge["paramNames"] = [renames.get(name, name) for name in judge["paramNames"]]
    problem = {
        **problem,
        "starterCode": {
            language: renamed(code) for language, code in problem["starterCode"].items()
        },
        "constraints": [renamed(line) for line in problem["constraints"]],
    }
    return problem, judge


def checked_text(problem_id: str, variant: object) -> dict:
    """The variant's prose, each field held to its size and to spoken text."""
    optional = {"entry", "className", "parameters", "terms"}
    if (
        not isinstance(variant, dict)
        or not VARIANT_KEYS <= set(variant) <= VARIANT_KEYS | optional
    ):
        raise RuntimeError(
            f"{problem_id}: a variant has exactly {sorted(VARIANT_KEYS)}"
        )
    for at, item in enumerate(
        sized(problem_id, "clarifications", variant["clarifications"], 3, 6)
    ):
        if not isinstance(item, dict) or set(item) != {"question", "answer"}:
            raise RuntimeError(
                f"{problem_id}: clarifications[{at}] is a question and an answer"
            )
        spoken_text(problem_id, f"clarifications[{at}].question", item["question"])
        spoken_text(problem_id, f"clarifications[{at}].answer", item["answer"])
    return {
        "title": spoken_text(problem_id, "title", variant["title"]),
        "brief": text_list(problem_id, "brief", variant["brief"], 1, 3),
        # The exact contract in the scenario's words. The interviewer judges
        # from this rather than from the published statement, which names the
        # problem as often as it states it.
        "contract": spoken_text(problem_id, "contract", variant["contract"]),
        "follow_ups": text_list(problem_id, "followUps", variant["followUps"], 2, 3),
        "hints": text_list(problem_id, "hints", variant["hints"], 3, 3),
    }


def check_new_names(problem: dict, judge: dict, variant: dict) -> None:
    """Every name the variant gives is new: entry or class, parameters, terms."""
    problem_id = problem["id"]
    # Both kinds are renamed: a function by its entry point, a class by its
    # name, since `LRUCache` on screen names the problem as plainly as a title.
    declared = "entry" if judge["kind"] == "function" else "className"
    if declared not in variant or ({"entry", "className"} - {declared}) & set(variant):
        raise RuntimeError(
            f"{problem_id}: a {judge['kind']} problem declares a new {declared}"
        )
    # Every name a variant gives, entry point, class, parameters and terms, is
    # held to the same list of names it may not bring back.
    published_names = {
        camel_words(name)
        for name in (problem.get("title"), judge.get("entry"), judge.get("className"))
        if name
    }
    pattern = r"[a-z][A-Za-z0-9]+" if declared == "entry" else r"[A-Z][A-Za-z0-9]+"
    if (
        not re.fullmatch(pattern, str(variant[declared]))
        or camel_words(variant[declared]) in published_names
    ):
        raise RuntimeError(
            f"{problem_id}: {declared} {variant[declared]!r} is not a new name"
        )
    parameters = variant.get("parameters", {})
    if not isinstance(parameters, dict) or not set(parameters) <= set(
        judge.get("paramNames", [])
    ):
        raise RuntimeError(f"{problem_id}: parameters renames only judge parameters")
    # Words a starter snippet carries that are neither the entry point nor a
    # parameter, such as a comment naming the published node type.
    terms = variant.get("terms", {})
    if not isinstance(terms, dict) or not all(
        re.fullmatch(r"[A-Za-z][A-Za-z0-9]*", str(word))
        for pair in terms.items()
        for word in pair
    ):
        raise RuntimeError(
            f"{problem_id}: terms maps one identifier-like word to another"
        )
    # A rename into the published entry point would put it back everywhere the
    # rename reaches, labels included.
    for target in [*parameters.values(), *terms.values()]:
        if camel_words(str(target)) in published_names:
            raise RuntimeError(f"{problem_id}: {target!r} is the published name")


def spoken_prose(text: dict, variant: dict) -> list[tuple[str, str]]:
    """Every line of variant prose the candidate or interviewer reads, by where.

    The brief and contract are on the page or in the prompt; the follow-ups,
    hints and clarifications reach the interviewer, who may say any of them
    aloud. One list, so a check over prose cannot miss a field another covers.
    """
    return [
        ("contract", text["contract"]),
        *[(f"brief[{at}]", line) for at, line in enumerate(text["brief"])],
        *[(f"followUps[{at}]", line) for at, line in enumerate(text["follow_ups"])],
        *[(f"hints[{at}]", line) for at, line in enumerate(text["hints"])],
        *[
            (f"clarifications[{at}].{part}", item[part])
            for at, item in enumerate(variant["clarifications"])
            for part in ("question", "answer")
        ],
    ]


def check_source_absent(
    problem: dict, variant: dict, text: dict, shipped: dict, graded: dict
) -> None:
    """The source title and site appear nowhere the browser receives."""
    problem_id, title = problem["id"], problem["title"]
    if "leetcode" in json.dumps(variant).lower():
        raise RuntimeError(f"{problem_id}: a variant never names the source site")
    for where, line in [("title", text["title"]), *spoken_prose(text, variant)]:
        if names_source(title, line):
            raise RuntimeError(f"{problem_id}: {where} names the source title")

    # What the editor opens with, and what the browser runs: the starters and
    # every judge case, labels, inputs and expected values alike.
    for language, code in shipped["starterCode"].items():
        if names_source(title, code):
            raise RuntimeError(
                f"{problem_id}: the {language} starter names the source title"
            )
    for case in graded["cases"]:
        if names_source(title, json.dumps(case)):
            raise RuntimeError(
                f"{problem_id}: judge case {case['label']!r} names the source title"
            )
    for at, line in enumerate(shipped["constraints"]):
        if names_source(title, line):
            raise RuntimeError(
                f"{problem_id}: constraints[{at}] names the source title"
            )


def check_entry_named(
    problem_id: str, brief: list, shipped: dict, graded: dict
) -> None:
    """The brief names what the judge calls, and every starter defines it."""
    name = graded["className"] if graded["kind"] == "class" else graded["entry"]
    if not any(re.search(rf"\b{re.escape(name)}\b", line) for line in brief):
        raise RuntimeError(f"{problem_id}: the brief never names {name}")
    for language, code in shipped["starterCode"].items():
        if not re.search(rf"\b{re.escape(name)}\b", code):
            raise RuntimeError(
                f"{problem_id}: the {language} starter does not define {name}"
            )


def published_cases(problem: dict, judge: dict) -> tuple[set, set]:
    """What the published examples give away, and which judge cases repeat it.

    Returns the values a prose line may not walk through, and the indexes of the
    judge cases a page may not show.
    """
    # Compared by the values an input holds, not by how it is written: the
    # published examples spell `x = 2.00000` and `addNum(1), addNum(2)` where a
    # judge case holds 2 and a list of operations.
    # A class example is published as calls, `addNum(1), addNum(2)`, or as the
    # judge's two lists; either way only its argument values say which case it is.
    operations = {judge.get("className")} | {
        name
        for case in judge["cases"]
        if judge["kind"] == "class"
        for name in case["input"][0]
    }
    published_inputs = {
        tuple(
            value for value in input_values(example["input"]) if value not in operations
        )
        for example in problem["examples"]
    }
    # A function case is published by any one argument of a published example,
    # a class case only by its whole argument list: its operation names repeat
    # across every case.
    published_arguments = (
        set()
        if judge["kind"] == "class"
        else {
            values
            for example in problem["examples"]
            for values in example_arguments(example["input"])
        }
    )
    published = {
        at
        for at, case in enumerate(judge["cases"])
        if (
            input_values(compact(case["input"][1]))
            if judge["kind"] == "class"
            else input_values(case_input(judge, case))
        )
        in published_inputs
        or any(
            input_values(compact(argument)) in published_arguments
            for argument in case["input"]
        )
    }
    return published_inputs | published_arguments, published


def rendered_examples(problem: dict, variant: dict, graded: dict) -> tuple[list, set]:
    """The examples as the page shows them, and the judge cases they show."""
    problem_id = problem["id"]
    cases = graded["cases"]
    examples = []
    shown = set()
    for at, example in enumerate(
        sized(problem_id, "examples", variant["examples"], 1, 2)
    ):
        if (
            not isinstance(example, dict)
            or not {"case"} <= set(example) <= {"case", "output", "explanation"}
            or example["case"] not in range(len(cases))
        ):
            raise RuntimeError(f"{problem_id}: examples[{at}] names one judge case")
        case = cases[example["case"]]
        shown.add(example["case"])
        # An override only where the judge's expected value is not what a reader
        # should see: the kept prefix of an in-place compaction, say, or one of
        # several accepted orders.
        rendered = {
            "input": case_input(graded, case),
            "output": compact(case["expected"]),
        }
        for key in ("output", "explanation"):
            if key in example:
                rendered[key] = spoken_text(
                    problem_id, f"examples[{at}].{key}", example[key]
                )
        # Judge data is on the page too, and a published sample sentence can
        # carry the title: "This is an example of text justification."
        if problem.get("title") and names_source(
            problem["title"], " ".join(rendered.values())
        ):
            raise RuntimeError(f"{problem_id}: examples[{at}] names the source title")
        examples.append(rendered)
    return examples, shown


def check_prose_quotes_nothing(
    problem_id: str, variant: dict, text: dict, quotable: set
) -> None:
    """No line the candidate or interviewer reads walks a published example."""
    prose = [
        *(line for _, line in spoken_prose(text, variant)),
        *(
            example[key]
            for example in variant["examples"]
            for key in ("output", "explanation")
            if key in example
        ),
    ]
    for quoted in quotable:
        for line in prose:
            if quotes_example(quoted, line):
                raise RuntimeError(
                    f"{problem_id}: {line[:60]!r} repeats a published example"
                )


def validated_variant(problem: dict, judge: dict, variant: object) -> dict:
    """The checks that keep a scenario gradeable and keep the source out of it.

    Gradeable: every example on the page is a real judge case, and the entry
    point the brief names is the one every starter snippet defines and the judge
    calls. Out of it: the source title is nowhere in what the candidate reads,
    and the entry point is not the published name.

    Returns the posed problem, the posed judge and the examples as the page
    shows them.
    """
    problem_id = problem["id"]
    text = checked_text(problem_id, variant)
    check_new_names(problem, judge, variant)
    shipped, graded = posed(problem, judge, variant)
    check_labels(problem_id, {**judge, "cases": graded["cases"]})
    original = problem.get("origin") == "original"
    if not original:
        check_source_absent(problem, variant, text, shipped, graded)
    check_entry_named(problem_id, text["brief"], shipped, graded)
    quotable, published = (
        published_cases(problem, judge) if not original else (set(), set())
    )
    examples, shown = rendered_examples(problem, variant, graded)
    if not original:
        check_prose_quotes_nothing(problem_id, variant, text, quotable)
    # The published examples are the most recognisable thing about a problem:
    # "pwwkew" or "paper" and "title" name it as surely as the title does. None
    # of them is shown, so a judge built only from them needs a case of its own.
    if shown & published:
        raise RuntimeError(
            f"{problem_id}: examples show published case {min(shown & published)}"
        )
    return {"problem": shipped, "judge": graded, "examples": examples}


def validated_variants(problems: list[dict], judges: dict, variants: object) -> dict:
    """Every problem's variant, in bank order, each validated against its judge.

    `variants` is the parsed bank file, handed in like the judges.
    """
    if not isinstance(variants, dict):
        raise RuntimeError("variants must be a JSON object keyed by problem id")
    check_judge_case_gaps(judges)
    ids = [problem["id"] for problem in problems]
    if list(variants) != ids:
        raise RuntimeError(
            "variants must list every problem once, in bank order; "
            f"missing {sorted(set(ids) - set(variants))}, extra {sorted(set(variants) - set(ids))}"
        )
    validated = {}
    for problem in problems:
        judge = judges[problem["id"]]
        variant = variants[problem["id"]]
        validated[problem["id"]] = {
            **validated_variant(problem, judge, variant),
            "variant": variant,
        }
    check_page_names([variant["title"] for variant in variants.values()], ids)
    return validated


def page_slug(title: str) -> str:
    """The name the interview URL carries, from the scenario rather than the id.

    The id is LeetCode's slug, and `?problem=coin-change` in the address bar
    names the problem before the page has rendered. The id still keys history,
    tokens and the agent, so it stays inside the page and in the judge's file
    name, where only a reader of the network panel sees it.
    """
    return re.sub(r"[^a-z0-9]+", "-", title.lower()).strip("-")


def check_page_names(titles: list[str], ids: list[str]) -> None:
    pages = [page_slug(title) for title in titles]
    if len(set(pages)) != len(pages):
        raise RuntimeError("variant titles must be unique, and unique as page names")
    # An old link carries an id, and the loader tries a page of that name before
    # the alias map, so a page named like another problem's id would take over
    # that problem's links.
    taken = sorted(set(pages) & set(ids))
    if taken:
        raise RuntimeError(f"page names must not equal a problem id: {taken}")


# What an import leaves behind when it drops the code a page was built around:
# headings and links pointing at nothing. A reviewer told to find "the full
# solution here" is reading a web page, not notes.
GUIDE_FURNITURE = (
    "```",
    "You can find the full",
    "Java Solution",
    "Explanation of the Solution",
)


def validated_guides(problems: list[dict], guides: object = None) -> dict:
    """The notes by problem, in bank order. `guides` is the parsed document,
    read from problem-bank/guides.json when it is not handed in."""
    if guides is None:
        guides = read_json(GUIDE_SOURCE)
    if not isinstance(guides, dict) or set(guides) != {
        "source",
        "note",
        "license",
        "notes",
    }:
        raise RuntimeError("guides.json has exactly source, note, license and notes")
    license_text = (
        "\n".join(guides["license"]) if isinstance(guides["license"], list) else ""
    )
    if (
        "Permission is hereby granted" not in license_text
        or "Copyright (c)" not in license_text
    ):
        raise RuntimeError("guides.json must carry the license its notes came under")
    ids = [problem["id"] for problem in problems]
    notes = guides["notes"]
    if not isinstance(notes, dict) or not set(notes) <= set(ids):
        raise RuntimeError(
            f"guides.json notes name problems the bank does not have: {sorted(set(notes) - set(ids))}"
        )
    for problem_id, text in notes.items():
        if not named(text) or not text.isascii():
            raise RuntimeError(f"{problem_id}: a guide note is non-empty ASCII text")
        for furniture in GUIDE_FURNITURE:
            if furniture in text:
                raise RuntimeError(
                    f"{problem_id}: a guide note still carries {furniture!r}"
                )
    return {problem_id: notes[problem_id] for problem_id in ids if problem_id in notes}

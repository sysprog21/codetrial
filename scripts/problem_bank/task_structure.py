"""Structural checks mirrored by src/tasks/validation.rs for downloaded banks."""

import re

from .bank import STARTER_LANGS

CHECKERS = {
    "exact",
    "approxNumber",
    "arrayBag",
    "balancedBst",
    "dependencyOrder",
    "indexPair",
    "integerCombinations",
    "integerRows",
    "palindrome",
    "tripletSet",
    "unorderedGroups",
}
JUDGE_OPTIONAL = {
    "entry",
    "className",
    "paramNames",
    "paramTypes",
    "returnType",
    "argTypes",
    "outputParam",
    "outputType",
    "outputPrefixParam",
    "cyclePosParam",
    "constructorArgTypes",
}
# The languages the catalog ships starters for, from the one list of them.
LANGUAGES = set(STARTER_LANGS)


def fields(value, required, optional=()):
    if not isinstance(value, dict) or not set(required) <= set(value) <= set(
        required
    ) | set(optional):
        raise ValueError("missing or unknown bank fields")


def text(value):
    if not isinstance(value, str) or not value.strip() or len(value.encode()) > 32768:
        raise ValueError("expected nonblank bank text up to 32 KiB")


def texts(value, minimum, maximum):
    if not isinstance(value, list) or not minimum <= len(value) <= maximum:
        raise ValueError("invalid bank text list")
    for item in value:
        text(item)


def name(value):
    if not isinstance(value, str) or not re.fullmatch(r"[A-Za-z][A-Za-z0-9_]*", value):
        raise ValueError("invalid identifier")


def judge_options(judge):
    if judge.get("kind") == "function" and "className" in judge:
        raise ValueError("judge carries a name for the wrong kind")
    if judge.get("kind") == "class" and "entry" in judge:
        name(judge["entry"])
    if "paramNames" in judge:
        if not isinstance(judge["paramNames"], list):
            raise ValueError("invalid parameter names")
        for item in judge["paramNames"]:
            name(item)
        if len(set(judge["paramNames"])) != len(judge["paramNames"]):
            raise ValueError("duplicate parameter names")
    for key in ("paramTypes", "argTypes", "constructorArgTypes"):
        if key in judge:
            if not isinstance(judge[key], list):
                raise ValueError("invalid judge type list")
            for item in judge[key]:
                if key == "argTypes" and item is None:
                    continue
                text(item)
    for key in ("returnType", "outputType"):
        if key in judge:
            text(judge[key])
    for key in ("outputParam", "outputPrefixParam", "cyclePosParam"):
        # The learner's CodeTrial reads these as u64; past that it refuses the
        # set, so packaging refuses it first.
        if key in judge and (
            type(judge[key]) is not int or not 0 <= judge[key] < 2**64
        ):
            raise ValueError("invalid judge parameter index")


def structural(problem, judge, variant):
    if not isinstance(problem, dict):
        raise ValueError("invalid problem")
    if "referenceCode" in problem:
        text(problem["referenceCode"])
    for key in ("summary", "optimal", "pitfalls"):
        text(problem.get(key))
    if not isinstance(problem.get("difficulty"), str) or problem["difficulty"] not in {
        "Easy",
        "Medium",
        "Hard",
    }:
        raise ValueError("invalid difficulty")
    texts(problem.get("topics"), 1, 8)
    topics = [topic.strip() for topic in problem["topics"]]
    if len(set(topics)) != len(topics):
        raise ValueError("duplicate topic")
    origin = problem.get("origin", "leetcode")
    if origin == "original":
        if "title" in problem or "examples" in problem:
            raise ValueError("original record has imported fields")
    elif origin == "leetcode":
        text(problem.get("title"))
        if not isinstance(problem.get("examples"), list):
            raise ValueError("missing imported examples")
    else:
        raise ValueError("invalid origin")
    starters = problem.get("starterCode")
    if (
        not isinstance(starters, dict)
        or "python" not in starters
        or not set(starters) <= LANGUAGES
    ):
        raise ValueError("invalid starter languages")
    for starter in starters.values():
        text(starter)
    fields(
        variant,
        {
            "title",
            "brief",
            "contract",
            "examples",
            "clarifications",
            "followUps",
            "hints",
        },
        {"entry", "className", "parameters", "terms"},
    )
    text(variant["title"])
    text(variant["contract"])
    texts(variant["brief"], 1, 3)
    texts(variant["followUps"], 2, 3)
    texts(variant["hints"], 3, 3)
    clarifications = variant["clarifications"]
    if not isinstance(clarifications, list) or not 3 <= len(clarifications) <= 6:
        raise ValueError("invalid clarifications")
    for row in clarifications:
        fields(row, {"question", "answer"})
        text(row["question"])
        text(row["answer"])
    fields(judge, {"kind", "checker", "cases"}, JUDGE_OPTIONAL)
    judge_options(judge)
    if not isinstance(judge["checker"], str) or judge["checker"] not in CHECKERS:
        raise ValueError("unsupported checker")
    if not isinstance(judge["kind"], str) or judge["kind"] not in {"function", "class"}:
        raise ValueError("unsupported judge kind")
    declared = "entry" if judge["kind"] == "function" else "className"
    wrong = "className" if declared == "entry" else "entry"
    name(judge.get(declared))
    name(variant.get(declared))
    if wrong in variant or judge[declared].lower() == variant[declared].lower():
        raise ValueError("invalid variant rename")
    if (
        declared == "entry"
        and not variant[declared][0].islower()
        or declared == "className"
        and not variant[declared][0].isupper()
    ):
        raise ValueError("invalid variant name case")
    renames = {}
    for key in ("terms", "parameters"):
        mapping = variant.get(key, {})
        if not isinstance(mapping, dict):
            raise ValueError("invalid rename map")
        for old, new in mapping.items():
            name(old)
            name(new)
            if old in renames:
                raise ValueError("overlapping rename keys")
            if key == "parameters" and old not in judge.get("paramNames", []):
                raise ValueError("unknown parameter rename")
            renames[old] = new
    if judge[declared] in renames:
        raise ValueError("rename key repeats source name")
    renames[judge[declared]] = variant[declared]
    if set(renames) & set(renames.values()):
        raise ValueError("chained rename is ambiguous")
    if len(set(renames.values())) != len(renames):
        raise ValueError("duplicate rename output")
    posed_parameters = [
        renames.get(value, value) for value in judge.get("paramNames", [])
    ]
    if len(set(posed_parameters)) != len(posed_parameters):
        raise ValueError("parameter rename collides")
    prefixes = set(variant.get("parameters", {}))
    if declared == "className":
        class_prefix = judge[declared][0].lower() + judge[declared][1:]
        if class_prefix in prefixes:
            raise ValueError("duplicate prefix rename key")
        prefixes.add(class_prefix)
    outputs = list(renames.values())
    if declared == "className":
        outputs.append(variant[declared][0].lower() + variant[declared][1:])
    if prefixes & set(outputs):
        raise ValueError("chained prefix rename")
    for prefix in prefixes:
        if any(
            value.startswith(prefix)
            and len(value) > len(prefix)
            and "A" <= value[len(prefix)] <= "Z"
            for value in outputs + list(prefixes)
        ):
            raise ValueError("ambiguous prefix rename")
        if any(
            prefix.startswith(output)
            and len(prefix) > len(output)
            and "A" <= prefix[len(output)] <= "Z"
            for output in outputs
        ):
            raise ValueError("ambiguous reverse prefix rename")
    cases = judge["cases"]
    if not isinstance(cases, list) or not 1 <= len(cases) <= 1000:
        raise ValueError("invalid judge cases")
    for case in cases:
        fields(case, {"label", "input", "expected"})
        text(case["label"])
        if not isinstance(case["input"], list):
            raise ValueError("invalid case input")
        if declared == "className":
            inputs = case["input"]
            if len(inputs) != 2 or not all(isinstance(row, list) for row in inputs):
                raise ValueError("invalid class input")
            operations, arguments = inputs
            expected = case["expected"]
            if (
                not operations
                or not isinstance(expected, list)
                or len(operations) != len(arguments)
                or len(expected) != len(operations)
                or operations[0] != judge[declared]
            ):
                raise ValueError("invalid class case lengths")
            for operation in operations:
                name(operation)
            if not all(isinstance(row, list) for row in arguments):
                raise ValueError("invalid class arguments")
    examples = variant["examples"]
    if not isinstance(examples, list) or not 1 <= len(examples) <= 2:
        raise ValueError("invalid examples")
    for row in examples:
        fields(row, {"case"}, {"output", "explanation"})
        if type(row["case"]) is not int or not 0 <= row["case"] < len(cases):
            raise ValueError("unresolved example")
        for key in ("output", "explanation"):
            if key in row:
                text(row[key])

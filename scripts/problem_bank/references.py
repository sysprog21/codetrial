"""Run each reference solution over its judge's cases.

The shape checks in rules.py cannot tell a wrong `expected` from a right one,
and a wrong one fails a correct candidate in the interview. A reference under
problem-bank/references/ is a correct answer, named after its judge, that
defines a function named after the judge's `entry`; --check runs it on every
case and compares what it returns as the page would. Nothing in that folder
is embedded or served.

Only `exact` function judges over plain values have a reference so far: the
other checkers live in web/lib.js and src/tasks/validation.rs, and a linked
list, a tree or a class needs a codec or a driver first.
"""

from __future__ import annotations

import importlib.util
import itertools
import json
import math
import sys
from decimal import Decimal
from pathlib import Path

from .bank import ROOT

REFERENCES = ROOT / "problem-bank" / "references"

# A distinct module name per load: the test runner checks references on
# several threads, which share sys.modules.
LOADS = itertools.count()


def check_references(judges: dict, directory: Path = REFERENCES) -> None:
    """Every reference loads, has a judge it can check, and passes every case."""
    for path in sorted(directory.glob("*.py")) if directory.is_dir() else []:
        judge_id = path.stem
        judge = judges.get(judge_id)
        if judge is None:
            raise RuntimeError(f"{judge_id}: no judge has this id")
        if judge.get("kind") != "function" or judge.get("checker") != "exact":
            raise RuntimeError(
                f"{judge_id}: only an exact judge of kind function has a reference so far"
            )
        if any(judge.get("argTypes") or []) or "outputType" in judge:
            raise RuntimeError(
                f"{judge_id}: a linked or tree value needs a codec before it has a reference"
            )
        # Loaded as a module that stays in sys.modules while its cases run:
        # runpy drops it once the file has run, and a dataclass built in the
        # entry function then finds no module to read.
        name = f"reference_{judge_id.replace('-', '_')}_{next(LOADS)}"
        spec = importlib.util.spec_from_file_location(name, path)
        module = importlib.util.module_from_spec(spec)
        sys.modules[name] = module
        try:
            try:
                spec.loader.exec_module(module)
            except Exception as error:  # a syntax error or a failed import
                raise RuntimeError(
                    f"{judge_id}: fails to load: {type(error).__name__}: {error}"
                ) from error
            run_cases(judge_id, judge, getattr(module, judge["entry"], None))
        finally:
            sys.modules.pop(name, None)


def run_cases(judge_id: str, judge: dict, solve) -> None:
    if not callable(solve):
        raise RuntimeError(f"{judge_id}: defines no function {judge['entry']}")
    for number, case in enumerate(judge["cases"], start=1):
        # A fresh copy, read as the page hands it over, so a reference that
        # changes its arguments changes nothing a later case or the generated
        # judge reads.
        args = json.loads(
            json.dumps(case["input"]),
            parse_int=js_number,
            parse_float=js_number,
            parse_constant=lambda _: None,
        )
        try:
            actual = answer(judge, args, solve(*args))
        except Exception as error:  # any failure is the case's
            raise RuntimeError(
                f"{judge_id}: case {number} raises {type(error).__name__}: {error}"
            ) from error
        if not same(actual, case["expected"]):
            raise RuntimeError(
                f"{judge_id}: case {number} expects {shown(case['expected'])}, "
                f"the reference returns {shown(actual)}"
            )


def answer(judge: dict, args: list, returned):
    """What the Python driver in web/runners.js hands the page for this call."""
    prefix = judge.get("outputPrefixParam")
    if isinstance(prefix, int):
        if type(returned) is not int or not 0 <= returned <= len(args[prefix]):
            raise TypeError("the kept prefix needs an integer length")
        actual = args[prefix][:returned]
    elif isinstance(judge.get("outputParam"), int):
        actual = args[judge["outputParam"]]
    else:
        actual = returned
    # JSON carries the value to the page: a tuple arrives as an array, and a
    # value that is not finite as null.
    return json.loads(json.dumps(finite(actual), allow_nan=False))


def finite(value):
    if isinstance(value, float) and not math.isfinite(value):
        return None
    if isinstance(value, (list, tuple)):
        return [finite(item) for item in value]
    return value


def same(left, right) -> bool:
    """`deepEqual` in web/lib.js: Object.is, element by element for arrays."""
    if isinstance(left, list) and isinstance(right, list):
        return len(left) == len(right) and all(map(same, left, right))
    if isinstance(left, bool) or isinstance(right, bool):
        return type(left) is type(right) and left == right
    if isinstance(left, (int, float)) and isinstance(right, (int, float)):
        # Object.is on what JSON.parse reads: 0 and -0 differ, and both kinds
        # are one double, so integers past 2**53 that round together are equal.
        left, right = double(left), double(right)
        return left == right and math.copysign(1, left) == math.copysign(1, right)
    if isinstance(left, (str, type(None))):
        return type(left) is type(right) and left == right
    return False


def js_number(text: str):
    """A JSON number as JSON.parse reads it and JSON.stringify writes it back."""
    number = float(text)
    if not math.isfinite(number):
        return None
    # JSON.stringify writes the shortest digits that read back as the same
    # double, as repr does, so 2**60 arrives as 1152921504606847000 rather
    # than the double's exact value. Past 1e21 it writes an exponent, which
    # reads back as a float.
    if number.is_integer() and abs(number) < 1e21:
        return int(Decimal(repr(number)))
    return number


def double(number) -> float:
    try:
        return float(number)
    except OverflowError:
        return math.inf if number > 0 else -math.inf


def shown(value) -> str:
    return json.dumps(value)

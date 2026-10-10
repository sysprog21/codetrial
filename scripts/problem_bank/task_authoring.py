"""Start a task bank from reference tasks, so an instructor edits a working
task instead of writing one from nothing."""

from __future__ import annotations

import re
import shutil
from copy import deepcopy
from pathlib import Path

from .bank import ROOT, STARTER_LANGS, read_json, write_json
from .task_package import load_bank
from .tasks import DEFAULTS, PROMPT_VERSION, RUBRIC_PROFILE, hint_ceiling

# Complete tasks that show what a sidecar can do. Each file holds `problem`,
# `judge`, `variant` and `sidecar`, as a bank stores them for one task.
EXAMPLES = ROOT / "problem-bank/task-examples"
# The bank files `new` changes, beside what `load_bank` reads; problems.json
# names the tasks, so it goes into place last.
BANK_FILES = ("judges", "variants", "task-mode", "problems")
TASK_ID = re.compile(r"[a-z0-9][a-z0-9-]{0,63}\Z")
# Where `new` stages a bank, and the mark that the staged bank validated and
# is being moved into place.
STAGING = ".task-new"
READY = "ready"


def _catalog():
    problems = {
        problem["id"]: problem
        for problem in read_json(ROOT / "problem-bank/problems.json")
    }
    judges = read_json(ROOT / "problem-bank/judges.json")
    variants = read_json(ROOT / "problem-bank/variants.json")
    return problems, judges, variants


def references():
    """Every task `new` can start from, as (id, title, kind): the shipped
    examples first, then the catalog."""
    examples = [
        (path.stem, read_json(path)["variant"]["title"], "example")
        for path in sorted(EXAMPLES.glob("*.json"))
    ]
    problems, _, variants = _catalog()
    catalog = [(task_id, variants[task_id]["title"], "catalog") for task_id in problems]
    return examples + catalog


def reference(reference_id):
    """The reference task `reference_id` as a bank stores it, its sidecar
    `None` when it has none."""
    path = EXAMPLES / f"{reference_id}.json"
    if path.exists():
        return read_json(path)
    problems, judges, variants = _catalog()
    if reference_id not in problems:
        raise ValueError(
            f"unknown reference task {reference_id!r}; `references` lists them"
        )
    return {
        "problem": deepcopy(problems[reference_id]),
        "judge": deepcopy(judges[reference_id]),
        "variant": deepcopy(variants[reference_id]),
        "sidecar": None,
    }


def default_sidecar(variant, languages):
    """The sidecar a task gets when it names none, written out in full so
    every field is there to edit."""
    return {
        "promptVersion": PROMPT_VERSION,
        "interactionPrompt": DEFAULTS["interactionPrompt"],
        "completionTargets": [],
        "understandingChecks": deepcopy(DEFAULTS["understandingChecks"]),
        "requiredCases": [],
        "maxHintRungs": hint_ceiling(variant),
        "rubricProfile": RUBRIC_PROFILE,
        "languages": languages,
    }


def _finish(bank, staged):
    """Moves a staged bank that validated into place, problems.json last, and
    removes the staging directory. Run again after a crash, it completes the
    same move."""
    for name in ("task-set", *BANK_FILES):
        if (staged / f"{name}.json").exists():
            (staged / f"{name}.json").replace(bank / f"{name}.json")
    # The mark goes last: until it does, a later run finishes this cleanup.
    for path in staged.iterdir():
        if path.name != READY:
            path.unlink()
    (staged / READY).unlink()
    staged.rmdir()


def recover(bank):
    """Completes a `new` that stopped after its bank validated, or explains
    the staging directory one left before that. True when one was completed."""
    staged = Path(bank) / STAGING
    if not staged.exists():
        return False
    if (staged / READY).exists():
        _finish(Path(bank), staged)
        return True
    # Emptied by a finished move that stopped before removing it.
    if not any(staged.iterdir()):
        staged.rmdir()
        return False
    raise ValueError(
        f"{staged} is left from a `new` that stopped before its bank validated,"
        " or one still running; the bank itself is unchanged. If no `new` is"
        " running, delete that directory and run `new` again"
    )


def _existing(bank):
    """The bank's files as they stand, refusing a bank that is already
    invalid rather than repairing it."""
    if not bank.exists():
        return {"problems": [], "judges": {}, "variants": {}, "task-mode": {}}
    present = [name for name in BANK_FILES if (bank / f"{name}.json").exists()]
    if present and present != ["task-mode"]:
        try:
            load_bank(bank, "preview", 1)
        except (OSError, ValueError, KeyError, TypeError) as error:
            raise ValueError(
                f"{bank} is already invalid ({error}); fix it before adding a task"
            ) from error
    empty = {"problems": [], "judges": {}, "variants": {}, "task-mode": {}}
    return {
        name: read_json(bank / f"{name}.json") if name in present else empty[name]
        for name in BANK_FILES
    }


def new_task(bank, reference_id, task_id=None, languages=None):
    """Adds a copy of `reference_id` to `bank`, creating the bank if needed.
    Returns the new task's id and notes on what the instructor should check.

    The copy keeps the reference's origin, so the checks that keep a catalog
    problem's source out of what the learner reads still hold as the
    instructor edits it. `languages` replaces the reference's list, dropping
    targets marked in a language no longer offered. The whole bank is
    validated before any file changes, an id already in it is refused, and
    files `new` does not change are left as they are. Not meant for two runs
    at once on one bank."""
    bank = Path(bank)
    recover(bank)
    row = reference(reference_id)
    task_id = row["problem"]["id"] if task_id is None else task_id
    if not TASK_ID.fullmatch(task_id):
        raise ValueError(
            f"invalid task id {task_id!r}: lowercase letters, digits and dashes,"
            " not starting with a dash"
        )
    notes = []
    sidecar = row["sidecar"] or default_sidecar(row["variant"], ["python"])
    starters = row["problem"].get("starterCode", {})
    if languages is not None:
        if not languages:
            raise ValueError("--languages names no language")
        for language in languages:
            if language not in STARTER_LANGS:
                raise ValueError(
                    f"unknown language {language!r}; choose from"
                    f" {', '.join(STARTER_LANGS)}"
                )
            if language not in starters:
                raise ValueError(f"{reference_id} has no {language} starter")
        kept = [
            target
            for target in sidecar["completionTargets"]
            if target["language"] in languages
        ]
        dropped = len(sidecar["completionTargets"]) - len(kept)
        if dropped:
            notes.append(
                f"dropped {dropped} completion target(s) marked in a language"
                " no longer offered"
            )
        marked = {target["language"] for target in kept}
        unmarked = [lang for lang in languages if lang not in marked]
        if sidecar["completionTargets"] and unmarked:
            notes.append(
                f"no completion target is marked in {', '.join(unmarked)}; add"
                " a marker to its starter and a target, or the interaction"
                " prompt should not ask for one"
            )
        sidecar["completionTargets"] = kept
        sidecar["languages"] = list(languages)
    row["problem"]["id"] = task_id
    if (
        task_id != reference_id
        and row["problem"].get("origin", "leetcode") == "leetcode"
    ):
        notes.append(
            "the statement, constraints and starters are the catalog's; edit"
            " them and the variant to make the task your own"
        )

    # Taken before reading the bank, so the read and the write are one step.
    staged = bank / STAGING
    staged.mkdir(parents=True, exist_ok=False)
    try:
        files = _existing(bank)
        if any(problem["id"] == task_id for problem in files["problems"]) or any(
            task_id in files[name] for name in ("judges", "variants", "task-mode")
        ):
            raise ValueError(f"{task_id} is already in {bank}; choose another --id")
        files["problems"].append(row["problem"])
        files["judges"][task_id] = row["judge"]
        files["variants"][task_id] = row["variant"]
        files["task-mode"][task_id] = sidecar
        for name, value in files.items():
            write_json(staged / f"{name}.json", value)
        rules = bank / "task-set.json"
        existing_rules = rules.exists()
        if existing_rules:
            shutil.copyfile(rules, staged / "task-set.json")
        else:
            write_json(staged / "task-set.json", _default_rules())
        load_bank(staged, "preview", 1)
    except BaseException:
        for path in staged.iterdir():
            path.unlink()
        staged.rmdir()
        raise
    # Validated with the bank's own rules, which stay as they were.
    if existing_rules:
        (staged / "task-set.json").unlink()
    (staged / READY).write_text("")
    _finish(bank, staged)
    return task_id, notes


def _default_rules():
    """The set rules a new bank starts with, every one named so it can be
    changed in place."""
    return {
        "durationMin": DEFAULTS["durationMin"],
        "maxHintRungs": DEFAULTS["maxHintRungs"],
        "closesAt": None,
        "lookAwaySeconds": DEFAULTS["lookAwaySeconds"],
        "rulesNote": None,
    }


def next_version(output, set_id):
    """The first version of `set_id` after every one `output` holds, and
    whether it holds none. Only `output` is looked at, so it has to be the
    directory that gets published."""
    versions = [
        int(path.name)
        for path in (Path(output) / "sets" / set_id).glob("*")
        if path.is_dir() and re.fullmatch(r"[0-9]+", path.name)
    ]
    return max(versions, default=0) + 1, not versions

#!/usr/bin/env python3
"""Check that TODO.md's task dependency graph resolves and is acyclic.

The recording section carries a hand-maintained dependency graph, and prose in
that file shows it was debugged by hand: task 5 explains that it cannot own the
`users.email` columns because that would make the schema task depend on the
identity task that depends on it, and 7a and 7b are split because task 10
depends on 7a and cannot also precede it. Both of those are cycles that were
caught by someone reading carefully.

A graph that needs prose to explain why it is acyclic will grow a cycle the
first time someone adds a task without reading all of it. This checks the two
properties prose cannot: every `Depends on` reference names a task that exists,
and following them never returns to where it started.

It deliberately does not parse the "Execution order" prose. That block is
written for people, and a parser for it would fail on the writing rather than
on the graph.

TODO.md is untracked by design, so a missing file is a skip, not a failure.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TODO = ROOT / "TODO.md"

# Each section numbers its tasks from scratch, so "task 1" means different work
# depending on where it is written. A reference carries its section only when it
# crosses one.
SECTIONS = {
    "Google Meet presentation mode": "meet",
    "Production recording pipeline": "recording",
    "Browser interviewer avatar": "avatar",
}

TASK = re.compile(r"^(\d+[a-z]?)\. \*\*(.+?)\*\*")
HEADING = re.compile(r"^#{2,3} (.+)$")
REF = re.compile(r"^(\d+)([a-z]?)$")
# A reference list ends at the first word that is not part of one. That is what
# keeps `Depends on 3 and precedes 4.` from reading "precedes 4" as a
# dependency, and what stops a trailing filename from being parsed at all.
#
# `task`/`tasks` is deliberately absent: it is only ever a connective directly
# after a section name, as in `avatar tasks 0 and 4`. Accepting it anywhere let
# ordinary prose through, so `Depends on 1. Task 2 owns the schema.` read "task
# 2" as a dependency and reported task 2 as depending on itself.
CONNECTIVES = {"and", "none", ","}


def natural(task_id):
    match = REF.match(task_id)
    return (int(match.group(1)), match.group(2))


def parse(text):
    """Returns {section: {id: {'title', 'line', 'depends_raw'}}} in file order."""
    sections = {}
    current = None
    tasks = {}
    order = []
    body = []

    def flush():
        if current and body and order:
            tasks[order[-1]]["body"] = " ".join(body)

    for number, line in enumerate(text.splitlines(), start=1):
        heading = HEADING.match(line)
        if heading:
            flush()
            body.clear()
            current = None
            for prefix, name in SECTIONS.items():
                if heading.group(1).startswith(prefix):
                    current = name
                    sections.setdefault(name, {})
                    order = []
                    tasks = sections[name]
            continue
        if current is None:
            continue
        task = TASK.match(line)
        if task:
            flush()
            body.clear()
            order.append(task.group(1))
            tasks[task.group(1)] = {"title": task.group(2), "line": number, "body": ""}
        if order:
            body.append(line.strip())
    flush()
    return sections


def references(body, own_section):
    """The task ids a `Depends on ...` clause names, as (section, id) pairs."""
    match = re.search(r"Depends\s+on\s+(.*)", body)
    if not match:
        return None
    # Both dashes. Only the en dash was split, so an ASCII `1-3` stayed one
    # token, failed to parse as a reference, and stopped the scan: every
    # dependency after it was dropped and the task reported as having none.
    # `expand`'s own docstring used that spelling.
    words = (
        match.group(1)
        .replace(",", " , ")
        .replace("\u2013", " - ")
        .replace("-", " - ")
        .split()
    )

    section = own_section
    found = []
    pending_range = None
    after_section = False
    for word in words:
        # Trailing punctuation only. A standalone comma strips to nothing, and
        # an empty token is separator, not the end of the list: stripping it and
        # then falling through to `break` silently dropped every reference after
        # the first comma.
        lowered = word.lower().rstrip(".;:,)")
        if not lowered:
            continue
        if lowered in SECTIONS.values():
            section = lowered
            after_section = True
            continue
        if lowered in ("task", "tasks"):
            # Only right after a section name; anywhere else this is prose.
            if not after_section:
                break
            continue
        after_section = False
        if lowered in CONNECTIVES:
            continue
        if lowered == "-":
            # The low endpoint was already recorded as a standalone reference;
            # take it back, or `1-3` yields 1 twice.
            pending_range = found.pop() if found else None
            continue
        if REF.match(lowered):
            if pending_range:
                if not isinstance(pending_range[1], str):
                    # `1 - 3 - 5`. Reported rather than crashed in natural().
                    return None
                found.append((section, ("range", pending_range[1], lowered)))
                pending_range = None
            else:
                found.append((section, lowered))
            continue
        break
    if pending_range is not None:
        return None
    return found


def expand(found, sections):
    """Turns range references into the ids they cover, by natural task order.

    Deduplicated, because `Depends on 1-3, 2b, and 5` names 2b both ways and a
    doubled edge would make the dependency count lie.
    """
    out = []
    for section, ref in found:
        if isinstance(ref, tuple):
            _, low, high = ref
            known = sections.get(section, {})
            covered = [
                task_id
                for task_id in known
                if natural(low) <= natural(task_id) <= natural(high)
            ]
            if not covered:
                out.append((section, f"{low}-{high}"))
            out.extend((section, task_id) for task_id in sorted(covered, key=natural))
        else:
            out.append((section, ref))
    seen = []
    for edge in out:
        if edge not in seen:
            seen.append(edge)
    return seen


def main():
    if not TODO.exists():
        return 0

    sections = parse(TODO.read_text())
    if not sections:
        # TODO.md is untracked and per-developer, so a contributor keeping their
        # own notes there has no task sections and must not get a red gate on a
        # file that is not part of the repo. Said out loud rather than passed in
        # silence: if a section was renamed, this is the only thing that says the
        # graph stopped being checked.
        print(
            "check-todo-deps: TODO.md has none of the known task sections "
            f"({', '.join(sorted(SECTIONS))}); nothing to check",
            file=sys.stderr,
        )
        return 0

    problems = []
    graph = {}
    for name, tasks in sections.items():
        for task_id, task in tasks.items():
            node = (name, task_id)
            found = references(task["body"], name)
            if found is None:
                problems.append(
                    f"TODO.md:{task['line']}: task {task_id} in the {name} section "
                    f"has no readable 'Depends on' clause; write 'Depends on none.' "
                    f"if it has none, and keep ranges as `1-3` or `1\u20133`"
                )
                graph[node] = []
                continue
            if not found and "none" not in task["body"].lower():
                # A clause the tokenizer could not read used to come back empty,
                # which is indistinguishable from a task with no dependencies.
                # Silence is the one answer a dependency checker must not give.
                problems.append(
                    f"TODO.md:{task['line']}: task {task_id} names a 'Depends on' "
                    f"clause that parsed to nothing; say 'Depends on none.' or "
                    f"write the references so they can be read"
                )
            edges = expand(found, sections)
            for section, ref in edges:
                if ref not in sections.get(section, {}):
                    problems.append(
                        f"TODO.md:{task['line']}: task {task_id} depends on "
                        f"{section} task {ref}, which does not exist"
                    )
                elif (section, ref) == node:
                    problems.append(
                        f"TODO.md:{task['line']}: task {task_id} depends on itself"
                    )
            graph[node] = edges

    # Depth-first, reporting the cycle as the path that closes it rather than
    # just naming one member: the useful part of a cycle report is the loop.
    WHITE, GREY, BLACK = 0, 1, 2
    colour = {node: WHITE for node in graph}

    def walk(node, path):
        colour[node] = GREY
        for edge in graph.get(node, []):
            if edge not in colour:
                continue
            if colour[edge] == GREY:
                loop = path[path.index(edge):] + [edge]
                problems.append(
                    "TODO.md: dependency cycle: "
                    + " -> ".join(f"{section}/{task}" for section, task in loop)
                )
            elif colour[edge] == WHITE:
                walk(edge, path + [edge])
        colour[node] = BLACK

    for node in graph:
        if colour[node] == WHITE:
            walk(node, [node])

    if problems:
        for problem in sorted(set(problems)):
            print(problem, file=sys.stderr)
        return 1

    total = sum(len(tasks) for tasks in sections.values())
    edges = sum(len(value) for value in graph.values())
    print(f"check-todo-deps: {total} tasks, {edges} dependencies, no cycles")
    return 0


if __name__ == "__main__":
    sys.exit(main())

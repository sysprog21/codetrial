#!/usr/bin/env python3

"""Check and replay the continuity-gap state table in docs/turn-timing.md.

The table is the specification: this script reads the fenced `gap-table` block
from that document, proves it is a total, deterministic function of
(state, event, guards), and replays event sequences through it while enforcing
invariants no row may break. It also applies the attributable-loss rule to arm
reports. Nothing here is product code; the product does not read the table.

  check [--doc PATH]       validate the table (exit 1 on a defect)
  classify ARMS.json       apply the attributable-loss rule to arm reports

Exit codes: 0 pass, 1 table defect, 2 unusable input.
"""

from __future__ import annotations

import argparse
import copy
import itertools
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_DOC = ROOT / "docs" / "turn-timing.md"

STATES = ("CLEAR", "OPEN", "DUE", "SENDING", "REQUESTED")
EVENTS = (
    "loss_silent",
    "loss_voiced",
    "sdk_unknown",
    "candidate_start",
    "floor_released",
    "busy_start",
    "busy_end",
    "send_ok",
    "send_failed",
    "request_undelivered",
    "evidence_turn",
    "session_end",
)
GUARDS = (
    "dup",
    "busy",
    "candidate_active",
    "budget_left",
    "attempts_left",
    "post_send",
    "late_loss",
)
ACTIONS = (
    "noop",
    "note_trim",
    "note_unknown",
    "note_dup",
    "open_gap",
    "preserve_debt",
    "merge_gap",
    "mark_late",
    "reopen_gap",
    "supersede",
    "defer_no_interrupt",
    "send_request",
    "commit_request",
    "keep_debt",
    "keep_budget_spent",
    "settle",
    "unrepaired_budget",
    "unrepaired_attempts",
    "end_unsettled",
    "invalid",
)
# Actions that end an episode: the debt is paid or explicitly written off.
ENDS_EPISODE = ("settle", "unrepaired_budget", "unrepaired_attempts", "end_unsettled")
CAP_NAMES = ("session_requests", "episode_attempts")


class TableError(Exception):
    """The table is malformed, partial or ambiguous."""


class ReplayError(Exception):
    """A replayed sequence broke an invariant or hit an impossible event."""


class ImpossibleEvent(ReplayError):
    """The table marks this event as unable to occur in the current state."""

    def __init__(self, message: str, line: int):
        super().__init__(message)
        self.line = line


def extract_block(text: str) -> str:
    found = re.findall(r"```gap-table\n(.*?)```", text, re.S)
    if len(found) != 1:
        raise TableError(f"expected exactly one gap-table block, found {len(found)}")
    return found[0]


class Table:
    def __init__(self, text: str):
        self.caps: dict[str, int] = {}
        self.rows: list[dict] = []
        for number, raw in enumerate(extract_block(text).splitlines(), 1):
            line = raw.strip()
            if not line or line.startswith("#"):
                continue
            if line.startswith("@cap"):
                parts = line.split()
                if (
                    len(parts) != 3
                    or parts[1] not in CAP_NAMES
                    or not parts[2].isdigit()
                ):
                    raise TableError(f"line {number}: bad cap {line!r}")
                if parts[1] in self.caps:
                    raise TableError(f"line {number}: cap {parts[1]} set twice")
                self.caps[parts[1]] = int(parts[2])
                continue
            self._row(number, line)
        for name in CAP_NAMES:
            if name not in self.caps:
                raise TableError(f"missing @cap {name}")

    def _row(self, number: int, line: str):
        fields = [field.strip() for field in line.split("|")]
        if len(fields) != 5:
            raise TableError(f"line {number}: want 5 fields, got {len(fields)}")
        state, event, guards, nxt, actions = fields
        if state != "*" and state not in STATES:
            raise TableError(f"line {number}: unknown state {state!r}")
        if event not in EVENTS:
            raise TableError(f"line {number}: unknown event {event!r}")
        literals: dict[str, bool] = {}
        if guards != "-":
            for item in guards.split(","):
                item = item.strip()
                name = item.lstrip("!")
                if name not in GUARDS:
                    raise TableError(f"line {number}: unknown guard {item!r}")
                if name in literals:
                    raise TableError(f"line {number}: guard {name} repeated")
                literals[name] = not item.startswith("!")
        if nxt != "=" and nxt not in STATES:
            raise TableError(f"line {number}: unknown next state {nxt!r}")
        names = tuple(a.strip() for a in actions.split(","))
        for name in names:
            if name not in ACTIONS:
                raise TableError(f"line {number}: unknown action {name!r}")
        if "invalid" in names and len(names) > 1:
            raise TableError(
                f"line {number}: invalid cannot combine with other actions"
            )
        for scope in STATES if state == "*" else (state,):
            self.rows.append(
                {
                    "state": scope,
                    "event": event,
                    "guards": literals,
                    "next": scope if nxt == "=" else nxt,
                    "actions": names,
                    "line": number,
                }
            )

    def match(self, state: str, event: str, guards: dict[str, bool]) -> list[dict]:
        return [
            row
            for row in self.rows
            if row["state"] == state
            and row["event"] == event
            and all(guards[name] == want for name, want in row["guards"].items())
        ]

    def validate(self) -> int:
        """Every (state, event, guard assignment) selects exactly one row."""
        selected: set[int] = set()
        problems = []
        combos = 0
        for state, event in itertools.product(STATES, EVENTS):
            for values in itertools.product((False, True), repeat=len(GUARDS)):
                combos += 1
                guards = dict(zip(GUARDS, values))
                rows = self.match(state, event, guards)
                if len(rows) == 1:
                    selected.add(id(rows[0]))
                    continue
                shown = sorted(k for k, v in guards.items() if v)
                problems.append(
                    f"{state}/{event} {shown}: {len(rows)} rows {[r['line'] for r in rows]}"
                )
        for row in self.rows:
            if id(row) not in selected:
                problems.append(f"line {row['line']}: row can never be selected")
        if problems:
            more = f" (+{len(problems) - 6} more)" if len(problems) > 6 else ""
            raise TableError("; ".join(problems[:6]) + more)
        return combos


class Machine:
    """One session's gap state, driven only by the table."""

    def __init__(self, table: Table):
        self.table = table
        self.state = "CLEAR"
        self.busy = False
        self.debt = False
        self.late = False
        self.candidate_active = False
        self.attempts = 0
        self.session_requests = 0
        self.seen: set[str] = set()
        # (event, state before, state after, actions, table line)
        self.history: list[tuple[str, str, str, tuple[str, ...], int]] = []

    def clone(self) -> "Machine":
        other = copy.copy(self)
        other.seen = set(self.seen)
        other.history = list(self.history)
        return other

    def step(self, event: str, ident: str | None = None, post_send: bool = False):
        if event not in EVENTS:
            raise ReplayError(f"unknown event {event!r}")
        if event == "loss_voiced" and not ident:
            raise ReplayError("loss_voiced needs an interval id")
        if event == "busy_start":
            self.busy = True
        elif event == "busy_end":
            self.busy = False
        elif event == "candidate_start":
            self.candidate_active = True
        elif event == "floor_released":
            self.candidate_active = False
        guards = {
            "dup": event == "loss_voiced" and ident in self.seen,
            "busy": self.busy,
            "candidate_active": self.candidate_active,
            "budget_left": self.session_requests < self.table.caps["session_requests"],
            "attempts_left": self.attempts < self.table.caps["episode_attempts"],
            "post_send": post_send,
            "late_loss": self.late,
        }
        rows = self.table.match(self.state, event, guards)
        if len(rows) != 1:
            raise ReplayError(f"{self.state}/{event}: {len(rows)} rows match")
        row = rows[0]
        before, actions = self.state, row["actions"]
        if "invalid" in actions:
            raise ImpossibleEvent(
                f"{event} is not a possible event in {before}", row["line"]
            )
        if event == "loss_voiced" and not guards["dup"]:
            self.seen.add(ident)
        for action in actions:
            self._apply(action, before, guards)
        self.state = row["next"]
        if self.debt != (self.state != "CLEAR"):
            raise ReplayError("preserved debt disagrees with the gap state")
        if "send_request" in actions and self.state != "SENDING":
            raise ReplayError("a request was sent but the state is not SENDING")
        self.history.append((event, before, self.state, actions, row["line"]))
        return self.state, actions

    def _apply(self, action: str, before: str, guards: dict[str, bool]):
        caps = self.table.caps
        if action in ("open_gap", "preserve_debt"):
            self.debt = True
        if action in ("open_gap", "reopen_gap"):
            self.attempts = 0
            self.late = False
        if action == "send_request":
            if guards["busy"]:
                raise ReplayError("a request was sent while the interviewer was busy")
            if self.candidate_active:
                raise ReplayError("a request was sent while the candidate was speaking")
            if before not in ("OPEN", "DUE"):
                raise ReplayError(f"a request was sent from {before}")
            if not self.debt:
                raise ReplayError("a request was sent with no gap owed")
            self.attempts += 1
            self.late = False
            if self.attempts > caps["episode_attempts"]:
                raise ReplayError("episode attempt cap exceeded")
        if action == "mark_late":
            self.late = True
        if action == "commit_request":
            if before != "SENDING":
                raise ReplayError(f"a request was committed from {before}")
            self.session_requests += 1
            if self.session_requests > caps["session_requests"]:
                raise ReplayError("session request cap exceeded")
        if action == "settle" and not (before == "REQUESTED" and guards["post_send"]):
            raise ReplayError("settled without post-request evidence")
        if action in ENDS_EPISODE:
            if before == "CLEAR":
                raise ReplayError(f"{action} with no open gap")
            self.debt = False
            self.attempts = 0
            self.late = False


def explore(table: Table):
    """Breadth-first over every reachable abstract state and event.

    Returns (rows reached, rows that raised ImpossibleEvent). An invariant
    break on any reachable path raises ReplayError. The interval ids are
    abstracted to "a new id" and "an id already seen".
    """
    reached: set[int] = set()
    impossible: set[int] = set()
    start = Machine(table)

    def key(m):
        return (
            m.state, m.busy, m.candidate_active, m.late, m.attempts,
            m.session_requests, bool(m.seen),
        )  # fmt: skip

    queue, known = [start], {key(start)}
    counter = 0
    while queue:
        base = queue.pop()
        for event in EVENTS:
            variants = [{}]
            if event == "loss_voiced":
                variants = [{"fresh": True}] + ([{"fresh": False}] if base.seen else [])
            if event == "evidence_turn":
                variants = [{"post_send": True}, {"post_send": False}]
            for variant in variants:
                machine = base.clone()
                ident = None
                if event == "loss_voiced":
                    counter += 1
                    ident = f"n{counter}" if variant["fresh"] else sorted(base.seen)[0]
                try:
                    machine.step(event, ident, variant.get("post_send", False))
                except ImpossibleEvent as error:
                    impossible.add(error.line)
                    continue
                reached.add(machine.history[-1][4])
                if key(machine) not in known:
                    known.add(key(machine))
                    queue.append(machine)
    return reached, impossible


def defensive(row: dict) -> bool:
    """A release or retry with the attempts already spent cannot be reached:
    spending the last attempt ends the gap. These rows stay so the table is
    total; any other row that is never reached is a defect."""
    return row["guards"].get("attempts_left") is False and row["event"] in (
        "floor_released",
        "busy_end",
    )


def unreachable(table: Table) -> list[dict]:
    """Rows no reachable path selects, other than the defensive ones.

    Raises ReplayError if any reachable path breaks an invariant.
    """
    reached, impossible = explore(table)
    return [
        row
        for row in table.rows
        if row["line"] not in reached | impossible and not defensive(row)
    ]


def replay(table: Table, events) -> Machine:
    """Run event names or (name, options) pairs; returns the final machine."""
    machine = Machine(table)
    for item in events:
        name, options = (item, {}) if isinstance(item, str) else item
        machine.step(name, options.get("id"), options.get("post_send", False))
    return machine


# The attributable-loss rule.

DEVIATIONS = ("missing", "repeated")


def classify_turn(control_status, turn, arm):
    """Verdict for one answer-key turn of a stall arm against its control.

    All of these must hold for `attributable`: the turn is present in the
    unstalled control, absent or repeated in this arm, its transmitted audio
    is verified, and a drop or quarantine interval from this same run overlaps
    that audio. Anything else is `inconclusive` with every unmet condition
    named (or `none` when the turn is intact). A speech-to-text mismatch is
    never enough, and a drop the SDK did not report is not an interval.
    """
    status = turn["status"]
    if status == "intact":
        return {"verdict": "none", "unmet": []}
    if status not in DEVIATIONS:
        return {"verdict": "inconclusive", "unmet": [f"status_{status}_is_not_a_loss"]}
    unmet = []
    if control_status != "intact":
        unmet.append("control_does_not_have_the_turn")
    audio = turn.get("transmitted_ms")
    if not turn.get("transmitted_verified") or not audio:
        unmet.append("transmitted_audio_not_verified")
    intervals = [
        i
        for i in arm.get("local_intervals", [])
        if i.get("run_id") == arm["run_id"] and i.get("kind") in ("drop", "quarantine")
    ]
    if not intervals:
        unmet.append("no_same_run_local_interval")
    elif not audio or not any(
        i["start_ms"] < i["end_ms"]
        and audio[0] < audio[1]
        and i["start_ms"] < audio[1]
        and audio[0] < i["end_ms"]
        for i in intervals
    ):
        unmet.append("no_overlap_with_transmitted_audio")
    return {"verdict": "inconclusive" if unmet else "attributable", "unmet": unmet}


def classify_report(report) -> dict:
    control_id = report["control"]
    arms = {arm["id"]: arm for arm in report["arms"]}
    control = arms[control_id]
    control_ids = [t["id"] for t in control["turns"]]
    if len(set(control_ids)) != len(control_ids) or not control_ids:
        raise ValueError("the control needs distinct, non-empty answer-key turns")
    control_status = {t["id"]: t["status"] for t in control["turns"]}
    out = []
    for arm in report["arms"]:
        if arm["id"] == control_id:
            continue
        if sorted(t["id"] for t in arm["turns"]) != sorted(control_ids):
            raise ValueError(
                f"arm {arm['id']} does not have exactly the control's turns"
            )
        turns = {
            t["id"]: classify_turn(control_status.get(t["id"]), t, arm)
            for t in arm["turns"]
        }
        attributable = [k for k, v in turns.items() if v["verdict"] == "attributable"]
        every_turn_lost = all(t["status"] in DEVIATIONS for t in arm["turns"])
        counted = any(arm.get("local_totals", {}).values()) or arm.get(
            "local_intervals"
        )
        silent = (
            not counted
            and arm.get("transcript_lines") == 0
            and arm.get("frames_dequeued", 0) > 0
            and control.get("transcript_lines", 0) > 0
            and every_turn_lost
        )
        out.append(
            {
                "arm": arm["id"],
                "turns": turns,
                "attributable_turns": attributable,
                "unexplained_whole_utterance_silence": silent and not attributable,
            }
        )
    if not out:
        raise ValueError("no stall arm to compare against the control")
    if any(a["attributable_turns"] for a in out):
        decision = "attributable-loss-present"
    elif all(t["status"] == "intact" for arm in report["arms"] for t in arm["turns"]):
        decision = "no-change"
    else:
        decision = "inconclusive"
    return {"decision": decision, "descriptive_only": True, "arms": out}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="command", required=True)
    check = sub.add_parser("check")
    check.add_argument("--doc", type=Path, default=DEFAULT_DOC)
    classify = sub.add_parser("classify")
    classify.add_argument("arms", type=Path)
    args = parser.parse_args(argv)
    try:
        if args.command == "check":
            table = Table(args.doc.read_text())
            combos = table.validate()
            dead = unreachable(table)
            if dead:
                raise TableError(
                    "rows no reachable path selects: "
                    + ", ".join(str(r["line"]) for r in dead)
                )
            print(
                f"gap-state table ok: {len(table.rows)} rows, {len(STATES)} states, "
                f"{len(EVENTS)} events, {combos} guard combinations each selecting one row"
            )
            return 0
        result = classify_report(json.loads(args.arms.read_text()))
    except (TableError, ReplayError) as error:
        print(f"gap-state table defect: {error}", file=sys.stderr)
        return 1
    except (
        OSError,
        ValueError,
        KeyError,
        TypeError,
        IndexError,
        AttributeError,
    ) as error:
        print(f"unusable input: {error!r}", file=sys.stderr)
        return 2
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())

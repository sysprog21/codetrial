#!/usr/bin/env python3
"""Fixtures for the continuity-gap state table in docs/turn-timing.md.

Each fixture is a sequence of events with the state and actions the table must
produce at every step. `GOOD` fixtures pass against the real table; each `BAD`
fixture states something the specification forbids (a duplicate or
contradictory settlement, a request on silence, a send while busy) and must
make the replay fail. `MUTANTS` change one line of the table and name the
fixture, or the totality check, that must notice.
"""

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "check-gap-state.py"
DOC = ROOT / "docs" / "turn-timing.md"

spec = importlib.util.spec_from_file_location("check_gap_state", SCRIPT)
gap = importlib.util.module_from_spec(spec)
sys.modules["check_gap_state"] = gap
spec.loader.exec_module(gap)

DOC_TEXT = DOC.read_text()


class FixtureFailure(Exception):
    def __init__(self, message, index=None):
        super().__init__(message)
        self.index = index


def lv(ident):
    return ("loss_voiced", {"id": ident})


POST = ("evidence_turn", {"post_send": True})
PRE = ("evidence_turn", {"post_send": False})


def step(event, state, *actions):
    return (event, state, tuple(actions))


def run_fixture(table, fixture):
    """Replay a fixture; any mismatch, invariant break or impossible event fails."""
    machine = gap.Machine(table)
    for index, (event, state, actions) in enumerate(fixture["steps"]):
        name, options = (event, {}) if isinstance(event, str) else event
        try:
            got_state, got_actions = machine.step(
                name, options.get("id"), options.get("post_send", False)
            )
        except gap.ReplayError as error:
            raise FixtureFailure(
                f"{fixture['name']} step {index} {name}: {error}", index
            )
        if (got_state, got_actions) != (state, actions):
            raise FixtureFailure(
                f"{fixture['name']} step {index} {name}: "
                f"got {got_state} {got_actions}, want {state} {actions}",
                index,
            )
    want = fixture.get("final", {})
    for key, value in want.items():
        if getattr(machine, key) != value:
            raise FixtureFailure(
                f"{fixture['name']}: {key} is {getattr(machine, key)}, want {value}"
            )
    return machine


def sent(machine):
    return sum("send_request" in h[3] for h in machine.history)


def settled(machine):
    return sum("settle" in h[3] for h in machine.history)


def settled_episode(ident):
    """One gap from first loss to settlement, spending one request."""
    return [
        step(lv(ident), "OPEN", "open_gap", "preserve_debt"),
        step("floor_released", "SENDING", "send_request"),
        step("send_ok", "REQUESTED", "commit_request"),
        step(POST, "CLEAR", "settle"),
    ]


GOOD = [
    {
        "name": "silence_only_trimming_sends_nothing",
        "steps": [
            step("loss_silent", "CLEAR", "note_trim"),
            step("loss_silent", "CLEAR", "note_trim"),
            step("floor_released", "CLEAR", "noop"),
            step(POST, "CLEAR", "noop"),
        ],
        "final": {"session_requests": 0, "debt": False},
    },
    {
        "name": "voiced_gap_one_request_settled_by_later_evidence",
        "steps": [
            step(lv("a"), "OPEN", "open_gap", "preserve_debt"),
            step("floor_released", "SENDING", "send_request"),
            step("send_ok", "REQUESTED", "commit_request"),
            step(POST, "CLEAR", "settle"),
        ],
        "final": {"session_requests": 1, "debt": False},
    },
    {
        "name": "repeated_notices_open_and_request_once",
        "steps": [
            step(lv("a"), "OPEN", "open_gap", "preserve_debt"),
            step(lv("a"), "OPEN", "note_dup"),
            step(lv("b"), "OPEN", "merge_gap"),
            step(lv("b"), "OPEN", "note_dup"),
            step("floor_released", "SENDING", "send_request"),
            step(lv("a"), "SENDING", "note_dup"),
            step("send_ok", "REQUESTED", "commit_request"),
            step(POST, "CLEAR", "settle"),
            step(lv("a"), "CLEAR", "note_dup"),
            step("floor_released", "CLEAR", "noop"),
        ],
        "final": {"session_requests": 1},
    },
    {
        "name": "thinking_defers_the_request_and_never_interrupts",
        "steps": [
            step(lv("a"), "OPEN", "open_gap", "preserve_debt"),
            step("busy_start", "OPEN", "noop"),
            step("floor_released", "DUE", "defer_no_interrupt"),
            step("floor_released", "DUE", "defer_no_interrupt"),
            step("busy_end", "SENDING", "send_request"),
            step("send_ok", "REQUESTED", "commit_request"),
            step(POST, "CLEAR", "settle"),
        ],
        "final": {"session_requests": 1},
    },
    {
        "name": "superseding_input_defers_and_pre_request_evidence_does_not_settle",
        "steps": [
            step(lv("a"), "OPEN", "open_gap", "preserve_debt"),
            step("busy_start", "OPEN", "noop"),
            step("floor_released", "DUE", "defer_no_interrupt"),
            step("candidate_start", "OPEN", "supersede"),
            step("busy_end", "OPEN", "noop"),
            step("floor_released", "SENDING", "send_request"),
            step("send_ok", "REQUESTED", "commit_request"),
            step("candidate_start", "REQUESTED", "noop"),
            step(PRE, "REQUESTED", "noop"),
            step(POST, "CLEAR", "settle"),
        ],
        "final": {"session_requests": 1},
    },
    {
        "name": "failed_send_retries_once_at_the_next_idle_edge_then_writes_off",
        "steps": [
            step(lv("a"), "OPEN", "open_gap", "preserve_debt"),
            step("floor_released", "SENDING", "send_request"),
            step("send_failed", "DUE", "keep_debt"),
            step("floor_released", "SENDING", "send_request"),
            step("send_failed", "CLEAR", "unrepaired_attempts"),
            step("floor_released", "CLEAR", "noop"),
        ],
        "final": {"session_requests": 0, "debt": False},
    },
    {
        "name": "failed_send_then_retry_succeeds_after_recovery",
        "steps": [
            step(lv("a"), "OPEN", "open_gap", "preserve_debt"),
            step("floor_released", "SENDING", "send_request"),
            step("send_failed", "DUE", "keep_debt"),
            step("busy_start", "DUE", "noop"),
            step("busy_end", "SENDING", "send_request"),
            step("send_ok", "REQUESTED", "commit_request"),
            step(POST, "CLEAR", "settle"),
        ],
        "final": {"session_requests": 1},
    },
    {
        "name": "unknown_sdk_drop_opens_no_gap",
        "steps": [
            step("sdk_unknown", "CLEAR", "note_unknown"),
            step("floor_released", "CLEAR", "noop"),
            step(POST, "CLEAR", "noop"),
        ],
        "final": {"session_requests": 0},
    },
    {
        "name": "loss_while_sending_reopens_after_the_committed_send",
        "steps": [
            step(lv("a"), "OPEN", "open_gap", "preserve_debt"),
            step("floor_released", "SENDING", "send_request"),
            step(lv("b"), "SENDING", "merge_gap", "mark_late"),
            step("send_ok", "OPEN", "commit_request", "reopen_gap"),
            step(POST, "OPEN", "noop"),
            step("floor_released", "SENDING", "send_request"),
            step("send_ok", "REQUESTED", "commit_request"),
            step(POST, "CLEAR", "settle"),
        ],
        "final": {"session_requests": 2},
    },
    {
        "name": "loss_after_the_request_reopens_and_needs_new_evidence",
        "steps": [
            step(lv("a"), "OPEN", "open_gap", "preserve_debt"),
            step("floor_released", "SENDING", "send_request"),
            step("send_ok", "REQUESTED", "commit_request"),
            step(lv("b"), "OPEN", "reopen_gap"),
            step(POST, "OPEN", "noop"),
        ],
        "final": {"session_requests": 1, "debt": True},
    },
    {
        "name": "undelivered_request_does_not_settle_and_returns_to_due",
        "steps": [
            step(lv("a"), "OPEN", "open_gap", "preserve_debt"),
            step("floor_released", "SENDING", "send_request"),
            step("send_ok", "REQUESTED", "commit_request"),
            step("request_undelivered", "DUE", "keep_budget_spent"),
            step(POST, "DUE", "noop"),
            step("floor_released", "SENDING", "send_request"),
            step("send_ok", "REQUESTED", "commit_request"),
            step("request_undelivered", "CLEAR", "unrepaired_attempts"),
        ],
        "final": {"session_requests": 2, "debt": False},
    },
    {
        "name": "the_session_budget_writes_off_a_fourth_gap_without_a_request",
        "steps": [
            step(lv("a"), "OPEN", "open_gap", "preserve_debt"),
            step("floor_released", "SENDING", "send_request"),
            step("send_ok", "REQUESTED", "commit_request"),
            step(POST, "CLEAR", "settle"),
            step(lv("b"), "OPEN", "open_gap", "preserve_debt"),
            step("floor_released", "SENDING", "send_request"),
            step("send_ok", "REQUESTED", "commit_request"),
            step(POST, "CLEAR", "settle"),
            step(lv("c"), "OPEN", "open_gap", "preserve_debt"),
            step("floor_released", "SENDING", "send_request"),
            step("send_ok", "REQUESTED", "commit_request"),
            step(POST, "CLEAR", "settle"),
            step(lv("d"), "OPEN", "open_gap", "preserve_debt"),
            step("floor_released", "CLEAR", "unrepaired_budget"),
        ],
        "final": {"session_requests": 3},
    },
    {
        "name": "failed_send_while_the_candidate_speaks_waits_for_the_next_release",
        "steps": [
            step(lv("a"), "OPEN", "open_gap", "preserve_debt"),
            step("floor_released", "SENDING", "send_request"),
            step("candidate_start", "SENDING", "noop"),
            step("send_failed", "OPEN", "supersede"),
            step("busy_start", "OPEN", "noop"),
            step("busy_end", "OPEN", "noop"),
            step("floor_released", "SENDING", "send_request"),
            step("send_ok", "REQUESTED", "commit_request"),
            step(POST, "CLEAR", "settle"),
        ],
        "final": {"session_requests": 1},
    },
    {
        "name": "undelivered_request_while_the_candidate_speaks_waits_for_the_next_release",
        "steps": [
            step(lv("a"), "OPEN", "open_gap", "preserve_debt"),
            step("floor_released", "SENDING", "send_request"),
            step("send_ok", "REQUESTED", "commit_request"),
            step("candidate_start", "REQUESTED", "noop"),
            step("request_undelivered", "OPEN", "keep_budget_spent", "supersede"),
            step("busy_end", "OPEN", "noop"),
            step("floor_released", "SENDING", "send_request"),
            step("send_ok", "REQUESTED", "commit_request"),
            step(POST, "CLEAR", "settle"),
        ],
        "final": {"session_requests": 2},
    },
    {
        "name": "a_due_gap_merges_dedups_and_ignores_idle_events",
        "steps": [
            step(lv("a"), "OPEN", "open_gap", "preserve_debt"),
            step("busy_start", "OPEN", "noop"),
            step("floor_released", "DUE", "defer_no_interrupt"),
            step(lv("a"), "DUE", "note_dup"),
            step(lv("b"), "DUE", "merge_gap"),
            step("busy_end", "SENDING", "send_request"),
            step("floor_released", "SENDING", "noop"),
            step("busy_start", "SENDING", "noop"),
            step("busy_end", "SENDING", "noop"),
            step(PRE, "SENDING", "noop"),
            step("send_ok", "REQUESTED", "commit_request"),
            step("floor_released", "REQUESTED", "noop"),
            step("busy_end", "REQUESTED", "noop"),
            step(lv("a"), "REQUESTED", "note_dup"),
            step(POST, "CLEAR", "settle"),
            step("busy_end", "CLEAR", "noop"),
            step("candidate_start", "CLEAR", "noop"),
            step("session_end", "CLEAR", "noop"),
        ],
        "final": {"session_requests": 1},
    },
    {
        "name": "candidate_speech_in_an_open_gap_changes_nothing_until_release",
        "steps": [
            step(lv("a"), "OPEN", "open_gap", "preserve_debt"),
            step("candidate_start", "OPEN", "noop"),
            step("session_end", "CLEAR", "end_unsettled"),
        ],
        "final": {"debt": False},
    },
    {
        "name": "session_end_while_due_writes_off",
        "steps": [
            step(lv("a"), "OPEN", "open_gap", "preserve_debt"),
            step("busy_start", "OPEN", "noop"),
            step("floor_released", "DUE", "defer_no_interrupt"),
            step("session_end", "CLEAR", "end_unsettled"),
        ],
    },
    {
        "name": "session_end_while_sending_writes_off",
        "steps": [
            step(lv("a"), "OPEN", "open_gap", "preserve_debt"),
            step("floor_released", "SENDING", "send_request"),
            step("session_end", "CLEAR", "end_unsettled"),
        ],
    },
    {
        "name": "session_end_while_requested_writes_off_without_settling",
        "steps": [
            step(lv("a"), "OPEN", "open_gap", "preserve_debt"),
            step("floor_released", "SENDING", "send_request"),
            step("send_ok", "REQUESTED", "commit_request"),
            step("session_end", "CLEAR", "end_unsettled"),
        ],
        "final": {"session_requests": 1},
    },
    {
        "name": "an_undelivered_request_at_the_session_cap_is_written_off_at_release",
        "steps": [
            *settled_episode("a"),
            *settled_episode("b"),
            step(lv("c"), "OPEN", "open_gap", "preserve_debt"),
            step("floor_released", "SENDING", "send_request"),
            step("send_ok", "REQUESTED", "commit_request"),
            step("request_undelivered", "DUE", "keep_budget_spent"),
            step("floor_released", "CLEAR", "unrepaired_budget"),
        ],
        "final": {"session_requests": 3, "debt": False},
    },
    {
        "name": "an_undelivered_request_at_the_session_cap_is_written_off_at_idle",
        "steps": [
            *settled_episode("a"),
            *settled_episode("b"),
            step(lv("c"), "OPEN", "open_gap", "preserve_debt"),
            step("floor_released", "SENDING", "send_request"),
            step("send_ok", "REQUESTED", "commit_request"),
            step("request_undelivered", "DUE", "keep_budget_spent"),
            step("busy_start", "DUE", "noop"),
            step("busy_end", "CLEAR", "unrepaired_budget"),
        ],
        "final": {"session_requests": 3, "debt": False},
    },
    {
        "name": "session_end_writes_off_an_open_gap_without_settling",
        "steps": [
            step(lv("a"), "OPEN", "open_gap", "preserve_debt"),
            step("session_end", "CLEAR", "end_unsettled"),
        ],
        "final": {"debt": False},
    },
]

# Each of these asserts something the specification forbids; replaying it must
# raise FixtureFailure. The comment states the rule it breaks.
BAD = [
    {  # a second settlement of one episode
        "name": "duplicate_settlement",
        "steps": [
            step(lv("a"), "OPEN", "open_gap", "preserve_debt"),
            step("floor_released", "SENDING", "send_request"),
            step("send_ok", "REQUESTED", "commit_request"),
            step(POST, "CLEAR", "settle"),
            step(POST, "CLEAR", "settle"),
        ],
    },
    {  # settlement credited to silence-only trimming
        "name": "settlement_after_silence_only",
        "steps": [
            step("loss_silent", "CLEAR", "note_trim"),
            step(POST, "CLEAR", "settle"),
        ],
    },
    {  # evidence that began before the request settling it
        "name": "settlement_on_pre_request_evidence",
        "steps": [
            step(lv("a"), "OPEN", "open_gap", "preserve_debt"),
            step("floor_released", "SENDING", "send_request"),
            step("send_ok", "REQUESTED", "commit_request"),
            step(PRE, "CLEAR", "settle"),
        ],
    },
    {  # a request sent while the interviewer is busy
        "name": "send_while_busy",
        "steps": [
            step(lv("a"), "OPEN", "open_gap", "preserve_debt"),
            step("busy_start", "OPEN", "noop"),
            step("floor_released", "SENDING", "send_request"),
        ],
    },
    {  # a second request in flight for the same episode
        "name": "second_concurrent_request",
        "steps": [
            step(lv("a"), "OPEN", "open_gap", "preserve_debt"),
            step("floor_released", "SENDING", "send_request"),
            step("floor_released", "SENDING", "send_request"),
        ],
    },
    {  # a repeat request for trimming
        "name": "request_on_silence_only_trim",
        "steps": [
            step("loss_silent", "CLEAR", "note_trim"),
            step("floor_released", "SENDING", "send_request"),
        ],
    },
    {  # an unknown SDK drop turned into a gap
        "name": "unknown_sdk_drop_opens_a_gap",
        "steps": [step("sdk_unknown", "OPEN", "open_gap", "preserve_debt")],
    },
    {  # a send result with no send in flight
        "name": "send_ok_with_nothing_sent",
        "steps": [step("send_ok", "CLEAR", "commit_request")],
    },
    {  # a repeated notice opening a second episode
        "name": "repeated_notice_reopens_after_settlement",
        "steps": [
            step(lv("a"), "OPEN", "open_gap", "preserve_debt"),
            step("floor_released", "SENDING", "send_request"),
            step("send_ok", "REQUESTED", "commit_request"),
            step(POST, "CLEAR", "settle"),
            step(lv("a"), "OPEN", "open_gap", "preserve_debt"),
        ],
    },
    {  # settlement dropped when the final request is undelivered
        "name": "undelivered_request_settles",
        "steps": [
            step(lv("a"), "OPEN", "open_gap", "preserve_debt"),
            step("floor_released", "SENDING", "send_request"),
            step("send_ok", "REQUESTED", "commit_request"),
            step("request_undelivered", "CLEAR", "settle"),
        ],
    },
    {  # a retry in a loop with no idle edge
        "name": "failed_send_retries_immediately",
        "steps": [
            step(lv("a"), "OPEN", "open_gap", "preserve_debt"),
            step("floor_released", "SENDING", "send_request"),
            step("send_failed", "SENDING", "send_request"),
        ],
    },
    {  # a third attempt for one episode
        "name": "third_attempt_for_one_episode",
        "steps": [
            step(lv("a"), "OPEN", "open_gap", "preserve_debt"),
            step("floor_released", "SENDING", "send_request"),
            step("send_failed", "DUE", "keep_debt"),
            step("floor_released", "SENDING", "send_request"),
            step("send_failed", "DUE", "keep_debt"),
        ],
    },
]

# One line of the table changed. `detector` is "table" when the totality check
# must refuse the result, otherwise the fixture that must fail.
MUTANTS = [
    (
        "repeat_notice_reopens",
        "CLEAR | loss_voiced | dup | = | note_dup",
        "CLEAR | loss_voiced | dup | OPEN | open_gap,preserve_debt",
        "repeated_notices_open_and_request_once",
    ),
    (
        "silence_opens_a_gap",
        "* | loss_silent | - | = | note_trim",
        "* | loss_silent | - | OPEN | open_gap,preserve_debt",
        "silence_only_trimming_sends_nothing",
    ),
    (
        "unknown_drop_opens_a_gap",
        "* | sdk_unknown | - | = | note_unknown",
        "* | sdk_unknown | - | OPEN | open_gap,preserve_debt",
        "unknown_sdk_drop_opens_no_gap",
    ),
    (
        "send_while_busy",
        "OPEN | floor_released | busy | DUE | defer_no_interrupt",
        "OPEN | floor_released | busy | SENDING | send_request",
        "thinking_defers_the_request_and_never_interrupts",
    ),
    (
        "supersede_still_sends",
        "DUE | candidate_start | - | OPEN | supersede",
        "DUE | candidate_start | - | SENDING | send_request",
        "superseding_input_defers_and_pre_request_evidence_does_not_settle",
    ),
    (
        "pre_request_evidence_settles",
        "REQUESTED | evidence_turn | !post_send | = | noop",
        "REQUESTED | evidence_turn | !post_send | CLEAR | settle",
        "superseding_input_defers_and_pre_request_evidence_does_not_settle",
    ),
    (
        "evidence_settles_without_a_request",
        "OPEN | evidence_turn | - | = | noop",
        "OPEN | evidence_turn | - | CLEAR | settle",
        "loss_while_sending_reopens_after_the_committed_send",
    ),
    (
        "budget_ignored",
        "OPEN | floor_released | !busy,!budget_left | CLEAR | unrepaired_budget",
        "OPEN | floor_released | !busy,!budget_left | SENDING | send_request",
        "the_session_budget_writes_off_a_fourth_gap_without_a_request",
    ),
    (
        "failed_send_forgets_the_debt",
        "SENDING | send_failed | attempts_left,!candidate_active | DUE | keep_debt",
        "SENDING | send_failed | attempts_left,!candidate_active | CLEAR | noop",
        "failed_send_retries_once_at_the_next_idle_edge_then_writes_off",
    ),
    (
        "unbounded_retry",
        "SENDING | send_failed | !attempts_left | CLEAR | unrepaired_attempts",
        "SENDING | send_failed | !attempts_left | DUE | keep_debt",
        "failed_send_retries_once_at_the_next_idle_edge_then_writes_off",
    ),
    (
        "late_loss_ignored",
        "SENDING | send_ok | late_loss | OPEN | commit_request,reopen_gap",
        "SENDING | send_ok | late_loss | REQUESTED | commit_request",
        "loss_while_sending_reopens_after_the_committed_send",
    ),
    (
        "loss_after_request_ignored",
        "REQUESTED | loss_voiced | !dup | OPEN | reopen_gap",
        "REQUESTED | loss_voiced | !dup | = | merge_gap",
        "loss_after_the_request_reopens_and_needs_new_evidence",
    ),
    (
        "undelivered_settles",
        "REQUESTED | request_undelivered | attempts_left,!candidate_active | DUE | keep_budget_spent",
        "REQUESTED | request_undelivered | attempts_left,!candidate_active | CLEAR | settle",
        "undelivered_request_does_not_settle_and_returns_to_due",
    ),
    (
        "session_end_settles",
        "OPEN | session_end | - | CLEAR | end_unsettled",
        "OPEN | session_end | - | CLEAR | settle",
        "session_end_writes_off_an_open_gap_without_settling",
    ),
    (
        "retry_without_idle_edge",
        "DUE | busy_end | budget_left,attempts_left | SENDING | send_request",
        "DUE | busy_end | budget_left,attempts_left | DUE | noop",
        "failed_send_then_retry_succeeds_after_recovery",
    ),
    (
        "committed_send_skips_the_wait_for_evidence",
        "SENDING | send_ok | !late_loss | REQUESTED | commit_request",
        "SENDING | send_ok | !late_loss | CLEAR | commit_request",
        "voiced_gap_one_request_settled_by_later_evidence",
    ),
    (
        "overlapping_partial_guards",
        "CLEAR | floor_released | - | = | noop",
        "CLEAR | floor_released | busy | = | noop\n"
        "CLEAR | floor_released | dup | = | noop\n"
        "CLEAR | floor_released | !busy,!dup | = | noop",
        "table",
    ),
    (
        "failed_send_ignores_a_speaking_candidate",
        "SENDING | send_failed | attempts_left,candidate_active | OPEN | supersede",
        "SENDING | send_failed | attempts_left,candidate_active | DUE | keep_debt",
        "failed_send_while_the_candidate_speaks_waits_for_the_next_release",
    ),
    (
        "undelivered_ignores_a_speaking_candidate",
        "REQUESTED | request_undelivered | attempts_left,candidate_active | OPEN | keep_budget_spent,supersede",
        "REQUESTED | request_undelivered | attempts_left,candidate_active | DUE | keep_budget_spent",
        "undelivered_request_while_the_candidate_speaks_waits_for_the_next_release",
    ),
    (
        "undelivered_refunds_the_budget",
        "REQUESTED | request_undelivered | attempts_left,!candidate_active | DUE | keep_budget_spent",
        "REQUESTED | request_undelivered | attempts_left,!candidate_active | DUE | noop",
        "undelivered_request_does_not_settle_and_returns_to_due",
    ),
    (
        "candidate_start_while_sending_drops_the_send",
        "SENDING | candidate_start | - | = | noop",
        "SENDING | candidate_start | - | DUE | supersede",
        "failed_send_while_the_candidate_speaks_waits_for_the_next_release",
    ),
    (
        "release_while_requested_resends",
        "REQUESTED | floor_released | - | = | noop",
        "REQUESTED | floor_released | !busy,budget_left,attempts_left | SENDING | send_request\n"
        "REQUESTED | floor_released | busy | = | noop\n"
        "REQUESTED | floor_released | !busy,!budget_left | = | noop\n"
        "REQUESTED | floor_released | !busy,budget_left,!attempts_left | = | noop",
        "reach",
    ),
    (
        "impossible_event_becomes_a_noop",
        "CLEAR | send_ok | - | = | invalid",
        "CLEAR | send_ok | - | = | noop",
        "impossible",
    ),
    (
        "impossible_event_becomes_a_noop_in_open",
        "OPEN | send_failed | - | = | invalid",
        "OPEN | send_failed | - | = | noop",
        "impossible",
    ),
    (
        "invalid_combined_with_another_action",
        "CLEAR | send_ok | - | = | invalid",
        "CLEAR | send_ok | - | = | invalid,noop",
        "table",
    ),
    (
        "requested_state_is_never_entered",
        "SENDING | send_ok | !late_loss | REQUESTED | commit_request",
        "SENDING | send_ok | !late_loss | OPEN | commit_request",
        "reach",
    ),
    (
        "missing_row_makes_the_table_partial",
        "REQUESTED | evidence_turn | !post_send | = | noop\n",
        "",
        "table",
    ),
    (
        "two_rows_for_one_case",
        "CLEAR | floor_released | - | = | noop",
        "CLEAR | floor_released | - | = | noop\nCLEAR | floor_released | busy | OPEN | noop",
        "table",
    ),
    (
        "session_cap_raised",
        "@cap session_requests 3",
        "@cap session_requests 9",
        "the_session_budget_writes_off_a_fourth_gap_without_a_request",
    ),
    (
        "attempt_cap_raised",
        "@cap episode_attempts 2",
        "@cap episode_attempts 3",
        "failed_send_retries_once_at_the_next_idle_edge_then_writes_off",
    ),
]


# The step at which each BAD fixture must be refused, so one cannot pass by a
# typo earlier in the sequence.
BAD_FAILS_AT = {
    "duplicate_settlement": 4,
    "settlement_after_silence_only": 1,
    "settlement_on_pre_request_evidence": 3,
    "send_while_busy": 2,
    "second_concurrent_request": 2,
    "request_on_silence_only_trim": 1,
    "unknown_sdk_drop_opens_a_gap": 0,
    "send_ok_with_nothing_sent": 0,
    "repeated_notice_reopens_after_settlement": 4,
    "undelivered_request_settles": 3,
    "failed_send_retries_immediately": 2,
    "third_attempt_for_one_episode": 4,
}


def mutate(old, new):
    if DOC_TEXT.count(old) != 1:
        raise AssertionError(f"mutation anchor must occur exactly once: {old!r}")
    return DOC_TEXT.replace(old, new)


def fixtures_failing(table):
    failing = set()
    for fixture in GOOD:
        try:
            run_fixture(table, fixture)
        except FixtureFailure:
            failing.add(fixture["name"])
    return failing


IMPOSSIBLE = (
    ["send_ok"],
    ["send_failed"],
    ["request_undelivered"],
    [lv("a"), "send_ok"],
    [lv("a"), "send_failed"],
    [lv("a"), "floor_released", "request_undelivered"],
)


def impossible_accepted(table):
    """Whether any impossible event is silently accepted by this table."""
    for events in IMPOSSIBLE:
        try:
            gap.replay(table, events)
        except gap.ReplayError:
            continue
        return True
    return False


def raise_if_bad(table):
    """The reachability check: a stray row or a broken invariant raises."""
    dead = gap.unreachable(table)
    if dead:
        raise gap.ReplayError(f"unreachable rows {[r['line'] for r in dead]}")


class RealTable(unittest.TestCase):
    def test_table_is_total_and_deterministic(self):
        table = gap.Table(DOC_TEXT)
        self.assertEqual(
            table.validate(), len(gap.STATES) * len(gap.EVENTS) * 2 ** len(gap.GUARDS)
        )

    def test_every_good_fixture_passes(self):
        table = gap.Table(DOC_TEXT)
        for fixture in GOOD:
            with self.subTest(fixture["name"]):
                run_fixture(table, fixture)

    def test_every_bad_fixture_is_refused(self):
        table = gap.Table(DOC_TEXT)
        self.assertEqual(set(BAD_FAILS_AT), {f["name"] for f in BAD})
        for fixture in BAD:
            with self.subTest(fixture["name"]):
                with self.assertRaises(FixtureFailure) as caught:
                    run_fixture(table, fixture)
                self.assertEqual(caught.exception.index, BAD_FAILS_AT[fixture["name"]])

    def test_fixture_names_are_unique_and_cover_the_named_cases(self):
        names = [f["name"] for f in GOOD + BAD]
        self.assertEqual(len(names), len(set(names)))
        joined = " ".join(f["name"] for f in GOOD)
        for needle in (
            "silence_only",
            "voiced_gap",
            "repeated_notices",
            "thinking",
            "superseding",
            "failed_send",
        ):
            self.assertIn(needle, joined)

    def test_request_counts_in_named_fixtures(self):
        table = gap.Table(DOC_TEXT)
        by_name = {f["name"]: f for f in GOOD}
        counts = {
            "silence_only_trimming_sends_nothing": (0, 0),
            "voiced_gap_one_request_settled_by_later_evidence": (1, 1),
            "repeated_notices_open_and_request_once": (1, 1),
            "unknown_sdk_drop_opens_no_gap": (0, 0),
            "session_end_writes_off_an_open_gap_without_settling": (0, 0),
            "failed_send_retries_once_at_the_next_idle_edge_then_writes_off": (2, 0),
        }
        for name, (sends, settles) in counts.items():
            machine = run_fixture(table, by_name[name])
            self.assertEqual((sent(machine), settled(machine)), (sends, settles), name)

    def test_every_reachable_path_keeps_the_invariants(self):
        table = gap.Table(DOC_TEXT)
        self.assertEqual(gap.unreachable(table), [])
        reached, impossible = gap.explore(table)
        invalid = {r["line"] for r in table.rows if r["actions"] == ("invalid",)}
        everything = {r["line"] for r in table.rows}
        exempt = {r["line"] for r in table.rows if gap.defensive(r)}
        self.assertEqual(impossible, invalid)
        self.assertEqual(reached | impossible | exempt, everything)
        self.assertEqual(reached & impossible, set())
        # The defensive rows are exactly these three, by state and event.
        self.assertEqual(
            sorted((r["state"], r["event"]) for r in table.rows if gap.defensive(r)),
            [
                ("DUE", "busy_end"),
                ("DUE", "floor_released"),
                ("OPEN", "floor_released"),
            ],
        )

    def test_the_good_fixtures_exercise_every_reachable_row(self):
        table = gap.Table(DOC_TEXT)
        reached, _ = gap.explore(table)
        covered = set()
        for fixture in GOOD:
            covered |= {h[4] for h in run_fixture(table, fixture).history}
        lines = {r["line"]: r for r in table.rows}
        missing = sorted(reached - covered)
        self.assertEqual(
            [
                (lines[n]["state"], lines[n]["event"], lines[n]["guards"])
                for n in missing
            ],
            [],
        )

    def test_impossible_events_are_refused_not_ignored(self):
        table = gap.Table(DOC_TEXT)
        for events in IMPOSSIBLE:
            with self.subTest(events=events):
                with self.assertRaises(gap.ReplayError):
                    gap.replay(table, events)

    def test_unknown_event_and_missing_id_are_refused(self):
        machine = gap.Machine(gap.Table(DOC_TEXT))
        with self.assertRaises(gap.ReplayError):
            machine.step("floor_releasd")
        with self.assertRaises(gap.ReplayError):
            machine.step("loss_voiced")


class Mutants(unittest.TestCase):
    def test_each_mutant_is_noticed(self):
        for label, old, new, detector in MUTANTS:
            with self.subTest(label):
                text = mutate(old, new)
                try:
                    table = gap.Table(text)
                    table.validate()
                except gap.TableError:
                    self.assertEqual(
                        detector, "table", f"{label}: refused by the table check"
                    )
                    continue
                self.assertNotEqual(
                    detector, "table", f"{label}: the table check missed it"
                )
                if detector == "impossible":
                    self.assertTrue(
                        impossible_accepted(table), f"{label}: still refused"
                    )
                    continue
                if detector == "reach":
                    with self.assertRaises(gap.ReplayError):
                        raise_if_bad(table)
                    continue
                failing = fixtures_failing(table)
                self.assertIn(
                    detector,
                    failing,
                    f"{label}: {detector} did not fail; failed {failing}",
                )

    def test_the_mutant_list_covers_the_required_scenarios(self):
        detectors = {m[3] for m in MUTANTS}
        for scenario in (
            "silence_only",
            "voiced_gap",
            "repeated_notices",
            "thinking",
            "superseding",
            "failed_send",
        ):
            self.assertTrue(any(scenario in d for d in detectors), scenario)


class Invariants(unittest.TestCase):
    """The replay refuses what a wrong table would let its own fixtures expect.

    Each case changes the table and then drives events whose expected outcome
    is exactly what the changed table produces, so only an invariant can fail.
    """

    def drive(self, old, new, events, message):
        table = gap.Table(mutate(old, new))
        table.validate()
        machine = gap.Machine(table)
        with self.assertRaisesRegex(gap.ReplayError, message):
            for item in events:
                name, options = (item, {}) if isinstance(item, str) else item
                machine.step(name, options.get("id"), options.get("post_send", False))

    def test_a_send_while_busy_is_refused(self):
        self.drive(
            "OPEN | floor_released | busy | DUE | defer_no_interrupt",
            "OPEN | floor_released | busy | SENDING | send_request",
            [lv("a"), "busy_start", "floor_released"],
            "while the interviewer was busy",
        )

    def test_a_second_send_in_flight_is_refused(self):
        self.drive(
            "SENDING | floor_released | - | = | noop",
            "SENDING | floor_released | - | = | send_request",
            [lv("a"), "floor_released", "floor_released"],
            "sent from SENDING",
        )

    def test_settlement_without_post_request_evidence_is_refused(self):
        self.drive(
            "REQUESTED | evidence_turn | !post_send | = | noop",
            "REQUESTED | evidence_turn | !post_send | CLEAR | settle",
            [lv("a"), "floor_released", "send_ok", PRE],
            "settled without post-request evidence",
        )

    def test_a_write_off_with_no_gap_is_refused(self):
        self.drive(
            "CLEAR | floor_released | - | = | noop",
            "CLEAR | floor_released | - | = | unrepaired_budget",
            ["floor_released"],
            "unrepaired_budget with no open gap",
        )

    def test_a_gap_whose_debt_is_dropped_is_refused(self):
        self.drive(
            "OPEN | candidate_start | - | = | noop",
            "OPEN | candidate_start | - | CLEAR | noop",
            [lv("a"), "candidate_start"],
            "debt disagrees",
        )

    def test_a_retry_while_the_candidate_speaks_is_refused(self):
        self.drive(
            "SENDING | send_failed | attempts_left,candidate_active | OPEN | supersede",
            "SENDING | send_failed | attempts_left,candidate_active | DUE | keep_debt",
            [lv("a"), "floor_released", "candidate_start", "send_failed", "busy_end"],
            "candidate was speaking",
        )

    def test_a_commit_with_no_send_in_flight_is_refused(self):
        self.drive(
            "OPEN | send_ok | - | = | invalid",
            "OPEN | send_ok | - | = | commit_request",
            [lv("a"), "send_ok"],
            "committed from OPEN",
        )

    def test_a_send_that_does_not_enter_sending_is_refused(self):
        self.drive(
            "OPEN | floor_released | !busy,budget_left,attempts_left | SENDING | send_request",
            "OPEN | floor_released | !busy,budget_left,attempts_left | OPEN | send_request",
            [lv("a"), "floor_released"],
            "not SENDING",
        )

    def test_a_request_beyond_the_session_cap_is_refused(self):
        table = gap.Table(
            mutate(
                "OPEN | floor_released | !busy,!budget_left | CLEAR | unrepaired_budget",
                "OPEN | floor_released | !busy,!budget_left | SENDING | send_request",
            )
        )
        machine = gap.Machine(table)
        with self.assertRaisesRegex(gap.ReplayError, "session request cap exceeded"):
            for ident in "abcd":
                machine.step("loss_voiced", ident)
                machine.step("floor_released")
                machine.step("send_ok")
                machine.step("evidence_turn", post_send=True)


def classified(report):
    return gap.classify_report(report)


def turn(ident, status, ms=(0, 1000), verified=False):
    return {
        "id": ident,
        "status": status,
        "transmitted_ms": list(ms),
        "transmitted_verified": verified,
    }


def arm(ident, turns, run="r1", intervals=(), lines=1, dequeued=100):
    return {
        "id": ident,
        "run_id": run,
        "turns": turns,
        "transcript_lines": lines,
        "frames_dequeued": dequeued,
        "local_intervals": list(intervals),
    }


def interval(start, end, run="r1", kind="drop"):
    return {"start_ms": start, "end_ms": end, "run_id": run, "kind": kind}


CONTROL = arm("c", [turn("t1", "intact"), turn("t2", "intact")], run="rc")


class Attribution(unittest.TestCase):
    def verdict(self, stall, control=CONTROL):
        result = classified({"control": "c", "arms": [control, stall]})
        return result, result["arms"][0]["turns"]["t1"]

    def test_all_five_conditions_make_a_loss_attributable(self):
        stall = arm(
            "s",
            [turn("t1", "missing", (100, 900), True), turn("t2", "intact")],
            intervals=[interval(500, 600)],
        )
        result, t1 = self.verdict(stall)
        self.assertEqual(t1, {"verdict": "attributable", "unmet": []})
        self.assertEqual(result["decision"], "attributable-loss-present")

    def test_a_repeated_turn_can_be_attributable_too(self):
        stall = arm(
            "s",
            [turn("t1", "repeated", (100, 900), True), turn("t2", "intact")],
            intervals=[interval(0, 150, kind="quarantine")],
        )
        self.assertEqual(self.verdict(stall)[1]["verdict"], "attributable")

    def test_stt_mismatch_alone_is_inconclusive(self):
        stall = arm(
            "s",
            [turn("t1", "inconclusive", (100, 900), True), turn("t2", "intact")],
            intervals=[interval(500, 600)],
        )
        result, t1 = self.verdict(stall)
        self.assertEqual(t1["verdict"], "inconclusive")
        self.assertEqual(result["decision"], "inconclusive")

    def test_a_missing_turn_without_a_local_interval_is_unattributed(self):
        stall = arm(
            "s", [turn("t1", "missing", (100, 900), True), turn("t2", "intact")]
        )
        result, t1 = self.verdict(stall)
        self.assertEqual(t1["unmet"], ["no_same_run_local_interval"])
        self.assertEqual(result["decision"], "inconclusive")

    def test_byte_totals_are_not_an_interval(self):
        stall = arm(
            "s", [turn("t1", "missing", (100, 900), True), turn("t2", "intact")]
        )
        stall["local_totals"] = {"dropped_bytes": 640000}
        self.assertEqual(self.verdict(stall)[1]["verdict"], "inconclusive")

    def test_an_interval_from_another_run_or_not_overlapping_does_not_count(self):
        for found in (
            interval(500, 600, run="other"),
            interval(900, 950),
            interval(0, 100),
        ):
            stall = arm(
                "s",
                [turn("t1", "missing", (100, 900), True), turn("t2", "intact")],
                intervals=[found],
            )
            self.assertEqual(self.verdict(stall)[1]["verdict"], "inconclusive", found)

    def test_unverified_transmission_is_inconclusive(self):
        stall = arm(
            "s",
            [turn("t1", "missing", (100, 900), False), turn("t2", "intact")],
            intervals=[interval(500, 600)],
        )
        self.assertIn("transmitted_audio_not_verified", self.verdict(stall)[1]["unmet"])

    def test_a_miss_also_in_the_control_is_inconclusive(self):
        control = arm("c", [turn("t1", "missing"), turn("t2", "intact")], run="rc")
        stall = arm(
            "s",
            [turn("t1", "missing", (100, 900), True), turn("t2", "intact")],
            intervals=[interval(500, 600)],
        )
        self.assertIn(
            "control_does_not_have_the_turn", self.verdict(stall, control)[1]["unmet"]
        )

    def test_a_delay_is_not_a_loss(self):
        stall = arm(
            "s",
            [turn("t1", "delayed", (100, 900), True), turn("t2", "intact")],
            intervals=[interval(500, 600)],
        )
        self.assertEqual(self.verdict(stall)[1]["verdict"], "inconclusive")

    def test_whole_utterance_silence_without_a_counter_is_flagged_unexplained(self):
        stall = arm(
            "s",
            [turn("t1", "missing", (100, 900), False), turn("t2", "missing")],
            lines=0,
            dequeued=5000,
        )
        result = classified({"control": "c", "arms": [CONTROL, stall]})
        self.assertTrue(result["arms"][0]["unexplained_whole_utterance_silence"])
        self.assertEqual(result["decision"], "inconclusive")

    def test_whole_utterance_silence_with_an_attributing_interval_is_not_unexplained(
        self,
    ):
        stall = arm(
            "s",
            [
                turn("t1", "missing", (100, 900), True),
                turn("t2", "missing", (1000, 1900), True),
            ],
            lines=0,
            dequeued=5000,
            intervals=[interval(0, 2000)],
        )
        result = classified({"control": "c", "arms": [CONTROL, stall]})
        self.assertFalse(result["arms"][0]["unexplained_whole_utterance_silence"])
        self.assertEqual(result["decision"], "attributable-loss-present")

    def test_all_intact_is_no_change(self):
        stall = arm("s", [turn("t1", "intact"), turn("t2", "intact")])
        self.assertEqual(
            classified({"control": "c", "arms": [CONTROL, stall]})["decision"],
            "no-change",
        )


class AttributionEdges(unittest.TestCase):
    def one(self, stall):
        return classified({"control": "c", "arms": [CONTROL, stall]})["arms"][0]

    def test_empty_or_inverted_intervals_and_audio_never_overlap(self):
        for found, audio in (
            (interval(500, 500), (100, 900)),
            (interval(600, 500), (100, 900)),
            (interval(500, 600), (500, 500)),
            (interval(500, 600), (900, 100)),
        ):
            stall = arm(
                "s",
                [turn("t1", "missing", audio, True), turn("t2", "intact")],
                intervals=[found],
            )
            self.assertEqual(
                self.one(stall)["turns"]["t1"]["verdict"],
                "inconclusive",
                (found, audio),
            )

    def test_a_stall_arm_must_carry_exactly_the_controls_turns(self):
        for turns in (
            [],
            [turn("t1", "intact")],
            [turn("t1", "intact"), turn("t2", "intact"), turn("t3", "intact")],
            [turn("t1", "intact"), turn("t1", "missing")],
        ):
            with self.assertRaises(ValueError, msg=str(turns)):
                classified({"control": "c", "arms": [CONTROL, arm("s", turns)]})

    def test_a_report_with_only_the_control_decides_nothing(self):
        with self.assertRaises(ValueError):
            classified({"control": "c", "arms": [CONTROL]})
        bad = arm("c", [turn("t1", "intact"), turn("t1", "intact")])
        with self.assertRaises(ValueError):
            classified(
                {"control": "c", "arms": [bad, arm("s", [turn("t1", "intact")])]}
            )

    def test_a_local_counter_or_interval_means_the_silence_is_not_counterless(self):
        base = [turn("t1", "missing"), turn("t2", "missing")]
        counted = arm("s", base, lines=0, dequeued=5000)
        counted["local_totals"] = {"dropped_bytes": 320, "quarantined_bytes": 0}
        elsewhere = arm("s", base, lines=0, dequeued=5000, intervals=[interval(0, 5)])
        for stall in (counted, elsewhere):
            self.assertFalse(self.one(stall)["unexplained_whole_utterance_silence"])
            self.assertEqual(self.one(stall)["attributable_turns"], [])

    def test_every_report_says_it_is_descriptive(self):
        stall = arm("s", [turn("t1", "intact"), turn("t2", "intact")])
        result = classified({"control": "c", "arms": [CONTROL, stall]})
        self.assertTrue(result["descriptive_only"])


class Cli(unittest.TestCase):
    def run_script(self, *args):
        return subprocess.run(
            [sys.executable, str(SCRIPT), *args],
            capture_output=True,
            text=True,
            check=False,
        )

    def test_check_passes_on_the_real_document(self):
        result = self.run_script("check")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("gap-state table ok", result.stdout)

    def test_check_refuses_a_partial_table_with_exit_one(self):
        with tempfile.TemporaryDirectory() as scratch:
            doc = Path(scratch) / "doc.md"
            doc.write_text(
                mutate("REQUESTED | evidence_turn | !post_send | = | noop\n", "")
            )
            result = self.run_script("check", "--doc", str(doc))
        self.assertEqual(result.returncode, 1)
        self.assertIn("gap-state table defect", result.stderr)

    def test_unusable_input_exits_two(self):
        with tempfile.TemporaryDirectory() as scratch:
            report = Path(scratch) / "arms.json"
            report.write_text("{}")
            self.assertEqual(self.run_script("classify", str(report)).returncode, 2)
            self.assertEqual(
                self.run_script(
                    "classify", str(Path(scratch) / "none.json")
                ).returncode,
                2,
            )

    def test_malformed_arm_reports_exit_two_not_a_traceback(self):
        stall = arm("s", [turn("t1", "missing"), turn("t2", "intact")])
        stall["turns"][0]["transmitted_ms"] = [5]
        stall["turns"][0]["transmitted_verified"] = True
        stall["local_intervals"] = [interval(0, 10)]
        with tempfile.TemporaryDirectory() as scratch:
            report = Path(scratch) / "arms.json"
            report.write_text(json.dumps({"control": "c", "arms": [CONTROL, stall]}))
            result = self.run_script("classify", str(report))
        self.assertEqual(result.returncode, 2, result.stderr)

    def test_classify_prints_the_decision(self):
        stall = arm("s", [turn("t1", "missing"), turn("t2", "intact")])
        with tempfile.TemporaryDirectory() as scratch:
            report = Path(scratch) / "arms.json"
            report.write_text(json.dumps({"control": "c", "arms": [CONTROL, stall]}))
            result = self.run_script("classify", str(report))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["decision"], "inconclusive")


if __name__ == "__main__":
    unittest.main()

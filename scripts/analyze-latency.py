#!/usr/bin/env python3
"""Break down CodeTrial reply latency from the agent's server log.

Reads `codetrial latency {JSON}` records, joins each reply's records by
(pid, room, session, turn), and partitions origin-to-first-native-capture into
consecutive segments so the dominant one is visible per turn and in aggregate.
"""

import argparse
import json
import math
import random
import re
import sys
from collections import Counter

MARKER = "codetrial latency "
VERSION = 2
ORIGINS = {
    "last_voiced_pcm",
    "prompt",
    "transcript",
    "provider_voice_end",
    "provider_event",
}
STAGES = {
    "tool_response",
    "audio_accepted",
    "native_audio_capture",
    "native_audio_cancelled",
    "audio_enqueue_failed",
    "turn_complete",
    "interrupted",
    "paused",
    "held",
    "socket_replaced",
}
TIMINGS = (
    "elapsed_ms",
    "observed_ms",
    "provider_voice_end_ms",
    "first_transcript_ms",
    "latest_transcript_ms",
    "candidate_yield_ms",
    "first_provider_output_ms",
    "first_tool_call_ms",
    "tool_ack_ms",
    "last_tool_response_ms",
    "tool_handler_ms",
    "provider_event_queue_ms",
    "stale_drop_ms",
    "enqueue_ms",
    "handler_tail_ms",
    "playout_ahead_ms",
    "output_queue_ms",
    "native_capture_ms",
    "server_total_ms",
    "tool_batch_ms",
    "tool_execute_ms",
    "board_resend_ms",
    "tool_send_ms",
    "silence_window_ms",
)
# Consecutive, so they sum to the server total. The configured silence window
# is a comparison allowance, not a measured provider span: explicit yielding
# and overlapping inference can change the actual endpointing wait.
SEGMENTS = (
    "candidate_yield",
    "silence_window",
    "endpointing",
    "response_start",
    "tool_decision",
    "tool_rounds",
    "post_tool_response",
    "output_to_audio",
    "output_queue",
    "native_capture",
)
SEGMENT_HELP = {
    "candidate_yield": "candidate's own time before choosing their turn is done",
    "silence_window": "configured silence allowance, not measured provider time",
    "endpointing": "rest of origin to provider END (uplink, VAD, any work before END)",
    "response_start": "END (or origin) to first provider output",
    "tool_decision": "END (or origin) to first tool call",
    "tool_rounds": "first tool call to last tool response sent",
    "post_tool_response": "last tool response to first audio accepted",
    "output_to_audio": "first output (transcript) to first audio accepted",
    "output_queue": "audio accepted to worker dequeue",
    "native_capture": "LiveKit capture_frame of the first frame",
}


# What an owed reply can end as, `open` for every record before its terminal
# one. `missing` is never logged: it is a reply with no terminal record.
OBLIGATION_OUTCOMES = ("open", "answered", "superseded", "ended")
MAX_EPISODES = 8


class Rejected(ValueError):
    pass


def is_number(value):
    if not isinstance(value, (int, float)) or isinstance(value, bool):
        return False
    # Production timestamps fit i64 and durations fit u64. Bound parsed
    # numbers too, so malformed logs cannot overflow aggregation arithmetic.
    return -(2**63) <= value <= 2**64 - 1 and math.isfinite(value)


def is_int(value):
    return isinstance(value, int) and not isinstance(value, bool)


def parse(line):
    """The record on this line, None for a line without one, or Rejected."""
    if MARKER not in line:
        return None
    payload = line.split(MARKER, 1)[1].strip()
    try:
        record = json.loads(payload)
    except (ValueError, RecursionError) as error:
        raise Rejected("not JSON") from error
    if not isinstance(record, dict):
        raise Rejected("not an object")
    if record.get("event") != "reply_latency" or record.get("version") != VERSION:
        raise Rejected("unsupported event or version")
    if not isinstance(record.get("room"), str):
        raise Rejected("room")
    for field in ("pid", "session", "socket", "turn"):
        if not is_int(record.get(field)):
            raise Rejected(field)
    if not isinstance(record.get("stage"), str) or record["stage"] not in STAGES:
        raise Rejected("stage")
    if not isinstance(record.get("origin"), str) or record["origin"] not in ORIGINS:
        raise Rejected("origin")
    for field in TIMINGS:
        value = record.get(field)
        if value is not None and not is_number(value):
            raise Rejected(field)
    signed_milestones = {
        "observed_ms",
        "provider_voice_end_ms",
        "first_transcript_ms",
        "latest_transcript_ms",
        "candidate_yield_ms",
        "first_provider_output_ms",
        "first_tool_call_ms",
        "last_tool_response_ms",
    }
    for field in TIMINGS:
        if field not in signed_milestones and (record.get(field) or 0) < 0:
            raise Rejected(field)
    for field in (
        "tool_batches",
        "tool_count",
        "origin_socket",
        "output_queue_frames",
        "context_tokens",
    ):
        if record.get(field) is not None and (
            not is_int(record[field]) or record[field] < 0
        ):
            raise Rejected(field)
    # Obligation identity, appended to the record with the lifecycle ids. A
    # record without these fields is a legacy record and is still accepted.
    for field in ("interview_ms", "owed_start_interview_ms", "first_accept_ms"):
        if record.get(field) is not None and not is_int(record[field]):
            raise Rejected(field)
    for field in (
        "obligation",
        "obligation_episode_count",
        "first_accept_socket",
    ):
        if record.get(field) is not None and (
            not is_int(record[field]) or record[field] < 0
        ):
            raise Rejected(field)
    outcome = record.get("obligation_outcome")
    if outcome is not None and outcome not in OBLIGATION_OUTCOMES:
        raise Rejected("obligation_outcome")
    episodes = record.get("obligation_episodes")
    if episodes is not None and not (
        isinstance(episodes, list)
        and len(episodes) <= MAX_EPISODES
        and all(is_int(e) and e > 0 for e in episodes)
    ):
        raise Rejected("obligation_episodes")
    if (record.get("obligation") is None) != (outcome is None):
        raise Rejected("obligation without outcome")
    names = record.get("tool_names")
    if names is not None and not (
        isinstance(names, list) and all(isinstance(n, str) for n in names)
    ):
        raise Rejected("tool_names")
    for field in ("captured", "sent", "enqueued", "continuing"):
        if field in record and not isinstance(record[field], bool):
            raise Rejected(field)
    if record.get("captured") is True and record["stage"] != "native_audio_capture":
        raise Rejected("capture stage")
    if record["stage"] == "native_audio_capture" and record.get("captured") is True:
        if record.get("elapsed_ms") is None or record.get("server_total_ms") is None:
            raise Rejected("missing capture timing")
    return record


# One summary per socket the interview let go: how full its event channel got
# and how long its reader waited for a slot. Counts only, never content.
PRESSURE_ENDS = {"replacement", "terminal", "discarded"}
PRESSURE_READER_ENDS = {
    "read_idle",
    "closed",
    "failed",
    "credential",
    "receiver_closed",
    "stopped",
}
PRESSURE_COUNTS = (
    "capacity",
    "occupancy_high_water",
    "blocked_sends",
    "blocked_ms_total",
    "blocked_ms_max",
    "received",
    "consumed",
)


def parse_pressure(line):
    """The socket_pressure record on this line, None for any other line, or
    Rejected. A field a record leaves out stays absent: it is unknown, not 0."""
    if MARKER not in line:
        return None
    payload = line.split(MARKER, 1)[1].strip()
    if '"socket_pressure"' not in payload:
        return None
    try:
        record = json.loads(payload)
    except (ValueError, RecursionError) as error:
        raise Rejected("not JSON") from error
    if not isinstance(record, dict) or record.get("event") != "socket_pressure":
        return None
    if record.get("version") != VERSION:
        raise Rejected("pressure version")
    if not isinstance(record.get("room"), str):
        raise Rejected("pressure room")
    for field in ("pid", "session", "socket"):
        if not is_int(record.get(field)) or record[field] < 0:
            raise Rejected(f"pressure {field}")
    for field, allowed in (
        ("end", PRESSURE_ENDS),
        ("reader_end", PRESSURE_READER_ENDS),
    ):
        value = record.get(field)
        if value is not None and (not isinstance(value, str) or value not in allowed):
            raise Rejected(f"pressure {field}")
    if record.get("interview_ms") is not None and not is_int(record["interview_ms"]):
        raise Rejected("pressure interview_ms")
    for field in PRESSURE_COUNTS:
        value = record.get(field)
        if value is not None and (not is_int(value) or not 0 <= value <= 2**63):
            raise Rejected(f"pressure {field}")

    impossible = pressure_impossible(record)
    if impossible:
        raise Rejected(f"pressure {impossible}")
    return record


def pressure_impossible(fields):
    """Why these fields cannot all be true of one socket, or None. Fields a
    record leaves out are not checked: unknown is not a violation."""

    def both(low, high):
        return fields.get(low) is not None and fields.get(high) is not None

    if both("occupancy_high_water", "capacity") and (
        fields["occupancy_high_water"] > fields["capacity"]
    ):
        return "high water over capacity"
    if both("consumed", "received") and fields["consumed"] > fields["received"]:
        return "consumed over received"
    if both("blocked_ms_max", "blocked_ms_total") and (
        fields["blocked_ms_max"] > fields["blocked_ms_total"]
    ):
        return "max over total"
    if fields.get("blocked_sends") == 0 and (
        fields.get("blocked_ms_total") or fields.get("blocked_ms_max")
    ):
        return "blocked time without a blocked send"
    return None


def pressure_report(records):
    """Merges the copies of each socket's summary and totals only what is
    known. A record that leaves a field out contributes nothing to it, and a
    total nobody reported is None, never 0."""
    merged = {}
    conflicts = 0
    for record in records:
        key = (record["pid"], record["room"], record["session"], record["socket"])
        row = merged.setdefault(key, {})
        present = {k: v for k, v in record.items() if v is not None}
        # A copy that disagrees with what is kept, or that would make the
        # merged socket impossible, is one conflict; the first value stays and
        # the copy adds nothing.
        fill = {k: v for k, v in present.items() if k not in row}
        if any(row[k] != v for k, v in present.items() if k in row) or (
            pressure_impossible({**row, **fill})
        ):
            conflicts += 1
        if not pressure_impossible({**row, **fill}):
            row.update(fill)
    rows = []
    for key in sorted(merged):
        row = merged[key]
        rows.append(
            {
                "pid": key[0],
                "room": key[1],
                "session": key[2],
                "socket": key[3],
                "end": row.get("end"),
                "reader_end": row.get("reader_end"),
                "interview_ms": row.get("interview_ms"),
                **{name: row.get(name) for name in PRESSURE_COUNTS},
            }
        )

    def known(name):
        return [r[name] for r in rows if r[name] is not None]

    def total(name):
        values = known(name)
        return sum(values) if values else None

    return {
        "records": len(records),
        "sockets": len(rows),
        "duplicates": len(records) - len(rows),
        "conflicts": conflicts,
        "partial": sum(
            any(r[name] is None for name in PRESSURE_COUNTS)
            or r["end"] is None
            or r["reader_end"] is None
            for r in rows
        ),
        "blocked_sockets": sum(v > 0 for v in known("blocked_sends")),
        "blocked_sends": total("blocked_sends"),
        "blocked_ms_total": total("blocked_ms_total"),
        "blocked_ms_max": max(known("blocked_ms_max"), default=None),
        "blocked_unknown": sum(r["blocked_sends"] is None for r in rows),
        "high_water_max": max(known("occupancy_high_water"), default=None),
        "full_sockets": sum(
            r["capacity"] is not None
            and r["occupancy_high_water"] is not None
            and r["occupancy_high_water"] == r["capacity"]
            for r in rows
        ),
        "read_idle_ends": sum(r["reader_end"] == "read_idle" for r in rows),
        "rows": rows,
    }


def offset(record):
    value = record.get("server_total_ms")
    return value if value is not None else record.get("elapsed_ms")


# Below this many values a p95 is one of the last two, so a single reply moves
# it; the summary marks such statistics instead of hiding them.
SMALL_SAMPLE = 20


def percentile(values, fraction):
    """Linear interpolation between closest ranks, as numpy's default does."""
    rank = (len(values) - 1) * fraction
    low = math.floor(rank)
    high = min(low + 1, len(values) - 1)
    value = round(values[low] + (values[high] - values[low]) * (rank - low), 1)
    return int(value) if value == int(value) else value


def statistics(values):
    values = sorted(values)
    if not values:
        return {"count": 0}
    return {
        "count": len(values),
        "min_ms": values[0],
        "p50_ms": percentile(values, 0.50),
        "mean_ms": round(sum(values) / len(values), 1),
        "p95_ms": percentile(values, 0.95),
        "max_ms": values[-1],
        "small_sample": len(values) < SMALL_SAMPLE,
    }


# Context sizes a reply started from, in tokens: each band's upper bound.
CONTEXT_BANDS = (8_000, 16_000, 32_000, 64_000)


def context_band(tokens):
    lower = 0
    for upper in CONTEXT_BANDS:
        if tokens < upper:
            return f"{lower // 1000}k_{upper // 1000}k"
        lower = upper
    return f"{lower // 1000}k_up"


def slope_per_1k(points):
    """Least-squares ms per thousand tokens within each kind of reply, or None
    without spread.

    Points are (kind, tokens, ms). Each kind is centered on its own means
    first: tool replies cost more and come later in an interview, so a slope
    fitted across kinds reads that mix as growth with context.
    """
    by_kind = {}
    for kind, x, y in points:
        by_kind.setdefault(kind, []).append((x, y))
    centered = []
    for pairs in by_kind.values():
        mean_x = sum(x for x, _ in pairs) / len(pairs)
        mean_y = sum(y for _, y in pairs) / len(pairs)
        centered.extend((x - mean_x, y - mean_y) for x, y in pairs)
    # Each kind spends a degree of freedom on its own mean, so a slope needs
    # two points to spare beyond them.
    if len(points) - len(by_kind) < 2:
        return None
    spread = sum(x * x for x, _ in centered)
    if not spread:
        return None
    rise = sum(x * y for x, y in centered)
    return round(rise / spread * 1000, 1)


def tool_label(row):
    """Whether a tool round came before the reply's audio."""
    return "tool" if row["tool_batches"] else "no_tool"


def reply_kind(row):
    """What a reply answered and whether a tool round came before its audio."""
    return f"{row['origin']}_{tool_label(row)}"


def tool_share(rows):
    """Per origin, how many replies a tool round came before, of how many."""
    return {
        origin: {
            "tool": sum(bool(r["tool_batches"]) for r in rows if r["origin"] == origin),
            "of": sum(r["origin"] == origin for r in rows),
        }
        for origin in sorted({r["origin"] for r in rows})
    }


def waterfall(first):
    """Consecutive segments from origin to the first native capture."""
    if not first or first.get("stage") != "native_audio_capture":
        return None
    accepted = first.get("elapsed_ms")
    output = first.get("first_provider_output_ms")
    end = first.get("provider_voice_end_ms")
    tool = first.get("first_tool_call_ms")
    responded = first.get("last_tool_response_ms")
    if accepted is None:
        return None
    if output is None or not 0 <= output <= accepted:
        output = accepted
    if end is not None and not 0 <= end <= output:
        end = None
    segments = {}

    # END can arrive with first output. The configured window supplies only
    # a baseline allowance, not evidence of when inference began.
    # A yield inside the window ends the candidate's turn in its place.
    window = first.get("silence_window_ms")
    yielded = first.get("candidate_yield_ms")
    cap = output if end is None else end
    if first.get("origin") == "last_voiced_pcm" and window:
        if yielded is not None and 0 <= yielded <= min(window, cap):
            segments["candidate_yield"] = yielded
        else:
            segments["silence_window"] = min(window, cap)
    base = segments.get("candidate_yield", segments.get("silence_window", 0))
    if end is not None:
        segments["endpointing"] = end - base
        base = end
    if (
        tool is not None
        and responded is not None
        and base <= tool <= responded <= accepted
    ):
        segments["tool_decision"] = tool - base
        segments["tool_rounds"] = responded - tool
        segments["post_tool_response"] = accepted - responded
    else:
        segments["response_start"] = output - base
        segments["output_to_audio"] = accepted - output
    segments["output_queue"] = first.get("output_queue_ms") or 0
    segments["native_capture"] = first.get("native_capture_ms") or 0
    total = first.get("server_total_ms")
    if total is not None:
        # Each field is rounded down to whole milliseconds separately.
        segments["unattributed"] = total - sum(segments.values())
    return segments


def obligation_summary(events, captured):
    """What the owed reply's records say about its identity, or None.

    A legacy reply has no obligation fields and has no summary. A reply with
    no terminal record is `missing`, which is a log cut short or a reply that
    never resolved, and either way is not an answer. `captured` is whether
    native capture succeeded for it.
    """
    owed = [e for e in events if e.get("obligation") is not None]
    if not owed:
        return None
    last = owed[-1]
    terminal = next(
        (
            e
            for e in reversed(owed)
            if e["obligation_outcome"] != "open" and e.get("continuing") is not True
        ),
        None,
    )
    count = max(e.get("obligation_episode_count") or 0 for e in owed)
    episodes = max((e.get("obligation_episodes") or [] for e in owed), key=len)
    return {
        "id": last["obligation"],
        "origin": last["origin"],
        "origin_socket": last.get("origin_socket"),
        "owed_start_interview_ms": last.get("owed_start_interview_ms"),
        "episodes": episodes,
        "episode_count": count,
        "first_accept_ms": last.get("first_accept_ms"),
        "first_accept_socket": last.get("first_accept_socket"),
        "native_capture": captured,
        "outcome": terminal["obligation_outcome"] if terminal else "missing",
    }


def obligation_report(rows):
    owed = [r["obligation"] for r in rows if r["obligation"] is not None]
    return {
        "count": len(owed),
        "outcomes": dict(Counter(o["outcome"] for o in owed)),
        "carried": sum(o["episode_count"] > 0 for o in owed),
        # A reply that crossed a replacement and was never answered is the case
        # the lifecycle ids exist to attribute.
        "carried_unanswered": sum(
            o["episode_count"] > 0 and o["outcome"] != "answered" for o in owed
        ),
    }


# The lifecycle lines the interview prints beside the latency records. Most
# carry (pid, session) and join by identity. `recovery ingress` and `candidate
# audio quarantine` predate the ids and carry only the room, so they are
# counted per room and never attributed to an episode.
LIFECYCLE_LINE = re.compile(
    r"(interview clock|runtime tasks|replacement start|replacement end|recovery ingress"
    r"|candidate audio quarantine): "
)
EPISODE_RESULTS = {"installed", "failed", "no_setup", "cancelled"}
# kind -> (whole numbers, whole numbers or `none`, words)
LIFECYCLE_FIELDS = {
    "interview clock": (
        ("pid", "session", "elapsed_ms", "sample"),
        ("start_unix_ms",),
        (),
    ),
    # `alive_tasks` is `none` where the runtime gave no count; a legacy log has
    # no such line at all. Neither is ever read as zero.
    "runtime tasks": (
        ("pid", "session", "elapsed_ms", "sample"),
        ("alive_tasks",),
        (),
    ),
    "replacement start": (
        ("pid", "session", "episode", "old_socket", "advisories"),
        ("obligation", "interview_ms"),
        ("trigger",),
    ),
    "replacement end": (
        ("pid", "session", "episode", "old_socket", "attempt"),
        (
            "new_socket",
            "replacement",
            "obligation",
            "notice_ms",
            "setup_ms",
            "connected_ms",
            "opener_ms",
            "install_ms",
            "interview_ms",
        ),
        ("result", "trigger", "resume_tried"),
    ),
    "recovery ingress": (
        (
            "dropped_bytes",
            "unconfirmed_bytes",
            "unsent_bytes",
            "omitted_contexts",
            "omitted_evictions",
        ),
        (),
        (),
    ),
    "candidate audio quarantine": (("bytes",), (), ()),
}


def whole_number(text):
    if not text.isascii() or not text.isdigit():
        raise Rejected("lifecycle number")
    value = int(text)
    if value > 2**63:
        raise Rejected("lifecycle number")
    return value


def parse_lifecycle(line):
    """(kind, fields) for a lifecycle line, None for any other line, or
    Rejected. The room is the rest of the line, because a name may hold spaces;
    every field before it is a number, `none` where the value is unknown, or a
    word."""
    match = LIFECYCLE_LINE.search(line)
    if MARKER in line or not match:
        return None
    kind = match.group(1)
    body = line[match.end() :].rstrip("\r\n")
    head, separator, room = body.partition(" room=")
    if not separator or not room:
        raise Rejected(f"{kind} room")
    fields = {"room": room}
    for token in head.split():
        name, equals, value = token.partition("=")
        if not equals or not name or name in fields:
            raise Rejected(f"{kind} field")
        fields[name] = value
    whole, unknown, words = LIFECYCLE_FIELDS[kind]
    for name in whole:
        if name not in fields:
            raise Rejected(f"{kind} {name}")
        fields[name] = whole_number(fields[name])
    for name in unknown:
        if name not in fields:
            raise Rejected(f"{kind} {name}")
        fields[name] = None if fields[name] == "none" else whole_number(fields[name])
    for name in words:
        if not fields.get(name):
            raise Rejected(f"{kind} {name}")
    if kind == "replacement end" and fields["result"] not in EPISODE_RESULTS:
        raise Rejected("replacement end result")
    if kind in ("replacement start", "replacement end") and fields["episode"] < 1:
        raise Rejected(f"{kind} episode")
    return kind, fields


# What an owed-origin reply record must carry for the analyzer to join it.
# `interview_ms` may be null where the clock is unknown, but its key is the
# emitter's promise that it keeps one.
REQUIRED_IDENTITY_KEYS = (
    "obligation",
    "obligation_outcome",
    "obligation_episode_count",
    "owed_start_interview_ms",
    "interview_ms",
)
REQUIRED_IDENTITY_VALUES = ("obligation", "obligation_outcome")


def identity_gaps(records):
    """Per missing field, how many owed-origin records lack it. A provider
    event owes nothing and has no obligation."""
    missing = Counter()
    for record in records:
        if record["origin"] == "provider_event":
            continue
        for name in REQUIRED_IDENTITY_KEYS:
            if name not in record or (
                name in REQUIRED_IDENTITY_VALUES and record[name] is None
            ):
                missing[name] += 1
    return dict(missing)


def clock_exposure(samples, observed_max):
    """The assessment-clock exposure of one scope in ms, or why there is none.

    The samples are verified when sample 0 anchors the start at zero, every
    copy of a sample number agrees, the start anchor never changes and elapsed
    never goes backwards with the sample number. A reused pid with another
    session is another scope; the same scope with a different anchor is a
    conflict, so it has no exposure rather than a guessed one. The exposure is
    the latest verified reading, extended to the latest record on that clock.
    """
    if not samples:
        return None, "no_clock_samples"
    by_number = {}
    for sample in samples:
        number = sample["sample"]
        if (
            number in by_number
            and by_number[number]["elapsed_ms"] != sample["elapsed_ms"]
        ):
            return None, "conflicting_samples"
        by_number.setdefault(number, sample)
    anchors = {s["start_unix_ms"] for s in samples} - {None}
    if len(anchors) > 1:
        return None, "conflicting_anchor"
    elapsed = [by_number[n]["elapsed_ms"] for n in sorted(by_number)]
    if any(b < a for a, b in zip(elapsed, elapsed[1:])):
        return None, "clock_went_backwards"
    first = by_number.get(0)
    if first is None or first["elapsed_ms"] != 0:
        return None, "no_start_sample"
    return max(elapsed[-1], observed_max or 0), None


def merged_length(intervals):
    """Total length of the union of (start, end) intervals: time owed once,
    however many replies or episodes it was owed through."""
    total = 0
    reach = None
    for start, end in sorted(intervals):
        if reach is None or start > reach:
            total += end - start
            reach = end
        elif end > reach:
            total += end - reach
            reach = end
    return total


def owed_gap(events, summary, run_end):
    """From the original obligation start to the first successful native
    capture, or to the observed end of the run for a reply that never got one.

    Nothing is dropped. A reply the log cut short keeps `censored` and its
    run-end endpoint; one whose start or end the log does not carry keeps
    `unknown` and no gap, so it stays in every denominator.
    """
    starts = [
        e["owed_start_interview_ms"]
        for e in events
        if e.get("owed_start_interview_ms") is not None
    ]
    start = min(starts) if starts else None
    success = next(
        (
            e
            for e in events
            if e["stage"] == "native_audio_capture" and e.get("captured") is True
        ),
        None,
    )
    end = None
    if success:
        state = "captured"
        censored = False
        end = success.get("interview_ms")
        source = "capture_record"
        # Offsets run from the origin the record itself carries, which a later
        # input can move before output; the gap still starts at the original.
        own = success.get("owed_start_interview_ms")
        if (
            end is None
            and own is not None
            and success.get("server_total_ms") is not None
        ):
            end = own + success["server_total_ms"]
            source = "origin_offset"
    elif summary["outcome"] in ("superseded", "ended"):
        state = summary["outcome"]
        censored = False
        source = "terminal_record"
        terminal = [
            e["interview_ms"]
            for e in events
            if e.get("obligation_outcome") == state
            and e.get("interview_ms") is not None
        ]
        end = max(terminal) if terminal else None
    else:
        # Answered with no audio ever captured, or no terminal record at all.
        state = "uncaptured" if summary["outcome"] == "answered" else "missing"
        censored = True
        end = run_end
        source = "run_end"
    unknown = None
    if start is None:
        unknown = "unknown_start"
    elif end is None:
        unknown = "unknown_end"
    elif end < start:
        unknown = "clock_backwards"
    gap = None if unknown else end - start
    return {
        "state": state,
        "start_interview_ms": start,
        "end_interview_ms": end,
        "end_source": source if end is not None else None,
        "censored": censored,
        "unknown": unknown,
        "owed_gap_ms": gap,
        "owed_gap_s": None if gap is None else round(gap / 1000, 3),
    }


def episode_class(episode, row):
    """Whether a replacement episode ended the debt it began with. Only audio
    accepted on the socket the episode installed can: a socket from before the
    episode never satisfies it, and audio on a later socket means this episode
    only carried the debt on."""
    if episode["result"] is None:
        return "incomplete"
    if episode["obligation"] is None:
        return "no_reply_owed"
    if row is None or row["obligation"] is None:
        return "missing_evidence"
    obligation = row["obligation"]
    accept = obligation["first_accept_socket"]
    if accept is not None and accept <= episode["old_socket"]:
        return "prior_socket"
    if episode["result"] != "installed":
        return "not_installed"
    new = episode["new_socket"]
    if new is None:
        return "missing_evidence"
    if accept is not None and accept < new:
        return "prior_socket"
    if obligation["outcome"] == "superseded":
        return "superseded"
    if row["owed"]["state"] == "missing":
        # No terminal record: the log may simply have ended.
        return "unresolved"
    if row["owed"]["state"] != "captured" or accept is None:
        return "unanswered"
    return "satisfied" if accept == new else "carried"


def build_episodes(starts, ends, rows, state_conflicted):
    by_obligation = {
        (r["pid"], r["room"], r["session"], r["turn"]): r
        for r in rows
        if r["obligation"] is not None
    }
    episodes = []
    for key in sorted(set(starts) | set(ends)):
        start = starts.get(key)
        end = ends.get(key)
        # The two lines of one episode name the same socket and obligation, or
        # neither is trusted for the join.
        conflict = key in state_conflicted or bool(
            start
            and end
            and (
                start["obligation"] != end["obligation"]
                or start["old_socket"] != end["old_socket"]
            )
        )
        base = {**(start or {}), **(end or {})}
        pid, room, session, episode = key
        obligation = base.get("obligation")
        row = (
            by_obligation.get((pid, room, session, obligation))
            if obligation is not None
            else None
        )
        entry = {
            "pid": pid,
            "room": room,
            "session": session,
            "episode": episode,
            "trigger": base.get("trigger"),
            "old_socket": base.get("old_socket"),
            "new_socket": end["new_socket"] if end else None,
            "obligation": obligation,
            "result": end["result"] if end else None,
            "start_interview_ms": start["interview_ms"] if start else None,
            "install_ms": end["install_ms"] if end else None,
            "setup_ms": end["setup_ms"] if end else None,
            "has_start": start is not None,
            "has_end": end is not None,
        }
        if conflict:
            entry["class"] = "conflict"
        elif start is None:
            entry["class"] = "missing_start"
        else:
            entry["class"] = episode_class(entry, row)
        entry["setup_to_first_output_ms"] = setup_to_first_output(entry, row)
        episodes.append(entry)
    return episodes


def setup_to_first_output(episode, row):
    """From the new socket's installation to the first provider output of the
    reply it was installed for, both on the assessment clock; None where either
    endpoint is unknown or the output came before the install. It is a
    different quantity from origin to native capture, which includes the
    candidate's silence and everything before the replacement."""
    if (
        episode["class"] != "satisfied"
        or episode["start_interview_ms"] is None
        or episode["install_ms"] is None
    ):
        return None
    audio = row["first_audio"] or {}
    first = audio.get("first_provider_output_ms")
    # The output offset runs from the origin of the record that holds it.
    origin = audio.get("owed_start_interview_ms")
    if first is None or origin is None:
        return None
    installed = episode["start_interview_ms"] + episode["install_ms"]
    gap = origin + first - installed
    return gap if gap >= 0 else None


def by_room(lines, names):
    """Totals of the room-scoped counters per room, plus how many lines."""
    out = {}
    for fields in lines:
        entry = out.setdefault(fields["room"], {"lines": 0, **dict.fromkeys(names, 0)})
        entry["lines"] += 1
        for name in names:
            entry[name] += fields[name]
    return out


def task_report(tasks):
    """The runtime's alive-task counts, or None when the log carries no sample.

    Absent is unknown, never zero: a legacy log has no sample, and a sample the
    runtime could not answer has `alive_tasks=none`. Only a number the log
    printed counts, so `alive_max` and `alive_last` are None when every sample
    is unanswered. The count is approximate on a multithreaded runtime.
    """
    samples = [(number, row) for rows in tasks.values() for number, row in rows.items()]
    if not samples:
        return None
    known = [r["alive_tasks"] for _, r in samples if r["alive_tasks"] is not None]
    latest = max(samples, key=lambda item: (item[1]["elapsed_ms"], item[0]))
    return {
        "samples": len(samples),
        "known": len(known),
        "unknown": len(samples) - len(known),
        "alive_max": max(known) if known else None,
        "alive_last": latest[1]["alive_tasks"],
        "scopes": len(tasks),
    }


def lifecycle_report(rows, state):
    """Owed-reply gaps, episodes and resource counts joined by identity."""
    episodes = build_episodes(state["starts"], state["ends"], rows, state["conflicted"])
    scopes = {}

    def scope(key):
        return scopes.setdefault(
            key, {"intervals": [], "owed": 0, "sockets": set(), "unknown": 0}
        )

    for row in rows:
        entry = scope((row["pid"], row["room"], row["session"]))
        entry["sockets"].update(
            e[name]
            for e in row["timeline"]
            for name in ("socket", "origin_socket")
            if e.get(name) is not None
        )
        owed = row["owed"]
        if owed is None:
            continue
        entry["owed"] += 1
        if owed["unknown"]:
            entry["unknown"] += 1
        else:
            entry["intervals"].append(
                (owed["start_interview_ms"], owed["end_interview_ms"])
            )
    for key, socket in state["pressure_sockets"]:
        scope(key)["sockets"].add(socket)
    for e in episodes:
        scope((e["pid"], e["room"], e["session"]))["sockets"].update(
            s for s in (e["old_socket"], e["new_socket"]) if s is not None
        )
    for key in state["clocks"]:
        scope(key)
    # An end derived from an origin offset is on the same clock too.
    for row in rows:
        if row["owed"] and row["owed"]["end_interview_ms"] is not None:
            observe(
                state,
                (row["pid"], row["room"], row["session"]),
                row["owed"]["end_interview_ms"],
            )

    scope_rows = []
    for key in sorted(scopes):
        entry = scopes[key]
        exposure, why = clock_exposure(
            state["clocks"].get(key, []), state["observed"].get(key)
        )
        scope_rows.append(
            {
                "pid": key[0],
                "room": key[1],
                "session": key[2],
                "owed_replies": entry["owed"],
                "unknown_gaps": entry["unknown"],
                "sockets": len(entry["sockets"]),
                "owed_interval_ms": merged_length(entry["intervals"]),
                "summed_gap_ms": sum(e - s for s, e in entry["intervals"]),
                "exposure_ms": exposure,
                "exposure_unverified": why,
                "observed_end_interview_ms": state["observed"].get(key),
            }
        )
    verified = [r for r in scope_rows if r["exposure_ms"] is not None]
    exposure_ms = sum(r["exposure_ms"] for r in verified)
    owed_ms = sum(r["owed_interval_ms"] for r in verified)
    owed = [r["owed"] for r in rows if r["owed"]]
    known = [o["owed_gap_ms"] for o in owed if o["owed_gap_ms"] is not None]
    tracks = [e["setup_to_first_output_ms"] for e in episodes]
    track_known = [t for t in tracks if t is not None]
    return {
        "count": len(owed),
        "states": dict(Counter(o["state"] for o in owed)),
        "censored": sum(o["censored"] for o in owed),
        "unknown": sum(o["unknown"] is not None for o in owed),
        "unknown_reasons": dict(Counter(o["unknown"] for o in owed if o["unknown"])),
        "known": len(known),
        # Counted by the first native record, a reply whose first capture failed
        # and a later one succeeded is `captured` here but not in `captured`.
        "captured_after_failed_first": sum(
            o["state"] == "captured" and not r["captured"]
            for r, o in ((r, r["owed"]) for r in rows if r["owed"])
        ),
        "gap_s_total": round(sum(known) / 1000, 3) if known else None,
        "gap_s_max": round(max(known) / 1000, 3) if known else None,
        "no_reply_owed": sum(
            r["obligation"] is None and r["origin"] == "provider_event" for r in rows
        ),
        "missing_identity": sum(
            r["obligation"] is None and r["origin"] != "provider_event" for r in rows
        ),
        "scopes": scope_rows,
        "unverified_scopes": len(scope_rows) - len(verified),
        # Owed time counted once however many replies and episodes it spans,
        # over the exposure of the scopes whose clock was verified. None
        # without any: unknown, never a rate of zero.
        "owed_s_in_verified_scopes": round(owed_ms / 1000, 3) if verified else None,
        "exposure_s": round(exposure_ms / 1000, 3) if verified else None,
        "owed_s_per_interview_hour": round(owed_ms / exposure_ms * 3600, 3)
        if exposure_ms
        else None,
        # A reply with no gap adds nothing and a censored one adds only what was
        # observed, so the rate then understates what was owed.
        "rate_is_lower_bound": any(o["unknown"] or o["censored"] for o in owed),
        "episodes": {
            "count": len(episodes),
            "classes": dict(Counter(e["class"] for e in episodes)),
            "rows": episodes,
        },
        "tracks": {
            "origin_to_native_capture_ms": statistics(
                [r["server_total_ms"] for r in rows if r["typical"]]
            ),
            "setup_to_first_output_ms": statistics(track_known),
            # Only a satisfied episode has the measurement; of those, how many
            # lacked an endpoint. The rest do not apply.
            "setup_to_first_output_unknown": sum(
                e["class"] == "satisfied" and e["setup_to_first_output_ms"] is None
                for e in episodes
            ),
            "setup_to_first_output_not_applicable": sum(
                e["class"] != "satisfied" for e in episodes
            ),
        },
        "resources": {
            "sockets_observed": sum(r["sockets"] for r in scope_rows),
            "recovery_ingress": by_room(
                state["ingress"],
                (
                    "dropped_bytes",
                    "unconfirmed_bytes",
                    "unsent_bytes",
                    "omitted_contexts",
                    "omitted_evictions",
                ),
            ),
            "audio_quarantine": by_room(state["quarantine"], ("bytes",)),
            "task": task_report(state["tasks"]),
        },
    }


def observe(state, scope, at):
    """Moves the latest assessment-clock reading seen in a scope forward."""
    if at is not None and at > state["observed"].get(scope, at - 1):
        state["observed"][scope] = at


def note_lifecycle(state, kind, fields):
    scope = scope_of(fields) if "pid" in fields else None
    if kind == "interview clock":
        seen = state["clocks"].setdefault(scope, [])
        # Both copies stay: the scope then has no verified exposure.
        state["conflicts"] += any(
            c["sample"] == fields["sample"] and c != fields for c in seen
        ) and not any(c == fields for c in seen)
        seen.append(fields)
        observe(state, scope, fields["elapsed_ms"])
    elif kind == "runtime tasks":
        seen = state["tasks"].setdefault(scope, {})
        earlier = seen.get(fields["sample"])
        if earlier is None:
            seen[fields["sample"]] = fields
        elif earlier != fields:
            state["conflicts"] += 1
        observe(state, scope, fields["elapsed_ms"])
    elif kind in ("replacement start", "replacement end"):
        table = state["starts" if kind == "replacement start" else "ends"]
        key = (*scope, fields["episode"])
        if key in table:
            if table[key] != fields:
                state["conflicts"] += 1
                state["conflicted"].add(key)
        else:
            table[key] = fields
        observe(state, scope, fields["interview_ms"])
    elif kind == "recovery ingress":
        state["ingress"].append(fields)
    else:
        state["quarantine"].append(fields)


def scope_of(fields):
    return (fields["pid"], fields["room"], fields["session"])


def ends_inside_marker(line):
    """Whether a cut last line stopped partway through a record's own prefix,
    which no parser sees as a record."""
    text = line.rstrip("\r\n")
    prefixes = [MARKER, *(f"{kind}: " for kind in LIFECYCLE_FIELDS)]
    # Past the first word, so a plain line that merely ends in "replacement"
    # is not taken for a cut one.
    return any(
        text.endswith(prefix[:k])
        for prefix in prefixes
        for k in range(prefix.index(" ") + 2, len(prefix))
    )


def analyze(lines, slow_ms=1500, stall_ms=5000, tail_open=False):
    turns = {}
    rejected = Counter()
    pressure = []
    records = []
    seen = set()
    duplicates = 0
    truncated_final = False
    # Lifecycle lines by identity. The first copy of an episode line is kept; a
    # copy that differs is a conflict, never a second episode.
    state = {
        "starts": {},
        "ends": {},
        "clocks": {},
        "tasks": {},
        "ingress": [],
        "quarantine": [],
        "pressure_sockets": set(),
        "observed": {},
        "conflicts": 0,
        "conflicted": set(),
    }

    lines = list(lines)
    for position, line in enumerate(lines):
        # A last line with no newline may be cut anywhere: a number or a room
        # name can be short and still parse, so a lifecycle line there is not
        # trusted. A cut JSON record fails to parse on its own.
        open_tail = tail_open and position == len(lines) - 1
        try:
            summary = parse_pressure(line)
            if summary is not None:
                pressure.append(summary)
                continue
            record = parse(line)
            if record is None:
                lifecycle = parse_lifecycle(line)
                if lifecycle is not None and open_tail:
                    truncated_final = True
                    continue
                if lifecycle is not None:
                    note_lifecycle(state, *lifecycle)
                elif open_tail and ends_inside_marker(line):
                    truncated_final = True
                continue
        except Rejected as error:
            rejected[str(error)] += 1
            truncated_final = truncated_final or open_tail
            continue
        payload = line.split(MARKER, 1)[1].strip()
        if payload in seen:
            duplicates += 1
            continue
        seen.add(payload)
        records.append(record)
        key = (record["pid"], record["room"], record["session"], record["turn"])
        turns.setdefault(key, []).append(record)
        observe(
            state,
            (record["pid"], record["room"], record["session"]),
            record.get("interview_ms"),
        )
    for summary in pressure:
        scope = (summary["pid"], summary["room"], summary["session"])
        state["pressure_sockets"].add((scope, summary["socket"]))
        observe(state, scope, summary.get("interview_ms"))
    rows = []
    for key, events in turns.items():
        # Origins can move while a reply waits; event order must use the
        # session clock, not offsets from those different origins.
        def ordering(event):
            at = event.get("observed_ms")
            if at is None:
                at = offset(event)
            return (at is None, at or 0)

        events.sort(key=ordering)
        native = next(
            (
                e
                for e in events
                if e["stage"].startswith("native_audio_")
                or e["stage"] == "audio_enqueue_failed"
            ),
            None,
        )
        accepted = next((e for e in events if e["stage"] == "audio_accepted"), None)

        # The accepted record carries the handler's own split, logged after
        # the worker's copy was taken.
        first = {**(accepted or {}), **(native or {})} or None
        captured = bool(first and first.get("captured") is True)
        segments = waterfall(first) if captured else None
        total = first.get("server_total_ms") if captured else None
        response = None
        dominant = None
        if segments and total is not None:
            allowance = ("candidate_yield", "silence_window")
            response = total - sum(segments.get(name, 0) for name in allowance)
            spent = {
                name: ms
                for name, ms in segments.items()
                if name not in (*allowance, "unattributed")
            }
            dominant = max(spent, key=spent.get) if spent else None
        outcome = next(
            (
                e["stage"]
                for e in reversed(events)
                if e["stage"]
                in ("turn_complete", "interrupted", "paused", "held", "socket_replaced")
                and not (
                    e["stage"] == "socket_replaced"
                    and e.get("continuing", e.get("first_provider_output_ms") is None)
                )
            ),
            "open",
        )
        obligation = obligation_summary(events, captured)
        rows.append(
            {
                "obligation": obligation,
                "owed": owed_gap(
                    events,
                    obligation,
                    state["observed"].get((key[0], key[1], key[2])),
                )
                if obligation
                else None,
                "pid": key[0],
                "room": key[1],
                "session": key[2],
                "turn": key[3],
                # Until output, a later input can replace the origin.
                "origin": (first or events[-1])["origin"],
                "outcome": outcome,
                # Rounds before the first audio, which is what held the reply
                # back; evidence recorded after it began is not.
                "tool_batches": (first.get("tool_batches") or 0)
                if first
                else max(e.get("tool_batches") or 0 for e in events),
                "first_audio": first,
                "captured": captured,
                "server_total_ms": total,
                "beyond_window_ms": response,
                "waterfall_ms": segments,
                "dominant": dominant,
                "slow": response is not None and response >= slow_ms,
                "stalled": response is not None and response >= stall_ms,
                "timeline": events,
            }
        )
    measured = [r for r in rows if r["captured"] and r["server_total_ms"] is not None]
    # Provider-event origins read near zero by construction.
    comparable = [r for r in measured if r["origin"] != "provider_event"]

    # A stall is a reply past `stall_ms`, a threshold set in the gap between
    # ordinary slow starts and sockets that stopped answering: it is counted as
    # a rate, and kept out of the distributions, where a few of them would set
    # every mean and tail.
    typical = [r for r in comparable if not r["stalled"]]
    chosen = {id(r) for r in typical}
    for row in rows:
        row["typical"] = id(row) in chosen

    # Whether replies slow as the conversation grows: the part of each wait
    # the model is responsible for, against the context it started from.
    by_context = [
        (reply_kind(r), r["first_audio"]["context_tokens"], r["beyond_window_ms"])
        for r in typical
        if r["first_audio"].get("context_tokens") is not None
    ]

    # On 3.1, END and the first output often arrive together, so the segment
    # named endpointing then also holds the model's own start.
    with_end = [
        r
        for r in comparable
        if r["first_audio"].get("provider_voice_end_ms") is not None
        and r["first_audio"].get("first_provider_output_ms") is not None
    ]
    lifecycle = lifecycle_report(rows, state)
    ingress = lifecycle["resources"]["recovery_ingress"]
    return {
        "slow_ms": slow_ms,
        "stall_ms": stall_ms,
        "owed": {k: v for k, v in lifecycle.items() if k != "resources"},
        "resources": {
            **lifecycle["resources"],
            "episode_ends": len(state["ends"]),
            # Those lines carry no ids. A driver prints at most one per episode
            # (a billing exit prints none), so more lines than ends means a
            # copy or a missing end.
            "ingress_lines_exceed_episode_ends": sum(
                v["lines"] for v in ingress.values()
            )
            > len(state["ends"]),
            # Counts with no ids cannot be told from copies of themselves, so
            # they are exact only where nothing in the log was a copy.
            "room_counts_exact": not duplicates
            and not state["conflicts"]
            and sum(v["lines"] for v in ingress.values()) <= len(state["ends"]),
        },
        "identity_gaps": identity_gaps(records),
        "duplicate_records": duplicates,
        "truncated_final": truncated_final,
        "lifecycle_conflicts": state["conflicts"],
        "stalls": {
            "count": len(comparable) - len(typical),
            "of": len(comparable),
        },
        "end_with_output": {
            "count": sum(
                abs(
                    r["first_audio"]["first_provider_output_ms"]
                    - r["first_audio"]["provider_voice_end_ms"]
                )
                <= 10
                for r in with_end
            ),
            "of": len(with_end),
        },
        "turns": rows,
        "obligations": obligation_report(rows),
        "socket_pressure": pressure_report(pressure),
        "rejected_records": sum(rejected.values()),
        "rejected_reasons": dict(rejected),
        "outcomes": dict(Counter(r["outcome"] for r in rows)),
        "unmeasured": dict(
            Counter(
                (r["first_audio"] or {}).get("stage", "no_audio")
                for r in rows
                if not r["captured"]
            )
        ),
        # Replies the statistics never see, by whether a tool round came
        # first: when most are tool turns, the distributions read fast.
        "unmeasured_by_tool": dict(
            Counter(tool_label(r) for r in rows if not r["captured"])
        ),
        "server_total_by_origin": {
            origin: statistics(
                [
                    r["server_total_ms"]
                    for r in measured
                    if r["origin"] == origin and not r["stalled"]
                ]
            )
            for origin in sorted({r["origin"] for r in measured})
        },
        "beyond_window": statistics([r["beyond_window_ms"] for r in typical]),
        # The same with the stalls left in, so the tail they set stays visible.
        "beyond_window_with_stalls": statistics(
            [r["beyond_window_ms"] for r in comparable]
        ),
        # With an acknowledgement played during a tool round, the candidate
        # hears something before the reply: how much sooner. The clip went out
        # on an empty queue, so its queueing is its capture; the reply's is its
        # native capture, which waited behind the clip when the clip was longer.
        "tool_ack_lead": statistics(
            [
                r["server_total_ms"] - r["first_audio"]["tool_ack_ms"]
                for r in typical
                if r["first_audio"].get("tool_ack_ms") is not None
            ]
        ),
        # A tool round costs the reply a second model pass, so how many
        # replies take one moves the typical wait as much as how fast each
        # kind is.
        "tool_share": tool_share(typical),
        "beyond_window_by_kind": {
            kind: statistics(
                [r["beyond_window_ms"] for r in typical if reply_kind(r) == kind]
            )
            for kind in sorted({reply_kind(r) for r in typical})
        },
        # How long before END the first input words arrived: the room an
        # early end-of-turn decision would have to judge a finished sentence.
        "transcript_before_end": statistics(
            [
                r["first_audio"]["provider_voice_end_ms"]
                - r["first_audio"]["first_transcript_ms"]
                for r in typical
                if r["first_audio"].get("provider_voice_end_ms") is not None
                and r["first_audio"].get("first_transcript_ms") is not None
            ]
        ),
        "beyond_window_by_context": {
            band: statistics(
                [ms for _, tokens, ms in by_context if context_band(tokens) == band]
            )
            for band in sorted(
                {context_band(tokens) for _, tokens, _ in by_context},
                key=lambda band: int(band.split("k_")[0]),
            )
        },
        "context_slope_ms_per_1k": slope_per_1k(by_context),
        "tool_calls": dict(
            Counter(
                name
                for r in rows
                for e in r["timeline"]
                if e["stage"] == "tool_response" and e.get("sent") is not False
                for name in e.get("tool_names") or ()
            )
        ),
        "segments": {
            name: statistics(
                [r["waterfall_ms"][name] for r in typical if name in r["waterfall_ms"]]
            )
            for name in SEGMENTS
        },
        "dominant": dict(Counter(r["dominant"] for r in typical if r["dominant"])),
        "slow_turns": [turn_id(r) for r in comparable if r["slow"]],
        "worst": sorted(comparable, key=lambda r: -r["beyond_window_ms"])[:5],
    }


# Resamples behind each interval. Seeded, so the same logs give the same report.
BOOTSTRAP_RESAMPLES = 2000


def median(values):
    return percentile(sorted(values), 0.50)


def median_shift(before, after, rng):
    """After's median less before's, with a 95% percentile-bootstrap interval.

    Replies are resampled one by one, which ignores what replies in one session
    share, so the interval is if anything too narrow.
    """
    shifts = sorted(
        median(rng.choices(after, k=len(after)))
        - median(rng.choices(before, k=len(before)))
        for _ in range(BOOTSTRAP_RESAMPLES)
    )
    return {
        "ms": round(median(after) - median(before), 1),
        "ci95_ms": [percentile(shifts, 0.025), percentile(shifts, 0.975)],
    }


def compare(before, after):
    """How replies of each origin changed between two reports.

    Two runs' medians alone mislead: provider speed moves a run's median by a
    few hundred milliseconds, and a run with more tool replies is slower with
    nothing else changed. So each kind of reply is compared on its own, and the
    change in the mean is split exactly into mix, the part from a different
    share of tool replies, and speed, the part from each kind getting faster or
    slower. A kind one side never saw is priced at the other side's mean, so it
    counts as mix.
    """
    rng = random.Random(0)
    sides = {
        name: [r for r in report["turns"] if r["typical"]]
        for name, report in (("before", before), ("after", after))
    }
    origins = sorted({r["origin"] for rows in sides.values() for r in rows})
    out = {}
    for origin in origins:
        kinds = {}
        mix = speed = 0.0
        totals = {
            name: sum(r["origin"] == origin for r in rows)
            for name, rows in sides.items()
        }
        for kind in ("no_tool", "tool"):
            values = {
                name: [
                    r["beyond_window_ms"]
                    for r in rows
                    if r["origin"] == origin and tool_label(r) == kind
                ]
                for name, rows in sides.items()
            }
            if not any(values.values()):
                continue
            share = {
                name: len(values[name]) / totals[name] if totals[name] else 0.0
                for name in sides
            }
            mean = {name: sum(v) / len(v) for name, v in values.items() if v}
            mean.setdefault("before", mean.get("after"))
            mean.setdefault("after", mean.get("before"))
            mix += (
                (share["after"] - share["before"])
                * (mean["after"] + mean["before"])
                / 2
            )
            speed += (
                (share["after"] + share["before"])
                / 2
                * (mean["after"] - mean["before"])
            )
            kinds[kind] = {
                "before": statistics(values["before"]),
                "after": statistics(values["after"]),
                "shift": median_shift(values["before"], values["after"], rng)
                if all(len(v) > 1 for v in values.values())
                else None,
            }
        means = {
            name: [r["beyond_window_ms"] for r in rows if r["origin"] == origin]
            for name, rows in sides.items()
        }
        out[origin] = {
            "tool_share": {
                name: tool_share(rows).get(origin, {"tool": 0, "of": 0})
                for name, rows in sides.items()
            },
            "kinds": kinds,
            "mean_ms": {
                name: round(sum(v) / len(v), 1) if v else None
                for name, v in means.items()
            },
            # With no replies on one side there is no mix to have shifted.
            "mix_ms": round(mix, 1) if all(totals.values()) else None,
            "speed_ms": round(speed, 1) if all(totals.values()) else None,
        }
    return {
        "origins": out,
        "stalls": {"before": before["stalls"], "after": after["stalls"]},
    }


def render_comparison(comparison):
    lines = [
        "Time beyond the silence window, after less before. Medians per kind of",
        "reply, with a 95% bootstrap interval; the mean's change split into mix",
        "(share of tool replies) and speed (each kind faster or slower).",
    ]
    for origin, entry in comparison["origins"].items():
        share = entry["tool_share"]
        lines.append(
            f"\n{origin}: tool round first in "
            + " -> ".join(
                f"{share[side]['tool']}/{share[side]['of']}"
                for side in ("before", "after")
            )
        )
        for kind, stats in entry["kinds"].items():
            cells = [
                f"n{stats[side]['count']} p50={stats[side]['p50_ms']}"
                if stats[side]["count"]
                else "none"
                for side in ("before", "after")
            ]
            line = f"  {kind:<8} {cells[0]} -> {cells[1]}"
            if stats["shift"]:
                low, high = stats["shift"]["ci95_ms"]
                line += f"  shift {stats['shift']['ms']:+} [{low:+}, {high:+}]"
            lines.append(line)
        mean = entry["mean_ms"]
        if None not in mean.values():
            lines.append(
                f"  mean     {mean['before']} -> {mean['after']} "
                f"({mean['after'] - mean['before']:+.1f}): "
                f"mix {entry['mix_ms']:+}, speed {entry['speed_ms']:+}"
            )
    stalls = comparison["stalls"]
    lines.append(
        "\nstalls "
        + " -> ".join(
            f"{stalls[side]['count']}/{stalls[side]['of']}"
            for side in ("before", "after")
        )
    )
    return "\n".join(lines)


# Paired per-session regression screen. The unit of evidence is a session, not
# a frame: replies inside one session share a provider connection, a context
# and a candidate, so they are not independent samples.
PAIRED_METRICS = {
    "beyond_window_ms": lambda row: row["beyond_window_ms"],
    "origin_to_native_capture_ms": lambda row: row["server_total_ms"],
    "origin_to_first_output_ms": lambda row: (row["first_audio"] or {}).get(
        "first_provider_output_ms"
    ),
}
PAIRED_SEED = 20_260_101
PAIRED_RESAMPLES = 2000
PAIRED_VERDICT = "descriptive-inconclusive"
PAIRED_CAVEAT = (
    "DESCRIPTIVE regression screen, inconclusive for this budget: no arm is "
    "registered, so no adoption verdict is given and the 30% latency target is "
    "not evaluated here. A resampled interval over few sessions can look narrow "
    "and still be wrong."
)


def paired_samples(report):
    """Per metric, per (room, session): valid values and failed/missing counts.

    A reply that never reached native capture is `failed`; a captured reply
    whose metric was not recorded is `missing`. Both stay in the denominator.
    Replies owed to a provider event read near zero by construction and are left
    out, as everywhere else. Stalls are kept: p95 is about the tail.
    """
    out = {name: {} for name in PAIRED_METRICS}
    for row in report["turns"]:
        if row["origin"] == "provider_event":
            continue
        for name, read in PAIRED_METRICS.items():
            cell = out[name].setdefault(
                (row["room"], row["session"]),
                {"values": [], "failed": 0, "missing": 0},
            )
            value = read(row) if row["captured"] else None
            if value is not None:
                cell["values"].append(value)
            elif row["captured"]:
                cell["missing"] += 1
            else:
                cell["failed"] += 1
    return out


def paired_arm(cells):
    valid = sum(len(c["values"]) for c in cells.values())
    failed = sum(c["failed"] for c in cells.values())
    missing = sum(c["missing"] for c in cells.values())
    total = valid + failed + missing
    pooled = sorted(v for c in cells.values() for v in c["values"])
    return {
        "sessions": len(cells),
        "samples": {
            "of": total,
            "valid": valid,
            "failed": failed,
            "missing": missing,
        },
        "failure_rate": round((failed + missing) / total, 3) if total else None,
        "p95_ms": percentile(pooled, 0.95) if pooled else None,
    }


def cluster_interval(pairs, rng):
    """95% percentile interval for the pooled-p95 shift, resampling sessions.

    Each draw picks whole sessions with replacement and takes both arms'
    samples of a picked session together, so the within-session correlation
    stays in the interval. Resampling individual replies would treat them as
    independent and understate the width.
    """
    shifts = []
    for _ in range(PAIRED_RESAMPLES):
        picked = rng.choices(pairs, k=len(pairs))
        before = sorted(v for b, _ in picked for v in b)
        after = sorted(v for _, a in picked for v in a)
        shifts.append(percentile(after, 0.95) - percentile(before, 0.95))
    shifts.sort()
    return [percentile(shifts, 0.025), percentile(shifts, 0.975)]


def paired_metric(before_cells, after_cells):
    matched = sorted(set(before_cells) & set(after_cells))
    usable = [
        key
        for key in matched
        if before_cells[key]["values"] and after_cells[key]["values"]
    ]
    sessions = []
    for key in usable:
        b = sorted(before_cells[key]["values"])
        a = sorted(after_cells[key]["values"])
        sessions.append(
            {
                "session": f"{key[0]}/s{key[1]}",
                "before_n": len(b),
                "after_n": len(a),
                "before_p95_ms": percentile(b, 0.95),
                "after_p95_ms": percentile(a, 0.95),
                "delta_ms": round(percentile(a, 0.95) - percentile(b, 0.95), 1),
            }
        )
    deltas = sorted(s["delta_ms"] for s in sessions)
    entry = {
        "arms": {
            "before": paired_arm(before_cells),
            "after": paired_arm(after_cells),
        },
        "pairs": {
            "matched": len(matched),
            "usable": len(usable),
            "no_valid_sample": len(matched) - len(usable),
            "only_before": len(set(before_cells) - set(after_cells)),
            "only_after": len(set(after_cells) - set(before_cells)),
        },
        "sessions": sessions,
        "median_session_delta_ms": median(deltas) if deltas else None,
        "pooled_p95_shift_ms": None,
        "interval": None,
    }
    pairs = [(before_cells[k]["values"], after_cells[k]["values"]) for k in usable]
    if pairs:
        before = sorted(v for pair in pairs for v in pair[0])
        after = sorted(v for pair in pairs for v in pair[1])
        entry["pooled_p95_shift_ms"] = round(
            percentile(after, 0.95) - percentile(before, 0.95), 1
        )
    if len(pairs) >= 2:
        entry["interval"] = {
            "method": "session-cluster percentile bootstrap of the pooled p95 shift",
            "seed": PAIRED_SEED,
            "resamples": PAIRED_RESAMPLES,
            "ci95_ms": cluster_interval(pairs, random.Random(PAIRED_SEED)),
        }
    return entry


def paired_report(before, after):
    cells = paired_samples(before), paired_samples(after)
    return {
        "verdict": PAIRED_VERDICT,
        "caveat": PAIRED_CAVEAT,
        "limitations": [
            "the session is the unit; replies in one session are not independent",
            "pairs match by (room, session) across two logs; arms are not randomised",
            "an interval over fewer than about 10 pairs is not a coverage claim",
            "a percentile bootstrap is biased for a p95 over few samples",
            "server-side origin to capture; not browser-perceived end to end",
        ],
        "metrics": {
            name: paired_metric(cells[0][name], cells[1][name])
            for name in PAIRED_METRICS
        },
    }


def render_paired(report):
    lines = [report["caveat"]]
    for name, entry in report["metrics"].items():
        pairs = entry["pairs"]
        lines.append(
            f"\n{name}: pairs matched={pairs['matched']} usable={pairs['usable']} "
            f"no_valid_sample={pairs['no_valid_sample']} "
            f"only_before={pairs['only_before']} only_after={pairs['only_after']}"
        )
        for side in ("before", "after"):
            arm = entry["arms"][side]
            n = arm["samples"]
            lines.append(
                f"  {side:<6} sessions={arm['sessions']} "
                f"samples={n['valid']}/{n['of']} "
                f"failed={n['failed']} missing={n['missing']} p95={arm['p95_ms']}"
            )
        for s in entry["sessions"]:
            lines.append(
                f"  {s['session']}: p95 {s['before_p95_ms']} (n{s['before_n']}) -> "
                f"{s['after_p95_ms']} (n{s['after_n']}) {s['delta_ms']:+}"
            )
        if entry["interval"]:
            low, high = entry["interval"]["ci95_ms"]
            lines.append(
                f"  pooled p95 shift {entry['pooled_p95_shift_ms']:+} "
                f"[{low:+}, {high:+}] (session-cluster bootstrap, seed "
                f"{entry['interval']['seed']}, "
                f"{entry['interval']['resamples']} resamples)"
            )
        else:
            lines.append("  no interval: fewer than 2 usable pairs")
        if entry["median_session_delta_ms"] is not None:
            lines.append(
                f"  median of per-session deltas {entry['median_session_delta_ms']:+}"
            )
    lines.append("\nLimitations:")
    lines.extend(f"  {text}" for text in report["limitations"])
    return "\n".join(lines)


# Control revisions the adapter knows. A control log predates the lifecycle
# ids, so only the metrics a legacy log carries are comparable; the adapter is
# pinned to the revision that produced it, and an unlisted revision is refused
# rather than assumed to behave the same.
CONTROL_REVISIONS = {
    "1ceda0efffe4e934d63c15867788b38258b92c95": "legacy reply_latency v2 without lifecycle ids",
}
OBSERVABLE_IN_CONTROL = (
    "reply count and captured count",
    "stalls as a rate",
    "beyond-window time by origin and tool round",
    "origin to native capture (server_total_ms)",
    "origin to first provider output",
)
UNOBSERVABLE_IN_CONTROL = (
    "owed_gap_s and censoring (no obligation start on the assessment clock)",
    "owed seconds per interview-hour (no interview clock samples)",
    "obligation outcomes: answered, superseded, ended, missing",
    "carried debt across replacements",
    "replacement episodes and whether a new socket satisfied them",
    "setup-to-first-output after a replacement",
    "per-socket event-channel pressure",
    "recovery ingress, quarantine and omission counts by episode",
    "sockets observed per interview",
    "runtime task counts (no runtime tasks lines)",
)


class AdapterRefused(ValueError):
    pass


def control_view(report):
    """The control report restricted to what a legacy log can show: the
    comparison reads only typical, measured replies and the stall rate."""
    keep = ("typical", "origin", "beyond_window_ms", "tool_batches")
    return {
        "turns": [{k: r[k] for k in keep} for r in report["turns"]],
        "stalls": report["stalls"],
    }


def legacy_metrics(report):
    """What a legacy log shows, from either side of the comparison."""
    captured_rows = [r for r in report["turns"] if r["captured"]]
    return {
        "replies": len(report["turns"]),
        "captured": len(captured_rows),
        "stalls": report["stalls"],
        "origin_to_native_capture_ms": report["server_total_by_origin"],
        "origin_to_first_output_ms": statistics(
            [
                r["first_audio"]["first_provider_output_ms"]
                for r in captured_rows
                if r["origin"] != "provider_event"
                and r["first_audio"].get("first_provider_output_ms") is not None
            ]
        ),
    }


def adapt_control(sha, control, candidate):
    """Compares a legacy control with a candidate on the legacy metrics only.

    The candidate must carry the lifecycle ids; there is no fallback that reads
    a candidate without them, because a candidate that quietly lost its ids
    would compare as healthy.
    """
    if sha not in CONTROL_REVISIONS:
        raise AdapterRefused(f"control revision {sha} has no adapter")
    if not candidate["turns"]:
        raise AdapterRefused("candidate log has no replies")
    if not any(r["origin"] != "provider_event" for r in candidate["turns"]):
        raise AdapterRefused(
            "candidate log has no reply owed to a local input, so there are "
            "no lifecycle ids to require"
        )
    if any(r["obligation"] is not None for r in control["turns"]):
        raise AdapterRefused(
            "control log carries lifecycle ids, so it is not a log of a legacy revision"
        )
    if candidate["rejected_records"]:
        reasons = ", ".join(
            f"{name} x{count}"
            for name, count in sorted(candidate["rejected_reasons"].items())
        )
        raise AdapterRefused(
            f"candidate log has rejected records, so its ids cannot be trusted: {reasons}"
        )
    if candidate["identity_gaps"]:
        missing = ", ".join(
            f"{name} x{count}"
            for name, count in sorted(candidate["identity_gaps"].items())
        )
        raise AdapterRefused(f"candidate log lacks required lifecycle ids: {missing}")
    comparison = compare(control_view(control), control_view(candidate))
    return {
        "control_revision": sha,
        "control_log": CONTROL_REVISIONS[sha],
        "comparison": comparison,
        "control": legacy_metrics(control),
        "candidate_legacy": legacy_metrics(candidate),
        "observable": list(OBSERVABLE_IN_CONTROL),
        "unobservable": list(UNOBSERVABLE_IN_CONTROL),
        "candidate": {
            "owed": {k: v for k, v in candidate["owed"].items() if k != "scopes"},
            "resources": candidate["resources"],
        },
    }


def turn_id(row):
    return f"{row['room']}/s{row['session']}/t{row['turn']}"


def verdict(report):
    if not report["stalls"]["of"]:
        return "NO_DATA"
    return "SLOW" if report["slow_turns"] else "HEALTHY"


def bar(ms, total, width=30):
    if not total or ms <= 0:
        return ""
    return "#" * max(1, round(width * ms / total))


def render(report):
    lines = [
        "Server reply latency, ms from each reply's origin (monotonic clock).",
        "Native capture is not browser playback; provider internals are unmeasured.",
    ]
    for row in report["turns"]:
        lines.append(
            f"\n{turn_id(row)} socket={row['timeline'][-1]['socket']} "
            f"origin={row['origin']} outcome={row['outcome']}"
            + (f" tool_batches={row['tool_batches']}" if row["tool_batches"] else "")
            + ("  SLOW" if row["slow"] else "")
        )
        segments = row["waterfall_ms"]
        if segments:
            total = row["server_total_ms"]
            lines.append(
                f"  total {total}  beyond window {row['beyond_window_ms']}  "
                f"dominant {row['dominant']}"
            )
            for name in (*SEGMENTS, "unattributed"):
                if name in segments:
                    ms = segments[name]
                    lines.append(f"  {name:<19}{ms:>7}  {bar(ms, total)}")
            first = row["first_audio"]
            overlays = [
                f"{name}={first[name]}"
                for name in (
                    "tool_handler_ms",
                    "provider_event_queue_ms",
                    "stale_drop_ms",
                    "enqueue_ms",
                    "handler_tail_ms",
                    "output_queue_frames",
                    "playout_ahead_ms",
                    "context_tokens",
                )
                if first.get(name) is not None
                and (name != "tool_handler_ms" or row["tool_batches"])
            ]
            if overlays:
                lines.append("  within: " + " ".join(overlays))
        else:
            stage = (row["first_audio"] or {}).get("stage", "no audio")
            lines.append(f"  first audio not captured ({stage})")
        for event in row["timeline"]:
            at = offset(event)
            detail = ""
            if event["stage"] == "tool_response":
                detail = " " + " ".join(
                    f"{name}={event.get(name)}"
                    for name in (
                        "tool_count",
                        "tool_names",
                        "tool_batch_ms",
                        "tool_execute_ms",
                        "board_resend_ms",
                        "tool_send_ms",
                        "sent",
                    )
                )
            lines.append(f"    +{'?' if at is None else at} {event['stage']}{detail}")
    lines.append("")
    lines.extend(summary_lines(report))
    return "\n".join(lines)


def stat_line(label, stats):
    if not stats["count"]:
        return f"{label}=n0"
    line = f"{label}=n{stats['count']} p50={stats['p50_ms']} p95={stats['p95_ms']}"
    return line + (" small_n" if stats["small_sample"] else "")


def summary_lines(report):
    out = [
        f"verdict={verdict(report)}",
        f"turns={len(report['turns'])}",
        f"captured={sum(r['captured'] for r in report['turns'])}",
        f"slow_ms={report['slow_ms']}",
        f"slow_turns={len(report['slow_turns'])}",
        f"stalls={report['stalls']['count']}/{report['stalls']['of']}",
        f"end_with_output={report['end_with_output']['count']}"
        f"/{report['end_with_output']['of']}",
        f"rejected_records={report['rejected_records']}",
    ]
    for name, count in sorted(report["outcomes"].items()):
        out.append(f"outcome_{name}={count}")
    obligations = report["obligations"]
    if obligations["count"]:
        out.append(f"obligations={obligations['count']}")
        for name, count in sorted(obligations["outcomes"].items()):
            out.append(f"obligation_{name}={count}")
        out.append(f"obligations_carried={obligations['carried']}")
        out.append(
            f"obligations_carried_unanswered={obligations['carried_unanswered']}"
        )
    owed = report["owed"]
    resources = report["resources"]
    if owed["count"] or owed["episodes"]["count"]:
        out.append(f"owed_replies={owed['count']}")
        out.append(f"owed_gap_known={owed['known']}")
        out.append(f"owed_gap_unknown={owed['unknown']}")
        out.append(f"owed_gap_censored={owed['censored']}")
        out.append(
            f"owed_captured_after_failed_first={owed['captured_after_failed_first']}"
        )
        for name, count in sorted(owed["states"].items()):
            out.append(f"owed_state_{name}={count}")
        for name, count in sorted(owed["unknown_reasons"].items()):
            out.append(f"owed_unknown_{name}={count}")
        for name in ("gap_s_total", "gap_s_max"):
            if owed[name] is not None:
                out.append(f"owed_{name}={owed[name]}")
        # Without a verified clock the rate is unknown; it is never printed as 0.
        rate = owed["owed_s_per_interview_hour"]
        out.append(f"owed_s_per_interview_hour={'unknown' if rate is None else rate}")
        out.append(f"owed_unverified_scopes={owed['unverified_scopes']}")
        out.append(f"no_reply_owed={owed['no_reply_owed']}")
        out.append(f"missing_identity={owed['missing_identity']}")
        out.append(f"episodes={owed['episodes']['count']}")
        for name, count in sorted(owed["episodes"]["classes"].items()):
            out.append(f"episode_{name}={count}")
        tracks = owed["tracks"]
        out.append(
            stat_line("origin_to_native_capture", tracks["origin_to_native_capture_ms"])
        )
        out.append(
            stat_line("setup_to_first_output", tracks["setup_to_first_output_ms"])
        )
        out.append(f"sockets_observed={resources['sockets_observed']}")
        for label, table in (
            ("recovery_ingress", resources["recovery_ingress"]),
            ("audio_quarantine", resources["audio_quarantine"]),
        ):
            for name in sorted({n for v in table.values() for n in v if n != "lines"}):
                out.append(f"{label}_{name}={sum(v[name] for v in table.values())}")
        task = resources["task"]
        if task is None:
            out.append("task=unknown")
        else:
            out.append(f"task_samples={task['samples']}")
            out.append(f"task_unknown_samples={task['unknown']}")
            for name in ("alive_max", "alive_last"):
                value = task[name]
                out.append(f"task_{name}={'unknown' if value is None else value}")
    if report["duplicate_records"]:
        out.append(f"duplicate_records={report['duplicate_records']}")
    if report["truncated_final"]:
        out.append("truncated_final=true")
    if report["lifecycle_conflicts"]:
        out.append(f"lifecycle_conflicts={report['lifecycle_conflicts']}")
    pressure = report["socket_pressure"]
    if pressure["sockets"]:
        out.append(f"socket_pressure={pressure['sockets']}")
        for name in (
            "duplicates",
            "conflicts",
            "partial",
            "blocked_sockets",
            "blocked_unknown",
            "full_sockets",
            "read_idle_ends",
        ):
            out.append(f"socket_pressure_{name}={pressure[name]}")
        # A total nobody reported is left out rather than printed as 0.
        for name in (
            "blocked_sends",
            "blocked_ms_total",
            "blocked_ms_max",
            "high_water_max",
        ):
            if pressure[name] is not None:
                out.append(f"socket_pressure_{name}={pressure[name]}")
    for name, count in sorted(report["unmeasured"].items()):
        out.append(f"unmeasured_{name}={count}")
    for name, count in sorted(report["unmeasured_by_tool"].items()):
        out.append(f"unmeasured_by_tool_{name}={count}")
    for origin, stats in report["server_total_by_origin"].items():
        if stats["count"]:
            out.append(stat_line(f"total_{origin}", stats))
    for label in (
        "beyond_window",
        "beyond_window_with_stalls",
        "transcript_before_end",
        "tool_ack_lead",
    ):
        if report[label]["count"]:
            out.append(stat_line(label, report[label]))
    for origin, share in report["tool_share"].items():
        out.append(f"tool_share_{origin}={share['tool']}/{share['of']}")
    for kind, stats in report["beyond_window_by_kind"].items():
        out.append(stat_line(f"kind_{kind}", stats))
    for band, stats in report["beyond_window_by_context"].items():
        out.append(stat_line(f"context_{band}", stats))
    if report["context_slope_ms_per_1k"] is not None:
        out.append(f"context_slope_ms_per_1k={report['context_slope_ms_per_1k']}")
    for name, count in sorted(report["tool_calls"].items()):
        out.append(f"tool_{name}={count}")
    for name in SEGMENTS:
        if report["segments"][name]["count"]:
            out.append(stat_line(f"segment_{name}", report["segments"][name]))
    for name, count in sorted(report["dominant"].items(), key=lambda i: -i[1]):
        out.append(f"dominant_{name}={count}")
    if report["worst"]:
        worst = report["worst"][0]
        out.append(
            f"worst={turn_id(worst)} beyond_window={worst['beyond_window_ms']} "
            f"dominant={worst['dominant']}"
        )
    return out


def strict_pass(report):
    """What --strict asks of a report beyond having records at all."""
    return (
        not report["rejected_records"]
        and not report["truncated_final"]
        and verdict(report) != "NO_DATA"
    )


def analyze_stream(stream, args):
    """Reads a log; a last line cut short of its newline is not trusted."""
    lines = list(stream)
    tail_open = bool(lines) and not lines[-1].endswith("\n")
    return analyze(lines, args.slow_ms, args.stall_ms, tail_open)


def render_adapted(adapted):
    lines = [
        f"Control revision {adapted['control_revision']} ({adapted['control_log']}).",
        render_comparison(adapted["comparison"]),
        *[
            f"{side}: replies={m['replies']} captured={m['captured']} "
            f"{stat_line('first_output', m['origin_to_first_output_ms'])}"
            for side, m in (
                ("control", adapted["control"]),
                ("candidate", adapted["candidate_legacy"]),
            )
        ],
        "\nObservable in the control:",
        *[f"  {name}" for name in adapted["observable"]],
        "Not observable in the control, so not compared:",
        *[f"  {name}" for name in adapted["unobservable"]],
    ]
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("log", nargs="?", help="server log; omit to read stdin")
    output = parser.add_mutually_exclusive_group()
    output.add_argument("--json", action="store_true", help="full report as JSON")
    output.add_argument(
        "--summary", action="store_true", help="key=value lines only, for scripts"
    )
    parser.add_argument(
        "--slow-ms",
        type=int,
        default=1500,
        help="flag a reply whose time beyond the silence window reaches this",
    )
    parser.add_argument(
        "--stall-ms",
        type=int,
        default=5000,
        help="count a reply this far beyond the window as a stall, not a slow reply",
    )
    parser.add_argument(
        "--compare",
        metavar="BEFORE_LOG",
        help="compare the log against this earlier one, kind of reply by kind",
    )
    parser.add_argument(
        "--paired",
        metavar="BEFORE_LOG",
        help="paired per-session p95 screen of the log against this earlier one; "
        "descriptive only, never an adoption verdict",
    )
    parser.add_argument(
        "--control-adapter",
        metavar="SHA",
        help="with --compare: read the earlier log as a legacy control from this "
        "pinned revision and compare only what it can show; the log under test "
        "must carry the lifecycle ids",
    )
    parser.add_argument(
        "--strict",
        action="store_true",
        help="also fail on rejected records or when no input-to-capture reply was measured",
    )
    args = parser.parse_args()
    if args.paired and args.compare:
        parser.error("--paired and --compare are separate reports")
    if args.control_adapter is not None and not args.compare:
        parser.error("--control-adapter needs --compare")
    if args.log:
        with open(args.log, encoding="utf-8", errors="replace") as stream:
            report = analyze_stream(stream, args)
    else:
        report = analyze_stream(sys.stdin, args)
    if args.paired:
        with open(args.paired, encoding="utf-8", errors="replace") as stream:
            before = analyze_stream(stream, args)
        if not report["turns"] or not before["turns"]:
            side = "log under test" if not report["turns"] else "earlier log"
            print(f"error: the {side} has no replies", file=sys.stderr)
            return 1
        paired = paired_report(before, report)
        print(json.dumps(paired, indent=2) if args.json else render_paired(paired))
        return int(args.strict and not all(map(strict_pass, (before, report))))
    if args.compare:
        with open(args.compare, encoding="utf-8", errors="replace") as stream:
            before = analyze_stream(stream, args)
        if not report["turns"] or not before["turns"]:
            if args.control_adapter is not None:
                side = "candidate" if not report["turns"] else "control"
                print(f"error: {side} log has no replies", file=sys.stderr)
            return 1
        if args.control_adapter is not None:
            try:
                adapted = adapt_control(args.control_adapter, before, report)
            except AdapterRefused as error:
                print(f"error: {error}", file=sys.stderr)
                return 1
            print(
                json.dumps(adapted, indent=2) if args.json else render_adapted(adapted)
            )
            return int(args.strict and not all(map(strict_pass, (before, report))))
        comparison = compare(before, report)
        print(
            json.dumps(comparison, indent=2)
            if args.json
            else render_comparison(comparison)
        )
        return int(args.strict and not all(map(strict_pass, (before, report))))
    if args.json:
        print(json.dumps(report, indent=2))
    elif args.summary:
        print("\n".join(summary_lines(report)))
    else:
        print(render(report))
    if not report["turns"]:
        return 1
    if args.strict and not strict_pass(report):
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())

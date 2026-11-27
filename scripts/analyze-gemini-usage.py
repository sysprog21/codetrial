#!/usr/bin/env python3
"""Summarize received Gemini counters without treating them as an invoice.

Reads codetrial server log lines (optionally prefixed by journald, docker or
timestamp tools) and prints JSON grouped by room and by Live session. Counts
are what the provider reported to the server, not what was billed.
"""

import argparse
import json
import re
import sys


DIRECTIONS = (
    ("prompt", "prompt_tokens"),
    ("response", "response_tokens"),
    ("tool_use", "tool_use_prompt_tokens"),
    ("cache", "cached_tokens"),
)
MODALITIES = ("text", "audio", "image", "video", "other")

COUNTERS = (
    "prompt_tokens",
    "response_tokens",
    "cached_tokens",
    "thought_tokens",
    "total_tokens",
    "tool_use_prompt_tokens",
    "usage_samples",
    "turn_complete_samples",
) + tuple(
    f"{direction}_{suffix}"
    for direction, _ in DIRECTIONS
    for suffix in ("detail_samples",) + tuple(f"{m}_tokens" for m in MODALITIES)
)

# Markers are searched anywhere in the line; fields are parsed only from the
# marker onward so a log prefix cannot contribute key=value pairs.
LIVE_SUMMARY = re.compile(r"\bcodetrial live_usage ")
LIVE_EVENT = re.compile(r"\bcodetrial live_turn_usage ")
HTTP_USAGE = re.compile(
    r"\bgemini (report|interim|phase|moments) (?!transport_failed )(?=.*\busage )"
)
# A room name may contain a colon; the interim line's delimiter is the colon
# followed by a space.
TRANSPORT_FAILED = re.compile(r"\bgemini report transport_failed room=(\S+)")
INTERIM_SKIPPED = re.compile(r"\binterim review skipped room=(\S+?): ")
PHASE_SKIPPED = re.compile(r"\bphase judge skipped room=(\S+?): ")
MOMENTS_SKIPPED = re.compile(r"\bkey moments skipped room=(\S+?): ")
CONTEXT_REFRESH = re.compile(r"\bcodetrial context_refresh ")

UNKNOWN = "unknown"


def fields(text):
    return dict(re.findall(r"(?:^|\s)([a-z_]+)=([^\s]+)", text))


def number(record, key):
    """Return the integer value of a counter, or None when absent or invalid."""
    value = record.get(key, "")
    return int(value) if re.fullmatch(r"\d+", value) else None


def token_sums(records):
    sums = {}
    for key in COUNTERS:
        values = [number(record, key) for record in records]
        known = [value for value in values if value is not None]
        if known:
            sums[key] = sum(known)
    return sums


def direction_complete(record, direction, scalar):
    samples = number(record, "usage_samples")
    total = number(record, scalar)
    detail = number(record, f"{direction}_detail_samples")
    if not samples or total is None:
        return False
    parts = [number(record, f"{direction}_{m}_tokens") for m in MODALITIES]
    return detail == samples and None not in parts and sum(parts) == total


def summarize(records):
    # A record without usage, such as the summary of a session whose first open
    # failed, has nothing to break down; it is counted, not judged.
    known = [record for record in records if number(record, "usage_samples")]
    return {
        "records": len(records),
        "records_without_known_usage": len(records) - len(known),
        "observed_token_sums": token_sums(records),
        "modality_coverage": {
            direction: coverage(known, direction, scalar)
            for direction, scalar in DIRECTIONS
        },
    }


def coverage(records, direction, scalar):
    if not records:
        return "partial_or_unknown"
    # Zero tokens and no breakdown: unused, or left out by the provider, and the
    # two look the same. Never reported as complete.
    if all(
        number(r, scalar) == 0 and not number(r, f"{direction}_detail_samples")
        for r in records
    ):
        return "none_reported"
    if all(direction_complete(r, direction, scalar) for r in records):
        return "complete"
    return "partial_or_unknown"


def turn_complete(records):
    return [
        record for record in records if number(record, "turn_complete_samples") == 1
    ]


def context_curve(events):
    completed = turn_complete(events)
    chosen = completed or events
    curve = [
        {
            "at": record.get("at"),
            "socket": number(record, "socket"),
            "cause": record.get("cause", UNKNOWN),
            "prompt_tokens": number(record, "prompt_tokens"),
        }
        for record in chosen
    ]
    points = [p for p in curve if p["prompt_tokens"] is not None]
    prompts = [p["prompt_tokens"] for p in points]
    growth = None
    if prompts:
        # Growth is measured within a socket: a cold replacement starts a new
        # context, and averaging across it would read the restart as shrinkage.
        by_socket = {}
        for point in points:
            by_socket.setdefault(point["socket"], []).append(point["prompt_tokens"])
        steps = sum(len(values) - 1 for values in by_socket.values())
        rise = sum(values[-1] - values[0] for values in by_socket.values())
        growth = {
            "basis": "turn_complete" if completed else "all_events",
            "first": prompts[0],
            "max": max(prompts),
            "last": prompts[-1],
            "completed_observations": len(prompts),
            "mean_growth_per_observation": round(rise / steps, 1) if steps else None,
        }
    return curve, growth


def by_cause(events):
    causes = {}
    for record in events:
        entry = causes.setdefault(
            record.get("cause", UNKNOWN),
            {"observations": 0, "prompt_tokens": 0, "response_tokens": 0},
        )
        entry["observations"] += 1
        for key in ("prompt_tokens", "response_tokens"):
            entry[key] += number(record, key) or 0
    return dict(sorted(causes.items()))


def session_report(room, sid, summaries, events, warnings):
    label = f"Room {room} session {sid}"
    if summaries:
        result = {"session": sid, "source": "session_summary"}
        for key in ("model", "elapsed_s", "outcome", "sockets"):
            if key in summaries[-1]:
                result[key] = summaries[-1][key]
        result.update(summarize(summaries))
        result["excluded_event_records"] = len(events)
        result["incomplete"] = False
        if len(summaries) > 1:
            warnings.append(f"{label}: {len(summaries)} summaries; they were added.")
        if events:
            event_samples = token_sums(events).get("usage_samples")
            if event_samples != result["observed_token_sums"].get("usage_samples"):
                warnings.append(
                    f"{label}: event and summary sample counts differ; "
                    "the log may be truncated."
                )
    else:
        result = {"session": sid, "source": "events_without_summary"}
        result.update(summarize(events))
        result["incomplete"] = True
        warnings.append(f"{label}: no session summary; usage is incomplete.")
    sums = result["observed_token_sums"]
    usage, completes = sums.get("usage_samples"), sums.get("turn_complete_samples")
    if usage is not None and completes is not None and usage != completes:
        warnings.append(
            f"{label}: usage_samples ({usage}) differs from turn_complete_samples "
            f"({completes}); some observations did not share a frame with "
            "turnComplete, so sums may include periodic snapshots and overstate "
            "usage."
        )
    if events:
        result["turn_complete_only_sums"] = token_sums(turn_complete(events))
        result["context_curve"], result["context_growth"] = context_curve(events)
        result["by_cause"] = by_cause(events)
    return result


class Room:
    def __init__(self):
        self.summaries = {}
        self.events = {}
        self.http = {}
        self.failures = {
            "report_transport_failed": 0,
            "interim_skipped": 0,
            "phase_judge_skipped": 0,
            "key_moments_skipped": 0,
        }
        self.refreshes = {}

    def report(self, room, warnings):
        result = {"room": room}
        # Events without a session id cannot be matched to a summarized session;
        # adding them beside that summary would count the same tokens twice.
        unattributed = []
        if self.summaries and UNKNOWN not in self.summaries:
            unattributed = self.events.pop(UNKNOWN, [])
            if unattributed:
                warnings.append(
                    f"Room {room}: {len(unattributed)} event records lack a session "
                    "id and were excluded because the room has session summaries."
                )
        sessions, selected = [], []
        for sid in dict.fromkeys(list(self.summaries) + list(self.events)):
            summaries = self.summaries.get(sid, [])
            events = self.events.get(sid, [])
            session = session_report(room, sid, summaries, events, warnings)
            refresh = self.refreshes.pop(sid, None)
            if refresh:
                session["context_refreshes"] = refresh
            sessions.append(session)
            selected.extend(summaries or events)
        if sessions:
            live = summarize(selected)
            live["sessions"] = len(sessions)
            live["excluded_event_records"] = len(unattributed) + sum(
                s.get("excluded_event_records", 0) for s in sessions
            )
            live["incomplete"] = any(s["incomplete"] for s in sessions)
            result["live"] = live
            result["live_sessions"] = sessions
        if self.http:
            result["http"] = {
                surface: summarize(records)
                for surface, records in sorted(self.http.items())
            }
        if any(self.failures.values()):
            result["http_failures"] = self.failures
        if self.refreshes:
            result["context_refreshes"] = {
                "count": sum(r["count"] for r in self.refreshes.values()),
                "bytes": sum(r["bytes"] for r in self.refreshes.values()),
            }
        return result


def analyze(lines, room_filter=None):
    rooms = {}
    warnings = []
    usage_records = 0

    def room_state(room):
        if room_filter is not None and room != room_filter:
            return None
        return rooms.setdefault(room, Room())

    for line in lines:
        # Every recognized line names one of these; most server lines do not.
        if not any(
            word in line
            for word in (
                "codetrial ",
                "gemini ",
                "interim ",
                "phase judge ",
                "key moments ",
            )
        ):
            continue
        failure = (
            TRANSPORT_FAILED.search(line)
            or INTERIM_SKIPPED.search(line)
            or PHASE_SKIPPED.search(line)
            or MOMENTS_SKIPPED.search(line)
        )
        if failure:
            state = room_state(failure.group(1))
            if state is not None:
                key = {
                    TRANSPORT_FAILED: "report_transport_failed",
                    INTERIM_SKIPPED: "interim_skipped",
                    PHASE_SKIPPED: "phase_judge_skipped",
                    MOMENTS_SKIPPED: "key_moments_skipped",
                }[failure.re]
                state.failures[key] += 1
            continue
        refresh = CONTEXT_REFRESH.search(line)
        if refresh:
            record = fields(line[refresh.start() :])
            state = room_state(record.get("room", UNKNOWN))
            if state is not None:
                sid = record.get("session", UNKNOWN)
                entry = state.refreshes.setdefault(sid, {"count": 0, "bytes": 0})
                entry["count"] += 1
                entry["bytes"] += number(record, "bytes") or 0
            continue
        for pattern in (LIVE_SUMMARY, LIVE_EVENT, HTTP_USAGE):
            marker = pattern.search(line)
            if marker:
                break
        else:
            continue
        record = fields(line[marker.start() :])
        state = room_state(record.get("room", UNKNOWN))
        if state is None:
            continue
        if not any(number(record, key) is not None for key in COUNTERS):
            warnings.append("A recognized usage record contained no valid counters.")
            continue
        usage_records += 1
        if pattern is HTTP_USAGE:
            state.http.setdefault(marker.group(1), []).append(record)
            continue
        sid = record.get("session", UNKNOWN)
        target = state.summaries if pattern is LIVE_SUMMARY else state.events
        target.setdefault(sid, []).append(record)
    report_rooms = [
        state.report(room, warnings) for room, state in sorted(rooms.items())
    ]
    return {
        "basis": "received_provider_observations_not_billed_tokens",
        "usage_records": usage_records,
        "rooms": report_rooms,
        "warnings": warnings,
    }


def main():
    parser = argparse.ArgumentParser(
        description=__doc__.splitlines()[0],
        epilog="Exits 2 when no usage record matches.",
    )
    parser.add_argument("log", nargs="?", help="Log file; omit to read stdin")
    parser.add_argument("--room", help="Include only this room")
    args = parser.parse_args()
    if args.log:
        with open(args.log, encoding="utf-8", errors="replace") as source:
            result = analyze(source, args.room)
    else:
        result = analyze(sys.stdin, args.room)
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0 if result["usage_records"] else 2


if __name__ == "__main__":
    sys.exit(main())

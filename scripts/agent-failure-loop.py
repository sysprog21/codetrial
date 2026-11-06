#!/usr/bin/env python3
"""Tell an agent when the same gate has failed twice in a row.

A PostToolUse and PostToolUseFailure hook for Bash. It reads the `failed gates:`
line that `scripts/test.sh` ends on, keeps the last set under the git directory,
and on the second consecutive identical failure prints a reminder to name the
shared premise (codetrial-improve, "Attack the premise"). A pass or a different
failure resets the count. It only reminds and always exits 0.
"""

import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
# A gate run is a command word, not a mention: reading or committing a file
# named scripts/test.sh must neither count nor reset a streak.
WRAPPERS = r"(?:(?:timeout\s+\S+|time|nice(?:\s+-n\s*\d+)?|env)\s+)*"
START = r"(?:^|[;&|({\n])\s*" + WRAPPERS + r"(?:\w+=\S*\s+)*"
GATE_RUN = re.compile(
    START + r"(?:(?:ba)?sh\s+)?(?:\S*/)?scripts/test\.sh\b"
    "|" + START + r"make\s+check\b"
)
FAILED = re.compile(r"^failed gates:(.*)$", re.MULTILINE)
THRESHOLD = 2

MESSAGE = (
    "The gate has now failed {count} runs in a row on the same gate(s): {gates}. "
    "Before another patch, write the premise your fixes share in one sentence "
    "and name which side (producer, consumer, clock, writer) holds the "
    "imbalance; remove or reassign that, then fix. See codetrial-improve, "
    "'Attack the premise after two failures'."
)


def state_path():
    done = subprocess.run(
        ["git", "rev-parse", "--git-common-dir"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    if done.returncode != 0:
        return None
    folder = (ROOT / done.stdout.strip() / "codetrial-review").resolve()
    folder.mkdir(parents=True, exist_ok=True)
    return folder / "failure-loop.json"


def text_of(value):
    if isinstance(value, str):
        return value
    if isinstance(value, dict):
        return "\n".join(text_of(v) for v in value.values())
    if isinstance(value, list):
        return "\n".join(text_of(v) for v in value)
    return ""


def update(output, path, failure=True):
    """The reminder to print, or None; state is updated either way.

    `failure` says the tool reported the command as failed. A failed command
    with no `failed gates:` line never reached the verdict (a usage error, a
    timeout) and says nothing; a command that succeeded is a pass.
    """
    found = FAILED.findall(output)
    if not found:
        if not failure:
            path.unlink(missing_ok=True)
        return None
    gates = " ".join(sorted(found[-1].split()))
    try:
        last = json.loads(path.read_text())
    except (OSError, ValueError):
        last = {}
    count = last.get("count", 0) + 1 if last.get("gates") == gates else 1
    path.write_text(json.dumps({"gates": gates, "count": count}))
    if count >= THRESHOLD:
        return MESSAGE.format(count=count, gates=gates)
    return None


def main():
    try:
        event = json.load(sys.stdin)
    except ValueError:
        return 0
    if event.get("tool_name") != "Bash" or event.get("is_interrupt"):
        return 0
    command = (event.get("tool_input") or {}).get("command", "")
    if "test.sh" not in command and "check" not in command:
        return 0  # cheap exit for nearly every Bash call
    if not GATE_RUN.search(command):
        return 0
    output = text_of(event.get("tool_response")) + "\n" + text_of(event.get("error"))
    try:
        path = state_path()
        if path is None:
            return 0
        message = update(
            output, path, event.get("hook_event_name") == "PostToolUseFailure"
        )
    except OSError:  # a read-only .git: a reminder is not worth a hook error
        return 0
    if message:
        print(
            json.dumps(
                {
                    "hookSpecificOutput": {
                        "hookEventName": event.get("hook_event_name", "PostToolUse"),
                        "additionalContext": message,
                    }
                }
            )
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

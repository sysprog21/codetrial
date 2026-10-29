#!/bin/sh
# Builds the ignored native SDK fixture once, pins it by hash, and launches one
# of its four scenarios under the run budget controller. Nothing here runs a
# scenario unless `run` is called, and `run` spends LiveKit and Gemini quota.
#
#   scripts/live-sdk-fixture.sh build DIR
#   CODETRIAL_LIVE_CONFIG=PATH scripts/live-sdk-fixture.sh run a|b|c|d DIR
#
# DIR holds the pinned binary and, per scenario, a manifest, an artifact
# directory (empty, as the controller requires) and the worker log. A scenario
# runs once per DIR: the controller refuses a non-empty artifact directory, and
# nothing here reruns.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
TEST_FILTER=livekit::recovery::tests::live::

die()
{
    printf '%s\n' "live-sdk-fixture: $*" >&2
    exit 2
}

digest()
{
    python3 -I -c 'import hashlib, sys; print(hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest())' "$1"
}

# The controller's default join-hash limit is 0.5 s. The stripped release
# fixture is a large binary and this shared host's hashing speed varies with
# load, so the default refuses live runs intermittently (before anything is
# spent). Live fixture runs, and only these, pass the controller's ceiling
# explicitly: 0.7 s is the permit timeout (1 s) minus the 0.3 s margin the rest
# of the join permit needs. The controller records the override in
# budget-result.json and fails closed at the permit if the re-hash itself runs
# over it. Nothing else raises the limit.
LIVE_MAX_JOIN_HASH_S=0.7

# Seconds one SHA-256 pass over $1 takes right now.
hash_seconds()
{
    python3 -I -c '
import hashlib, sys, time
began = time.monotonic()
hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest()
print("%.2f" % (time.monotonic() - began))
' "$1"
}

# What must not reach a log: a key or token carried in a URL query.
redact()
{
    sed -u -E 's/((access_token|key|token)=)[^&[:space:]"'"'"')]+/\1REDACTED/g'
}

build()
{
    mkdir -p "$1"
    dir=$(cd "$1" && pwd)
    cd "$ROOT"
    exe=$(cargo test --locked -p codetrial --lib --release --no-run --message-format=json \
        | python3 -I -c '
import json, sys
found = None
for line in sys.stdin:
    try:
        message = json.loads(line)
    except ValueError:
        continue
    if (
        message.get("reason") == "compiler-artifact"
        and message.get("profile", {}).get("test")
        and "lib" in message.get("target", {}).get("kind", [])
        and message.get("executable")
    ):
        found = message["executable"]
print(found or "")
')
    [ -n "$exe" ] || die "cargo did not report the lib test executable"
    cp "$exe" "$dir/live-fixture"
    chmod 755 "$dir/live-fixture"

    # The controller re-hashes every pinned binary inside the join permit's
    # limit (0.5 s by default, 0.7 s for live runs), so the pin has to be small:
    # a debug test binary (hundreds of megabytes) is refused before anything
    # runs, hence the release build. Stripping the copy changes nothing it does.
    if command -v strip > /dev/null 2>&1; then
        strip "$dir/live-fixture"
    fi
    sum=$(digest "$dir/live-fixture")
    seconds=$(hash_seconds "$dir/live-fixture")
    printf 'hashing the pin takes %ss; live runs allow %ss (controller default 0.5s)\n' \
        "$seconds" "$LIVE_MAX_JOIN_HASH_S"
    if awk -v s="$seconds" -v m="$LIVE_MAX_JOIN_HASH_S" 'BEGIN { exit !(s > m) }'; then
        echo "WARNING: that is over the live limit; the controller refuses a run over it" >&2
    fi
    printf '%s\n' "$sum" > "$dir/live-fixture.sha256"
    printf 'pinned %s sha256=%s\n' "$dir/live-fixture" "$sum"
}

run()
{
    letter=$1
    [ -d "$2" ] || die "$2 is not a directory; run build first"
    dir=$(cd "$2" && pwd)
    case $letter in a | b | c | d) ;; *) die "scenario must be a, b, c or d" ;; esac
    [ -n "${CODETRIAL_LIVE_CONFIG:-}" ] || die "set CODETRIAL_LIVE_CONFIG to the isolated live config"
    [ -f "$CODETRIAL_LIVE_CONFIG" ] || die "CODETRIAL_LIVE_CONFIG is not a file"
    bin=$dir/live-fixture
    [ -x "$bin" ] || die "no pinned binary in $dir; run build first"
    [ "$(digest "$bin")" = "$(cat "$dir/live-fixture.sha256")" ] \
        || die "the pinned binary changed since build"
    artifacts=$dir/artifacts-$letter
    [ ! -e "$artifacts" ] || die "$artifacts exists; a scenario runs once per directory"
    name=$("$bin" --list --ignored 2> /dev/null \
        | sed -n "s/: test\$//p" | grep "^${TEST_FILTER}live_sdk_control_presence_and_report_delivery\$") \
        || die "the pinned binary lists no live fixture"
    writer=$("$bin" --list 2> /dev/null \
        | sed -n "s/: test\$//p" | grep "^${TEST_FILTER}live_write_manifest\$") \
        || die "the pinned binary lists no manifest writer"

    manifest=$dir/manifest-$letter.json
    CODETRIAL_LIVE_MANIFEST_OUT=$manifest CODETRIAL_LIVE_SCENARIO=$letter \
        CODETRIAL_LIVE_CANDIDATE=$bin "$bin" --exact "$writer" > /dev/null
    python3 -I "$ROOT/scripts/interview-run-budget.py" validate \
        --manifest "$manifest" --verify-binaries

    printf 'pin hashes in %ss now; the join-hash limit for this run is %ss\n' \
        "$(hash_seconds "$bin")" "$LIVE_MAX_JOIN_HASH_S" >&2

    log=$dir/worker-$letter.log
    status_file=$dir/launcher-$letter.status
    printf 'launching scenario %s; the log is %s\n' "$letter" "$log" >&2
    {
        if CODETRIAL_LIVE_SCENARIO=$letter python3 -I "$ROOT/scripts/interview-run-budget.py" run \
            --manifest "$manifest" --artifact-dir "$artifacts" \
            --max-join-hash-s "$LIVE_MAX_JOIN_HASH_S" -- \
            "$bin" --ignored --exact "$name" --nocapture 2>&1; then
            printf '0\n' > "$status_file"
        else
            printf '%s\n' "$?" > "$status_file"
        fi
    } | redact | tee "$log"
    status=$(cat "$status_file")
    rm -f "$status_file"
    printf 'launcher exit %s; observations: grep LIVE_OBS %s\n' "$status" "$log" >&2
    exit "$status"
}

case ${1:-} in
    build)
        [ $# -eq 2 ] || die "usage: build DIR"
        build "$2"
        ;;
    run)
        [ $# -eq 3 ] || die "usage: run a|b|c|d DIR"
        run "$2" "$3"
        ;;
    *) die "usage: build DIR | run a|b|c|d DIR" ;;
esac

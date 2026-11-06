#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
. "$ROOT/scripts/session-cookie.sh"
. "$ROOT/scripts/wait-for-server.sh"
BROWSER_CHECK_AGENT=${BROWSER_CHECK_AGENT:-home}

# Flows: voice (default), report, barge, avatar, soak. `avatar` is dispatched
# before the agent mode, so it needs no credentials and runs the same under any
# valid BROWSER_CHECK_AGENT. `soak` is the long-conversation acceptance: it
# holds one interview past the Live API's connection cap and judges the handover
# onto a new socket, so it is the one flow whose duration is worth naming.
#
# When BROWSER_CHECK_BARGE_AUDIO_FILE is the fake microphone for a flow other
# than barge, the clip is looped, and Gemini ends an utterance only after
# GEMINI_SILENCE_MS (1500 ms by default) of quiet. A clip whose trailing and
# leading silence together fall short of that never ends: the candidate seems to
# talk for the whole run, so the interviewer never takes the floor and the soak
# reads as a silent handover. End the clip with at least two seconds of silence.
#
# A soak exits 77 when Gemini sent no GoAway during the window: it learned
# nothing about the handover, which is neither a pass nor a failure of it. Any
# other nonzero status is a failure, a structural one included.
#
# BROWSER_CHECK_SOAK_VERDICT_MODE is strict (the default) or pilot. Strict holds
# the soak to the one-watchdog-rebuild cap and is the qualification. Pilot
# leaves only that cap out of its verdict, reports the strict outcome beside the
# structural one, and prints qualification=not_evaluated, so a pilot that
# completed is never read as having qualified.
#
# `long-interview` holds one real interview for the duration a budgeted run
# manifest names (scripts/long-interview-plan.cjs). BROWSER_CHECK_DURATION_MIN
# is its harness-only length: 10, 20, 30, 60 or 90, 20 when unset, inside the
# product's own range and recording cap. Every other flow keeps the page's 20
# minutes, so selecting a duration for one of them is refused rather than read
# as a long-interview result.
#
# The long interview plays the schedule of scripts/long-interview-schedule.cjs
# into the session, each action through the run budget
# (scripts/long-interview-driver.cjs). BROWSER_CHECK_LONG_CLIP_DIR names a
# directory holding manifest.json and the recorded speech clips it lists,
# authored outside the checkout. BROWSER_CHECK_LONG_TONE_FIXTURE=1 instead plays
# synthetic tones, for a smoke run only: it contains no speech and is reported
# as a validation fixture, never as a speech run.
# BROWSER_CHECK_LONG_ARTIFACT_DIR holds the failure-<class>.json artifacts; it
# defaults to a directory beside BROWSER_CHECK_CAPTURE. The exit code is the
# failure class's: 4 budget, 5 cleanup, 6 artifact, 77 no GoAway, 1 other.
BROWSER_CHECK_FLOW=${BROWSER_CHECK_FLOW:-voice}
case $BROWSER_CHECK_FLOW in
    voice | report | barge | avatar | soak | long-interview) ;;
    *)
        echo "BROWSER_CHECK_FLOW must be voice, report, barge, avatar, soak, or long-interview." >&2
        exit 2
        ;;
esac

# Long enough to outlive one connection by default, because a soak that stops
# short of the cap proves only that the room stayed up. Overridable for a
# deliberately longer run; a shorter one is answered by the .cjs, which is where
# the cap that makes it meaningless is written down.
if [ "$BROWSER_CHECK_FLOW" = soak ]; then
    SOAK_SECONDS=${BROWSER_CHECK_SOAK_SECONDS:-660}
elif [ "${BROWSER_CHECK_SOAK_SECONDS+x}" ]; then

    # Ignoring it is how a mistyped invocation spends eleven minutes proving
    # nothing and then exits green.
    echo "BROWSER_CHECK_SOAK_SECONDS applies to BROWSER_CHECK_FLOW=soak," >&2
    echo "not to $BROWSER_CHECK_FLOW." >&2
    exit 2
else
    SOAK_SECONDS=0
fi

if [ "$BROWSER_CHECK_FLOW" != long-interview ] && [ "${BROWSER_CHECK_DURATION_MIN+x}" ]; then
    echo "BROWSER_CHECK_DURATION_MIN applies to BROWSER_CHECK_FLOW=long-interview," >&2
    echo "not to $BROWSER_CHECK_FLOW." >&2
    exit 2
fi

if [ "${BROWSER_CHECK_REQUIRE_MODEL:-}" = "1" ]; then
    if [ "$BROWSER_CHECK_FLOW" != avatar ]; then
        echo "BROWSER_CHECK_REQUIRE_MODEL applies to BROWSER_CHECK_FLOW=avatar," >&2
        echo "not to $BROWSER_CHECK_FLOW." >&2
        exit 2
    fi
elif [ -n "${BROWSER_CHECK_REQUIRE_MODEL:-}" ]; then
    echo "BROWSER_CHECK_REQUIRE_MODEL must be 1 when set." >&2
    exit 2
fi
if [ "$BROWSER_CHECK_FLOW" = soak ]; then
    SOAK_VERDICT_MODE=${BROWSER_CHECK_SOAK_VERDICT_MODE:-strict}
    case $SOAK_VERDICT_MODE in
        strict | pilot) ;;
        *)
            echo "BROWSER_CHECK_SOAK_VERDICT_MODE must be strict or pilot." >&2
            exit 2
            ;;
    esac
elif [ -n "${BROWSER_CHECK_SOAK_VERDICT_MODE:-}" ]; then
    echo "BROWSER_CHECK_SOAK_VERDICT_MODE applies to BROWSER_CHECK_FLOW=soak," >&2
    echo "not to $BROWSER_CHECK_FLOW." >&2
    exit 2
else
    SOAK_VERDICT_MODE=
fi
case $SOAK_SECONDS in
    *[!0-9]*)
        echo "BROWSER_CHECK_SOAK_SECONDS must be a whole number of seconds." >&2
        exit 2
        ;;
esac
if [ "$BROWSER_CHECK_FLOW" = soak ]; then
    case $BROWSER_CHECK_AGENT in
        rust | dispatch) ;;
        *)

            # The other agents drive a stub, so there is no Gemini socket for a
            # soak to outlive and nothing it could judge.
            echo "BROWSER_CHECK_FLOW=soak needs BROWSER_CHECK_AGENT=rust or" >&2
            echo "dispatch, not $BROWSER_CHECK_AGENT." >&2
            exit 2
            ;;
    esac
fi
if [ "${BROWSER_CHECK_COMPILER_EXPLORER_BASE_URL+x}" ]; then
    COMPILER_EXPLORER_BASE_URL_ARG=$BROWSER_CHECK_COMPILER_EXPLORER_BASE_URL
else
    COMPILER_EXPLORER_BASE_URL_ARG=__default__
fi
TMP=$(mktemp -d)
SERVER_LOG="$TMP/web.log"
BARGE_AUDIO_FILE=${BROWSER_CHECK_BARGE_AUDIO_FILE:-}
FIXED_ROOM=0
DISPATCH_MODE=0
LOGIN_DB_PATH="$TMP/accounts.db"
LOGIN_SESSION_SECRET=browser-check-session
CONFIG_PATH="$TMP/codetrial.env.local"

# The in-process serving fixture (scripts/fixture-server.sh): a pinned test
# executable run as the server in place of `codetrial web`, so the interview
# leaves behind what the arming observer saw. Off unless a test name is given.
FIXTURE_TEST=${BROWSER_CHECK_FIXTURE_TEST:-}
FIXTURE_DIR="$TMP/fixture"
FIXTURE_STOP_WAIT_S=${BROWSER_CHECK_FIXTURE_STOP_WAIT_S:-150}

# The fixture's connection drain, passed to it, and the slack left after its
# settle: the settle is whatever the stop wait has left after both.
FIXTURE_DRAIN_S=10
FIXTURE_SLACK_S=20

# Asked to stop and waited for, not killed: the serving fixture has connections
# to drain, rooms and reports to finish and a report of its own to write, and
# that report is kept before the scratch directory goes.
cleanup_fixture()
{
    fixture_status=0
    "$ROOT/scripts/fixture-server.sh" stop "$FIXTURE_DIR" "$SERVER_PID" "$FIXTURE_STOP_WAIT_S" \
        || fixture_status=1
    wait "$SERVER_PID" 2> /dev/null || {
        echo "The serving fixture exited with a failure; its report may say why." >&2
        fixture_status=1
    }

    "$ROOT/scripts/fixture-server.sh" keep "$FIXTURE_DIR" "$BROWSER_CHECK_FIXTURE_OUT" "$SERVER_LOG" \
        || fixture_status=1
    return "$fixture_status"
}

# The reply content is judged once the serving fixture has stopped and its
# report is kept: the worker finished before that and could not read it. The
# verdict is written into the capture file and never into this script's own
# status, so a run that could not be judged is still the run it was.
judge_reply_content()
{
    [ -n "${BROWSER_CHECK_CAPTURE:-}" ] && [ -f "$BROWSER_CHECK_CAPTURE" ] || return 0
    node "$ROOT/scripts/long-interview-evidence.cjs" \
        --capture "$BROWSER_CHECK_CAPTURE" --fixture "$BROWSER_CHECK_FIXTURE_OUT" \
        || echo "Reply content was not judged; the run's own result stands." >&2
}

cleanup()
{
    cleanup_status=$?
    trap - EXIT INT TERM
    if [ "${SERVER_PID:-}" ] && [ -n "$FIXTURE_TEST" ]; then
        cleanup_fixture || cleanup_status=1
        judge_reply_content
    elif [ "${SERVER_PID:-}" ]; then
        kill "$SERVER_PID" 2> /dev/null || true
        wait "$SERVER_PID" 2> /dev/null || true
    fi
    if [ "${FLOOR_CAPTURE_PID:-}" ]; then
        : > "$TMP/floor.done"
        wait "$FLOOR_CAPTURE_PID" || cleanup_status=1
    fi
    if [ "${BROWSER_CHECK_FLOOR_LOG:-}" ]; then
        echo "Floor diagnostics retained at $BROWSER_CHECK_FLOOR_LOG" >&2
    fi
    if [ "${BROWSER_CHECK_LATENCY_LOG:-}" ]; then
        echo "Latency diagnostics retained at $BROWSER_CHECK_LATENCY_LOG" >&2
    fi
    rm -r "$TMP" 2> /dev/null || true
    exit "$cleanup_status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

if [ "$BROWSER_CHECK_AGENT" = "rust" ] || [ "$BROWSER_CHECK_AGENT" = "dispatch" ]; then
    CONFIG_ENV=${CODETRIAL_CONFIG_ENV:-}
    if [ -z "$CONFIG_ENV" ]; then
        for candidate in "$ROOT/config/codetrial.env.local" "$ROOT/codetrial.env.local"; do
            [ -f "$candidate" ] && {
                CONFIG_ENV=$candidate
                break
            }
        done
    fi
    if [ -f "${CONFIG_ENV:-}" ]; then
        set -a
        . "$CONFIG_ENV"
        set +a
    fi
    : "${LIVEKIT_URL:?set LIVEKIT_URL for credentialed browser check}"
    : "${LIVEKIT_API_KEY:?set LIVEKIT_API_KEY for credentialed browser check}"
    : "${LIVEKIT_API_SECRET:?set LIVEKIT_API_SECRET for credentialed browser check}"
    : "${GOOGLE_API_KEY:?set GOOGLE_API_KEY for credentialed browser check}"
fi

case $BROWSER_CHECK_AGENT in
    home | offline)
        printf '%s\n' \
            'LIVEKIT_URL=wss://example.livekit.cloud' \
            'LIVEKIT_API_KEY=browser-check-key' \
            'LIVEKIT_API_SECRET=browser-check-secret' > "$CONFIG_PATH"
        ;;
    rust | dispatch)

        # The operator's own file, named rather than copied. `provider_dir` is
        # the config file's directory, so a copy in $TMP has no
        # `codetrial.env.<id>` siblings beside it and the pool collapses to the
        # primary project. The credentialed check is the only one that can
        # exercise pooling at all, and a copy is exactly what stops it: an
        # external server minting a pooled room would hand it to an agent that
        # has never heard of that provider.
        #
        # A config assembled from the environment is the fallback, for the
        # runner that has the four variables and no file to point at.
        if [ -f "${CONFIG_ENV:-}" ]; then
            CONFIG_PATH=$CONFIG_ENV
        else
            printf 'LIVEKIT_URL=%s\nLIVEKIT_API_KEY=%s\nLIVEKIT_API_SECRET=%s\nGOOGLE_API_KEY=%s\n' \
                "$LIVEKIT_URL" "$LIVEKIT_API_KEY" "$LIVEKIT_API_SECRET" "$GOOGLE_API_KEY" > "$CONFIG_PATH"
        fi
        ;;
    *)
        echo "BROWSER_CHECK_AGENT must be home, offline, rust, or dispatch." >&2
        exit 2
        ;;
esac

# A run launched by scripts/interview-run-budget.py carries its budget in these
# variables. They are all or nothing: a worker holding some of them could spend
# without a permit, so a partial set is refused rather than guessed at. Checked
# here, ahead of the validate-only exit, so the same refusals can be exercised
# without a browser or credentials.
PREBUILT_BIN=
BUDGET_VARS=
for name in BROWSER_CHECK_RUN_MANIFEST BROWSER_CHECK_RUN_ID \
    BROWSER_CHECK_RUN_MANIFEST_SHA256 BROWSER_CHECK_BUDGET_SOCKET; do
    eval "value=\${$name:-}"
    [ -z "$value" ] || BUDGET_VARS="$BUDGET_VARS $name"
done
if [ -n "$BUDGET_VARS" ]; then
    if [ -z "${BROWSER_CHECK_RUN_MANIFEST:-}" ] || [ -z "${BROWSER_CHECK_RUN_ID:-}" ] \
        || [ -z "${BROWSER_CHECK_RUN_MANIFEST_SHA256:-}" ] \
        || {
            [ -z "${BROWSER_CHECK_VALIDATE_ENV_ONLY:-}" ] && [ ! -S "${BROWSER_CHECK_BUDGET_SOCKET:-}" ]
        }; then
        echo "A budgeted run needs BROWSER_CHECK_RUN_MANIFEST, BROWSER_CHECK_RUN_ID," >&2
        echo "BROWSER_CHECK_RUN_MANIFEST_SHA256 and a live BROWSER_CHECK_BUDGET_SOCKET;" >&2
        echo "start it through scripts/interview-run-budget.py run. Set:$BUDGET_VARS" >&2
        exit 2
    fi
    if [ "${CODETRIAL_WEB_URL:-}" ]; then

        # An external server is shared and was not built from the pinned
        # executable, so neither its exposure nor its identity can be bounded.
        echo "A budgeted run launches the pinned candidate itself; unset CODETRIAL_WEB_URL." >&2
        exit 2
    fi
    BUDGET_PYTHON=
    for candidate in ${PYTHON:-} python3 python3.14 python3.13 python3.12 \
        python3.11 python3.10 python3.9; do
        if command -v "$candidate" > /dev/null 2>&1 \
            && "$candidate" -c 'import sys; sys.exit(sys.version_info < (3, 9))' > /dev/null 2>&1; then
            BUDGET_PYTHON=$candidate
            break
        fi
    done
    if [ -z "$BUDGET_PYTHON" ]; then
        echo "A budgeted run needs Python 3.9 or newer; set PYTHON to one." >&2
        exit 2
    fi
    BUDGET_TOOL="$ROOT/scripts/interview-run-budget.py"

    # The manifest, the pinned executables and the identity the launcher hashed
    # must all agree before anything is built or started.
    manifest_sha=$("$BUDGET_PYTHON" "$BUDGET_TOOL" validate \
        --manifest "$BROWSER_CHECK_RUN_MANIFEST" --verify-binaries --print sha) || exit 2
    manifest_run_id=$("$BUDGET_PYTHON" "$BUDGET_TOOL" validate \
        --manifest "$BROWSER_CHECK_RUN_MANIFEST" --print run_id) || exit 2
    if [ "$manifest_sha" != "$BROWSER_CHECK_RUN_MANIFEST_SHA256" ]; then
        echo "BROWSER_CHECK_RUN_MANIFEST_SHA256 does not match the manifest." >&2
        exit 2
    fi
    if [ "$manifest_run_id" != "$BROWSER_CHECK_RUN_ID" ]; then
        echo "BROWSER_CHECK_RUN_ID does not match the manifest." >&2
        exit 2
    fi
    PREBUILT_BIN=$("$BUDGET_PYTHON" "$BUDGET_TOOL" binary \
        --manifest "$BROWSER_CHECK_RUN_MANIFEST" candidate) || exit 2
fi

if [ -n "$FIXTURE_TEST" ]; then
    if [ -z "$PREBUILT_BIN" ] || [ "$BROWSER_CHECK_AGENT" != dispatch ]; then
        echo "BROWSER_CHECK_FIXTURE_TEST needs a budgeted run (its pinned executable is the" >&2
        echo "fixture) and BROWSER_CHECK_AGENT=dispatch." >&2
        exit 2
    fi
    if [ -z "${BROWSER_CHECK_FIXTURE_OUT:-}" ] || [ -e "$BROWSER_CHECK_FIXTURE_OUT" ] \
        || [ -e "$BROWSER_CHECK_FIXTURE_OUT.server.log" ] \
        || [ -L "$BROWSER_CHECK_FIXTURE_OUT.server.log" ] \
        || [ ! -d "$(dirname -- "$BROWSER_CHECK_FIXTURE_OUT")" ] \
        || [ ! -w "$(dirname -- "$BROWSER_CHECK_FIXTURE_OUT")" ]; then
        echo "BROWSER_CHECK_FIXTURE_OUT must name a report file that does not exist yet," >&2
        echo "in a directory that does and can be written." >&2
        exit 2
    fi

    # The verdict is written into the capture after the fixture stops, so a
    # capture left by an earlier run would be judged against this run's report.
    if [ -n "${BROWSER_CHECK_CAPTURE:-}" ] && [ -e "$BROWSER_CHECK_CAPTURE" ]; then
        echo "BROWSER_CHECK_CAPTURE must not exist yet when the serving fixture runs." >&2
        exit 2
    fi

    # Neither exists yet, so their directories are compared: the report kept at
    # cleanup would otherwise replace the capture the run just wrote.
    if [ -n "${BROWSER_CHECK_CAPTURE:-}" ] && [ -d "$(dirname -- "$BROWSER_CHECK_CAPTURE")" ]; then
        capture_at="$(cd -P -- "$(dirname -- "$BROWSER_CHECK_CAPTURE")" && pwd)/$(basename -- "$BROWSER_CHECK_CAPTURE")"
        out_at="$(cd -P -- "$(dirname -- "$BROWSER_CHECK_FIXTURE_OUT")" && pwd)/$(basename -- "$BROWSER_CHECK_FIXTURE_OUT")"

        # Each also stages files beside it (the log beside the report, the
        # judged capture at .partial), so neither may be a prefix of the other.
        case $out_at in "$capture_at"*) collides=1 ;; *) collides= ;; esac
        case $capture_at in "$out_at"*) collides=1 ;; esac
        if [ -n "$collides" ]; then
            echo "BROWSER_CHECK_CAPTURE and BROWSER_CHECK_FIXTURE_OUT must name different files." >&2
            exit 2
        fi
    fi

    # A leading zero too: shell arithmetic reads it as octal while the tests
    # below read it as decimal, so the reserve check would ask for too little.
    case $FIXTURE_STOP_WAIT_S in
        '' | *[!0-9]* | 0[0-9]*)
            echo "BROWSER_CHECK_FIXTURE_STOP_WAIT_S must be a number of seconds." >&2
            exit 2
            ;;
    esac

    # One number drives both sides: the fixture settles for this long less its
    # connection drain and the slack, so a longer settle cannot make the wait
    # below give up on a fixture that is still working. At least 10 s of it.
    fixture_min_wait_s=$((FIXTURE_DRAIN_S + FIXTURE_SLACK_S + 10))
    if [ "$FIXTURE_STOP_WAIT_S" -lt "$fixture_min_wait_s" ]; then
        echo "BROWSER_CHECK_FIXTURE_STOP_WAIT_S must be at least $fixture_min_wait_s seconds." >&2
        exit 2
    fi

    # The stop waits for a drain and a settle, so the cleanup window has to
    # outlast it with the launcher's own cleanup still to come.
    reserve=$("$BUDGET_PYTHON" "$BUDGET_TOOL" validate \
        --manifest "$BROWSER_CHECK_RUN_MANIFEST" --print cleanup_reserve_s) || exit 2
    if [ "$reserve" -lt $((FIXTURE_STOP_WAIT_S + 60)) ]; then
        echo "cleanup_reserve_s is $reserve; the serving fixture's stop wait of" >&2
        echo "${FIXTURE_STOP_WAIT_S}s needs at least $((FIXTURE_STOP_WAIT_S + 60))s of it." >&2
        exit 2
    fi

    # When the budget runner starts its reap, for the fixture's own deadline.
    case ${BROWSER_CHECK_BUDGET_REAP_AT:-} in
        '' | *[!0-9]*)
            echo "BROWSER_CHECK_FIXTURE_TEST needs BROWSER_CHECK_BUDGET_REAP_AT from the budget runner." >&2
            exit 2
            ;;
    esac
fi

# The duration, run kind, end mode and observation target of a long interview
# are judged before anything is built or started. The same module answers when
# browser-check.cjs is run directly.
if [ "$BROWSER_CHECK_FLOW" = long-interview ]; then
    BROWSER_CHECK_AGENT="$BROWSER_CHECK_AGENT" \
        BROWSER_CHECK_FLOW="$BROWSER_CHECK_FLOW" \
        BROWSER_CHECK_CONFIG_PATH="$CONFIG_PATH" \
        node "$ROOT/scripts/long-interview-plan.cjs" || exit 2
fi

if [ "${BROWSER_CHECK_VALIDATE_ENV_ONLY:-}" ]; then
    exit 0
fi

# Runs the pin, so it waits until validation-only runs are out of the way.
if [ -n "$FIXTURE_TEST" ]; then
    "$ROOT/scripts/fixture-server.sh" check "$PREBUILT_BIN" "$FIXTURE_TEST" || exit 2
fi

# Anything that serves `web/` needs the fetched vendor binaries, and this script
# launches its own server rather than going through the Makefile. Without this a
# fresh clone serves a MediaPipe loader whose .wasm and model 404, and face
# detection reports itself unavailable instead of failing: the exact shape of
# the bug that started this, where the binaries were missing and everything
# still looked like it worked.
"$ROOT/scripts/fetch-vendor.sh"

# Resolved after the env-only exit above, which never needs Playwright. Comes
# from the repo's own devDependencies so the usual path is `npm install` with no
# environment variable. PLAYWRIGHT_PATH stays an override for a Playwright
# installed elsewhere.
if [ -z "${PLAYWRIGHT_PATH:-}" ]; then
    PLAYWRIGHT_PATH=$(node -e "console.log(require.resolve('playwright', { paths: ['$ROOT'] }))" 2> /dev/null || true)
fi
if [ -z "$PLAYWRIGHT_PATH" ]; then
    echo "Playwright is not installed. From the repo root:" >&2
    echo "  npm install && npx playwright install chromium" >&2
    echo "Or set PLAYWRIGHT_PATH to an existing Playwright module." >&2

    # Its own code, so a caller can tell "no browser here" from "the check
    # failed". tests/web.rs skips on this one and fails on everything else;
    # without the distinction that test had to stay #[ignore] forever, which is
    # how it went unrun long enough for one of its branches to rot.
    exit 3
fi

# Ahead of the build, so a barge run on a machine that cannot make its audio is
# refused before it spends a cold compile.
if [ "$BROWSER_CHECK_FLOW" = "barge" ] && [ -z "$BARGE_AUDIO_FILE" ]; then
    if ! command -v say > /dev/null 2>&1 || ! command -v ffmpeg > /dev/null 2>&1; then
        echo "BROWSER_CHECK_FLOW=barge requires BROWSER_CHECK_BARGE_AUDIO_FILE, or macOS say plus ffmpeg." >&2
        exit 2
    fi
    say -o "$TMP/barge.aiff" "I need to interrupt now. I am still here. Please stop and listen."
    ffmpeg -hide_banner -loglevel error \
        -i "$TMP/barge.aiff" \
        -ar 48000 -ac 1 "$TMP/barge.wav"
    BARGE_AUDIO_FILE="$TMP/barge.wav"
fi

# A dispatched agent logs to the server that spawned it. An external server
# keeps that log, so the barge check could only time out without its records.
if [ "$BROWSER_CHECK_FLOW" = barge ] && [ "$BROWSER_CHECK_AGENT" = dispatch ] && [ "${CODETRIAL_WEB_URL:-}" ]; then
    echo "BROWSER_CHECK_FLOW=barge with dispatch needs this script's own server; unset CODETRIAL_WEB_URL." >&2
    exit 2
fi

if [ "$BROWSER_CHECK_AGENT" = rust ] || [ "$BROWSER_CHECK_AGENT" = dispatch ]; then
    # Outside TMP: failure and signal cleanup must preserve early observations.
    BROWSER_CHECK_FLOOR_LOG=$(mktemp "${TMPDIR:-/tmp}/codetrial-floor.XXXXXX")
    BROWSER_CHECK_LATENCY_LOG=$(mktemp "${TMPDIR:-/tmp}/codetrial-latency.XXXXXX")
    export BROWSER_CHECK_FLOOR_LOG BROWSER_CHECK_LATENCY_LOG
    # Dispatched agents log to this server; a Rust-mode agent has its own pipes.
    if [ "$BROWSER_CHECK_AGENT" = dispatch ] && [ -z "${CODETRIAL_WEB_URL:-}" ]; then
        node "$ROOT/scripts/floor-capture.cjs" "$SERVER_LOG" "$BROWSER_CHECK_FLOOR_LOG" "$TMP/floor.done" "$BROWSER_CHECK_LATENCY_LOG" &
        FLOOR_CAPTURE_PID=$!
    elif [ "$BROWSER_CHECK_AGENT" = dispatch ]; then
        printf '%s\n' 'codetrial capture_incomplete family=floor reason=external-server-log-unavailable' >> "$BROWSER_CHECK_FLOOR_LOG"
        printf '%s\n' 'codetrial capture_incomplete family=latency reason=external-server-log-unavailable' >> "$BROWSER_CHECK_LATENCY_LOG"
    fi
fi

if [ "${CODETRIAL_WEB_URL:-}" ]; then
    BASE_URL=${CODETRIAL_WEB_URL%/}
else

    # A cold build can take minutes. Keep it outside the server's readiness
    # deadline, and ahead of choosing a port that could be taken meanwhile, so
    # compiler errors reach the caller before anything is launched.
    #
    # A budgeted run never builds here: the candidate was built and hashed
    # before the paid window opened, and a rebuild inside it would spend the
    # window on a binary nobody pinned. The server command is held in the
    # positional parameters, which nothing else in this script uses.
    if [ -n "$PREBUILT_BIN" ]; then
        set -- "$PREBUILT_BIN"
    else
        cargo build --manifest-path "$ROOT/Cargo.toml" --bin codetrial
        set -- cargo run --quiet --manifest-path "$ROOT/Cargo.toml" --
    fi

    PORT=${PORT:-}
    if [ -z "$PORT" ]; then
        PORT=$(node -e "const s=require('net').createServer();s.listen(0,'127.0.0.1',()=>{console.log(s.address().port);s.close();});")
    fi
    BASE_URL="http://127.0.0.1:$PORT"

    if curl -fsS "$BASE_URL" > /dev/null 2>&1; then
        echo "Port $PORT is already serving HTTP; set PORT to a free port." >&2
        exit 1
    fi

    case $BROWSER_CHECK_AGENT in
        home | offline)
            env -u LIVEKIT_URL -u LIVEKIT_API_KEY -u LIVEKIT_API_SECRET -u GOOGLE_API_KEY \
                SESSION_SECRET="$LOGIN_SESSION_SECRET" \
                CODETRIAL_DB_PATH="$LOGIN_DB_PATH" \
                "$@" web --config "$CONFIG_PATH" --web-addr "127.0.0.1:$PORT" --web-dir "$ROOT/web" \
                > "$SERVER_LOG" 2>&1 &
            ;;
        dispatch)

            # The production shape: no fixed room, so the server invents one per
            # request and has to put an interviewer in it by itself. This is the
            # mode that a missing dispatch breaks, and nothing else covers it.
            unset INTERVIEW_ROOM_NAME
            DISPATCH_MODE=1
            if [ -n "$FIXTURE_TEST" ]; then

                # The pinned test executable, running only the fixture. Its
                # account database is inside its own directory by construction.
                mkdir "$FIXTURE_DIR"

                # In a session of its own where it can be, so a signal sent to
                # this script's process group reaches this script, whose trap
                # asks the fixture to stop and waits, and not the fixture first:
                # an INT would end it without a report, and a TERM in its first
                # moments, before it can take one as a stop request, likewise.
                # This script has no job control, so setsid runs the fixture in
                # place and $! is the fixture; were it to fork, wait_for_server
                # would see the launcher exit and fail the run at once.
                set -- "$PREBUILT_BIN" --ignored --exact "$FIXTURE_TEST" --nocapture --test-threads=1
                if command -v setsid > /dev/null 2>&1; then
                    set -- setsid "$@"
                fi

                # A backstop only: cleanup asks it to stop first. As late as it
                # can be and still leave the stop wait and some margin before
                # the runner's reap, counted from now, since the fixture counts
                # its deadline from its own start.
                FIXTURE_DEADLINE_S=$((BROWSER_CHECK_BUDGET_REAP_AT - $(date +%s) - \
                FIXTURE_STOP_WAIT_S - FIXTURE_SLACK_S))
                if [ "$FIXTURE_DEADLINE_S" -le 0 ]; then
                    echo "No wall time is left for the serving fixture to run and stop." >&2
                    exit 2
                fi
                SESSION_SECRET="$LOGIN_SESSION_SECRET" \
                    CODETRIAL_FIXTURE_CONFIG="$CONFIG_PATH" \
                    CODETRIAL_FIXTURE_DIR="$FIXTURE_DIR" \
                    CODETRIAL_FIXTURE_ADDR="127.0.0.1:$PORT" \
                    CODETRIAL_FIXTURE_DEADLINE_S="$FIXTURE_DEADLINE_S" \
                    CODETRIAL_FIXTURE_DRAIN_S="$FIXTURE_DRAIN_S" \
                    CODETRIAL_FIXTURE_SETTLE_S="$((FIXTURE_STOP_WAIT_S - FIXTURE_DRAIN_S - FIXTURE_SLACK_S))" \
                    "$@" > "$SERVER_LOG" 2>&1 &
            else
                SESSION_SECRET="$LOGIN_SESSION_SECRET" \
                    CODETRIAL_DB_PATH="$LOGIN_DB_PATH" \
                    "$@" web --config "$CONFIG_PATH" --web-addr "127.0.0.1:$PORT" --web-dir "$ROOT/web" \
                    > "$SERVER_LOG" 2>&1 &
            fi
            ;;
        rust)
            INTERVIEW_ROOM_NAME=${INTERVIEW_ROOM_NAME:-interview-browser-$(date +%s)-$$}
            export INTERVIEW_ROOM_NAME
            FIXED_ROOM=1
            SESSION_SECRET="$LOGIN_SESSION_SECRET" \
                CODETRIAL_DB_PATH="$LOGIN_DB_PATH" \
                "$@" web --config "$CONFIG_PATH" --web-addr "127.0.0.1:$PORT" --web-dir "$ROOT/web" \
                > "$SERVER_LOG" 2>&1 &
            ;;
    esac
    SERVER_PID=$!
fi

wait_for_server "$BASE_URL" 60 "$SERVER_LOG" "${SERVER_PID:-}" || exit 1

SESSION_COOKIE=${BROWSER_CHECK_SESSION_COOKIE:-}
if [ -z "$SESSION_COOKIE" ]; then
    SESSION_COOKIE=$(login_session_cookie "$BASE_URL" browser-check) || exit 2
fi

# `ROOT="$ROOT"` reads to shellcheck as a self-assignment whose value the
# argument below will not see. Right about the mechanism, wrong about the
# intent: the argument wants this shell's ROOT, and the prefix exists to hand
# node the same value. Nothing here wants the forked one.
# shellcheck disable=SC2097,SC2098
BASE_URL="$BASE_URL" \
    BROWSER_CHECK_AGENT="$BROWSER_CHECK_AGENT" \
    BROWSER_CHECK_FLOW="$BROWSER_CHECK_FLOW" \
    BROWSER_CHECK_SOAK_SECONDS="$SOAK_SECONDS" \
    BROWSER_CHECK_SOAK_VERDICT_MODE="$SOAK_VERDICT_MODE" \
    BROWSER_CHECK_REQUIRE_MODEL="${BROWSER_CHECK_REQUIRE_MODEL:-}" \
    BROWSER_CHECK_CAPTURE="${BROWSER_CHECK_CAPTURE:-}" \
    BROWSER_CHECK_BARGE_AUDIO_FILE="$BARGE_AUDIO_FILE" \
    BROWSER_CHECK_COMPILER_EXPLORER_BASE_URL="$COMPILER_EXPLORER_BASE_URL_ARG" \
    BROWSER_CHECK_SESSION_COOKIE="$SESSION_COOKIE" \
    BROWSER_CHECK_PREBUILT_BIN="$PREBUILT_BIN" \
    BROWSER_CHECK_FIXED_ROOM="$FIXED_ROOM" \
    BROWSER_CHECK_CONFIG_PATH="$CONFIG_PATH" \
    INTERVIEW_ROOM_NAME="${INTERVIEW_ROOM_NAME:-}" \
    PLAYWRIGHT_PATH="$PLAYWRIGHT_PATH" \
    ROOT="$ROOT" \
    SERVER_LOG="$SERVER_LOG" \
    node "$ROOT/scripts/browser-check.cjs"

# The flow above only proves the candidate reached an interviewer. In dispatch
# mode it must have been this server that sent one, so check its own log rather
# than infer it from the browser.
if [ "$DISPATCH_MODE" = "1" ]; then
    if ! grep -q "codetrial dispatch room=" "$SERVER_LOG"; then
        echo "server never dispatched an interviewer for the room it minted:" >&2
        cat "$SERVER_LOG" >&2
        exit 1
    fi
    if ! grep -q "starting interview: room=" "$SERVER_LOG"; then
        echo "the dispatched interviewer never started the interview:" >&2
        cat "$SERVER_LOG" >&2
        exit 1
    fi
fi

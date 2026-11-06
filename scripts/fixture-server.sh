#!/bin/sh

# The two steps scripts/browser-check.sh needs around the in-process serving
# fixture (tests/unit/livekit/probe/serving.rs), kept here so they can be run
# against a stand-in executable without a browser.
#
#   fixture-server.sh check PIN TEST
#       PIN is the pinned test executable. Fails unless it lists TEST as
#       exactly one ignored test, so the run cannot start some other test, or
#       none, under a name that only looks right. It runs the pin with --list,
#       which builds nothing.
#
#   fixture-server.sh stop DIR PID WAIT_SECONDS
#       Asks the fixture running as PID to stop by creating DIR/stop, then waits
#       up to WAIT_SECONDS for it to finish its drain and settle and write its
#       report. A fixture that has not exited by then is terminated (killed after
#       FIXTURE_SERVER_TERM_GRACE_S, default 5), and that is a failure: its
#       report cannot be trusted to be complete. The fixture also treats a TERM
#       that reaches it as the same request, but only until it has begun to stop:
#       after the stop file it ignores TERM, so the escalation is the kill.
#       When DIR/stop cannot be written, the request is a TERM with the same
#       wait, and that too is a failure.
#
#   fixture-server.sh keep DIR OUT SERVER_LOG
#       Keeps what the fixture left before the scratch directory goes: the
#       server log beside the report as OUT.server.log (owner only), and DIR/out.json
#       as OUT. When the fixture wrote no report, OUT holds one that says so
#       and the log tail is printed. Fails when anything could not be kept, and
#       when the kept report lacks the shape the judge reads or does not say
#       the fixture ended well.

set -u

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

usage()
{
    echo "usage: ${0##*/} check PIN TEST | stop DIR PID WAIT_SECONDS | keep DIR OUT SERVER_LOG" >&2
    exit 2
}

# A child that has exited but not been waited for still answers kill -0, and the
# caller is the one that waits for it, so once the cheap probe says yes its
# state is asked for.
alive()
{
    kill -0 "$1" 2> /dev/null || return 1
    state=$(ps -o stat= -p "$1" 2> /dev/null) || return 1

    # Some systems pad the column.
    state=${state#"${state%%[! ]*}"}
    case $state in
        '' | [Zz]*) return 1 ;;
        *) return 0 ;;
    esac
}

# Whether $1 exited within $2 seconds.
exited_within()
{
    waited=0
    while alive "$1" && [ "$waited" -lt "$2" ]; do
        sleep 1
        waited=$((waited + 1))
    done
    ! alive "$1"
}

check()
{
    [ $# -eq 2 ] || usage
    pin=$1
    test_name=$2
    case $test_name in
        '' | *[[:space:]]*)
            echo "fixture-server: the test name must be one exact name" >&2
            return 2
            ;;
    esac
    if [ ! -x "$pin" ]; then
        echo "fixture-server: $pin is not an executable" >&2
        return 2
    fi
    listed=$("$pin" --list --ignored 2> /dev/null) || {
        echo "fixture-server: $pin would not list its ignored tests" >&2
        return 2
    }
    count=$(printf '%s\n' "$listed" | grep -cxF "$test_name: test")
    if [ "$count" != 1 ]; then
        echo "fixture-server: $pin lists $count ignored tests named $test_name, not 1" >&2
        return 2
    fi
}

stop()
{
    [ $# -eq 3 ] || usage
    dir=$1
    pid=$2
    wait_seconds=$3

    # Not 0 or a leading zero, since `kill 0` signals this script's whole
    # process group rather than the fixture, and not 1, a container's init.
    case $pid in
        '' | *[!0-9]* | 0* | 1) usage ;;
    esac
    case $wait_seconds in
        '' | *[!0-9]*) usage ;;
    esac
    grace=${FIXTURE_SERVER_TERM_GRACE_S:-5}

    # Not `:`, whose failed redirection would end this script, a special
    # built-in's, and leave the caller waiting on a fixture nothing asked to
    # stop.
    if ! true > "$dir/stop"; then
        echo "fixture-server: could not write the stop file; terminating the fixture" >&2

        # While it serves, the fixture takes a TERM as the same request as the
        # stop file, so the TERM gets the whole wait before the kill.
        grace=$wait_seconds
    elif exited_within "$pid" "$wait_seconds"; then
        return 0
    else
        echo "fixture-server: the fixture had not stopped after ${wait_seconds}s; terminating it" >&2
    fi
    kill -TERM "$pid" 2> /dev/null || true
    exited_within "$pid" "$grace" || kill -KILL "$pid" 2> /dev/null || true
    return 1
}

keep()
{
    [ $# -eq 3 ] || usage
    dir=$1
    out=$2
    server_log=$3
    kept_status=0

    # Beside the report, because the scratch directory goes next and the
    # server's own log is the only account of why a fixture misbehaved. Every
    # write here refuses a file that is already there (set -C), so nothing kept
    # can replace another run's file or the capture this one just wrote.
    if ! (umask 077 && set -C && cat "$server_log" > "$out.server.log") 2> /dev/null; then
        echo "fixture-server: could not keep the server log beside the report" >&2
        kept_status=1
    fi
    if [ ! -f "$dir/out.json" ]; then
        echo "fixture-server: the serving fixture left no report; the end of its log:" >&2
        tail -n 40 "$server_log" >&2 || true

        # A report that says so, so a reader of the report path never finds
        # nothing where a verdict was owed.
        (umask 077 && set -C && printf '%s\n' '{"outcome":"the serving fixture left no report","arming_failure":null,"capture":null}' \
            > "$out") 2> /dev/null \
            || echo "fixture-server: could not write that at $out; a file already there is not this run's report" >&2
        return 1
    fi

    # Owner only, like the log: it holds the interviewer's replies and the tool
    # results.
    if ! (umask 077 && set -C && cat "$dir/out.json" > "$out"); then

        # The scratch directory goes next, and this is the only copy.
        echo "fixture-server: could not keep the serving fixture's report; it said:" >&2
        cat "$dir/out.json" >&2
        return 1
    fi

    # Accepted only if it has the shape the judge reads and says it ended well.
    node "$here/long-interview-evidence.cjs" --check-fixture "$out" || kept_status=1
    return "$kept_status"
}

case ${1:-} in
    check)
        shift
        check "$@"
        ;;
    stop)
        shift
        stop "$@"
        ;;
    keep)
        shift
        keep "$@"
        ;;
    *) usage ;;
esac

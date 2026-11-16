#!/bin/sh
# Stands in for node on browser-check.cjs: leaves the capture a finished long
# interview would, then exits as told. With STAND_IN_JUDGE_EXIT set it also
# stands in for the judge, which then dies with that status before saying
# anything. Everything else runs the real node.
case $1 in
    */scripts/browser-check.cjs)
        printf '%s' "$STAND_IN_CAPTURE" > "$BROWSER_CHECK_CAPTURE"
        exit "${STAND_IN_WORKER_EXIT:-0}"
        ;;
    */scripts/long-interview-evidence.cjs)

        # A judge that dies before it says anything, when a test asks for one;
        # the fixture check that runs the same file is left alone.
        if [ "${2:-}" = --capture ] && [ -n "${STAND_IN_JUDGE_EXIT:-}" ]; then
            exit "$STAND_IN_JUDGE_EXIT"
        fi
        ;;
esac
exec "$REAL_NODE" "$@"

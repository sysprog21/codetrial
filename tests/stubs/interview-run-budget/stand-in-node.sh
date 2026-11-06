#!/bin/sh
# Stands in for node on browser-check.cjs only: leaves the capture a finished
# long interview would, then exits as told. Everything else runs the real node.
case $1 in
    */scripts/browser-check.cjs)
        printf '%s' "$STAND_IN_CAPTURE" > "$BROWSER_CHECK_CAPTURE"
        exit "${STAND_IN_WORKER_EXIT:-0}"
        ;;
esac
exec "$REAL_NODE" "$@"

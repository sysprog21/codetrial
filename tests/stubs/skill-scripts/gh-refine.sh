#!/bin/sh
# gh for review-finish.sh's refine comment: the live head, the viewer, the
# comment history (a JSON array) in $FAKE_HISTORY, and a log of what a pr
# comment posts and which comments are deleted. $FAKE_API_FAIL and
# $FAKE_COMMENT_FAIL make those calls fail, and $FAKE_DELETE_FAIL the delete.
case "$*" in
    "pr view"*) echo "$FAKE_LIVE" ;;
    "api user"*) echo viewer ;;
    "api -X DELETE"*)
        [ -z "${FAKE_DELETE_FAIL:-}" ] || exit 1
        echo "$*" >> "$FAKE_LOG"
        ;;
    "api "*)
        [ -z "$FAKE_API_FAIL" ] || exit 1
        printf '%s\n' "$FAKE_HISTORY"
        ;;
    "pr comment"*)
        [ -z "$FAKE_COMMENT_FAIL" ] || exit 1
        for a; do last=$a; done
        echo comment >> "$FAKE_LOG"
        cat "$last" >> "$FAKE_LOG"
        ;;
esac

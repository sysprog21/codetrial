#!/bin/sh
case "$*" in
    "api user --jq .login") echo viewer ;;
    *collaborators*)
        [ -z "${FAKE_NO_PUSHERS:-}" ] || exit 1
        printf 'maint\nviewer\n'
        ;;
    "pr view 5 --repo sysprog21/codetrial --json number,"*) cat "$FAKE_PR" ;;
    "pr view 5 --repo sysprog21/codetrial --json headRefOid --jq .headRefOid") echo h1 ;;
    "pr diff 5 "*)
        [ -n "${FAKE_DIFF:-}" ] || exit 1
        printf '%s\n' "$FAKE_DIFF"
        ;;
    "api repos/sysprog21/codetrial/pulls/5/commits --paginate") cat "$FAKE_COMMITS" ;;
    "api repos/sysprog21/codetrial/pulls/5/comments --paginate")
        if [ -n "${FAKE_INLINE:-}" ]; then
            printf '%s\n' "$FAKE_INLINE"
        else
            echo '[]'
        fi
        ;;
    "api repos/sysprog21/codetrial/commits/h1/check-suites "*)
        [ -z "${FAKE_NO_SUITE:-}" ] || exit 0
        echo "${FAKE_PUSHED:-2026-10-20T12:00:00Z}"
        ;;
    *) exit 1 ;;
esac

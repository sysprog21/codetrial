#!/bin/sh
case "$*" in
    "pr view 5 --repo sysprog21/codetrial --json title,"*) cat "$FAKE_PR" ;;
    "pr diff 5 "*) cat "$FAKE_DIFF" ;;
    # gh applies --jq and pages itself; the fake prints one object per file.
    "api repos/sysprog21/codetrial/pulls/5/files?per_page=100 --paginate "*)
        cat "$FAKE_FILES"
        ;;
    "api repos/sysprog21/codetrial/pulls/5/comments?per_page=100 --paginate "*)
        [ -z "${FAKE_NO_COMMENTS:-}" ] || exit 1
        echo '{"path":"web/a.js","line":11,"author":"maint","reply":true,"commit":"h1","body":"Also here."}'
        ;;
    "pr view 5 --repo sysprog21/codetrial --json headRefOid,baseRefOid --jq "*)
        echo "${FAKE_AFTER:-h1 b1}"
        ;;
    # A space reaches GitHub encoded; a failed read is named, not guessed at.
    "api repos/sysprog21/codetrial/contents/web/old.js?ref=h1 "*)
        echo "a removed file is not read" >&2
        exit 1
        ;;
    "api repos/sysprog21/codetrial/contents/web/gone.js?ref=h1 "*) exit 1 ;;
    "api repos/sysprog21/codetrial/contents/web/a%20b.txt?ref=h1 "*) echo spaced ;;
    "api repos/sysprog21/codetrial/contents/"*"?ref=h1 "*) echo "at head" ;;
    # gh applies --jq itself, so the fake prints the rows.
    "api user --jq .login") echo "${FAKE_VIEWER:-someone}" ;;
    "api graphql --paginate "*)
        printf 'false\tfalse\tweb/a.js\t11\tmaint\tHandle the empty list.\n'
        printf 'true\ttrue\tweb/a.js\t3\tmaint\tFixed.\n'
        ;;
    # gh applies --jq itself: the status, then how many merges the run holds.
    "api repos/sysprog21/codetrial/compare/${FAKE_SINCE:-c0ffee1}...${FAKE_HEAD:-h1} --jq "*)
        [ "${FAKE_COMPARE:-}" != fail ] || exit 1
        echo "${FAKE_COMPARE:-ahead 0}"
        ;;
    "api repos/sysprog21/codetrial/compare/c0ffee1...h1 -H Accept: application/vnd.github.diff")
        cat "$FAKE_DIFF"
        ;;
    *)
        echo "unexpected gh call: $*" >&2
        exit 1
        ;;
esac

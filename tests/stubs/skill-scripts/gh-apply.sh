#!/bin/sh
# gh for apply-pr-actions.sh: logs every call to $FAKE_LOG and answers the few
# reads and approval writes the script makes. FAKE_MOVED lists PRs whose live
# head differs from the sweep's. A pr comment logs its body, the merged list is
# $FAKE_MERGED, and maint is the one maintainer.
echo "$*" >> "$FAKE_LOG"
case "$*" in
    "pr view "*"--jq .headRefOid")
        case " ${FAKE_MOVED:-} " in
            *" $3 "*) echo moved ;;
            *) echo "h$3" ;;
        esac
        ;;
    "pr view 3 "*) echo "${FAKE_VIEW:-CONFLICTING h3}" ;;
    "pr comment "*)
        for last; do :; done
        cat "$last" >> "$FAKE_LOG"
        ;;
    "pr list "*) printf '%s\n' "${FAKE_MERGED:-[]}" ;;
    "api --paginate "*collaborators*) echo maint ;;
    "api graphql "*"mutation("*)
        if [ -n "${FAKE_REFUSED:-}" ]; then
            echo "Could not resolve to a node" >&2
            exit 1
        fi

        # FAKE_GONE: after the timed-out attempt the node no longer exists, as
        # when that attempt had in fact deleted it.
        if [ -n "${FAKE_GONE:-}" ] && [ -f "$FAKE_LOG.flaky" ]; then
            echo "Could not resolve to a node" >&2
            exit 1
        fi
        # FAKE_FLAKY makes the first mutation fail, as a network timeout does.
        if [ -n "${FAKE_FLAKY:-}" ] && [ ! -f "$FAKE_LOG.flaky" ]; then
            : > "$FAKE_LOG.flaky"
            echo "dial tcp: operation timed out" >&2
            exit 1
        fi
        : > "$FAKE_LOG.written"
        ;;
    "api graphql "*"query("*)
        if [ -n "${FAKE_THREADS_AFTER_WRITE:-}" ] && [ -f "$FAKE_LOG.written" ]; then
            printf '%s\n' "$FAKE_THREADS_AFTER_WRITE"
        elif [ -n "${FAKE_THREADS:-}" ]; then
            printf '%s\n' "$FAKE_THREADS"
        else
            printf '%s\n' '{"data":{"repository":{"pullRequest":{"reviewThreads":{"totalCount":1,"nodes":[{"isResolved":true,"isOutdated":false,"comments":{"totalCount":1,"nodes":[{"pullRequestReview":{"id":"PRR_2"}}]}}]}}}}}'
        fi
        ;;
    "run view "*) echo "${FAKE_RUN_STATUS:-in_progress}" ;;
    "api -X POST repos/"*/approve)
        [ -z "${FAKE_NOT_WAITING:-}" ] && exit 0
        echo "This workflow run is not waiting for approval" >&2
        exit 1
        ;;
esac

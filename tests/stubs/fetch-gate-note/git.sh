#!/bin/sh
# git for tests/test_fetch_gate_note.py. It logs every call and the git-related
# environment it was given, then either answers a fetch from a canned failure
# (FAKE_FETCH_ERR, FAKE_FETCH_RC, FAKE_FETCH_SLEEP) or runs the real git
# ($REAL_GIT) with https://github.com/ rewritten to the local directory
# $FAKE_REMOTE, so the notes logic runs against real objects without a network.
# FAKE_NOTES_ERR fails `git notes` with that message. FAKE_DROP_FILTER removes
# --filter, as a server that ignores it would. FAKE_CATFILE_ERR fails `git
# cat-file` with that message. Each call also logs the file size limit it runs
# under, as FILE_SIZE_LIMIT.
{
    echo '--call--'
    for a in "$@"; do
        printf 'arg=%s\n' "$a"
    done
    env | grep -E '^(GIT_|HOME=|LC_ALL=|TMPDIR=)' | sed 's/^/env=/'
    echo "env=FILE_SIZE_LIMIT=$(ulimit -f)"
} >> "$FAKE_LOG"

case " $* " in
    *" fetch "*)
        if [ -n "${FAKE_FETCH_SLEEP:-}" ]; then
            sleep "$FAKE_FETCH_SLEEP"
        fi
        if [ -n "${FAKE_FETCH_ERR:-}" ]; then
            printf '%s\n' "$FAKE_FETCH_ERR" >&2
            exit "${FAKE_FETCH_RC:-128}"
        fi
        ;;
    *" cat-file "*)
        if [ -n "${FAKE_CATFILE_ERR:-}" ]; then
            printf '%s\n' "$FAKE_CATFILE_ERR" >&2
            exit 128
        fi
        ;;
    *" notes "*)
        if [ -n "${FAKE_NOTES_ERR:-}" ]; then
            printf '%s\n' "$FAKE_NOTES_ERR" >&2
            exit 128
        fi
        ;;
esac

for a in "$@"; do
    shift
    case $a in
        https://github.com/*) a=file://$FAKE_REMOTE/${a#https://github.com/} ;;
        protocol.allow=never) a=protocol.allow=always ;;
        --filter=*) [ -z "${FAKE_DROP_FILTER:-}" ] || continue ;;
    esac
    set -- "$@" "$a"
done
exec "$REAL_GIT" "$@"

#!/bin/sh
set -eu

# The recordings whose media should not exist any more, and a way to bring one
# forward. The Rust sweeper does the deleting, every minute, and this is the
# door an operator uses: to see the list, or to say that a particular recording
# should go now rather than at its deadline.
#
# The listing rule is the sweeper's own, restated in SQL rather than
# reimplemented in it: a recording is due when its retention deadline has
# passed, when it failed with `consent_withdrawn`, or when its interview's
# consent was withdrawn, and never while its Egress job has not been confirmed
# stopped.
#
# What this does not do is talk to Drive or GCS. Deleting media needs the
# service account, the ordering and the per-step progress the server already
# has, and a second implementation of a deletion is a second thing that can be
# wrong about what it deleted. So a marked recording is deleted by the running
# server, and if no server is running, nothing is deleted until one is.

usage()
{
    cat >&2 << 'USAGE'
usage: recording-cleanup.sh [--db PATH] [--now EPOCH] [--dry-run]
       recording-cleanup.sh [--db PATH] [--now EPOCH] --expire ID [ID...]

  (no arguments)  list the recordings that are due and change nothing
  --dry-run       the same thing, said out loud
  --expire ID...  bring those recordings' deadlines forward, so the running
                  server's sweeper deletes their media on its next pass and
                  records the deletion as an operator's
  --db PATH       the account database, default $CODETRIAL_DB_PATH or codetrial.db
  --now EPOCH     the clock to measure deadlines against, default now
USAGE
    exit 2
}

db=${CODETRIAL_DB_PATH:-codetrial.db}
now=$(date +%s)
expire=

while [ $# -gt 0 ]; do
    case "$1" in
        --dry-run) ;;
        --expire)
            shift
            [ $# -ge 1 ] || usage
            while [ $# -gt 0 ]; do
                case "$1" in
                    --*) break ;;
                    *)
                        expire="$expire $1"
                        shift
                        ;;
                esac
            done
            continue
            ;;
        --db)
            [ $# -ge 2 ] || usage
            db=$2
            shift
            ;;
        --now)
            [ $# -ge 2 ] || usage
            now=$2
            shift
            ;;
        -h | --help) usage ;;
        *)
            echo "unknown argument: $1" >&2
            usage
            ;;
    esac
    shift
done

case "$now" in
    '' | *[!0-9]*)

        # Interpolated into SQL below, so it is checked here rather than
        # trusted.
        echo "--now takes epoch seconds" >&2
        exit 2
        ;;
esac

command -v sqlite3 > /dev/null 2>&1 || {
    echo "required command is unavailable: sqlite3" >&2
    exit 2
}
[ -f "$db" ] || {
    echo "no database at $db" >&2
    exit 2
}

quote()
{
    # One id per argument, single-quoted for SQL, with any embedded quote
    # doubled. Recording ids are 22 characters of base64url and contain none of
    # this, which is exactly why it costs nothing to be sure.
    printf "'%s'" "$(printf '%s' "$1" | sed "s/'/''/g")"
}

if [ -n "$expire" ]; then
    ids=
    for id in $expire; do
        [ -z "$ids" ] || ids="$ids,"
        ids="$ids$(quote "$id")"
    done

    # `deleted_by` is written now and read by the sweeper, so the tombstone says
    # a person asked rather than that a deadline arrived.
    changed=$(sqlite3 "$db" "
UPDATE recordings
SET expires_at = $now, deleted_by = 'operator'
WHERE deleted_at IS NULL
  AND state IN ('ready', 'failed', 'cleanup_failed')
  AND id IN ($ids);
SELECT changes();
")
    echo "$changed marked; the running server deletes their media on its next sweep." >&2
    if [ "$changed" -eq 0 ]; then
        echo "no such recording, or it is already deleted" >&2
        exit 1
    fi
    exit 0
fi

due=$(sqlite3 "$db" "
SELECT id FROM recordings
WHERE deleted_at IS NULL
  AND state IN ('ready', 'failed', 'cleanup_failed')
  AND (egress_id IS NULL OR stopped_at IS NOT NULL)
  AND ((expires_at IS NOT NULL AND expires_at <= $now)
       OR error = 'consent_withdrawn'
       OR interview_id IN (SELECT id FROM interviews WHERE consent_withdrawn_at IS NOT NULL))
ORDER BY id
")
if [ -z "$due" ]; then
    echo "nothing is due for deletion"
    exit 0
fi

echo "$due"
count=$(printf '%s\n' "$due" | wc -l | tr -d ' ')
echo "$count due; nothing was changed here. The server's sweeper deletes them." >&2

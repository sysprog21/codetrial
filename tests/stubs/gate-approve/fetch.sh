#!/bin/sh
# Stand-in for scripts/fetch-gate-note.sh: logs its arguments, then prints the
# note in $FAKE_DIR or fails with $FAKE_NOTE_RC and one line on stderr.
echo "FETCH $*" >> "$FAKE_LOG"
if [ -n "${FAKE_NOTE_RC:-}" ]; then
    echo "fetch-gate-note: canned failure $FAKE_NOTE_RC" >&2
    exit "$FAKE_NOTE_RC"
fi
cat "$FAKE_DIR/note"

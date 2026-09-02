#!/bin/sh
set -eu

# Vendored bytes run in the candidate's browser: the face detector decides
# whether an interview gets terminated, and livekit-client.js carries every byte
# of interview media. A hash recorded but never checked is a comment.
#
# The iteration is deliberately inverted. Walking the SHA256SUMS files verifies
# whatever already opted in and silently skips everything that did not, which is
# exactly backwards: the file nobody pinned is the one worth catching. So
# enumerate the vendored tree first and require every path to be covered.
#
# This is the single owner of the check. The Makefile target and scripts/test.sh
# both call it rather than keeping their own copy.

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
VENDOR="$ROOT/web/vendor"

[ -d "$VENDOR" ] || exit 0

# macOS ships shasum but not sha256sum; most Linux distros ship the reverse.
# Both read the same `hash name` lines, so SHA256SUMS needs no variant. Bind one
# at startup so a machine with neither fails here rather than mid-walk.
# sha256sum is tried first because shasum is a Perl script and slower.
if command -v sha256sum > /dev/null 2>&1; then
    sha256_check()
    {
        sha256sum -c SHA256SUMS
    }
elif command -v shasum > /dev/null 2>&1; then
    sha256_check()
    {
        shasum -a 256 -c SHA256SUMS
    }
else
    echo "verify-vendor: need sha256sum or shasum" >&2
    exit 1
fi

status=0

# Every vendored file needs a line in the SHA256SUMS beside it. `basename` is
# what the sums files record, matching the `cd && sha256sum -c` convention.
# README and LICENSE are provenance, not bytes the browser runs, so they are the
# only things allowed to be unpinned. The ignored legacy model is no longer
# shipped or served, but can remain in pre-migration worktrees.
#
# Collected rather than piped into the loop, for two reasons. A pipeline reports
# the status of its last command, so `find | while` skips a subtree it cannot
# read and still exits 0, which is the one outcome a supply-chain check must
# never produce on an unexamined tree. Its loop body is also a subshell, so a
# failure there has to be signalled by exiting rather than by `status`.
#
# The `||` has to stay out here rather than move into a helper the two walks
# share: `exit` inside `$(...)` leaves the substitution, not the script, and the
# silent pass is back.
files=$(find "$VENDOR" -type f ! -path "$VENDOR/avatar/jim.vrm" ! -name SHA256SUMS ! -name FETCH ! -name 'README*' ! -name 'LICENSE*') \
    || {
        echo "verify-vendor: cannot scan ${VENDOR#"$ROOT"/}" >&2
        exit 1
    }

while read -r file; do
    [ -n "$file" ] || continue
    sums=$(dirname "$file")/SHA256SUMS
    if [ ! -f "$sums" ] || ! grep -qF "  $(basename "$file")" "$sums"; then
        echo "unpinned vendored file: ${file#"$ROOT"/}" >&2
        status=1
    fi
done << EOF
$files
EOF

[ "$status" -eq 0 ] || exit 1

# Then verify the recorded hashes still match the bytes on disk. Every directory
# is checked even after one fails, so a bad checkout reports all of its damage
# in one run instead of one bad file per rerun.
sums_files=$(find "$VENDOR" -name SHA256SUMS) \
    || {
        echo "verify-vendor: cannot scan ${VENDOR#"$ROOT"/}" >&2
        exit 1
    }

while read -r sums; do
    [ -n "$sums" ] || continue
    (cd "$(dirname "$sums")" && sha256_check) || status=1
done << EOF
$sums_files
EOF

exit "$status"

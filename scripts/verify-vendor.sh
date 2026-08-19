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

status=0

# Every vendored file needs a line in the SHA256SUMS beside it. `basename` is
# what the sums files record, matching the `cd && shasum -c` convention. README
# and LICENSE are provenance, not bytes the browser runs, so they are the only
# things allowed to be unpinned.
find "$VENDOR" -type f ! -name SHA256SUMS ! -name FETCH ! -name 'README*' ! -name 'LICENSE*' | while read -r file; do
  sums=$(dirname "$file")/SHA256SUMS
  if [ ! -f "$sums" ] || ! grep -qF "  $(basename "$file")" "$sums"; then
    echo "unpinned vendored file: ${file#"$ROOT"/}" >&2
    exit 1
  fi
done || status=1

[ "$status" -eq 0 ] || exit 1

# Then verify the recorded hashes still match the bytes on disk.
find "$VENDOR" -name SHA256SUMS | while read -r sums; do
  (cd "$(dirname "$sums")" && shasum -a 256 -c SHA256SUMS)
done

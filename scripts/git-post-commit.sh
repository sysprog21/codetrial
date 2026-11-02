#!/bin/sh

# Only a candidate validated by pre-commit and commit-msg can be repaired here.
# Skipped hooks leave no matching claim and are caught by pre-push verification.
set -eu
root=$(git rev-parse --show-toplevel)
status=0
python3 "$root/scripts/hashchain.py" fix || status=$?
if [ -f "$root/.claude/skills/codetrial-review/commit-sizes.py" ] \
    && [ "$(git rev-parse --is-shallow-repository)" = false ] \
    && git rev-parse --verify 'HEAD^' > /dev/null 2>&1; then
    python3 "$root/scripts/check-interactive-review.py" mark --base 'HEAD^' || {
        [ "$status" -ne 0 ] || status=$?
    }
fi
exit "$status"

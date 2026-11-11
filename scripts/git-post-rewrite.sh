#!/bin/sh

# A rebase or an amend gives the branch a new head, and the interactive review
# of the old one no longer covers it. Say so once the rewrite is done, while the
# contributor still has the change in mind; the pre-push hook is what refuses.
# Advice only: nothing here may fail a rebase.
root=$(git rev-parse --show-toplevel) || exit 0
[ -f "$root/scripts/check-interactive-review.py" ] || exit 0
[ "$(git rev-parse --is-shallow-repository)" = false ] || exit 0
command -v python3 > /dev/null 2>&1 || exit 0
advice=$(python3 "$root/scripts/check-interactive-review.py" rewritten 2> /dev/null) || exit 0
[ -z "$advice" ] || printf 'Note: %s\n' "$advice" >&2
exit 0

#!/bin/sh
# Print one hash naming the working tree as it stands: HEAD, every tracked
# change against it, and the name and content of each untracked file that is not
# ignored. Two runs print the same value only when the tree is the same, which
# is what lets a gate receipt (scripts/test.sh) be matched to the tree a claim
# is about. Of the directory named by $1, else of the repository this script
# lives in, whatever the caller's directory. Exits 1 outside a git work tree.

set -eu
cd "${1:-$(dirname "$0")/..}"

# A pipeline reports its last command, so the one step that must work is checked
# alone: outside a work tree or on an unborn HEAD there is nothing to name.
git rev-parse --verify --quiet HEAD > /dev/null || exit 1
{
    git rev-parse HEAD
    git diff HEAD --binary
    git ls-files -o --exclude-standard -z | xargs -0 -r git hash-object --no-filters --
    git ls-files -o --exclude-standard
} | git hash-object --stdin

#!/bin/sh

# Run the fast half of scripts/test.sh over what is staged, and only over what
# is staged. The gate itself reads the working tree, which is the wrong tree at
# commit time: a half-finished edit next to a staged one either fails a commit
# that is fine or passes one that is not. Everything below therefore runs
# against a checkout of the index, not against the files on disk.
#
# What is deliberately absent: cargo build, cargo test, clippy, and the
# generated-artifact drift checks. Those need the whole tree and take minutes,
# and scripts/test.sh already owns them. A hook that costs a coffee break is a
# hook people disable.

set -u

ROOT=$(git rev-parse --show-toplevel) || exit 1
cd "$ROOT" || exit 1

git diff --cached --quiet && exit 0

failed=0
work=$(mktemp -d) || exit 1
trap 'rm -rf "$work"' EXIT HUP INT TERM

# One copy of the index for everything below, scripts/indent.sh included, so a
# file staged from an editor while this runs cannot be judged by one lane and
# missed by the next. Under `git commit -a` git has already pointed
# GIT_INDEX_FILE at a temporary index, and --git-path follows it. Relative to
# this directory, the top level, and not asked for with --path-format=absolute,
# which git before 2.31 echoes back as a path. A linked worktree gets an
# absolute answer, which Git for Windows spells with a drive letter.
index=$(git rev-parse --git-path index) || exit 1
case "$index" in /* | [A-Za-z]:/*) ;; *) index=$ROOT/$index ;; esac
cp "$index" "$work/.index" || exit 1
GIT_INDEX_FILE=$work/.index
export GIT_INDEX_FILE

# quotePath off, so a name with a byte past ASCII comes back as itself rather
# than quoted and silently matched by no lane below.
staged=$(git -c core.quotePath=false diff --cached --name-only --diff-filter=ACMRT)

# The whole index, not only the staged paths, even though only the staged paths
# are checked below. It is what puts eslint.config.mjs, .shellcheckrc and a
# script's sourced neighbours where the linters look for them. Checking out
# everything costs a copy of a tree that is already small, and it keeps the
# property this exists for: the content judged is the index's, so an unstaged
# edit neither fails the commit nor rides along in it.
git checkout-index -a -f --prefix="$work/" || {
    echo "pre-commit: cannot read the staged content" >&2
    exit 1
}

[ -d node_modules ] && ln -s "$ROOT/node_modules" "$work/node_modules"

# Word splitting on the lists below is the point, which makes a path containing
# whitespace two paths and the check meaningless, and a name git still quotes (a
# quote, a backslash, a control character) is not the file's name. Refuse both
# by name instead: there are none in this tree, and a commit that adds one
# should hear why it was not checked rather than be told everything passed.
# scripts/indent.sh refuses the same names for the tree, which never comes
# through here.
odd=$(printf '%s\n' "$staged" | grep -E '^"|[[:blank:]]')
if [ -n "$odd" ]; then
    printf '%s\n' "$odd" | sed 's/^/  /' >&2
    echo "pre-commit: the staged paths above cannot be checked; rename them" >&2
    exit 1
fi
# Vendored code keeps upstream's formatting and edition, so no lane checks it.
matching()
{
    printf '%s\n' "$staged" | grep -E "$1" | grep -vE '^(web/)?vendor/'
}

run_in_work()
{
    (cd "$work" && "$@") || failed=1
}

# The linters below are optional: a contributor without one hears that it was
# skipped, and the verdict at the end names it rather than calling the commit
# checked. CI installs every one of them.
skipped=""
have()
{
    command -v "$1" > /dev/null 2>&1 && return 0
    echo "pre-commit: ${1##*/} not installed, skipping that lane${2:+ ($2)}" >&2
    skipped="$skipped ${1##*/}"
    return 1
}

js=$(matching '\.(js|mjs|cjs)$')
if [ -n "$js" ] && have node_modules/.bin/eslint "run npm ci"; then
    # shellcheck disable=SC2086
    run_in_work "$ROOT/node_modules/.bin/eslint" --no-warn-ignored $js
fi

# Without ruff the lint is skipped here and the commit is refused anyway: ruff
# is also the Python formatter, which indent.sh below requires.
py=$(matching '\.py$')
if [ -n "$py" ] && have ruff; then
    # shellcheck disable=SC2086
    run_in_work ruff check $py
fi

sh_files=$(matching '\.sh$')
if [ -n "$sh_files" ]; then
    for file in $sh_files; do
        run_in_work sh -n "$file"
    done

    # `-x` because the gate reaches the same conclusion by another route: it
    # passes every script at once, so a sourced file is an input there and is
    # followed. Here only the staged ones are named, and without this a script
    # that sources another is SC1091 the moment it is committed alone, for a
    # file the checkout above already put beside it.
    # shellcheck disable=SC2086
    have shellcheck && run_in_work shellcheck -x $sh_files
fi

# Formatting is scripts/indent.sh's, against this checkout and this index, so
# the hook and the gate cannot disagree about a file list or a tool order. It
# refuses the commit when a staged file needs a formatter that is not installed.
# Last, because it formats the checkout in place to compare it, and the linters
# above have to read what was staged.
"$ROOT/scripts/indent.sh" --staged "$work" || failed=1

# Trailing whitespace, a space before a tab, and a conflict marker. Cheap, and
# each one is noise that a reviewer would otherwise have to spend a comment on.
git diff --cached --check || failed=1

if [ "$failed" -ne 0 ]; then

    # The checkers name files inside the index checkout, which is not a path
    # anyone can edit. Say so once rather than have someone go looking for it.
    echo "Paths above are a checkout of the index; fix the file in the tree." >&2
    exit 1
fi

echo "Staged checks passed${skipped:+, except what was not checked:$skipped}."
echo "The message rules run next: what and why, 50 and 72."

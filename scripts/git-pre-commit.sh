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

staged=$(git diff --cached --name-only --diff-filter=ACMR)

# The whole index, not only the staged paths, even though only the staged paths
# are checked below. rustfmt follows `mod` declarations out of the file it is
# given, so a commit that staged src/agent.rs without src/agent/integrity.rs
# beside it failed here on a module that exists in the repository and was
# missing only from this directory. It is also what puts eslint.config.mjs,
# .shellcheckrc and .editorconfig where the linters look for them, which used to
# need a copy loop of its own. Checking out everything costs a copy of a tree
# that is already small, and it keeps the property this exists for: the content
# judged is the index's, so an unstaged edit neither fails the commit nor rides
# along in it.
git checkout-index -a -f --prefix="$work/" || {
    echo "pre-commit: cannot read the staged content" >&2
    exit 1
}

[ -d node_modules ] && ln -s "$ROOT/node_modules" "$work/node_modules"

# Word splitting on the lists below is the point, which makes a path containing
# whitespace two paths and the check meaningless. Refuse it by name instead:
# there are none in this tree, and a commit that adds one should hear why it was
# not checked rather than be told everything passed.
if printf '%s\n' "$staged" | grep -q '[[:blank:]]'; then
    printf '%s\n' "$staged" | grep '[[:blank:]]' | sed 's/^/  /' >&2
    echo "pre-commit: the staged paths above contain whitespace" >&2
    exit 1
fi
matching()
{
    printf '%s\n' "$staged" | grep -E "$1" | grep -vE '^web/vendor/'
}

run_in_work()
{
    (cd "$work" && "$@") || failed=1
}

# Every lane is optional in the same way the gate treats an uninstalled tool: a
# contributor without it gets a note rather than a failure they cannot act on,
# and CI installs the ones that must not skip.
have()
{
    command -v "$1" > /dev/null 2>&1 && return 0
    echo "pre-commit: $1 not installed, skipping that lane" >&2
    return 1
}

# The edition is rustfmt's only piece of Cargo.toml, and passing it keeps this
# working without a manifest beside the staged copies.
rs=$(matching '\.rs$')
if [ -n "$rs" ] && have rustfmt; then
    edition=$(git show :Cargo.toml 2> /dev/null \
        | sed -n 's/^edition *= *"\([0-9]*\)".*/\1/p' | sed -n 1p)
    # shellcheck disable=SC2086
    run_in_work rustfmt --edition "${edition:-2024}" --check $rs
fi

js=$(matching '\.(js|mjs|cjs)$')
if [ -n "$js" ]; then
    if [ -x node_modules/.bin/eslint ]; then
        # shellcheck disable=SC2086
        run_in_work "$ROOT/node_modules/.bin/eslint" --no-warn-ignored $js
    else
        echo "pre-commit: no eslint, run \`npm install\` to enable it" >&2
    fi
fi

py=$(matching '\.py$')
if [ -n "$py" ]; then
    if have ruff; then
        # shellcheck disable=SC2086
        run_in_work ruff check $py
        # shellcheck disable=SC2086
        run_in_work ruff format --check $py
    else

        # py_compile is not ruff, but it is always there, and a Python file that
        # does not parse is the failure worth catching before it is committed.
        for file in $py; do
            run_in_work python3 -m py_compile "$file"
        done
    fi
fi

# Shell gets commentflow and shfmt; Rust cannot. commentflow puts a blank line
# before a comment inside a chain and `cargo fmt` takes it back out, so that
# pair is only stable as a composition, which is what scripts/indent.sh checks
# in the gate. Shell has no such argument, and shfmt reads the .editorconfig
# copied in above rather than its own defaults.
sh_files=$(matching '\.sh$')
if [ -n "$sh_files" ]; then
    for file in $sh_files; do
        run_in_work sh -n "$file"
    done
    # shellcheck disable=SC2086
    have shellcheck && run_in_work shellcheck $sh_files
    # shellcheck disable=SC2086
    have commentflow && run_in_work commentflow --check $sh_files
    # shellcheck disable=SC2086
    have shfmt && run_in_work shfmt -d $sh_files
fi

# Trailing whitespace, a space before a tab, and a conflict marker. Cheap, and
# each one is noise that a reviewer would otherwise have to spend a comment on.
git diff --cached --check || failed=1

if [ "$failed" -ne 0 ]; then

    # The checkers name files inside the index checkout, which is not a path
    # anyone can edit. Say so once rather than have someone go looking for it.
    echo "Paths above are a checkout of the index; fix the file in the tree." >&2
    exit 1
fi

echo "Staged checks passed. The message rules run next: what and why, 50 and 72."

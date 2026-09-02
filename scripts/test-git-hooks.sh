#!/bin/sh

# Exercise the git hooks against a scratch repository.
#
# The hooks are the one part of this tree with no other gate behind them: they
# run on a contributor's machine, in a directory this repository's tests never
# look at, and a hook that stops rejecting is indistinguishable from a hook that
# passes. Every case below is one that was checked by hand while the hooks were
# written, which is exactly the set that would otherwise be checked by hand
# again on the next edit and eventually not at all.
#
# Scratch repository rather than this one: a test that stages files has to stage
# them somewhere, and it must not be here.

set -u

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
failures=0
cases=0

work=$(mktemp -d) || exit 1
trap 'rm -rf "$work"' EXIT HUP INT TERM

fail()
{
    echo "  FAIL  $1" >&2
    failures=$((failures + 1))
}

# `expected` is an exit status, `label` says what the case is about. The command
# runs with its output captured, and the output is printed only when the case
# fails: a passing run says nothing, a failing one says everything.
expect()
{
    expected=$1
    label=$2
    shift 2
    cases=$((cases + 1))
    output=$("$@" 2>&1)
    status=$?
    [ "$status" -eq "$expected" ] && return 0
    fail "$label (exit $status, expected $expected)"
    printf '%s\n' "$output" | sed 's/^/        /' >&2
}

contains()
{
    label=$1
    file=$2
    pattern=$3
    cases=$((cases + 1))
    grep -q "$pattern" "$file" && return 0
    fail "$label (no /$pattern/ in $file)"
}

absent()
{
    label=$1
    file=$2
    pattern=$3
    cases=$((cases + 1))
    grep -q "$pattern" "$file" || return 0
    fail "$label (unexpected /$pattern/ in $file)"
}

# Through the installed hook rather than the script it wraps, so a wrapper that
# resolves the wrong path fails these cases instead of passing them.
#
# Invoked through `expect`, which shellcheck cannot follow. 0.9.0 calls that
# SC2317 and newer versions SC2329, and CI runs the older one.
# shellcheck disable=SC2317,SC2329
message()
{
    printf '%s\n' "$1" | "$hooks/commit-msg" -
}

# The scratch repository carries copies of the scripts, so the installer links
# what a real clone would link and the hooks resolve their own paths the way
# they will in the field.
mkdir -p "$work/repo/scripts" || exit 1
cp "$ROOT"/scripts/git-*.sh "$ROOT/scripts/check-commit-log.sh" \
    "$ROOT/scripts/install-git-hooks.sh" "$work/repo/scripts/" || exit 1
for config in .editorconfig .shellcheckrc; do
    [ -e "$ROOT/$config" ] && cp "$ROOT/$config" "$work/repo/$config"
done
cd "$work/repo" || exit 1
git init -q . || exit 1
git config user.email hooks@test
git config user.name "Hook Test"
git config commit.gpgsign false

# Asked of git, not spelled `.git/hooks`: a contributor with a global
# core.hooksPath sends the installer somewhere else, and assertions against the
# default path would then test a directory nothing writes to.
hooks=$(git rev-parse --path-format=absolute --git-path hooks) || exit 1

echo "  HOOKS   installer"
expect 0 "installer installs the hooks" ./scripts/install-git-hooks.sh
for hook in commit-msg pre-commit pre-push prepare-commit-msg; do
    cases=$((cases + 1))
    if [ ! -x "$hooks/$hook" ]; then
        fail "installer did not install an executable $hook"
    fi
done
expect 2 "installer rejects an unknown flag" ./scripts/install-git-hooks.sh --bogus

# An installer that overwrites is an installer that deletes somebody's work, so
# a name it wants but does not own has to come back as KEEP rather than a link.
rm -f "$hooks/pre-push"
printf '#!/bin/sh\nexit 0\n' > "$hooks/pre-push"
chmod +x "$hooks/pre-push"
./scripts/install-git-hooks.sh > "$work/install.out" 2>&1
contains "installer keeps a hook it did not write" "$work/install.out" "KEEP"
cases=$((cases + 1))
if [ ! -e "$hooks/pre-push" ]; then
    fail "installer removed a hook it did not write"
fi
rm -f "$hooks/pre-push"
./scripts/install-git-hooks.sh > /dev/null 2>&1

echo "  HOOKS   commit-msg"
expect 0 "a message that follows the rules" \
    message "Add the scratch repository

One paragraph saying why this exists, wrapped inside the
limit the hook enforces."
expect 0 "the rules can be printed" sh "$ROOT/scripts/git-commit-msg.sh" --rules
expect 0 "a fixup is left alone" message "fixup! Add the scratch repository"
expect 0 "a merge subject is left alone" message "Merge branch 'topic' into main"
expect 1 "a lowercase subject" message "add the scratch repository"
expect 1 "a past-tense subject" message "Added the scratch repository"
expect 1 "a subject ending in a period" message "Add the scratch repository."
# Single quotes on purpose: the backtick is the subject of the case.
# shellcheck disable=SC2016
expect 1 "a subject with a backtick" message 'Add the `scratch` repository'
expect 1 "a conventional-commit prefix" message "feat: add the scratch repository"
expect 1 "a one-word subject" message "Scratch"
expect 1 "a subject past 50 characters" \
    message "Add the scratch repository the hook tests run inside of"
expect 1 "a body with no blank line above it" message "Add the repository
Body on the second line."
expect 1 "a body line past 72 characters" message "Add the repository

This line exists to run past the seventy-two column limit that the hook is
supposed to enforce, and it does."

# A Greek letter rather than the punctuation an editor actually substitutes,
# because the byte width is the point and a fixture should look like one.
expect 1 "a non-ASCII message" message "Add the repository

The rejected character is a Greek alpha, α, standing in for whatever an
editor autocorrected."

echo "  HOOKS   prepare-commit-msg"
printf '' > "$work/empty.msg"
expect 0 "an empty message gets the rules" \
    sh ./scripts/git-prepare-commit-msg.sh "$work/empty.msg"
contains "the rules are rendered" "$work/empty.msg" "Commit rules"
contains "the enforced list is what is shown" "$work/empty.msg" "imperative mood"

# `git commit -v` puts the diff below a scissors line and drops everything after
# it, so the block has to land above the scissors or the author never sees it.
printf '\n# hint\n# ------------------------ >8 ------------------------\ndiff --git a/x b/x\n' \
    > "$work/scissors.msg"
expect 0 "a commit -v message gets the rules" \
    sh ./scripts/git-prepare-commit-msg.sh "$work/scissors.msg"
cases=$((cases + 1))
rules_line=$(grep -n "Commit rules" "$work/scissors.msg" | cut -d: -f1)
cut_line=$(grep -n ">8" "$work/scissors.msg" | cut -d: -f1)
if [ -z "$rules_line" ] || [ -z "$cut_line" ] || [ "$rules_line" -ge "$cut_line" ]; then
    fail "the rules landed below the scissors line"
fi
contains "the diff survives" "$work/scissors.msg" "^diff --git"

printf 'Already written\n' > "$work/written.msg"
expect 0 "a message that already has prose" \
    sh ./scripts/git-prepare-commit-msg.sh "$work/written.msg"
absent "prose is left alone" "$work/written.msg" "Commit rules"

echo "  HOOKS   pre-commit"
printf '#!/bin/sh\n\nset -eu\n\necho staged\n' > tidy.sh
git add tidy.sh
expect 0 "a staged file the checkers accept" sh ./scripts/git-pre-commit.sh

# The whole reason the hook checks out the index: an edit nobody staged must
# neither fail this commit nor ride along in it.
#
# The defect is a syntax error rather than a formatting one, because `sh -n` is
# always there while shfmt and shellcheck are lanes the hook skips when the tool
# is missing. A case that needs an optional tool turns a laptop without it red.
printf '#!/bin/sh\nif true; then\n  echo "no fi below me"\n' > untidy.sh
expect 0 "an unstaged file is not judged" sh ./scripts/git-pre-commit.sh
git add untidy.sh
expect 1 "the same file once staged" sh ./scripts/git-pre-commit.sh
git rm -q --cached untidy.sh
rm -f untidy.sh

echo "  HOOKS   pre-push"
git commit -q -m "Add the scratch repository" \
    -m "The push tests need a commit that the rules accept." > /dev/null 2>&1 \
    || fail "the hooks refused a commit that follows the rules"
git init -q --bare "$work/remote.git"
git remote add origin "$work/remote.git"
expect 0 "a branch whose messages pass" git push -q origin HEAD:refs/heads/main
git commit -q --allow-empty --no-verify -m "wip: skipped the hook"
expect 1 "a commit that skipped the hook" git push -q origin HEAD:refs/heads/main

# Linked worktrees share .git/hooks. The installed wrapper must therefore find
# the worktree that runs it, rather than remain a link into this checkout.
git worktree add -q -b linked "$work/linked" || exit 1
(mkdir -p "$work/linked/scripts" \
    && cp scripts/git-*.sh scripts/install-git-hooks.sh "$work/linked/scripts/") \
    || exit 1
rm -f "$hooks/pre-commit"
ln -s "$PWD/scripts/git-pre-commit.sh" "$hooks/pre-commit" || exit 1
(cd "$work/linked" && ./scripts/install-git-hooks.sh > "$work/linked.out") || exit 1
absent "linked installer keeps no hook from another worktree" "$work/linked.out" "KEEP"
cases=$((cases + 1))
if [ -L "$hooks/pre-commit" ]; then
    fail "linked installer did not replace the old link"
fi
printf '#!/bin/sh\nexit 1\n' > scripts/git-pre-commit.sh
(cd "$work/linked" \
    && printf '#!/bin/sh\n\necho staged\n' > linked.sh \
    && git add linked.sh \
    && git commit -q -m "Add the linked worktree") \
    || fail "a linked worktree runs its own hook"

if [ "$failures" -eq 0 ]; then
    printf '  HOOKS   %d checks passed\n' "$cases"
    exit 0
fi

printf '  HOOKS   %d of %d checks failed\n' "$failures" "$cases" >&2
exit 1

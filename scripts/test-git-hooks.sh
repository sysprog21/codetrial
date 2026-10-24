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
# fails: a passing run says nothing, a failing one says everything. It is also
# left in $work/last.out for `contains` to read.
expect()
{
    expected=$1
    label=$2
    shift 2
    cases=$((cases + 1))
    output=$("$@" 2>&1)
    status=$?
    printf '%s\n' "$output" > "$work/last.out"
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
    "$ROOT/scripts/install-git-hooks.sh" "$ROOT/scripts/indent.sh" \
    "$work/repo/scripts/" || exit 1
for config in .editorconfig .shellcheckrc; do
    [ -e "$ROOT/$config" ] && cp "$ROOT/$config" "$work/repo/$config"
done
cd "$work/repo" || exit 1

# The contributor's own git configuration stays out: a global core.hooksPath
# would send every case below to a directory the installer refuses by design.
GIT_CONFIG_GLOBAL=/dev/null
GIT_CONFIG_NOSYSTEM=1
export GIT_CONFIG_GLOBAL GIT_CONFIG_NOSYSTEM
git init -q . || exit 1
git config user.email hooks@test
git config user.name "Hook Test"
git config commit.gpgsign false

# Asked of git, not spelled `.git/hooks`: a contributor with a global
# core.hooksPath sends the installer somewhere else, and assertions against the
# default path would then test a directory nothing writes to. Joined by hand, as
# the installer does, for git before 2.31.
hooks=$(git rev-parse --git-path hooks) || exit 1
case "$hooks" in /*) ;; *) hooks=$PWD/$hooks ;; esac

echo "  HOOKS   installer"
expect 0 "installer installs the hooks" ./scripts/install-git-hooks.sh
for hook in commit-msg pre-commit pre-push prepare-commit-msg; do
    expect 0 "installer installs an executable $hook" test -x "$hooks/$hook"
done
expect 2 "installer rejects an unknown flag" ./scripts/install-git-hooks.sh --bogus

expect 0 "the check agrees with the install" ./scripts/install-git-hooks.sh --check
expect 0 "a second install changes nothing" ./scripts/install-git-hooks.sh
absent "a second install writes no hook" "$work/last.out" "HOOK \|UPDATE"

# Git skips a hook without the executable bit and says nothing, so the check has
# to look at the bit as well as the bytes, and the installer has to restore it.
chmod -x "$hooks/pre-commit"
expect 1 "the check refuses a hook git would skip" \
    ./scripts/install-git-hooks.sh --check
expect 0 "the installer restores the bit" ./scripts/install-git-hooks.sh
rm -f "$hooks/pre-commit"
expect 1 "the check refuses a missing hook" ./scripts/install-git-hooks.sh --check
./scripts/install-git-hooks.sh > /dev/null 2>&1

# A wrapper an older installer wrote is still ours: it is reported as stale and
# rewritten, not kept as a hook somebody else owns. Single quotes on purpose:
# the wrapper is what expands them.
# shellcheck disable=SC2016
printf '#!/bin/sh\nexec "$(git rev-parse --show-toplevel)/scripts/git-pre-commit.sh" "$@"\n' \
    > "$hooks/pre-commit"
expect 1 "the check refuses an older wrapper" ./scripts/install-git-hooks.sh --check
expect 0 "the installer upgrades an older wrapper" ./scripts/install-git-hooks.sh
contains "the upgrade is reported" "$work/last.out" "UPDATE  pre-commit"

# An installer that overwrites is an installer that deletes somebody's work, so
# a name it wants but does not own is kept, and the install fails: that hook is
# not in effect, and a zero exit would say it was.
rm -f "$hooks/pre-push"
printf '#!/bin/sh\ncat > "%s"\nexit 0\n' "$work/local.in" > "$hooks/pre-push"
chmod +x "$hooks/pre-push"
expect 1 "installer fails over a hook it did not write" \
    ./scripts/install-git-hooks.sh
contains "installer reports a hook it did not write" "$work/last.out" "FOREIGN.*left in place"
contains "installer says how to keep both" "$work/last.out" "pre-push.local"
contains "installer names that hook as not in effect" "$work/last.out" "FOREIGN"
contains "installer left the user's hook as it was" "$hooks/pre-push" 'exit 0'
expect 1 "the check refuses a hook it did not write" \
    ./scripts/install-git-hooks.sh --check

# The integration the message asks for: the user's hook moved beside ours runs
# after it, with the refs ours read on stdin.
mv "$hooks/pre-push" "$hooks/pre-push.local"
expect 0 "a hook moved to .local is installed behind ours" \
    ./scripts/install-git-hooks.sh
contains "the check names the hook behind ours" "$work/last.out" "then pre-push.local"
expect 0 "uninstalling puts the user's hook back" \
    ./scripts/install-git-hooks.sh --uninstall
contains "the user's hook is restored" "$hooks/pre-push" 'exit 0'
mv "$hooks/pre-push" "$hooks/pre-push.local"
./scripts/install-git-hooks.sh > /dev/null 2>&1

# A foreign hook beside a .local already there: moving it would overwrite one of
# the user's two hooks, so the advice must not be to move it.
printf '#!/bin/sh\nexit 0\n' > "$hooks/commit-msg.local"
chmod +x "$hooks/commit-msg.local"
mv "$hooks/commit-msg" "$work/commit-msg.ours"
printf '#!/bin/sh\nexit 0\n' > "$hooks/commit-msg"
expect 1 "installer fails over a hook with a .local beside it" \
    ./scripts/install-git-hooks.sh
absent "installer does not advise overwriting the .local" "$work/last.out" "mv "
contains "installer says the two need merging" "$work/last.out" "merge the two"
mv "$work/commit-msg.ours" "$hooks/commit-msg"

# What reaches the hook behind ours: the same arguments, stdin byte for byte,
# including trailing blank lines and a missing final newline, and its exit
# status. Ours refusing stops it from running at all.
printf '#!/bin/sh\nprintf "%%s\\n" "$@" > "%s"\ncat > "%s"\nexit 3\n' \
    "$work/local.args" "$work/local.in" > "$hooks/commit-msg.local"
printf 'Add the scratch repository\n\nOne paragraph saying why this exists, wrapped\ninside the limit.\n' \
    > "$work/good.msg"
printf 'refs\n\n\nno final newline' > "$work/stdin"
# The sh -c body expands its own arguments.
# shellcheck disable=SC2016
expect 3 "the hook behind ours decides the status" \
    sh -c '"$1/commit-msg" "$2" < "$3"' sh "$hooks" "$work/good.msg" "$work/stdin"
contains "the hook behind ours gets the arguments" "$work/local.args" "good.msg"
expect 0 "the hook behind ours gets stdin unaltered" \
    cmp -s "$work/stdin" "$work/local.in"
rm -f "$work/local.in"
printf 'bad subject\n' > "$work/bad.msg"
# The sh -c body expands its own arguments.
# shellcheck disable=SC2016
expect 1 "ours refusing stops the hook behind it" \
    sh -c '"$1/commit-msg" "$2" < /dev/null' sh "$hooks" "$work/bad.msg"
expect 1 "the hook behind ours does not run after ours refused" \
    test -e "$work/local.in"
rm -f "$hooks/commit-msg.local"

# A core.hooksPath outside the repository is usually a global one, shared by
# every repository on the machine, and writing wrappers there would break the
# ones that have no scripts/ to run.
git config core.hooksPath "$work/shared"
expect 1 "installer refuses a shared hooks directory" \
    ./scripts/install-git-hooks.sh
contains "installer says the directory is shared" "$work/last.out" "SHARED"
expect 1 "installer writes nothing into a shared hooks directory" \
    test -e "$work/shared"

# The same directory reached through a symlink inside the repository.
mkdir "$work/shared"
ln -s "$work/shared" linked-hooks
git config core.hooksPath linked-hooks
expect 1 "installer refuses a shared directory behind a symlink" \
    ./scripts/install-git-hooks.sh
expect 0 "installer writes nothing through a symlink to a shared directory" \
    test -z "$(ls "$work/shared")"
rm -f linked-hooks
git config --unset core.hooksPath

# A branch from before the hook scripts existed has nothing to run; refusing
# every commit there for a missing file would only teach --no-verify.
empty=$(git hash-object -t tree /dev/null)
git worktree add -q --detach "$work/old" "$(git commit-tree "$empty" -m old)" || exit 1
# shellcheck disable=SC2016
expect 0 "a checkout without the scripts commits" \
    sh -c 'cd "$1" && "$2/pre-commit"' sh "$work/old" "$hooks"
git worktree remove --force "$work/old"

# Nothing to resolve a checkout from is a hook that cannot run, not one with
# nothing to do, and must not pass as if it had checked something.
expect 1 "a hook that cannot find its checkout" \
    env GIT_DIR="$work/nowhere" "$hooks/pre-commit"

# A merge being concluded runs no script of ours: the tree is already the
# incoming one. A message the rules refuse shows the script did not run.
merge_head=$(git rev-parse --git-path MERGE_HEAD)
git rev-parse HEAD > "$merge_head"
printf 'bad subject\n' > "$work/merge.msg"
expect 0 "a merge runs no script of ours" "$hooks/commit-msg" "$work/merge.msg"
rm -f "$merge_head"

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
expect 1 "a subject with no body" message "Add the scratch repository"
expect 1 "a body of whitespace only" message "Add the scratch repository

   "
expect 1 "a body of comments only" message "Add the scratch repository

# Please enter the commit message for your changes."
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

# Plain text, because the cases below have to hold on a machine without the
# optional linters or the formatters: a case that needs a tool turns a laptop
# without it red. `git diff --check` is always there.
printf 'tidy\n' > notes.txt
git add notes.txt
expect 0 "a staged file the checkers accept" sh ./scripts/git-pre-commit.sh

# The whole reason the hook checks out the index: an edit nobody staged must
# neither fail this commit nor ride along in it.
printf 'untidy \n' > notes.txt
expect 0 "an unstaged file is not judged" sh ./scripts/git-pre-commit.sh
git add notes.txt
expect 1 "the same file once staged" sh ./scripts/git-pre-commit.sh
printf 'tidy\n' > notes.txt
git add notes.txt

# The scratch repository has no node_modules, so this is JavaScript with no
# Prettier, which must not pass as checked.
printf 'export const x = 1;\n' > staged.js
git add staged.js
expect 1 "a staged file whose formatter is missing" sh ./scripts/git-pre-commit.sh
contains "the missing formatter is named" "$work/last.out" "prettier: run npm ci"

# A commit of nothing but web files gives the chain no file at all, which is not
# the same as a file it could not read. A stand-in Prettier that finds nothing
# to change lets the case run without node.
mkdir -p node_modules/.bin
printf '#!/bin/sh\nexit 0\n' > node_modules/.bin/prettier
chmod +x node_modules/.bin/prettier
expect 0 "a commit of web files alone" sh ./scripts/git-pre-commit.sh
rm -rf node_modules

# A setting changes how files nobody staged are formatted, so a commit carrying
# only the setting is checked against every file it governs.
git commit -q --no-verify -m "Add a script the hook did not read" \
    -m "The setting staged below has to reach this file."
printf '{}\n' > .prettierrc.json
git add .prettierrc.json
expect 1 "a formatter setting alone widens the check" sh ./scripts/git-pre-commit.sh
contains "the widened check reaches the unstaged file" "$work/last.out" "prettier"
git rm -q --cached .prettierrc.json
rm -f .prettierrc.json

# The edges of what is staged, after the cases pre-commit's own stash dance
# guards against: a file staged in part, `commit -a` and its temporary index, a
# rename, a name past ASCII, one git still quotes, and an intent-to-add.
part=$(printf '#!/bin/sh\nif true; then\n' | git hash-object -w --stdin)
printf '#!/bin/sh\necho fixed\n' > part.sh
git update-index --add --cacheinfo "100644,$part,part.sh"
expect 1 "the staged half of a file is what is judged" sh ./scripts/git-pre-commit.sh
git rm -q -f --cached part.sh
rm -f part.sh

printf 'untidy \n' > notes.txt
expect 1 "commit -a judges what -a stages" git commit -q -a \
    -m "Change the notes" -m "Trailing space the hook has to see through -a."
git checkout -q -- notes.txt

git mv staged.js renamed.js
expect 1 "a renamed file is judged under its new name" sh ./scripts/git-pre-commit.sh
contains "the renamed file reaches its formatter" "$work/last.out" "prettier"
git mv renamed.js staged.js

accented=$(printf 'caf\303\251.js')
printf 'export const x = 1;\n' > "$accented"
git add "$accented"
expect 1 "a name past ASCII is judged" sh ./scripts/git-pre-commit.sh
contains "that name reaches its formatter" "$work/last.out" "prettier"
git rm -q --cached "$accented"
rm -f "$accented"

printf 'x\n' > 'say"hi.txt'
git add 'say"hi.txt'
expect 1 "a name git quotes is refused by name" sh ./scripts/git-pre-commit.sh
contains "the refusal says why" "$work/last.out" "cannot be checked"
git rm -q --cached 'say"hi.txt'
rm -f 'say"hi.txt'

printf 'export const later = 1;\n' > later.js
git add -N later.js
expect 0 "an intent-to-add file carries nothing to judge" sh ./scripts/git-pre-commit.sh
git rm -q --cached later.js
rm -f later.js

echo "  HOOKS   pre-push"
git commit -q --allow-empty -m "Add the scratch repository" \
    -m "The push tests need a commit that the rules accept." > /dev/null 2>&1 \
    || fail "the hooks refused a commit that follows the rules"
git init -q --bare "$work/remote.git"
git remote add origin "$work/remote.git"
expect 0 "a branch whose messages pass" git push -q origin HEAD:refs/heads/main
contains "the hook behind ours read the same refs" "$work/local.in" "refs/heads/main"
git commit -q --allow-empty --no-verify -m "wip: skipped the hook"
expect 1 "a commit that skipped the hook" git push -q origin HEAD:refs/heads/main

# The same message is skipped only when Dependabot wrote it, and only by the
# author address, so a person's commit on its branch is still read.
bump="Bump a-crate-with-a-long-name from 1.0.0 to 1.0.1 in the rust group"
git commit -q --allow-empty --no-verify -m "$bump" \
    --author "dependabot[bot] <49699333+dependabot[bot]@users.noreply.github.com>"
expect 0 "a Dependabot commit is not held to the rules" \
    sh -c 'git rev-parse HEAD | ./scripts/check-commit-log.sh'
git commit -q --allow-empty --no-verify -m "$bump"
expect 1 "the same message from anyone else is" \
    sh -c 'git rev-parse HEAD | ./scripts/check-commit-log.sh'

# The same address decides whether a pull request's formatting drift is a bot's
# to report: only when every commit is Dependabot's, and there is one.
expect 0 "Dependabot's commits alone are a bot's" \
    sh -c 'git rev-parse HEAD~1 | ./scripts/check-commit-log.sh --bot-only'
expect 1 "a person's commit beside them is not" \
    sh -c 'git rev-list HEAD~2..HEAD | ./scripts/check-commit-log.sh --bot-only'
expect 1 "no commit at all is not" \
    sh -c ': | ./scripts/check-commit-log.sh --bot-only'

# The pushes above went to main of a remote that is not upstream and passed,
# because until now this clone had no upstream remote to compare against. It
# gets one here, never fetched: its tracking ref is written by hand, so no case
# touches the network. An orphan branch keeps the commits above, whose messages
# fail on purpose, out of these pushes.
previous=$(git symbolic-ref --short HEAD)
git switch -q --orphan pushed-from-main
git commit -q --allow-empty -m "Start the fork" \
    -m "Upstream has this commit, so a fork may sync it."
git remote add upstream https://github.com/sysprog21/codetrial
git update-ref refs/remotes/upstream/main HEAD
git init -q --bare "$work/fork.git"
git remote add fork "$work/fork.git"
expect 0 "a fork's main synced with upstream" git push -q fork HEAD:refs/heads/main
git commit -q --allow-empty -m "Change something upstream lacks" \
    -m "The pull request for this belongs on a topic branch."
expect 1 "new work on a fork's main" git push -q fork HEAD:refs/heads/main
expect 0 "the same work on a topic branch" git push -q fork HEAD:refs/heads/topic

# Upstream's own main takes new work, which is how it gets merged. Through the
# hook with the arguments git would pass, because the push itself would leave
# this machine.
#
# Invoked through `expect`, for the same reason `message` carries the directive.
# shellcheck disable=SC2317,SC2329
push_main()
{
    printf 'refs/heads/main %s refs/heads/main %040d\n' "$(git rev-parse HEAD)" 0 \
        | "$hooks/pre-push" "$1" "$2"
}
expect 0 "new work pushed to upstream's main" \
    push_main upstream https://github.com/sysprog21/codetrial

# The URL is matched at the host, so a look-alike is not upstream, and GitHub's
# SSH fallback on port 443 still is, as a push target and as the remote the
# fork's main is compared against.
expect 1 "new work on main of a look-alike host" \
    push_main impostor https://notgithub.com/sysprog21/codetrial
ssh443=ssh://git@ssh.github.com:443/sysprog21/codetrial
expect 0 "new work pushed to upstream's main over port 443" \
    push_main upstream "$ssh443"
git remote set-url upstream "$ssh443"
expect 1 "new work on a fork's main, upstream over port 443" \
    git push -q fork HEAD:refs/heads/main
git switch -q "$previous"

# Linked worktrees share .git/hooks. The installed wrapper must therefore find
# the worktree that runs it, rather than remain a link into this checkout.
git worktree add -q -b linked "$work/linked" || exit 1
cp -R scripts "$work/linked/" || exit 1
rm -f "$hooks/pre-commit"
(cd "$work/linked" && ./scripts/install-git-hooks.sh > "$work/linked.out") || exit 1
contains "linked installer installs its hook" "$work/linked.out" "HOOK    pre-commit"
cases=$((cases + 1))
printf '#!/bin/sh\nexit 1\n' > scripts/git-pre-commit.sh
(cd "$work/linked" \
    && printf 'linked\n' > linked.txt \
    && git add linked.txt \
    && git commit -q -m "Add the linked worktree" \
        -m "The hook must resolve to this worktree's scripts." > /dev/null) \
    || fail "a linked worktree runs its own hook"

# A commit that finds no hook passes too, so the case above cannot tell its own
# hook from none. A linked worktree is where git answers --git-path with an
# absolute path, and Git for Windows spells it with a drive letter, which the
# installer and the hook both once took for relative: the installer wrote into a
# directory under the worktree, and the hook could not copy the index.
expect 0 "the linked installer writes the shared hook" test -f "$hooks/pre-commit"
printf 'tidy\n' > "$work/linked/tidy.txt"
git -C "$work/linked" add tidy.txt
# The sh -c body expands its own arguments.
# shellcheck disable=SC2016
expect 0 "a linked worktree judges what it staged" \
    sh -c 'cd "$1" && sh ./scripts/git-pre-commit.sh' sh "$work/linked"
printf 'untidy \n' > "$work/linked/tidy.txt"
git -C "$work/linked" add tidy.txt
# shellcheck disable=SC2016
expect 1 "a linked worktree refuses what it staged" \
    sh -c 'cd "$1" && sh ./scripts/git-pre-commit.sh' sh "$work/linked"
contains "the refusal is the staged content's" "$work/last.out" "trailing whitespace"

if [ "$failures" -eq 0 ]; then
    printf '  HOOKS   %d checks passed\n' "$cases"
    exit 0
fi

printf '  HOOKS   %d of %d checks failed\n' "$failures" "$cases" >&2
exit 1

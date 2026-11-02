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
cp "$ROOT"/scripts/git-*.sh "$ROOT/scripts/git-commit-msg.awk" \
    "$ROOT/scripts/check-commit-log.sh" "$ROOT/scripts/check-secrets.py" \
    "$ROOT/scripts/check-interactive-review.py" "$ROOT/scripts/hashchain.py" \
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

# Nested commits exercise hooks without launching this suite again. A setting of
# this scratch repository, so nothing exported by a user can switch it.
git config codetrial.hookSuite true

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

# The release-tag refspec is the installer's to add and to take back, and it
# must leave the remote's own fetch setting alone.
git remote add origin "$work/refspec.git"
expect 0 "the installer adds the release refspec" ./scripts/install-git-hooks.sh
expect 0 "the release refspec is configured" \
    sh -c 'git config --get-all remote.origin.fetch | grep -qxF "+refs/tags/*latest:refs/tags/*latest"'
git config --add remote.origin.fetch '+refs/tags/latest:refs/tags/latest'
git config --add remote.origin.fetch '+refs/tags/desktop-latest:refs/tags/desktop-latest'
expect 0 "reinstalling migrates the earlier exact mappings" ./scripts/install-git-hooks.sh
expect 1 "the exact mappings are removed" sh -c \
    'git config --get-all remote.origin.fetch | grep -qE "^[+]refs/tags/(desktop-)?latest:"'
git config --add remote.origin.fetch '+refs/tags/latest:refs/tags/latest'
expect 0 "uninstalling removes only the release refspec" sh -c \
    './scripts/install-git-hooks.sh --uninstall > /dev/null \
    && ! git config --get-all remote.origin.fetch | grep -qF latest \
    && git config --get-all remote.origin.fetch | grep -qxF "+refs/heads/*:refs/remotes/origin/*"'
git remote remove origin
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
: > "$merge_head"
printf 'bad subject\n' > "$work/merge.msg"
expect 0 "a merge runs no script of ours" "$hooks/commit-msg" "$work/merge.msg"
rm -f "$merge_head"

echo "  HOOKS   commit-msg"
expect 0 "a message that follows the rules" \
    message "Add the scratch repository

One paragraph saying why this exists, wrapped inside the
limit the hook enforces."
expect 0 "the rules can be printed" sh "$ROOT/scripts/git-commit-msg.sh" --rules
# Each with a body, so the marker is the only thing the rules can refuse.
body="

One paragraph saying why this exists."
for marker in fixup squash amend; do
    expect 1 "a $marker! subject" message "$marker! Add the scratch repository$body"
    contains "the $marker! refusal names the fold" "$work/last.out" "git commit --amend"
done
for verb in Wiring Wires Lets Wrote Builds; do
    expect 1 "the non-imperative $verb" message "$verb the scratch repository$body"
done
for verb in Seed Alias Bring; do
    expect 0 "the imperative $verb" message "$verb the scratch repository$body"
done
expect 0 "a merge subject is left alone" message "Merge branch 'topic' into main"
expect 1 "a lowercase subject" message "add the scratch repository"
expect 1 "a past-tense subject" message "Added the scratch repository"
expect 1 "a subject ending in a period" message "Add the scratch repository."
# Single quotes on purpose: the backtick is the subject of the case.
# shellcheck disable=SC2016
expect 1 "a subject with a backtick" message 'Add the `scratch` repository'
expect 1 "a conventional-commit prefix" message "feat: add the scratch repository"
expect 1 "a one-word subject" message "Scratch"

# A subject that only names a file says where, not what, and the refusal says
# so.
for subject in \
    "Update web/interview.js" \
    "Update web/interview.js and web/app.js" \
    "Fix app.js" \
    "Modify README.md" \
    "Update README" \
    "Tweak the scripts/test.sh" \
    "Update docs/" \
    "Update .gitignore" \
    "Update src/web/ and web/app.js"; do
    expect 1 "the file-only subject $subject" message "$subject$body"
    contains "the refusal says why" "$work/last.out" "where the change is, not what it does"
done
for subject in \
    "Update web/interview.js to recheck setup" \
    "Fix the app.js crash on empty input" \
    "Update the lobby" \
    "Update CI/CD" \
    "Fix input/output" \
    "Correct Math.floor" \
    "Fix Promise.all" \
    "Remove web/old.js" \
    "Rename a.js to b.js" \
    "Move scripts/a.sh scripts/b.sh" \
    "Add scripts/wait-for-server.sh"; do
    expect 0 "the subject $subject" message "$subject$body"
done
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

# The wrap limit gives way for exactly the tokens a wrap would have to break.
url="https://github.com/sysprog21/codetrial/blob/main/docs/provider-cost-and-degradation.md"
expect 0 "a link alone on a line past 72 characters" message "Add the repository

See $url
for the reasoning."
expect 0 "a file name longer than a line, in a sentence" message "Add the repository

The change is in scripts/some-directory/another-directory/yet-another-directory/a-rather-long-file-name.sh today."
expect 0 "a long path with a line number" message "Add the repository

See scripts/some-directory/another-directory/yet-another-directory/a-rather-long-file-name.sh:123 for it."
expect 1 "a link that fits on a line of its own, beside prose" message "Add the repository

See https://github.com/sysprog21/codetrial/pull/123 for the discussion that led here."
expect 0 "a long human co-author trailer" message "Add the repository

One paragraph saying why this exists.

Co-authored-by: Ada Lovelace <1234567+ada-the-countess@users.noreply.github.com>"
expect 1 "a sentence with a colon is not a trailer" message "Add the repository

Note: this sentence is long enough on its own to run past the limit that the hook enforces."
expect 1 "short file names do not stretch the limit" message "Add the repository

This sentence uses foo.rs and bar.rs and baz.rs and qux.rs and needs wrapping."
expect 1 "prose past 72 characters beside a link" message "Add the repository

This sentence is long enough on its own to run past the limit, and it also names $url
so only the sentence is at fault."
expect 1 "a body line past 72 characters with no link" message "Add the repository

This line exists to run past the seventy-two column limit that the hook is
supposed to enforce, and it does."

# A message whose last line is the case: the one place a credit or a closing
# reference does its work.
with_last()
{
    with_status=$1
    with_label=$2
    expect "$with_status" "$with_label" message "Add the scratch repository

One paragraph saying why this exists.

$3"
}

# An attribution names a tool, not a collaborator. A person who shares a tool's
# name, or works where it is made, is still credited.
for credit in \
    "Co-Authored-By: Claude <noreply@anthropic.com>" \
    "Co-authored-by: Agent <codex@openai.com>" \
    "Co-authored-by: Copilot <198982749+Copilot@users.noreply.github.com>" \
    "Co-authored-by: Copilot <198982749+copilot[bot]@users.noreply.github.com>" \
    "Generated with [Claude Code](https://claude.com/claude-code)" \
    "Assisted-by: Claude:claude-opus-5" \
    "Assisted-by: Claude Code" \
    "Co-authored-by: Claude <claude@anthropic.com>" \
    "Co-authored-by: Claude Opus 4.5 (1M context) <noreply@anthropic.com>" \
    "Co-authored-by: ChatGPT <noreply@openai.com>" \
    "Co-authored-by: aider (gpt-5) <noreply@aider.chat>" \
    "Co-authored-by: Amp <amp@ampcode.com>" \
    "Co-authored-by: Jules <161369871+google-labs-jules[bot]@users.noreply.github.com>" \
    "Co-authored-by: Bot <161369871+google-labs-jules[bot]@users.noreply.github.com>" \
    "Signed-off-by: Bot <noreply@anthropic.com>" \
    "AI-Assisted-By: Claude" \
    "Generated-by: Aider" \
    "Assisted-by: Bot <codex@openai.com>" \
    "Session: https://claude.ai/code/session_01abc" \
    "Generated by Codex." \
    "Assisted-by: Claude <claude@anthropic.com>" \
    "Generated-by: ChatGPT <ai@example.com>" \
    "Generated with Amp" \
    "Generated with Jules" \
    "Created with Cursor" \
    "Made by an AI"; do
    with_last 2 "the AI attribution $credit" "$credit"
    contains "the refusal names the attribution" "$work/last.out" "AI attribution"
done
for credit in \
    "Co-authored-by: Claude Shannon <claude@example.com>" \
    "Co-authored-by: Ada Lovelace <ada@anthropic.com>" \
    "Assisted-by: Ada Lovelace <ada@example.com>" \
    "Assisted-by: Claude Shannon <claude@example.com>" \
    "Co-authored-by: Aider Smith <aider@example.com>" \
    "Co-authored-by: Amp Nguyen <amp@example.com>" \
    "Co-authored-by: Devin <devin@example.com>" \
    "Co-authored-by: Jules <jules@example.org>" \
    "Created with cursor (0, 0) to cover empty input." \
    "Created with an AI-generated fixture for the parser." \
    "Made with cursor positions saved in the file." \
    "Written by Claude Shannon in 1948." \
    "Skip files generated by codex in the walk."; do
    with_last 0 "the credit $credit" "$credit"
done
expect 2 "an attribution on a merge, which the style rules leave alone" \
    message "Merge branch 'topic' into main

Generated by Codex"
expect 3 "an autosquash subject that also credits a tool" \
    message "fixup! Add the scratch repository

Generated by Codex"
expect 4 "a directory where a message should be" \
    sh "$ROOT/scripts/git-commit-msg.sh" "$work"

# The status is documented in scripts/git-commit-msg.sh: 3 is a broken rule and
# a credit together, and 4 is a message or a rules file that cannot be used.
expect 4 "a message file that is not there" \
    sh "$ROOT/scripts/git-commit-msg.sh" "$work/no-such.msg"
mv scripts/git-commit-msg.awk "$work/git-commit-msg.awk"
expect 4 "a rules file that is not there" message "Add the scratch repository

One paragraph saying why this exists."
printf 'this is { not awk\n' > scripts/git-commit-msg.awk
expect 4 "a rules file awk cannot parse, whose status 2 is not a credit" message "Add the scratch repository

One paragraph saying why this exists."
mv "$work/git-commit-msg.awk" scripts/git-commit-msg.awk
expect 3 "a broken rule and a credit in one message" \
    message "add the scratch repository

One paragraph saying why this exists.

Co-Authored-By: Claude <noreply@anthropic.com>"

# A message a commit already holds had no scissors cut and no comments stripped
# by git, so a line that looks like either hides nothing. In the editor the same
# text is cut, because git cuts it before the commit exists.
# shellcheck disable=SC2317,SC2329
stored()
{
    printf '%s\n' "$1" | sh "$ROOT/scripts/git-commit-msg.sh" --stored -
}
under_scissors="Add the scratch repository

One paragraph saying why this exists.

-------- >8 --------
Co-authored-by: Claude <noreply@anthropic.com>"
expect 2 "a credit under a scissors-looking line, in a stored message" \
    stored "$under_scissors"
expect 0 "the same text in the editor, where git cuts it" \
    message "$under_scissors"
under_comment="Add the scratch repository

One paragraph saying why this exists.

# Generated with Claude Code"
expect 2 "a credit on a comment-looking line, in a stored message" \
    stored "$under_comment"
expect 0 "the same text in the editor, where git strips it" \
    message "$under_comment"
# A Greek letter, as in the ASCII case above: the byte width is the point.
expect 1 "a non-ASCII comment-looking line, in a stored message" \
    stored "Add the scratch repository

One paragraph saying why this exists.

# α"

# A commit made in GitHub's web editor holds CRLF. It is judged on its text, so
# the one thing wrong with this one is the subject, and the refusal says so.
crlf=$(printf 'Update web/interview.js\r\n\r\nRecheck setup before joining.\r\n')
expect 1 "a CRLF message from the web editor, refused for its subject alone" \
    stored "$crlf"
contains "the refusal is about the subject" "$work/last.out" "only names a file"
absent "no complaint about the line endings" "$work/last.out" "printable ASCII"
expect 0 "a CRLF message with a subject that says what changed" \
    stored "$(printf 'Recheck setup after the picker closes\r\n\r\nDevices can change while it is open.\r\n')"
expect 4 "no message given" sh "$ROOT/scripts/git-commit-msg.sh"
git config commit.cleanup verbatim
expect 0 "a comment kept as written under verbatim cleanup" \
    message "Add the scratch repository

# Please enter the commit message for your changes."
git config --unset commit.cleanup
expect 0 "three paragraphs are a note and not a failure" message "Add the scratch repository

First paragraph saying why.

Second paragraph saying more.

Third paragraph saying still more."
contains "the note says how many" "$work/last.out" "3 paragraphs"

# Close finishes an issue and goes last; Closes is accepted. A Refs is context
# in the description.
with_last 1 "a Refs line last" "Refs #12"
contains "the Refs refusal names the alternative" "$work/last.out" "Close #N"
with_last 1 "a Refs with a repository last" "Refs: sysprog21/codetrial#12"
with_last 1 "a References line last, trailing blanks and all" "References #12

"
with_last 1 "several references on the last line" "Refs #11, #12 and sysprog21/codetrial#13."
with_last 1 "an Oxford-comma list of references last" "Refs #11, #12, and #13"
with_last 1 "an issue URL last" "Refs https://github.com/sysprog21/codetrial/issues/12"
with_last 1 "a pull request URL with a fragment last" \
    "Refs https://github.com/sysprog21/codetrial/pull/12#issuecomment-5"
with_last 1 "references separated by a semicolon last" "Refs: #12; #13"
with_last 0 "a sentence that starts with a reference, last" \
    "Refs #12 explains why the old behavior was retained."
with_last 0 "a Close line last" "Close #12"
with_last 0 "a Closes line last, which is accepted too" "Closes #12"
expect 0 "a Refs in the description with a Close last" message "Add the scratch repository

One paragraph saying why this exists. Refs #11 is the first half.
Refs #10 explains the cap.

Close #12"
expect 0 "a Refs in the middle of a paragraph" message "Add the scratch repository

One paragraph saying why this exists, as Refs #12 describes it,
and nothing more."

echo "  HOOKS   prepare-commit-msg"
printf '' > "$work/empty.msg"
expect 0 "an empty message gets the rules" \
    sh ./scripts/git-prepare-commit-msg.sh "$work/empty.msg"
contains "the rules are rendered" "$work/empty.msg" "Commit rules"
contains "the enforced list is what is shown" "$work/empty.msg" "imperative mood"

cp "$work/empty.msg" "$work/once.msg"
expect 0 "reopening the editor preserves the existing guidance" \
    sh ./scripts/git-prepare-commit-msg.sh "$work/empty.msg"
expect 0 "guidance is not duplicated" cmp -s "$work/empty.msg" "$work/once.msg"
contains "guidance includes a change summary" "$work/empty.msg" "Change summary"

printf '\n# ------------------------ >8 ------------------------\n+Commit rules, enforced by scripts/git-commit-msg.sh:\n' \
    > "$work/quoted.msg"
expect 0 "a diff quoting the guidance heading is not the guidance" \
    sh ./scripts/git-prepare-commit-msg.sh "$work/quoted.msg"
contains "guidance is added above a diff that quotes it" "$work/quoted.msg" "# Commit rules"

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

printf 'ghp_%036d\n' 1 > credential.txt
git add credential.txt
expect 1 "the pre-commit hook refuses a staged credential" sh ./scripts/git-pre-commit.sh
contains "the pre-commit refusal is redacted" "$work/last.out" "GitHub token (redacted)"
git rm -q --cached credential.txt
rm credential.txt

echo "  HOOKS   path-triggered checks"
printf '#!/bin/sh\necho hook-suite-fixture\nexit 1\n' > scripts/test-git-hooks.sh
git add scripts/test-git-hooks.sh
expect 1 "a changed hook suite runs and refuses failures" \
    env GIT_CONFIG_PARAMETERS="'codetrial.hookSuite=false'" sh ./scripts/git-pre-commit.sh
contains "the hook suite was invoked" "$work/last.out" "hook-suite-fixture"
printf '#!/bin/sh\necho hook-suite-fixture\nexit 0\n' > scripts/test-git-hooks.sh
expect 1 "an unstaged suite fix does not pass the staged suite" \
    env GIT_CONFIG_PARAMETERS="'codetrial.hookSuite=false'" sh ./scripts/git-pre-commit.sh
git add scripts/test-git-hooks.sh
expect 0 "a passing staged hook suite permits the commit" \
    env GIT_CONFIG_PARAMETERS="'codetrial.hookSuite=false'" sh ./scripts/git-pre-commit.sh
git rm -q --cached scripts/test-git-hooks.sh
rm scripts/test-git-hooks.sh

mkdir -p .claude/skills/codetrial-verify
printf '# Fixture skill\n' > .claude/skills/codetrial-verify/SKILL.md
printf 'print("skill-refs-fixture")\nraise SystemExit(1)\n' > scripts/check-skill-refs.py
git add .claude/skills/codetrial-verify/SKILL.md scripts/check-skill-refs.py
expect 1 "skill changes run the reference checker" sh ./scripts/git-pre-commit.sh
contains "the reference checker was invoked" "$work/last.out" "skill-refs-fixture"
printf 'print("skill-refs-fixture")\n' > scripts/check-skill-refs.py
expect 1 "an unstaged reference fix does not pass staged content" sh ./scripts/git-pre-commit.sh
git add scripts/check-skill-refs.py
expect 0 "passing staged skill references permit the commit" sh ./scripts/git-pre-commit.sh
git rm -q --cached .claude/skills/codetrial-verify/SKILL.md scripts/check-skill-refs.py
rm -rf .claude/skills/codetrial-verify
rm scripts/check-skill-refs.py

echo "  HOOKS   pre-push"
git commit -q --allow-empty -m "Add the scratch repository" \
    -m "The push tests need a commit that the rules accept." > /dev/null 2>&1 \
    || fail "the hooks refused a commit that follows the rules"
git init -q --bare "$work/remote.git"
git remote add origin "$work/remote.git"
./scripts/install-git-hooks.sh > /dev/null
git tag latest
git tag desktop-latest
git -c core.hooksPath=/dev/null push -q origin refs/tags/latest refs/tags/desktop-latest
expect 0 "a branch whose messages pass" git push -q origin HEAD:refs/heads/main
contains "the hook behind ours read the same refs" "$work/local.in" "refs/heads/main"
for tag in latest desktop-latest; do
    git update-ref "refs/tags/$tag" HEAD~1
done
expect 0 "push refreshes moving release tags" git push -q origin HEAD:refs/heads/main
for tag in latest desktop-latest; do
    expect 0 "push updated $tag" test "$(git rev-parse "$tag")" = "$(git rev-parse HEAD)"
    git update-ref "refs/tags/$tag" HEAD~1
done
# A minimal PATH models stock macOS without either GNU timeout command.
mkdir "$work/deadline-bin"
for tool in git cat awk mktemp rm sh; do
    ln -s "$(command -v "$tool")" "$work/deadline-bin/$tool"
done
git update-ref refs/tags/latest HEAD~1
expect 0 "missing timeout tools do not refuse the push" \
    env PATH="$work/deadline-bin" "$hooks/pre-push" origin "$work/remote.git" < /dev/null
contains "missing timeout tools say the sync was skipped" "$work/last.out" "release tags not synchronized"
expect 0 "missing timeout tools leave release tags unchanged" \
    test "$(git rev-parse latest)" = "$(git rev-parse HEAD~1)"
expect 0 "another remote does not require timeout tools" \
    env PATH="$work/deadline-bin" "$hooks/pre-push" backup "$work/backup.git" < /dev/null
for tool in grep sed; do
    ln -s "$(command -v "$tool")" "$work/deadline-bin/$tool"
done
# The generated tool reads its own arguments.
# shellcheck disable=SC2016
printf '#!/bin/sh\n[ "$1 $2 $3" = "-k 5 55" ] || exit 2\nshift 3\nexec "$@"\n' > "$work/deadline-bin/gtimeout"
chmod +x "$work/deadline-bin/gtimeout"
expect 0 "gtimeout receives the one-minute bound and refreshes tags" \
    env PATH="$work/deadline-bin" "$hooks/pre-push" origin "$work/remote.git" < /dev/null
expect 0 "gtimeout path refreshed the moving tag" test "$(git rev-parse latest)" = "$(git rev-parse HEAD)"
for tag in latest desktop-latest; do
    git update-ref "refs/tags/$tag" HEAD~1
done
git --git-dir="$work/remote.git" update-ref refs/tags/v1-latest "$(git rev-parse HEAD)"
git update-ref refs/tags/v1-latest HEAD~1
expect 0 "push synchronization succeeds with an unrelated latest-suffixed tag" \
    git push -q origin HEAD:refs/heads/main
expect 0 "push synchronization preserves unrelated local tags" \
    test "$(git rev-parse v1-latest)" = "$(git rev-parse HEAD~1)"
expect 0 "plain fetch takes a latest-suffixed tag from origin" git fetch origin
expect 0 "the glob refspec updates a latest-suffixed tag" \
    test "$(git rev-parse v1-latest)" = "$(git rev-parse HEAD)"
git branch --set-upstream-to=origin/main > /dev/null
expect 0 "an up-to-date pull refreshes release tags" git pull --ff-only
for tag in latest desktop-latest; do
    expect 0 "pull updated $tag" test "$(git rev-parse "$tag")" = "$(git rev-parse HEAD)"
done
git --git-dir="$work/remote.git" update-ref -d refs/tags/desktop-latest
expect 0 "a missing release tag does not refuse the push" git push -q origin HEAD:refs/heads/main
expect 0 "a missing release tag does not fail a plain fetch" git fetch origin
git --git-dir="$work/remote.git" update-ref refs/tags/desktop-latest "$(git rev-parse HEAD)"
git remote set-url origin "$work/unreachable.git"
expect 1 "an unreachable origin refuses a push to origin" "$hooks/pre-push" origin "$work/remote.git" < /dev/null
contains "unreachable origin names connectivity" "$work/last.out" "Check origin connectivity"
expect 0 "an unreachable origin does not block another remote" "$hooks/pre-push" backup "$work/backup.git" < /dev/null
git remote set-url origin "$work/remote.git"
git init -q --bare "$work/tagless-fork.git"
git remote set-url origin "$work/tagless-fork.git"
expect 0 "a fork without release tags accepts the push" "$hooks/pre-push" origin "$work/tagless-fork.git" < /dev/null
expect 0 "a fork without release tags accepts a plain fetch" git fetch origin
git remote set-url origin "$work/remote.git"
git update-ref refs/tags/latest HEAD~1
printf '%s %s %s %s\n' refs/tags/latest "$(git rev-parse HEAD~1)" refs/tags/latest "$(git rev-parse HEAD)" \
    | "$hooks/pre-push" origin "$work/remote.git" > /dev/null 2>&1
expect 0 "a push of a release tag keeps the local value" test "$(git rev-parse latest)" = "$(git rev-parse HEAD~1)"
for tag in latest desktop-latest; do
    git update-ref "refs/tags/$tag" HEAD~1
done
expect 0 "an explicit branch pull succeeds" git pull --ff-only origin main
for tag in latest desktop-latest; do
    expect 0 "explicit pull leaves $tag unchanged" test "$(git rev-parse "$tag")" = "$(git rev-parse HEAD~1)"
done
expect 0 "publication restored allows synchronization" git push -q origin HEAD:refs/heads/main
printf 'ghp_%036d\n' 2 > credential.txt
git add credential.txt
git -c core.hooksPath=/dev/null commit -q -m "Add a credential fixture" \
    -m "The hook must inspect outgoing commits made without local checks."
expect 1 "the pre-push hook refuses credentials in outgoing commits" \
    git push -q origin HEAD:refs/heads/main
contains "the pre-push refusal names the secret check" "$work/last.out" "outgoing history failed the secret check"
git -c core.hooksPath=/dev/null push -q origin HEAD:refs/heads/leak
git init -q --bare "$work/backup.git"
expect 0 "a new remote does not rescan history another remote holds" \
    git push -q "$work/backup.git" HEAD:refs/heads/main
git push -q origin :refs/heads/leak
git update-ref -d refs/remotes/origin/leak
git reset -q --hard HEAD~1

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

# A small topic head without a recorded review is pushed with a note; one it
# recorded is pushed without. A checkout without the skill or an upstream review
# policy, as every push above, gets no note at all.
absent "no note without the review skill" "$work/last.out" "no review on record"
mkdir -p .claude/skills/codetrial-review "$(git rev-parse --git-common-dir)/codetrial-review"
: > .claude/skills/codetrial-review/SKILL.md
cp "$ROOT/.claude/skills/codetrial-review/commit-sizes.py" .claude/skills/codetrial-review/
git commit -q --allow-empty -m "Change the topic further" \
    -m "A head nobody has reviewed yet, pushed to the same topic branch."
expect 0 "an unreviewed topic head is pushed" git push -q fork HEAD:refs/heads/topic
contains "the push says the head has no review" "$work/last.out" "no review on record"
git commit -q --allow-empty -m "Change the topic once more" \
    -m "A head the review skill recorded before it was pushed."
git rev-parse HEAD >> "$(git rev-parse --git-common-dir)/codetrial-review/reviewed"
expect 0 "a reviewed topic head is pushed" git push -q fork HEAD:refs/heads/topic
absent "a reviewed head gets no note" "$work/last.out" "no review on record"
git commit -q --allow-empty -m "Change the topic for the note cases" \
    -m "An unreviewed head for the pushes that must not get a note."

expect 0 "an unreviewed head pushed to upstream's main" \
    push_main upstream https://github.com/sysprog21/codetrial
absent "main gets no note" "$work/last.out" "no review on record"
git tag -a -m "A tag of an unreviewed head" unreviewed-tag
expect 0 "an unreviewed head pushed as a tag" \
    git push -q fork refs/tags/unreviewed-tag
absent "a tag gets no note" "$work/last.out" "no review on record"
expect 0 "two unreviewed topic heads in one push" \
    git push -q fork HEAD:refs/heads/topic HEAD:refs/heads/second
contains "the push names the missing review" "$work/last.out" "no review on record"
if [ "$(grep -c "no review on record" "$work/last.out")" -ne 1 ]; then
    fail "one push says it once"
fi
cases=$((cases + 1))
git commit -q --allow-empty --no-verify -m "wip: skipped the hook again"
expect 1 "a refused topic push" git push -q fork HEAD:refs/heads/topic
absent "a refused push gets no note" "$work/last.out" "no review on record"

# One refused ref among several: the unreviewed head beside it is not advised on
# either, since the push as a whole is not going anywhere.
expect 1 "a refused topic push beside an unreviewed one" \
    git push -q fork HEAD:refs/heads/topic HEAD~1:refs/heads/third
absent "a push with a refused ref gets no note" "$work/last.out" "no review on record"
git reset -q --hard HEAD~1
rm -rf .claude

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
expect 0 "a linked worktree judges what it staged" \
    git -C "$work/linked" commit -q -m "Add a tidy file" \
    -m "The shared hook must judge what this worktree staged."
printf 'untidy \n' > "$work/linked/tidy.txt"
git -C "$work/linked" add tidy.txt
expect 1 "a linked worktree refuses what it staged" \
    git -C "$work/linked" commit -q -m "Leave a trailing space" \
    -m "The shared hook must refuse what this worktree staged."
contains "the refusal is the staged content's" "$work/last.out" "trailing whitespace"

if [ "$failures" -eq 0 ]; then
    printf '  HOOKS   %d checks passed\n' "$cases"
    exit 0
fi

printf '  HOOKS   %d of %d checks failed\n' "$failures" "$cases" >&2
exit 1

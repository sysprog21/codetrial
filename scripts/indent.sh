#!/bin/sh

# Apply the formatters, or check that a snapshot is already where they would put
# it.
#
# The one place that says which files are formatted, by which tool and in which
# order. `make indent` runs it with --write, `make indent-check` and
# scripts/test.sh with --check, and scripts/git-pre-commit.sh with --staged, so
# the entry points differ only in the snapshot they judge: the working tree, or
# the index. .claude/skills/codetrial-verify/SKILL.md describes the lanes.
#
# The order is not a preference. commentflow puts a blank line before a comment
# that sits inside a method chain, an array literal or an argument list, and
# rustfmt takes that line back out; running the formatters last lets them have
# the final word. That is also why the check runs the whole chain against a copy
# of the snapshot rather than asking commentflow alone whether it would change
# something: on its own it always would, and the answer would be a gate nobody
# can satisfy. It is why the pre-commit hook goes through here too: it used to
# run rustfmt alone, so a comment commentflow would reflow passed the commit and
# failed in CI.
#
# Style flags are deliberately absent. shfmt reads .editorconfig, which is where
# this repository's shell settings already live, and passing -i here would
# silently override the file for one caller.
#
# A formatter the selected files need is required: a run that cannot format the
# JavaScript it was given must not end by calling the tree clean. Only the gate
# passes --skip-missing, because scripts/test.sh collects each skip into its
# summary and fails CI on any of them; this script then says which lanes were
# not checked instead of calling the tree clean.

set -u

usage()
{
    echo "usage: ${0##*/} [--check [--skip-missing] | --write | --staged DIR]" >&2
    exit 2
}

mode=check
skip_missing=0
while [ $# -gt 0 ]; do
    case "$1" in
        --check) mode=check ;;
        --write) mode=apply ;;
        --skip-missing) skip_missing=1 ;;
        --staged)
            mode=staged
            work=${2:-}
            [ -d "$work" ] || usage
            shift
            ;;
        *) usage ;;
    esac
    shift
done
[ "$skip_missing" -eq 0 ] || [ "$mode" = check ] || usage

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT" || exit 1

# What decides the output besides the files themselves: the formatters'
# settings, Prettier's pinned version, and this script. A commit that changes
# one of them changes how every file formats, so the index check widens from the
# staged files to all of them, and a tree check copies them along. Cargo.toml is
# left out, though rustfmt reads its edition: every dependency bump would
# otherwise recheck the whole tree, and CI catches the rare edition change.
configs='.editorconfig rustfmt.toml ruff.toml .prettierrc.json .prettierignore
package.json scripts/indent.sh'

# Against HEAD the index is this commit's changes; against the empty tree it is
# every file the commit will hold. Either way `diff --cached` and not
# `ls-files`, which also lists a file added with --intent-to-add, whose content
# the commit does not carry and the checkout below writes out empty.
since=
# shellcheck disable=SC2086
if [ "$mode" = staged ] && ! git diff --cached --quiet -- $configs; then
    since=$(git hash-object -t tree /dev/null)
fi
base=$ROOT
[ "$mode" = staged ] && base=$work

# The tree includes untracked but non-ignored files, so a new script is covered
# before it is staged; the index is only what this commit adds or changes,
# unless a setting above changed. Vendored code keeps upstream's formatting and
# edition, so no lane reads it. Only regular files: a formatter handed a symlink
# writes wherever it points, which for a check is outside its copy.
#
# quotePath off, or a name with a byte past ASCII comes back quoted, is not a
# file under that name, and drops out of the check without a word. What git
# still quotes, and a name with a blank, cannot pass through the word splitting
# below; those are kept so the check after the lists can refuse them by name.
list()
{
    if [ "$mode" = staged ]; then
        # shellcheck disable=SC2086
        git -c core.quotePath=false diff --cached --name-only \
            --diff-filter=ACMRT $since -- "$@"
    else
        git -c core.quotePath=false ls-files --cached --others \
            --exclude-standard -- "$@"
    fi | grep -vE '^(web/)?vendor/' | sort -u | while IFS= read -r file; do
        case "$file" in
            \"* | *[[:blank:]]*) printf '%s\n' "$file" ;;
            *) [ -f "$base/$file" ] && ! [ -L "$base/$file" ] \
                && printf '%s\n' "$file" ;;
        esac
    done
}

rust=$(list 'build.rs' 'src/*.rs' 'tests/*.rs')

# The desktop app is a crate of its own, so `cargo fmt` at the root never sees
# it and it needs its manifest named.
desktop=$(list 'desktop/build.rs' 'desktop/src/*.rs' 'desktop/tests/*.rs')
shell=$(list '*.sh')

# tests/ as well as scripts/: the Python under tests/ drives the recording and
# calibration harnesses, and leaving it out of the formatter while `ruff check`
# reads it in the gate is a split nobody can keep track of.
python=$(list 'scripts/*.py' 'tests/*.py')

# The issue forms are in this list because Prettier is the only lane here that
# reads YAML: actionlint takes .github/workflows and stops. An unquoted colon in
# a description is a mapping where YAML wants a scalar, and GitHub answers the
# issue chooser with a parse error for it. Syntax only, so a mistyped key or an
# element `type` the schema does not have formats clean and still breaks the
# chooser: a green lane here is not a rendered form.
web=$(list '*.html' '*.js' '*.mjs' '*.cjs' \
    '.github/ISSUE_TEMPLATE/*.yml' '.github/ISSUE_TEMPLATE/*.yaml')
prettier=$ROOT/node_modules/.bin/prettier

files=$(printf '%s\n%s\n%s\n%s\n' "$rust" "$desktop" "$shell" "$python" | grep -v '^$')

# The same refusal as scripts/git-pre-commit.sh makes for the linters, here for
# the tree, which never goes through the hook.
odd=$(printf '%s\n%s\n' "$files" "$web" | grep -E '^"|[[:blank:]]')
if [ -n "$odd" ]; then
    printf '%s\n' "$odd" | sed 's/^/  /' >&2
    echo "${0##*/}: cannot check the paths above; rename them" >&2
    exit 2
fi
[ -n "$files$web" ] || exit 0

# Decided once, up front, because the chain runs in a subshell and a write that
# stopped halfway for a missing tool would leave a tree no lane agrees with.
# `--help` rather than a PATH lookup, because rustup puts a rustfmt proxy on
# PATH whether or not the component is installed, and the proxy fails when run.
# Prettier is a path, and starting node to ask it costs a tenth of a second.
#
# scripts/test.sh reads the skip line into its summary, so keep its shape.
missing=""
skipped=""
lane()
{
    [ -n "$3" ] || return 1
    case "$1" in
        /*) [ -x "$1" ] && return 0 ;;
        *) "$1" --help > /dev/null 2>&1 && return 0 ;;
    esac
    if [ "$skip_missing" -eq 1 ]; then
        echo "${0##*/}: ${1##*/} not installed, skipping that lane ($2)" >&2
        skipped="$skipped ${1##*/}"
    else
        missing="$missing
  ${1##*/}: $2"
    fi
    return 1
}

lane commentflow "install a release from github.com/sysprog21/commentflow" \
    "$rust$desktop$shell" && use_commentflow=1
lane rustfmt "install Rust with rustup, then rustup component add rustfmt" \
    "$rust$desktop" && use_rustfmt=1
lane ruff "pipx install ruff" "$python" && use_ruff=1
lane shfmt "brew install shfmt, or apt-get install shfmt" "$shell" && use_shfmt=1
lane "$prettier" "run npm ci" "$web" && use_prettier=1

if [ -n "$missing" ]; then
    echo "${0##*/}: these files need formatters that are not installed:$missing" >&2
    echo "Nothing was formatted or checked." >&2
    exit 2
fi

# Run in the current directory, on the lists above. Both Rust branches are the
# same rustfmt with the manifest's edition, and rustfmt.toml being there keeps a
# contributor's ~/.rustfmt.toml out of the answer. The tree takes `cargo fmt`,
# which formats each file of the crate once: rustfmt handed every file follows
# `mod` from the roots and formats the children a second time, 3.2s against 1.2s
# here. A commit takes rustfmt on what it staged, so one file does not pay for
# the crate. Each crate is formatted against its own manifest.
#
# rustfmt_crate MANIFEST FILES...
rustfmt_crate()
{
    manifest=$1
    shift
    [ $# -gt 0 ] || return 0
    if [ "$mode" != staged ]; then
        cargo fmt --manifest-path "$manifest"
    else
        edition=$(sed -n 's/^edition *= *"\([0-9]*\)".*/\1/p' "$manifest" | sed -n 1p)
        rustfmt --edition "${edition:-2024}" "$@"
    fi
}

format()
{
    if [ -n "${use_commentflow:-}" ]; then
        # shellcheck disable=SC2086
        commentflow $rust $desktop $shell || return 1
    fi
    if [ -n "${use_rustfmt:-}" ]; then
        # shellcheck disable=SC2086
        rustfmt_crate Cargo.toml $rust || return 1
        # shellcheck disable=SC2086
        rustfmt_crate desktop/Cargo.toml $desktop || return 1
    fi
    if [ -n "${use_ruff:-}" ]; then
        # shellcheck disable=SC2086
        ruff format --quiet $python || return 1
    fi
    if [ -n "${use_shfmt:-}" ]; then
        # shellcheck disable=SC2086
        shfmt -w $shell || return 1
    fi
}

# Prettier shares no file with the chain above, so it needs neither the order
# nor the copy: the tree check reads the tree in place, which keeps the path
# stable enough for --cache to skip files unchanged since the last run, and it
# runs alongside the chain rather than after it. The cache lives under target/
# because `npm ci` empties node_modules, and a staged run, whose directory is
# new every time, gets none. $1 is --write or --list-different.
pretty()
{
    [ -n "${use_prettier:-}" ] || return 0
    cache="--cache --cache-strategy content --cache-location target/prettier-cache"
    [ "$mode" = staged ] && cache=
    # shellcheck disable=SC2086
    "$prettier" $cache "$1" $web
}

if [ "$mode" = apply ]; then
    pretty --write > /dev/null &
    format
    chain=$?
    wait $! && [ "$chain" -eq 0 ]
    exit
fi

# The check writes too, just not here: the chain runs against a copy so that a
# dirty tree is never rewritten by a gate. For the tree, the settings listed
# above come along, so the copy formats the way the tree itself would. For the
# index, the caller hands over a checkout of the whole index, not only the
# staged paths, read through a GIT_INDEX_FILE it pinned: rustfmt follows `mod`
# declarations out of the file it is given, and Prettier finds its config and
# ignore file there. The directory is formatted in place, so the caller runs
# this last. Nothing is ever added to the index, so an unstaged edit neither
# fails the commit nor rides along in it.
#
# This script's own notes go in a directory of their own rather than beside the
# files, where a tracked file of the same name would be overwritten.
notes=$(mktemp -d) || exit 2
trap 'rm -rf "$notes"' EXIT HUP INT TERM
if [ "$mode" = check ]; then
    work=$(mktemp -d "$notes/tree.XXXXXX") || exit 2

    # One tar pipe rather than a mkdir and a cp per file: 81 files was 162
    # process spawns and most of this gate's wall clock. Cargo.toml because
    # `cargo fmt` reads the manifest to find the crate, and the desktop manifest
    # for the same reason. A setting the tree does not have is left out, or tar
    # would refuse the whole copy over it.
    copy=$files
    [ -z "$desktop" ] || copy="$copy
desktop/Cargo.toml"
    for setting in $configs; do
        [ -f "$setting" ] && copy="$copy
$setting"
    done
    printf '%s\n' "$copy" Cargo.toml \
        | tar -cf - -T - \
        | (cd "$work" && tar -xf -) || exit 2
fi

# A failed chain still waits for Prettier, so it does not outlive the script
# writing into a directory the trap has already removed. Exit status 1 from
# --list-different means drift, which the file list below reports.
(cd "$base" && pretty --list-different) > "$notes/prettier" &
(cd "$work" && format)
chain=$?
wait $!
[ $? -le 1 ] && [ "$chain" -eq 0 ] || exit 2

# For the index, blob ids in two processes rather than `git diff` against the
# checkout, which first refreshes the whole index: every entry is racily clean
# in the second the checkout wrote it, so a one-file commit rehashed them all.
# No filters, because the checkout already wrote what the index holds. Both
# lists come out in git's byte order and pair line by line.
drift=$(cat "$notes/prettier")
if [ "$mode" = staged ] && [ -n "$files" ]; then
    # shellcheck disable=SC2086
    printf '%s\n' $files | LC_ALL=C sort > "$notes/files"
    (cd "$work" && git hash-object --no-filters --stdin-paths < "$notes/files") \
        > "$notes/new" || exit 2
    # shellcheck disable=SC2086
    git --literal-pathspecs ls-files -s -- $files | cut -f1 | cut -d' ' -f2 \
        > "$notes/old" || exit 2
    drift="$drift $(paste -d' ' "$notes/old" "$notes/new" "$notes/files" \
        | awk '$1 != $2 { print $3 }')"
else
    for file in $files; do
        cmp -s "$file" "$work/$file" || drift="$drift $file"
    done
fi

# Word splitting trims the blank a clean run leaves behind.
# shellcheck disable=SC2086
set -- $drift
if [ $# -gt 0 ]; then
    hint="run 'make indent'"
    [ "$mode" = staged ] && hint="$hint and stage the result"
    echo "Formatting drift; $hint:" >&2
    printf '  %s\n' "$@" >&2
    exit 1
fi

echo "Formatting clean${skipped:+ for the lanes that ran; not checked:$skipped}."

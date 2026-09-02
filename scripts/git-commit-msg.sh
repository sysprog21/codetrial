#!/bin/sh

# Hold a commit message to the seven rules this log already follows, with the
# widths measured off it: subjects sit at 46 characters in the median and the
# bodies wrap at 72. See scripts/install-git-hooks.sh for installation.
#
# A message arriving as `-` is read from standard input instead of from a file,
# which is how scripts/check-commit-log.sh replays an existing commit through
# the same rules. One list, so a contributor with the hooks installed and one
# without are judged the same way.

set -u

# The rules as prose, printed for scripts/git-prepare-commit-msg.sh to put in
# front of whoever is writing the message. Here rather than there so the list a
# contributor reads and the list this script enforces cannot drift apart.
if [ "${1:-}" = "--rules" ]; then
    cat << 'EOF'
1. Separate the subject from the body with a blank line
2. Keep the subject within 50 characters, and say more than one word
3. Capitalize the subject; no trailing period, no backticks
4. Use the imperative mood: "Fix", not "Fixed" or "Fixes"
5. No conventional-commit prefix: not "fix:", not "feat(web):"
6. Wrap the body at 72 characters
7. Say what and why; the diff already says how

Printable ASCII throughout. One paragraph carries most changes here; the
per-decision detail belongs in the comment next to the code.
EOF
    exit 0
fi

message_file=$1

if [ "$message_file" = "-" ]; then
    message_file=$(mktemp) || exit 1
    trap 'rm -f "$message_file"' EXIT
    cat > "$message_file"
fi

# `git commit -v` puts the diff after a scissors line and drops it before the
# message is stored, so the rules must not see it. Comments go the same way.
# Under `commit.cleanup=verbatim` the comment lines are kept in the stored
# message, so stripping them would judge a message nobody commits. Everything
# below the scissors goes either way: git drops it before storing.
cleanup=$(git config --get commit.cleanup 2> /dev/null) || cleanup=default
message=$(sed '/-\{8,\}[[:space:]]*>8[[:space:]]*-\{8,\}/,$d' "$message_file")
if [ "$cleanup" = verbatim ]; then
    : # Kept as written, blank lines included: that is what verbatim means.
else
    message=$(printf '%s\n' "$message" | git stripspace --strip-comments)
fi

subject=$(printf '%s\n' "$message" | sed -n '1p')
second=$(printf '%s\n' "$message" | sed -n '2p')
body=$(printf '%s\n' "$message" | sed -n '2,$p')
failed=0

error()
{
    printf 'Commit message: %s\n' "$1" >&2
    failed=1
}

# A fixup lands in the commit it names, and a merge subject is git's wording,
# not the author's. Judging either is judging a message nobody wrote.
case "$subject" in
    "fixup! "* | "squash! "* | "amend! "*) exit 0 ;;
    "Merge branch "* | "Merge branches "* | "Merge tag "* | "Merge commit "* | \
        "Merge pull request "* | "Merge remote-tracking branch "*) exit 0 ;;
esac

if [ -z "$subject" ]; then
    error "a descriptive subject is required"
elif [ "${#subject}" -gt 50 ]; then
    error "subject is ${#subject} characters; keep it within 50"
else
    case "$subject" in
        *[[:space:]]*) ;;
        *) error "subject must say more than one word" ;;
    esac
fi

# ASCII covers three rules at once: no tabs or control characters, no CJK, and
# no em dash or typographic quote smuggled in by an editor that autocorrects.
if printf '%s' "$message" | LC_ALL=C grep -q '[^ -~]'; then
    error "message must be printable ASCII"
fi

case "$subject" in
    [[:space:]]*) error "subject must not start with whitespace" ;;
    [[:lower:]]*) error "capitalize the subject" ;;
esac

case "$subject" in
    *.) error "subject must not end with a period" ;;
esac

# The subject completes "this commit will ...", so a past tense or a gerund in
# the first word is the whole tell. Checking one word catches nearly all of it
# and never argues with a legitimate "Fix the parser" style subject.
first_word=$(printf '%s' "$subject" | sed 's/[[:space:]].*//' | tr '[:upper:]' '[:lower:]')
case "$first_word" in
    added | adds | adding | adjusted | adjusts | adjusting | allowed | allows | \
        allowing | avoided | avoids | avoiding | bumped | bumps | bumping | \
        changed | changes | changing | checked | checks | checking | cleaned | \
        cleans | cleaning | corrected | corrects | correcting | created | creates | \
        creating | deleted | deletes | deleting | disabled | disables | disabling | \
        dropped | drops | dropping | enabled | enables | enabling | fixed | fixes | \
        fixing | handled | handles | handling | implemented | implements | \
        implementing | improved | improves | improving | included | includes | \
        including | introduced | introduces | introducing | made | makes | making | \
        moved | moves | moving | refactored | refactors | refactoring | removed | \
        removes | removing | renamed | renames | renaming | replaced | replaces | \
        replacing | reverted | reverts | reverting | tested | tests | testing | \
        tidied | tidies | tidying | updated | updates | updating | used | uses | \
        using)
        error "use the imperative mood: \"$first_word\" describes what you did"
        ;;
esac

# This log has no conventional-commit prefixes and no backticks in a subject.
# Both read as an import from another project's tooling.
if printf '%s' "$subject" | grep -qE '^[A-Za-z]+(\([^)]*\))?!?: '; then
    error "subject must not use conventional-commit syntax"
fi

case "$subject" in
    *'`'*) error "subject must not contain backticks" ;;
esac

# `second` is line 2 and `body` is line 2 onward, so a non-empty line 2 is a
# body that started without a blank line between it and the subject.
if [ -n "$second" ]; then
    error "separate the subject from the body with a blank line"
fi

# The message is ASCII by the rule above, so a byte is a column and grep can
# answer this in one pass.
if printf '%s\n' "$body" | grep -q '.\{73,\}'; then
    error "wrap body lines at 72 characters"
fi

# The reasoning per decision lives in the comment next to the code in this tree.
# A body that walks through the implementation duplicates it and then goes stale
# on its own.
if printf '%s\n' "$body" \
    | grep -qiE '^(how|implementation( (details|notes|steps))?|steps?|changes)[[:space:]]*:'; then
    error "the body carries what and why; how is the diff's job"
fi

# Not an error: long bodies exist in this history. Still worth saying once,
# because the house shape is a premise and its trade, not a retelling.
blanks=$(printf '%s\n' "$body" | git stripspace | grep -c '^$')
if [ "$blanks" -gt 1 ]; then
    printf 'Commit message: note, %s paragraphs; one usually carries it.\n' \
        "$((blanks + 1))" >&2
fi

exit "$failed"

#!/bin/sh

# Hold a commit message to the seven rules this log already follows, with the
# widths measured off it: subjects sit at 46 characters in the median and the
# bodies wrap at 72. See scripts/install-git-hooks.sh for installation.
#
# A message arriving as `-` is read from standard input instead of from a file,
# which is how scripts/check-commit-log.sh replays an existing commit through
# the same rules. One list, so a contributor with the hooks installed and one
# without are judged the same way.
#
# The checks themselves are in scripts/git-commit-msg.awk, one process for the
# whole message. This file only prepares the text the way git will store it.
#
#   git-commit-msg.sh [--stored] FILE|-   judge a message. The exit status adds up what is
#                              wrong: 1 a style rule is broken, 2 some line
#                              credits an AI tool, so 3 is both. 4 means the
#                              message or the rules could not be read, or awk
#                              failed to run, which is none of those and never
#                              read as one. --stored says the message is one
#                              a commit already holds (check-commit-log.sh and
#                              the PR sweep replay those): git cut no scissors
#                              and stripped no comments from it, so neither is
#                              done here, and a line that looks like either
#                              cannot hide a trailer under it.
#   git-commit-msg.sh --rules  print the rules

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
5. No conventional-commit prefix: not "fix:", not "feat(web):",
   and no "fixup!", "squash!" or "amend!": fold those before committing
6. Wrap the body at 72 characters; a link or file name longer than a line, and
   a trailer such as Co-authored-by, may run past
7. Write a body saying what and why; the diff says how. A subject that only
   names a file ("Update web/app.js") says where, not what

Printable ASCII throughout. One paragraph carries most changes here; the
per-decision detail belongs in the comment next to the code.
No AI attribution: no Co-Authored-By for a tool, no "Generated with" footer.
Finish an issue with Close #N on the last line; a Refs #N belongs in the
description, never on the last line.
EOF
    exit 0
fi

stored=0
if [ "${1:-}" = "--stored" ]; then
    stored=1
    shift
fi
if [ $# -ne 1 ]; then
    echo "usage: git-commit-msg.sh [--stored] FILE|-   or   --rules" >&2
    exit 4
fi
message_file=$1
case $0 in
    */*) script_dir=${0%/*} ;;
    *) script_dir=. ;;
esac
awk_script=$script_dir/git-commit-msg.awk

# A shell exits 1 or 2 on a redirection it cannot make, which are the statuses
# for a broken rule and a credit to a tool, so a message that cannot be read is
# caught before it is opened.
if [ "$message_file" != "-" ]; then
    if [ ! -r "$message_file" ] || [ -d "$message_file" ]; then
        echo "git-commit-msg.sh: cannot read $message_file" >&2
        exit 4
    fi
    # Everything below reads standard input, so a file is only a redirection.
    exec < "$message_file"
fi

# `git commit -v` puts the diff after a scissors line and drops it before the
# message is stored, so the rules must not see it. Comments go the same way.
# Under `commit.cleanup=verbatim` the comment lines are kept in the stored
# message, so stripping them would judge a message nobody commits. Everything
# below the scissors goes either way: git drops it before storing.
cleanup=default
if [ "$stored" -eq 0 ]; then
    cleanup=$(git config --get commit.cleanup 2> /dev/null) || cleanup=default
fi
strip_comments()
{
    if [ "$stored" -eq 1 ]; then

        # Trailing whitespace goes either way, a carriage return included: a
        # commit made in GitHub's web editor holds CRLF, and judging the CR
        # would answer "printable ASCII" for a message that reads fine.
        sed 's/[[:space:]]*$//'
    elif [ "$cleanup" = verbatim ]; then
        cat
    else
        git stripspace --strip-comments
    fi
}
scissors()
{
    if [ "$stored" -eq 1 ]; then
        cat
    else
        sed '/-\{8,\}[[:space:]]*>8[[:space:]]*-\{8,\}/,$d'
    fi
}

# The rules run in the C locale: the ASCII rule counts bytes, and the character
# ranges in them mean what they say only there. The awk speaks on standard
# output; its complaints belong on standard error.
LC_ALL=C
export LC_ALL
scissors | strip_comments | awk -f "$awk_script" >&2

# The awk adds 16 to a verdict, so any other status is awk failing to run (it
# exits 2 on a program it cannot open or parse, which must not read as a credit
# to a tool).
status=$?
case $status in
    16)
        if [ "$stored" -eq 0 ] && [ "$message_file" != - ]; then
            python3 "$script_dir/hashchain.py" message "$message_file" || exit 4
        fi
        exit 0
        ;;
    17 | 18 | 19) exit $((status - 16)) ;;
    *)
        echo "git-commit-msg.sh: awk failed with status $status" >&2
        exit 4
        ;;
esac

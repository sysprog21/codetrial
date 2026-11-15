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
#   git-commit-msg.sh --subject FILE|-   judge the first line alone by the
#                              subject rules, as stored text: a pull request
#                              title, which has no body behind it.
#   git-commit-msg.sh --rules  print the rules

set -u

# The rules as prose, printed for scripts/git-prepare-commit-msg.sh to put in
# front of whoever is writing the message. Here rather than there so the list a
# contributor reads and the list this script enforces cannot drift apart.
if [ "${1:-}" = "--rules" ]; then
    cat << 'EOF'
1. Separate the subject from the body with a blank line
2. Limit the subject to 50 characters, and say more than one word
3. Capitalize the subject: it opens with a capital letter
4. Do not end the subject with a period, or with "!", "?", ",", ";" or ":"
5. Use the imperative mood: "Fix", not "Fixed", "Fixes" or "Fixing",
   and open with the verb, not "The", "Now" or "Also"
6. Wrap the body at 72 characters; a link or file name longer than a line,
   and a trailer in the last paragraph, may run past
7. Use the body to explain what and why, not how: prose, not headings
   ("Summary:"), "This commit adds" or changelog lines ("Added a cache.
   Updated main.rs."), and more than the subject said again

Here as well: no "fix:", "feat(web):", "web:" or "[Bug]" prefix and no
backticks in the subject; no "fixup!", "squash!" or "amend!", which fold
before committing; a subject that only names a file ("Update web/app.js")
says where, not what, and one that would fit any commit ("Fix bug",
"Minor changes") says nothing. Printable ASCII throughout. One
paragraph carries most changes; the per-decision detail belongs in the
comment next to the code.
No AI attribution: no Co-Authored-By for a tool, no "Generated with" footer.
Finish an issue with Close #N on the last line; a Refs #N belongs in the
description, never on the last line.
EOF
    exit 0
fi

stored=0
subject_only=0
case "${1:-}" in
    --stored)
        stored=1
        shift
        ;;
    --subject)
        stored=1
        subject_only=1
        shift
        ;;
esac
if [ $# -ne 1 ]; then
    echo "usage: git-commit-msg.sh [--stored|--subject] FILE|-   or   --rules" >&2
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

# The rules judge the text git will store, so the cleanup here is git's own
# (git-commit(1), --cleanup). Git sets GIT_EDITOR=: for every commit hook when
# no editor will open (githooks(5)), which is how -m and -F commit, and an agent
# commits that way. Then the default cleanup is "whitespace": a "#" line is
# stored as written, so stripping it would pass a message that pre-push and CI
# then refuse. In an editor the default strips comments. Under "verbatim"
# nothing is cleaned and under "strip" comments go either way.
#
# A caller may export GIT_EDITOR=: itself, and then git still opens ":" as the
# editor for --amend, a plain commit or a rebase that stops, and strips the help
# and status block it wrote into the file. That block is the tell: in every
# translation it holds a line that is the comment character alone, which a
# message given with -m ("## Summary", "## Issue #12") does not. With
# commit.status off git writes no block, and there is no comment of git's to
# misjudge.
#
# Git cuts at a scissors line only in an editor and only at its own exact line,
# under -v (in any cleanup mode) or the "scissors" mode; a line merely shaped
# like one is text it stores. -v on the command line is not visible to a hook,
# but what it leaves is: git's diff right below the line. So the cut is made
# there or under "scissors", and git's line typed into a message without either
# is judged as git stores it. A --cleanup on the command line is not visible
# either, and is judged by the configured mode.
#
# A stored message (pre-push and CI replay one per commit) needs none of this,
# so none of it runs there.
#
# Under core.commentChar=auto git picks the first of #;@!$%^&|: that opens no
# line of the message and writes its help block with it, so the block's line of
# that character alone names it; with no block there is nothing of git's to
# strip, and "#" stands.
cleanup=default
comment_char='#'
editor=1
if [ "$stored" -eq 0 ]; then
    cleanup=$(git config --get commit.cleanup 2> /dev/null) || cleanup=default
    comment_char=$(git config --get core.commentChar 2> /dev/null) || comment_char='#'
    auto=0
    case "$comment_char" in
        '') comment_char='#' ;;
        auto) comment_char='#' auto=1 ;;
    esac
    if [ "${GIT_EDITOR:-}" = : ] || [ "$auto" -eq 1 ]; then

        # The tell is read before the message is judged, so a message on
        # standard input is kept for both.
        seen=$message_file
        if [ "$message_file" = - ]; then
            seen=$(mktemp) || exit 4
            trap 'rm -f "$seen"' EXIT
            cat > "$seen" || exit 4
            exec < "$seen"
        fi
        if [ "$auto" -eq 1 ]; then
            for c in '#' ';' '@' '!' '$' '%' '^' '&' '|' ':'; do
                if grep -qxF -- "$c" "$seen"; then
                    comment_char=$c
                    break
                fi
            done
        fi
        if [ "${GIT_EDITOR:-}" = : ]; then
            grep -qxF -- "$comment_char" "$seen" || editor=0
        fi
    fi
fi
strip_comments()
{
    case $cleanup:$editor in
        verbatim:*) cat ;;
        strip:* | default:1)
            git -c core.commentChar="$comment_char" stripspace --strip-comments
            ;;
        *) git stripspace ;;
    esac
}
# Below git's own line come two comment lines, then the diff -v writes.
scissors()
{
    awk -v cut="$comment_char ------------------------ >8 ------------------------" \
        -v c="$comment_char" -v mode="$cleanup" '
        held {
            buf = buf $0 "\n"
            if (index($0, c) == 1)
                next
            if (mode == "scissors" || $0 ~ /^diff --git /) {
                held = 0
                exit
            }
            printf "%s", buf
            held = 0
            buf = ""
            next
        }
        $0 == cut { held = 1; buf = $0 "\n"; next }
        { print }
        END { if (held && mode != "scissors") printf "%s", buf }'
}

# The rules run in the C locale: the ASCII rule counts bytes, and the character
# ranges in them mean what they say only there. The awk speaks on standard
# output; its complaints belong on standard error.
LC_ALL=C
export LC_ALL
judge()
{
    awk -v subject_only="$subject_only" -f "$awk_script" >&2
}
if [ "$stored" -eq 1 ]; then

    # Trailing whitespace goes either way, a carriage return included: a commit
    # made in GitHub's web editor holds CRLF, and judging the CR would answer
    # "printable ASCII" for a message that reads fine.
    sed 's/[[:space:]]*$//' | judge
elif [ "$editor" -eq 1 ]; then
    scissors | strip_comments | judge
else
    strip_comments | judge
fi

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
    17 | 18 | 19)

        # Git says nothing about the refused text, which reads as lost and gets
        # retyped. It is still in the file git handed over, and git reads -F
        # before it writes that file again. A ragged or over-long body is the
        # refusal a tool can settle, so the pass that rejoins and rewraps it is
        # named beside the way back in. Absolute, since git hands over a path
        # relative to the top of the tree and the commit may have been made from
        # a subdirectory. -v as well: the file may still hold the scissors line
        # and diff of a -v commit, which git cuts only under -v and otherwise
        # stores as part of the message.
        if [ "$stored" -eq 0 ] && [ "$message_file" != - ]; then
            case $message_file in
                /*) kept=$message_file ;;
                *) kept=$(pwd)/$message_file ;;
            esac
            tools=$(CDPATH= cd -- "$script_dir" && pwd) || tools=$script_dir
            echo "Your message is kept; edit it with: git commit -v -e -F $kept" >&2
            echo "To rewrap its body at 72 first: python3 $tools/commit-reflow.py $kept" >&2
        fi
        exit $((status - 16))
        ;;
    *)
        echo "git-commit-msg.sh: awk failed with status $status" >&2
        exit 4
        ;;
esac

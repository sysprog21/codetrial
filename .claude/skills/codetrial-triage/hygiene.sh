#!/bin/sh
# Read-only: which of the requests maintainers keep typing by hand a pull
# request needs at its current head. Each is a rule a script can decide, and
# each carries the wording the maintainers already used for it, so a sweep posts
# the result as a Request changes review without drafting anything (pulls.md,
# "Contributor hygiene"). A conflict is not here: rebase.md owns it, and the CI
# regression item asks for the same thing in the same words.
#
# Usage: hygiene.sh <pr> [repo]
#        Prints {pr, head, skip, items: [{key, text}], body, comments: [{key,
#        text}], hide: [{id, key, why}]}. skip says why the PR gets nothing
#        (the skip calls below), and items, comments and hide are then empty.
#        hide lists the visible copies of earlier requests to hide once the
#        review is up: why is repeated (a newer copy stands) or stale (the
#        evidence is gone). body is the review text, empty when
#        no item applies. comments are the requests posted as a plain comment
#        rather than a Request changes review: the screenshot request, which
#        does not block the merge by itself.
#        Exits 2 on bad arguments.
#
# An item a maintainer already raised at this head, in a review left on it or a
# comment after GitHub's first check suite for it (the push, by GitHub's clock
# rather than the contributor's), is left out: that request stands. So is one
# whose own text is still visible on any earlier head while its evidence holds,
# which is what keeps a push from posting the same review again.

set -eu

usage='usage: hygiene.sh <pr> [repo]'
pr=${1:?$usage}
repo=${2:-sysprog21/codetrial}
case $pr in
    *[!0-9]*)
        echo "hygiene.sh: $pr is not a pull request number" >&2
        exit 2
        ;;
esac
root=$(cd "$(dirname "$0")/../../.." && pwd)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

gh pr view "$pr" --repo "$repo" --json \
    number,headRefOid,headRefName,isCrossRepository,isDraft,baseRefName,mergeable,author,body,files,reviews,comments \
    > "$work/pr.json"
head=$(jq -r .headRefOid "$work/pr.json")

# A PR nobody should be asked about is decided before the costly reads: the
# commits, and ci-failure.sh's job logs.
skip()
{
    jq -n --arg why "$1" --slurpfile p "$work/pr.json" \
        '{pr: $p[0].number, head: $p[0].headRefOid, skip: $why, items: [], body: "",
          comments: [], hide: []}'
    exit 0
}
why=$(jq -r 'if .isDraft then "draft"
    elif .baseRefName != "main" then "base is not main"
    elif .author.is_bot or (.author.login | test("\\[bot\\]$|^(app/)?dependabot"))
    then "a bot'\''s pull request"
    else "" end' "$work/pr.json")
[ -z "$why" ] || skip "$why"
author=$(jq -r .author.login "$work/pr.json")

# As in pr-sweep.sh: on a refusal gh prints the error body to stdout, so only a
# successful call is read, and a pipe would hide the failure behind jq's status.
# Without the listing nobody can be told apart as a maintainer, and a request
# one of them already made would be asked again. Listing it needs push access,
# so the viewer is on it, and their own PR is skipped as a maintainer's.
logins=$(gh api --paginate "repos/$repo/collaborators?permission=push" \
    --jq '.[].login' 2> /dev/null) || skip "the push listing failed"
printf '%s\n' "$logins" | grep -qxF "$author" && skip "a maintainer's pull request"
pushers=$(printf '%s\n' "$logins" | jq -Rn '[inputs | select(length > 0)]')

# Which reviews are hidden: `gh pr view` reports it for comments and not for
# reviews. Without it a hidden copy of a request would read as one still
# standing, so a failed read skips the PR as the push listing does.
# shellcheck disable=SC2016  # $owner and the others below are GraphQL variables
minimized=$(gh api graphql -F owner="${repo%%/*}" -F name="${repo##*/}" -F number="$pr" \
    -f query='query($owner: String!, $name: String!, $number: Int!) {
      repository(owner: $owner, name: $name) { pullRequest(number: $number) {
        reviews(first: 100) { pageInfo { hasNextPage } nodes { id isMinimized } } } } }' \
    --jq '.data.repository.pullRequest.reviews
        | if .pageInfo.hasNextPage then error("more than one page")
          else [.nodes[] | select(.isMinimized) | .id] end' \
    2> /dev/null) || skip "the review listing failed"

# When the head reached GitHub, by GitHub's clock: its first check suite. The
# committer date is set by the contributor's clock and dates nothing about the
# push, so without a check suite the PR is skipped rather than guessed.
pushed=$(gh api "repos/$repo/commits/$head/check-suites" \
    --jq '[.check_suites[].created_at] | min // empty' 2> /dev/null) || pushed=
[ -n "$pushed" ] || skip "no check suite dates the push"
gh api "repos/$repo/pulls/$pr/commits" --paginate > "$work/commits.json"

# Inline review comments, where a maintainer may ask on a line rather than in
# the review body.
gh api "repos/$repo/pulls/$pr/comments" --paginate > "$work/inline.json"

# Whether the PR changes what a web page shows. HTML and CSS always do.
# JavaScript does when a changed line (an added or a removed one) writes to the
# DOM or its styling, a heuristic and not a parse: vendored code and tests are
# left out, and a diff GitHub will not serve yields no evidence, so only HTML
# and CSS count then.
shows_page=false
image_re='<img|!\[|user-attachments/|\.(png|jpe?g|gif|webp|mp4|mov)\b'

# The diff is read only when a script could still decide the answer: not when
# the description already shows an image or a stylesheet or page already counts.
if jq -e --arg img "$image_re" 'any(.files[]; .path | test("^web/(.*/)?[^/]+\\.js$") and (test("^web/vendor/|\\.test\\.") | not))
    and (any(.files[]; .path | test("^web/.*\\.(html|css)$")) | not)
    and ((.body // "") | test($img; "i") | not)' \
    "$work/pr.json" > /dev/null && gh pr diff "$pr" --repo "$repo" > "$work/pr.diff" 2> /dev/null; then
    awk '/^diff --git / { f = $4; next }
        f ~ /^b\/web\// && f !~ /^b\/web\/vendor\// && f ~ /\.js$/ && f !~ /\.test\./ \
            && /^[-+]/ && !/^(\+\+\+|---)/ \
            && /textContent|innerHTML|innerText|classList|className|\.style|createElement|\.hidden|setAttribute|insertAdjacent|replaceChildren|\.append|\.prepend/ \
            { found = 1; exit }
        END { exit !found }' "$work/pr.diff" && shows_page=true
fi

# Each commit through the commit-msg hook's own rules, the ones CI applies too.
# Its exit status adds up what is wrong, so one call answers both questions and
# the AI-credit item and a refused commit cannot disagree: bit 1 is a style rule
# broken, bit 2 a credit to a tool, and anything above 3 (the hook itself
# failing) counts as a broken rule. A credit is looked for in every commit. The
# style rules skip a merge, which is git's wording, and Dependabot's commits,
# which are skipped by author as check-commit-log.sh does. Per commit, one line
# saying whether the style rules apply and one JSON string for the message, so
# its own newlines survive the loop.
bad_messages=0
ai_messages=0
jq -c '.[] | ((.parents | length) < 2
        and .commit.author.email != "49699333+dependabot[bot]@users.noreply.github.com"
        | if . then 1 else 0 end), .commit.message' "$work/commits.json" > "$work/commits.lines"
while IFS= read -r styled && IFS= read -r message; do
    printf '%s\n' "$message" | jq -r . > "$work/message"
    status=0
    sh "$root/scripts/git-commit-msg.sh" --stored "$work/message" > /dev/null 2>&1 || status=$?
    if [ "$styled" -eq 1 ] && {
        [ "$status" -gt 3 ] || [ $((status & 1)) -ne 0 ]
    }; then
        bad_messages=$((bad_messages + 1))
    fi
    if [ $((status & 2)) -ne 0 ]; then
        ai_messages=$((ai_messages + 1))
    fi
done < "$work/commits.lines"

# Failing checks at this head, and whether main shares them. A diagnosis that
# fails, or that read another head, yields no CI item rather than a guess.
# HYGIENE_CI names the script to ask (ci-cached.sh, which shares one read of the
# job logs per head with the sweep), for tests.
sh "${HYGIENE_CI:-$root/.claude/skills/codetrial-verify/ci-cached.sh}" "$pr" "$repo" \
    > "$work/ci.json" 2> /dev/null || echo '{}' > "$work/ci.json"
# Every read above describes one head only if the PR did not move meanwhile.
again=$(gh pr view "$pr" --repo "$repo" --json headRefOid --jq .headRefOid)

jq -n -L "$(dirname "$0")" --slurpfile p "$work/pr.json" --slurpfile c "$work/commits.json" \
    --slurpfile ci "$work/ci.json" --slurpfile inline "$work/inline.json" \
    --argjson bad "$bad_messages" --argjson ai "$ai_messages" \
    --argjson pushers "$pushers" --argjson minimized "$minimized" --arg pushed "$pushed" --argjson shows "$shows_page" --arg img "$image_re" \
    --arg again "$again" --rawfile fix "$(dirname "$0")/rebase.md" '
  $p[0] as $pr | ($c | add) as $commits
  | (if $ci[0].head == $pr.headRefOid then $ci[0] else {} end) as $ci
  | def staff: .author.login | IN($pushers[]);
  # The aggregate check job only collects the others, so it proves nothing.
  # A job whose log could not be read has null gates: no evidence either way.
  ($ci.failing // [] | map(select(.current == true and .name != "check"
      and .gates != null))) as $failing
  | (if $again != $pr.headRefOid or ($commits | last | .sha) != $pr.headRefOid
     then "the head moved while it was read"
     else null end) as $skip
  # Every request, with whether the evidence for it holds at this head.
  | def item($key; $when; $marker; $text):
      {key: $key, when: $when, marker: $marker, text: $text};
  ([
      item("fork_main"; $pr.isCrossRepository and $pr.headRefName == "main";
        "branch of your fork|topic branch";
        "This pull request is opened from the `main` branch of your fork, so every later commit you push there joins it. Open a new pull request from a topic branch and close this one; see `CONTRIBUTING.md`."),
      item("merge_commit"; any($commits[]; (.parents | length) > 1);
        "git merge|merge commit";
        "Do not merge `main` into this branch with `git merge`. Drop the merge commits, put your own commits on top of the latest `main`, and force-push."),
      item("commit_rules"; $bad > 0; "cbea\\.ms|chris\\.beams|make hooks";
        "Improve git commit messages per https://cbea.ms/git-commit/ and `CONTRIBUTING.md`; CI applies the same rules. Hint: run `make hooks` once per clone, so the hooks check every message before you push."),
      item("co_authored"; $ai > 0;
        "co-authored|attribution|agent marker";
        "Remove AI agent markers (`Co-authored-by` an agent, \"Generated with\") from Git commit messages, as this attribution does not improve collaboration."),
      item("indent"; any($failing[]; .on_main == false
          and any(.gates[]; . == "indent")); "make indent";
        "CI fails on the `indent` gate. Run `make indent`, then amend and force-push."),
      # A conflicting PR is asked to rebase first, and a rebase may be the
      # whole answer to the failure; UNKNOWN may still turn out a conflict.
      item("ci_regression";
        $pr.mergeable == "MERGEABLE" and ($ci.stale_base != true)
        and any($failing[]; .on_main == false and .gates != ["indent"]);
        "regression|rebase";
        $fix | rtrimstr("\n"))
    ]) as $defs
  | ($defs | map(select(.when) | .key)) as $active
  # The request that rests on CI is unknown, not gone, when the diagnosis
  # failed or read another head, a check is still running or held, or the log of a job
  # could not be read. An unknown request keeps the copy that carries it
  # (nothing is hidden on no evidence) and is not posted either.
  | (($ci.head == null) or (($ci.pending // []) | length > 0)
     or (($ci.held // []) | length > 0)
     or any($ci.failing // [] | .[]; .current == true and .name != "check" and .gates == null)
    ) as $unknown
  | def holds: . as $k
      | ($k | IN($active[])) or ($unknown and $k == "indent");
  # The copies of these requests that are still visible, whatever head they
  # were left on, newest first. A request stands while its evidence holds, so
  # a push that leaves the failure as it was does not ask again. A copy that
  # carries a request whose evidence is gone no longer describes the PR and is
  # hidden; the requests it carried that still hold go out in the new review.
  # The CI regression belongs to rebase.md and pr-sweep.sh hides it.
  def own($id; $at; $body):
      [$defs[] | select(.key != "ci_regression") | . as $d
       | select($body | contains($d.text))] as $found
      # Only a body made of our texts: a maintainer who added feedback of their
      # own to one is not asked to lose it by having the review hidden.
      | select(($found | length > 0)
          and (reduce $found[] as $d ($body; split($d.text) | join(""))
               | gsub("\\s"; "") == ""))
      | {id: $id, at: $at, keys: [$found[].key]};
  ([$pr.reviews[] | select(staff and (.id | IN($minimized[]) | not))
      | own(.id; .submittedAt; .body // "")]
   + [$pr.comments[] | select(staff and (.isMinimized | not))
      | own(.id; .createdAt; .body // "")]
   | sort_by(.at) | reverse) as $ours
  | def keep: all(.keys[]; holds)
      or ($unknown and any(.keys[]; . == "indent"));
  ($ours | map(select(keep | not))) as $stale
  | (reduce ($ours[] | select(keep)) as $o
      ({covered: [], drop: []};
       . as $st
       | if all($o.keys[]; . as $k | $k | IN($st.covered[]))
         then .drop += [$o] else .covered += $o.keys end)) as $kept
  | (if $skip then [] else
       [$kept.drop[] | {id, key: (.keys | join(",")), why: "repeated"}]
       + [$stale[] | {id, key: (.keys | join(",")), why: "stale"}]
     end) as $hide
  # What maintainers already said at this head, but for a copy about to be hidden.
  | ($stale | map(.id)) as $going
  | ([$pr.reviews[] | select(staff and .commit.oid == $pr.headRefOid
        and (.id | IN($going[], $minimized[]) | not)) | .body]
   + [$pr.comments[] | select(staff and .createdAt > $pushed and (.isMinimized | not)
        and (.id | IN($going[]) | not)) | .body]
   + [$inline | add // [] | .[]
      | select((.user.login | IN($pushers[])) and .commit_id == $pr.headRefOid)
      | .body]
   | join("\n")) as $said
  | (if $skip then [] else
       [$defs[] | . as $d
        | select($d.when and ($said | test($d.marker; "i") | not)
          and ($d.key | IN($kept.covered[]) | not))
        | {key, text}]
     end) as $items
  # A screenshot is asked for once: any visible maintainer comment or review
  # that mentions one, whenever it was left, stands until the description shows
  # an image, so a push does not bring the request back.
  | ([$pr.reviews[] | select(staff and (.id | IN($minimized[]) | not)) | .body]
     + [$pr.comments[] | select(staff and (.isMinimized | not)) | .body]
     + [$inline | add // [] | .[] | select(.user.login | IN($pushers[])) | .body]
     | any(test("screenshot"; "i"))) as $asked
  | (if $skip or $asked or ($pr.body // "" | test($img; "i"))
        or (($shows or any($pr.files[]; .path | test("^web/.*\\.(html|css)$"))) | not)
     then []
     else [{key: "screenshot",
            text: "Add screenshots to the pull request description to illustrate your work, as @ColtenOuO suggests. Example: #349"}]
     end) as $comments
  | {pr: $pr.number, head: $pr.headRefOid, skip: $skip, items: $items,
     body: ([$items[].text] | join("\n\n")), comments: $comments, hide: $hide}
'

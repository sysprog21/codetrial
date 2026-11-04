#!/bin/sh
# Record a non-review skill invocation's local evidence report (experience.md,
# "Prepare and record"; the contract is in codetrial-conventions, "Local
# evidence at closeout"): fill the fixed fields, refuse a report that does not
# state its own outcome and coverage, and run `memory.py experience record`. It
# supplies boilerplate only. Findings, history evidence and coverage are the
# invocation's own, so a report without them is refused rather than completed.
#
# Usage: experience-close.sh <fields.json> [full-head]
#        fields.json holds at least {outcome: "complete"|"incomplete",
#        skills_used: [...], coverage: "..."}; any other report field it
#        carries is kept. The head defaults to HEAD of the working tree, and
#        must be a full 40-character commit. SCRATCH names where the report is
#        kept (experience.json), and stays there after a failure, which is
#        printed rather than hidden. Exits with the record command's status,
#        2 on bad arguments.
#
# Never call this from an exit trap or on a path that did not finish its work: a
# complete outcome means the stated scope was completed.

set -eu

usage='usage: experience-close.sh <fields.json> [full-head]'
[ $# -ge 1 ] || {
    echo "$usage" >&2
    exit 2
}
fields=$1
scratch=${SCRATCH:-}
[ -n "$scratch" ] || {
    echo "experience-close.sh: SCRATCH is not set" >&2
    exit 2
}
head=${2:-$(git rev-parse HEAD)}
printf '%s' "$head" | grep -Eq '^[0-9a-f]{40}$' || {
    echo "experience-close.sh: $head is not a full commit" >&2
    exit 2
}
jq -se 'length == 1 and (.[0] | type == "object"
    and (.outcome | IN("complete", "incomplete"))
    and (.skills_used | type == "array" and length > 0 and all(type == "string"))
    and (.coverage | type == "string" and test("\\S")))' "$fields" > /dev/null 2>&1 || {
    echo "experience-close.sh: $fields must state outcome (complete|incomplete), skills_used and coverage" >&2
    exit 2
}

# Say whether a gate receipt backs a claim that the tree passed. Disclosure
# only: the report is recorded either way, and the wording of its coverage stays
# the invocation's own.
checker=$(dirname "$0")/../../../scripts/agent-closeout-check.py
[ ! -f "$checker" ] || python3 "$checker" --receipts >&2 || true
memory=${EXPERIENCE_MEMORY:-$(dirname "$0")/memory.py}
mkdir -p "$scratch"
report=$(mktemp "$scratch/experience.XXXXXX")
trap 'rm -f "$report"' EXIT
jq --arg head "$head" '{schema_version: 1, head: $head, paths: [], findings: [],
    refuted: [], questions: [], next_checks: [],
    history_evidence: {used: [], consulted: [], rejected: []}} + .
    + {schema_version: 1, head: $head}' "$fields" > "$report"
# The recorder takes one skill, the first listed; the report keeps them all.
skill=$(jq -r '.skills_used[0]' "$fields")
mv "$report" "$scratch/experience.json"
python3 "$memory" experience record --skill="$skill" --report "$scratch/experience.json"

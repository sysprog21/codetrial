# Framework rubric calibration

Status: **NOT CALIBRATED** as of 2026-08-31. No real blinded human-rating set is
checked into this repository, and the synthetic analyzer fixtures are tests, not
evidence. REACTO and STAR phase scores are formative coaching signals and must not
be used mechanically in a hiring decision.

## Frozen release gate

Calibrate one exact contract tuple: bundle, report prompt, rubric, report schema,
and provider model identity. Any change to one of those values invalidates the
result and requires a fresh review. Do not pool ratings across tuples.

A release candidate needs at least 30 completed sessions selected before scoring.
The sample must include Easy, Medium, and Hard problems; at least three supported
implementation languages; coding-only and coding-plus-behavioral loops; clear,
degraded, and missing transcripts; every rubric score band; zero and nonzero hint
use; and both complete and intentionally unassessable phases. Selection must not
depend on the model's decision.

Unassessable coverage requires at least one applicable session/phase pair whose
model and human scores are all `null`, across the whole dataset. All REACTO
phases apply to both loop types; STAR phases apply only to coding-plus-behavioral
loops. The inapplicable STAR `null` values in coding-only sessions do not meet
this requirement. A non-null STAR score from the model or any human reviewer
in a coding-only session is invalid input (exit 2), even if all scores agree;
inapplicable scores never enter phase coverage or numeric metrics.
Human keys for these inapplicable phases may be omitted or recorded as `null`;
no human judgments are required for them.
Unassessable coverage is not required for every phase or stratum. The existing
input rules still apply, including at least two distinct human reviewers per
sample and every required model phase key.

Each assessable phase is scored independently by at least two trained reviewers
who see a random sample id, candidate evidence, the frozen rubric, and no model
score, model decision, identity, demographic information, or other reviewer's
rating. Reviewers use `null` when evidence cannot support a score. Every recorded
human judgment in an applicable phase must agree with the model's assessability;
a null and a numeric score disagree even when two other reviewers gave numeric
scores. Such a mismatch is invalid input (exit 2), before coverage or metric
results are reported. After independent ratings are locked, disagreements
spanning two rubric bands or 20 points are adjudicated by a third reviewer;
original ratings remain in the analysis input.
An adjudicator who reviews only disputed phases omits the other keys from their
`scores` object. An omitted key means no judgment was recorded; an explicit
`null` means the reviewer judged that phase unassessable. Reviewer order does
not assign roles. Only recorded judgments enter assessability validation and
numeric metrics; missing judgments are never replaced with scores or `null`.
Unknown phase keys and invalid score values remain invalid input. Each applicable
phase needs at least two recorded human judgments from different reviewer IDs;
an adjudicator's recorded judgment also counts. The input carries no reviewer
roles, so the analyzer does not establish that these are two independent
original ratings. Independence and blinding must be ensured
by the review process.
Consistently null phases contribute no numeric score to the metrics.

Model/human comparisons use the mean of all recorded numeric judgments,
including partial ratings. Inter-rater QWK uses only ratings whose `scores`
contain every applicable phase key for that sample. Recorded `null` values count
toward this completeness check; omitted inapplicable keys do not disqualify a
rating. A partial rating contributes no QWK pairs, avoiding extra weight for
phases selected for adjudication. A full applicable-phase rating still enters
QWK, including one from an adjudicator. Missing QWK coverage is a calibration
failure (exit 1), not invalid input.

For each phase, calculate QWK separately for each fixed, unordered pair of
eligible reviewer IDs across their jointly scored sessions. Average these pair
values within the phase, weighted by each pair's number of numeric joint
observations, then average the phase values equally. A pair with more joint
observations has proportionally more weight within its phase; a phase with more
observations does not gain weight over another phase. Do not concatenate
different reviewer pairs into two anonymous score columns: their marginal
distributions would depend on the ordering of reviewer IDs. Renaming reviewer
pseudonyms or reordering ratings and sessions must not change QWK.
Keep pair values and both aggregation steps as exact fractions, converting to
a decimal only once for the final metric. Converting pair values early can
round a weighted QWK of exactly 0.67 below the gate threshold.

Use the same private reviewer pseudonym throughout a packet; do not generate a
new reviewer ID for each session. The analyzer can check uniqueness within a
session but cannot establish whether IDs across sessions belong to the same
person. The existing QWK coverage minimum remains one eligible numeric pair
per assessed phase, without a new minimum per reviewer pair. A pair with all
ratings in one rubric band retains QWK 1; a singleton pair whose two ratings
occupy different bands has QWK 0. Sparse pairs and constant ratings do not
establish reliable agreement estimates;
synthetic tests and a passing analyzer gate are not evidence of real calibration.

The gate passes only when every condition holds:

- at least 30 sessions and all required strata;
- at least one applicable session/phase where the model and all recorded human
  judgments are null;
- at least one eligible human/human QWK pair for every numerically assessed phase;
- mean per-phase QWK, with observation-weighted reviewer pairs within each
  phase, at least 0.67;
- model versus mean-human mean absolute error no more than 10 points;
- model and rounded mean-human rubric-band agreement at least 0.80;
- no difference greater than 5 points between eligible subgroup MAEs.

Language and transcript-quality cohorts are required. Accent analysis is optional
and may use only a candidate's voluntary, pseudonymous broad cohort; never infer
accent, ethnicity, nationality, or identity from audio. A subgroup is eligible
only with at least five assessed phase comparisons. Small cohorts are reported as
insufficient rather than merged or exposed. Retain the minimum evidence required
for review, keep the key mapping sample ids outside the packet, restrict access,
and delete review material under the same retention/withdrawal policy as the
source interview.

## Reproducible review

Copy `tests/fixtures/calibration/blinded-review-template.json`, fill the frozen
contract and samples, randomize ids, and give each reviewer a private copy with
only their `reviewerId` row. Merge locked rows without changing scores, then run:

```sh
python3 scripts/calibrate-framework.py ratings.json --format markdown
python3 scripts/calibrate-framework.py ratings.json --format json
```

Exit 0 means every gate passed; exit 1 means valid but not calibrated; exit 2
means invalid input. Archive both outputs, the sampling query, reviewer training
version, adjudication log, and contract tuple. A failed or sparse result keeps the
product formative. The synthetic passing/failing/malformed fixtures exist only to
regress the analyzer and are regenerated with
`python3 scripts/gen-calibration-fixtures.py`; they can never change this status.
The synthetic report-stability probe in `tests/fixtures/report-eval/` measures
how much a report moves under repeats and controlled edits. It is not a human
rating set and can never change this status either.

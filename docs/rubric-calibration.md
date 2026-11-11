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
this requirement. It is not required for every phase or stratum. The existing
input rules still apply, including at least two distinct human reviewers per
sample and every required model phase key. Each phase needs at least two
recorded human judgments, all numeric for an assessed phase or all `null` for
an unassessable phase.

Each assessable phase is scored independently by at least two trained reviewers
who see a random sample id, candidate evidence, the frozen rubric, and no model
score, model decision, identity, demographic information, or other reviewer's
rating. Reviewers use `null` when evidence cannot support a score. A phase is
invalid when model and humans disagree about assessability. After independent
ratings are locked, disagreements spanning two rubric bands or 20 points are
adjudicated by a third reviewer; original ratings remain in the analysis input.
An adjudicator who reviews only disputed phases omits the other keys from their
`scores` object. An omitted key means no judgment was recorded; an explicit
`null` means the reviewer judged that phase unassessable. Reviewer order does
not assign roles. Only recorded judgments enter assessability validation and
numeric metrics; missing judgments are never replaced with scores or `null`.
Unknown phase keys and invalid score values remain invalid input. Omissions
cannot reduce any phase below two independent recorded human judgments.
Every recorded human judgment participates in assessability validation: a null
and a numeric score disagree even when two other reviewers gave numeric scores.
Such a mismatch is invalid input (exit 2), before coverage or metric results are
reported. Consistently null phases contribute no numeric score to the metrics.

The gate passes only when every condition holds:

- at least 30 sessions and all required strata;
- mean per-phase quadratic-weighted inter-rater agreement at least 0.67;
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

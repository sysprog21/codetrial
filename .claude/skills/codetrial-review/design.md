# Review memory: distribution, evidence, and usefulness

This is the design for the next review-memory changes, not a list of shipped
features. The snapshot builder, verified `ensure`, source-safe evidence output
and public credential projection are implemented. The publisher workflow and
release management are implemented; real workflow-token and signed-download
validation still require deployment. An offline bootstrap harness checks pipeline
consistency without claiming network performance. Retrieval expansion, inflight
links, per-thread freshness and ranking work remain planned.

## Reference and boundary

The reference is huggingface/relore at commit
`4d1794cd2ac69bbad8e52d6612fb4016c01f5168`. Its
[expansion](https://github.com/huggingface/relore/blob/4d1794cd2ac69bbad8e52d6612fb4016c01f5168/relore/search/expansion.py),
[freshness](https://github.com/huggingface/relore/blob/4d1794cd2ac69bbad8e52d6612fb4016c01f5168/relore/freshness.py),
[brief](https://github.com/huggingface/relore/blob/4d1794cd2ac69bbad8e52d6612fb4016c01f5168/relore/api/brief.py),
[dataset](https://github.com/huggingface/relore/blob/4d1794cd2ac69bbad8e52d6612fb4016c01f5168/relore/bench/dataset.py),
and [fieldrun](https://github.com/huggingface/relore/blob/4d1794cd2ac69bbad8e52d6612fb4016c01f5168/benchmarks/fieldrun/README.md)
show the mechanisms being adapted. Its numbers, SQL dialect choices, weight
values, and model judgements are not CodeTrial measurements. Some architecture
prose lags the shipped implementation; executable paths and tests settle what
was actually inspected.

Only the key ideas are reference material. Do not install or invoke relore, use
its service/API, import or vendor its modules, or copy its implementation.
Implement the selected ideas independently within CodeTrial and validate them
against CodeTrial evidence. The external checkout stays unchanged.

CodeTrial keeps its local, worktree-shared SQLite store and Python standard
library CLI. GitHub Releases distribute a clean initial snapshot. Incremental
GitHub collection remains the existing poll/detail/sweep protocol. There is no
new HTTP service, Postgres deployment, embedding model, source-code index,
relore runtime dependency, or remote execution of release content.

## The question a review must answer

The primary path is a bounded evidence brief for a fixed review head:

1. Identify accepted scope and other claims on the same linked issue.
2. Retrieve earlier maintainer reasoning, relevant failures, and prior fixes.
3. Check that evidence against the actual head, callers, contracts and tests.
4. Reopen the decisive full source document when a snippet cannot settle a claim.
5. Record a finding only after its concrete trigger is verified at that head.

A downloaded database makes this path available sooner. It does not by itself
make the answer correct or demonstrate that review quality improved.

## Data and trust contracts

### Source history and current code have different jobs

GitHub discussion and provenance are source evidence. Current behavior is checked
in the review bundle and the local checkout at the manifest's commit. A file
existing, or three hunk lines still matching, is a mechanical applicability hint,
not proof that an old ruling settles the current trigger. No generic code-search
service is needed when the reviewer already has the checkout.

Existing raw patches and commits remain part of the review-history design.
Relore's documented no-source-code storage invariant is not copied literally: removing
these would change CodeTrial's current fix indexing and provenance contract.

### Authority is admission, not a ranking boost

Rationale requires identifiable maintainer evidence. Failure reports can come
from other people. A merged fix is evidence of an accepted change, not proof
that every comment in its thread is a maintainer ruling. Bots stay excluded
unless explicitly requested, including after expansion or fallback.

Existing decision summaries remain disposable local annotations. They never
enter FTS, raw source records, or a public snapshot. A summary cannot suppress a
finding on its own: its current source documents must be shown and checked.
Stale summaries are labelled and cannot be presented as binding decisions. Do
not automatically write agent conclusions back into the shared evidence corpus.

### A signed artifact is not automatically safe public content

A clean rebuild already excludes retained edits, deleted rows and local notes.
Public copies mechanically redact recognizable credential spans in decoded raw
strings, including patches and metadata. Match both original and canonical text
using the same hidden-character rules as rendering. Preserve unrelated bytes;
refuse credential-bearing structural locator identities and dictionary keys.
This policy follows a live collection that found credential-shaped hypothetical
URLs in ordinary source history, which made blanket rejection impractical.

Keep local source bytes unchanged. Public records carry explicit projection
metadata; compare incoming projected text for those records to preserve incremental
polling. Do not publish hashes of original redacted text: these would allow guesses
about a hidden value. Public artifacts exclude all local decisions; new decisions
on imported stores hash the stored public body. Re-export preserves disclosure,
and projected review records report code applicability as unknown. A fresh public
copy must pass the credential gate after projection. This catches recognizable
patterns, not arbitrary sensitive information. See snapshots.md for the contract.

At render time, quote every line of other people's text, scrub control and
model-delimiter impersonation, and apply redaction before signals or snippets
could re-expose a detected secret. Keep code identifiers readable and preserve
source links. Tool-generated counts, trust labels and head IDs must remain
visibly distinct from quoted content. Formatting is a defense, not a guarantee
that a model ignores malicious prose; the review's verification step remains.

## Retrieval changes worth implementing

### Ask several bounded questions without widening scope

Search discloses its AND to OR fallback (`fallback=` in the retrieval line,
`--strict` to forbid it). Measure a bounded expansion layer:
original text, normalized errors, symbols, paths, and caller filters without text.
A single-term query cannot be widened; report a fallback even when it remains
empty only when the effective query actually changed.
Add test IDs only when the corpus demonstrates useful coverage.

Every leg preserves explicit repository, trust, kind, state, date, exclusion and
signal constraints. Derived signals cannot replace an explicitly selected field
or widen its existing repeat-value semantics. Deduplicate identical legs and
source documents; cap work, candidates, per-thread contributions, and output.

Candidate generation and ranking are separate. Keep the original strict answer
available. Score against the whole question rather than a leg's single filter.
Never turn on recency decay, imported weights, or embeddings without a frozen
CodeTrial measurement showing an improvement. Record why each result appeared.
A filters-only page full of recent sign-offs is not a useful answer even when
it contains the right PR number somewhere.

### Progressive disclosure must have a working next step

Keep context small and deterministic, with stable evidence keys and source URLs.
Expose structured query/coverage metadata alongside the readable brief. An
omitted section must offer a precise supported follow-up; a shortened source
must be retrievable in full through the existing show command. If a new
thread-focus view is justified, focus orders an already selected thread rather
than accidentally filtering every comment away.

Selection and diversity caps run in SQL (`--candidate-limit`); keep them there
as snapshots grow. Materialize FTS5 rank before applying window functions, and
validate the minimum supported SQLite version instead of assuming every host
supports the same SQL. Select the best evidence before applying a
presentation-only newest sort, so recency does not pick LGTM as a
representative.

### Inflight work is a relation, not just path overlap

Use existing closing-reference edges, explicitly labelled as closing references
in PR body text, to show other open PRs claiming the same
issue even when their file lists differ. This does not cover commit-message or
GitHub sidebar links; a missing body-text edge cannot rule those out.
Keep same-file work as a separate,
weaker signal, identifying whether a path came from a diff, an inline anchor,
or prose. A mention is not evidence that a thread changed that file.

Show returned and known-total counts and the collection boundary. Where file
lists or closing relationships are incomplete, absence is unknown. Do not emit
"nobody is working on this" from a capped or stale list.

## Freshness is not one timestamp

The release time describes packaging. Poll start is a source cursor; pass end
is collector telemetry. Neither guarantees every thread's detail is current.
Raw fetched timestamps currently describe stored payload changes, not a full
last-checked time, because unchanged payloads return early.

Persist narrowly defined per-thread detail checks, their checked head, and failed
attempts in a KEPT table keyed by thread, so index rebuilds cannot erase them.
Keep known/collected file counts derived from the stored PR and file payloads.
Check the PR head on both sides of the detail fetch before recording a successful
head-consistent check; a moved head remains stale without committing mixed data.
Legacy rows have unknown check time until actually refreshed.
Report missing or failed detail explicitly. Preserve
separate poll, detail, sweep and authority outcomes. Do not label a thread fully
fresh unless all components required for that assertion were actually checked.
Queries stay local; a targeted refresh, if needed, is explicit and separately
budgeted. A read path must not silently turn into a new backfill or network walk.

These changes affect the snapshot schema: update source compatibility and safe
schema validation deliberately, add migrations and fixtures, and verify that a
previous artifact either migrates safely or is clearly refused. Do not weaken
the exact-schema check merely to accept unfamiliar SQLite objects.

## Measure four different outcomes

1. Bootstrap: cold backfill versus verified download/install/catch-up; record
   actual elapsed time, bytes, API calls, tool versions and failure/retry costs.
2. Retrieval: rationale, failure, precedent and inflight slices over a frozen
   CodeTrial corpus, with source keys, judgement provenance, leakage declarations
   and fixed scope. Measure document relevance, Recall@k, MRR, empty pages,
   diversity, noise and correctness of cited reasons. Score the existing search
   before expansion or ranking changes. Label model judgements as model-made.
3. Agent flow: memory versus GitHub-plus-checkout, same task/model/budget, repeated
   runs. Record calls to first verified evidence, output bytes, repeated lines,
   rerolls, unseen citations, and total time/tokens without attributing trajectory
   variance to one tool change. Both arms retain current-code verification.
4. Review quality: blinded human adjudication of paired findings, false positives,
   valid defects, citation applicability and protected-subject misses. This needs
   genuinely time-correct historical evidence or a prospective collection; filtering
   today's latest payloads cannot reconstruct missing historical state. Cutoffs apply
   to signal tables, relationship/merge state, authority and scoring as well as text.
   Current-tree annotations are not historical-state evidence. A post-filter cannot
   undo ranking already influenced by future signals; exclude unverifiable cases.

Current-state retrieval evaluation does not require pretending to reconstruct a
past review. Keep it separate from retrospective review-quality claims. Freeze
before tuning, keep a holdout, disclose same-thread/query leakage, and do not
average leaking and non-leaking slices. The earlier 80%, 90% and 25% targets
remain hypotheses, not publication gates or reported improvements.

## Delivery order and non-goals

The evidence/coverage and safety tests and the frozen retrieval controls ship
(see evaluation.md). Implement bounded expansion, inflight links, per-thread
freshness and focused context in small measured changes. Build the
publisher using the already verified snapshot/bootstrap path, with bounded,
resumable preparation and limited retention. Do not delay release distribution
on a statistically conclusive human experiment; do not claim it improves review
quality merely because it is faster.

The publisher restores only a verified artifact or a trusted resumable checkpoint,
separately records partial preparation, and publishes only a fully eligible clean
snapshot. Actual API measurements decide batch sizes and sweep cadence. Automated
cleanup covers obsolete memory releases and interrupted staging without touching
product releases or other stores. A failed validation leaves the last good
snapshot available.

A server migration and model-written canonical evidence are non-goals.
Embedding retrieval, recency weighting and parser infrastructure require separate
CodeTrial measurements before being considered. No relore code, package, CLI,
API or deployment becomes part of CodeTrial.

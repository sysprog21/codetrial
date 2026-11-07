# Review memory snapshots

The distribution contract is source format 3. It is separate from `VERSION`,
which controls disposable search indexes. Readers accept source format 3 only;
a different format is refused before the database is opened. An index version
change rebuilds derived tables from the saved source records without GitHub calls.

The official release tag is `review-memory-<YYYYMMDDTHHMMSSZ>-<run ID>`.
Its assets are `memory.db.gz`, manifest.json, and `SHA256SUMS`. Both the gzip
and manifest require provenance from the repository's publisher workflow on main.
The digest file is a convenience, not proof of origin. Memory releases do not
change the product latest pointer.

A publisher constructs a new database from current public `raw` records of
kinds issue, issue_comment, review_comment, pull, review, files, and commits,
the authority list, and completed poll and sweep marks. These marks come from
`sync_state.mark`, not the completion time in `sync_state.at`.
It rebuilds the derived tables. Retained edit/deletion history and locally
authored decision summaries are excluded. In-progress walk/backfill marks and
stale PR detail make a source ineligible. An empty source is ineligible too.
This is a snapshot of what was collected, not a claim that upstream cannot
change or delete something after collection.

The publisher persists authority provenance explicitly before building a
snapshot; a populated authority table alone does not establish its freshness.
Publishing the push list exposes collaborator logins otherwise available only
to authorized GitHub API readers; these logins support reproducible trust
classification and must be approved as part of the publication policy.

The signed manifest contains repository, source_format, index_version,
generator_commit (40 lowercase hex digits), release_id, generated_at,
passes (completed checkpoint timestamps), counts (raw, threads, documents,
stale_prs, incomplete_files, redacted_records), authority_source (push-list or badge-fallback),
compressed_bytes, database_bytes, and sha256 of memory.db.gz.
Timestamps are UTC in `YYYY-MM-DDTHH:MM:SSZ` form. Poll and sweep checkpoints
are mandatory, cannot exceed generation time, and keep their original values.
The sweep must be at most seven days old when the artifact is generated.
Incomplete file coverage is disclosed rather than silently represented as full.
Generation time permits at most ten minutes of clock skew into the future.
Unknown manifest fields are ignored; duplicate JSON keys are refused by loaders.
The download limit is 512 MiB compressed and 2 GiB uncompressed, enforced on
actual bytes read, independently of declared sizes.

A corrupt, unsigned, incompatible, incomplete, wrong-repository, or oversized
snapshot must leave existing user data untouched. Bootstrap never replaces a
populated local store. Missing or unverifiable history is reported as degraded;
manual backfill remains available. Public release publication and live credentials
are separate from credential-free validation.

## Building an artifact

Run the snapshot command against a dedicated store synchronized from a public
repository. The command checks GitHub's repository visibility, refuses pending
PR detail and unfinished passes, and requires a successful sweep within seven
days. It refuses an existing output directory. The release ID and generator
commit are explicit inputs; no timestamp is substituted for a sync checkpoint.

A stored record keeps only the payload fields memory reads (`SOURCE_FIELDS` in
memory.py): the identity, author, timestamps, text, diff locators and patches.
API links, avatars, reactions and the head and base repository objects are
dropped on the way in, so the derived tables are unchanged by the trim. A store
or snapshot built before index version 11 is trimmed once when opened, and an
imported redaction count drops with any placeholder the trim removed; a field
the trim does not keep needs a fresh collection to come back. Public copies apply a deterministic,
credential-only projection across decoded strings, including patches and hunks.
Matched spans are replaced with typed placeholders; unrelated text, CRLF and
model delimiters remain byte-for-byte unchanged. Matching also examines the
canonical rendered view, so hidden-character-split tokens are redacted as whole
spans. Credential-bearing dictionary keys or structural locators are refused
rather than renamed. Commit email addresses remain unless they match a detector.

The kept `public_projection` table records each projected raw key and the number
of fields redacted. Manifest counts disclose `redacted_records`. Re-exports retain
this metadata. Incremental ingestion first compares exact source bytes, then
compares the same projection for previously projected records. Unchanged public
listings therefore do not schedule PR detail. Actual source changes replace the
record using the same projection, preserving body hashes across unrelated edits.
Projection metadata clears when no redactions remain. If a new locator or key
cannot be projected, actual source is retained locally and export is refused. No original credential-bearing hashes
are published. Public stores use hashes of their stored bodies for local notes;
public artifacts exclude the producer's local decisions entirely. Changes confined
to masked spans cannot be distinguished by a public store; its evidence concerns
the visible projected text, not the original hidden value.

A projected record's hunk applicability is unknown, even if the redaction affected
another field in that record. A literal placeholder in unprojected source does
not trigger this rule. Publication checks the projected database again and refuses
remaining recognizable credentials. This does not certify arbitrary sensitive
content as safe. Missing/binary/omitted patches cannot be scanned because GitHub
did not supply those bytes.
A fresh rebuild excludes retained deleted/edited text and local summaries
from both tables and unused database pages. The authority source records whether
the push list was available or badge fallback was used. Failed refreshes retain
an earlier known push list for local reviews, but block snapshot publication.
The authority_source and authority_attempt metadata accompany the list. A sweep
with an unconfirmed deletion does not advance its successful checkpoint and
blocks publication until a later sweep confirms every missing record.

## Bootstrap and catch-up

Run `memory.py ensure` from a checkout. It paginates published releases, verifies
managed candidate manifests, and chooses among the newest verified compatible
memory releases by generation time. The overall ensure deadline bounds discovery;
a repository with many product releases can require multiple API pages.
It then verifies memory.db.gz against the repository's publisher workflow,
`refs/heads/main`, GitHub-hosted runners, the generator commit, and the manifest
digest. Verification requires a gh version supporting those attestation flags;
unsupported tooling degrades instead of bypassing verification.

Downloads follow only GitHub HTTPS redirects, carry no authentication headers,
and enforce actual byte limits. Staging is private and adjacent to the store,
with free space checked before the database download. Manifest JSON rejects
invalid types and duplicate keys. Database objects must exactly match the locally
known safe schema before integrity, counts, checkpoint, and provenance checks.
An older or newer index is accepted only if its physical schema is known; it is
then rebuilt locally. An unknown schema is refused regardless of its version.

A safely empty store is checked through SQLite, including hot-journal recovery,
not by file existence. Installation copies only verified canonical tables in one
SQLite transaction while the sync lock is held, with deadline checks and rollback.
Private `local_review_*` tables and their protective marker survive bootstrap;
a store containing only these experiences is eligible for its first snapshot.
Public snapshots and publisher checkpoints omit those tables and the marker.
Other populated or ambiguous stores are preserved.
The ensure command refuses platforms without fcntl locking; the older manual
sync commands keep their existing platform behavior.

The command emits one JSON object on stdout: ready (exit 0), degraded (exit 2),
or error (exit 1), for syntactically valid invocations. Argparse usage errors
retain standard exit 2 and write usage to stderr rather than a result object.
Ctrl-C exits 130 and may produce no result JSON; treat it as incomplete preparation.
Callers must explicitly handle exit 2 to continue without
fresh history. Network, verification and time-budget failures are degraded;
a corrupt local store or an unavailable exclusive lock is an error. The latter
has code `busy`; callers may read existing local context while the writer runs.
Other storage errors leave history unavailable until inspected. Progress
goes to stderr. The default total budget is 60 seconds with at most 30 PRs of
detail. On Unix the deadline interrupts blocking operations and rolls back an
uncommitted page or PR; committed progress remains. Use `--budget 600` for a
large initial artifact on a slow connection. Downloads restart after interruption;
API synchronization resumes. If a local index rebuild exceeds the budget,
run status explicitly to finish that migration or increase the ensure budget.
Automatic review/triage context uses `--no-migrate`, opening a ready index
read-only without waiting; an unfinished migration leaves history unavailable
instead of restarting without a deadline. An unavailable authority refresh
keeps cached roles but returns degraded with code `authority-unavailable`.
Budget exhaustion is distinct from an upstream failure and is reported as
degraded, not as a corrupt store. No background backfill starts automatically.
Interrupted staging directories for the same database are reclaimed on the next
ensure while its exclusive lock is held. Other databases use different prefixes.

`--offline` reports available history without making network calls.
`--bootstrap-only` installs a verified snapshot if needed and leaves catch-up to
the caller. Existing data is never refreshed by replacing it with another
release. A store without a checkpoint requires an explicit manual backfill.
Status and context expose successful pass checkpoints, attempt outcomes, missing
PR detail, incomplete file lists, authority source and bootstrap provenance.
Generation time is always distinct from evidence freshness.

## Evidence output

Prose is redacted before derived indexing; exact source locators remain unchanged for identity checks. Local unprojected
hunks retain their bytes; projected hunks report applicability as unknown. Output renders hidden
characters in locator fields visibly rather than pretending they name a different
file. Old retained history is scrubbed and redacted when displayed as well.
Raw payloads and raw-source decision hashes do not change during sanitizer/index
upgrades, so unchanged source polling does not trigger refetches. Local annotations
show source snippets first; stale or path-unsupported annotation text is withheld.
Machine exclusion uses GitHub account metadata and cannot identify hidden machine
authorship behind a human account. Structural open-work entries label author trust, including bot-owned work so
competing work is visible. Fix retrieval excludes machine-authored PRs by default.
Neither structural records nor their titles are maintainer rulings.

If source text becomes exactly equal to a previously stored placeholder, the
conservative projection marker may remain until a later source change. This
continues to report applicability unknown rather than infer original hidden text.

Maintainers: see [publishing.md](publishing.md) for workflow permissions, resumable
collection, release retention, rollback and complete takedown.

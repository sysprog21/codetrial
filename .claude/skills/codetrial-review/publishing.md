# Publishing and recovering review memory

This describes `.github/workflows/review-memory.yml`. Remote operation requires
that reviewed workflow and helper to be merged to upstream main and enabled.
A local test does not establish that an official release exists or that a signed
download has been measured. See [snapshots.md](snapshots.md) for the client and
public-content contract.

## Permissions and cadence

The upstream main workflow runs every six hours and on manual dispatch. It uses
one non-cancelling concurrency group. Collection has contents/actions/attestations
read permissions. A separate checkpoint signer has OIDC and attestations write,
and verifies opaque package digests without opening SQLite. Only publication and
maintenance have contents write. No job accepts a PR trigger or executes source
text from a database. New attestations use GitHub-hosted runners and the exact
main workflow; the producer's commit is bound into each artifact.

Preparation has a 900-second source budget, a maximum batch of 80 PR details,
and a separate 180-second checkpoint/build budget within a 30-minute job. A ready
release less than seven days old normally skips collection. A pending signed
checkpoint continues even when that release is fresh. This allows an initial
backfill to span several runs instead of assuming a single hourly token quota
can cover the repository. Partial detail, an unfinished listing, a failed sweep,
or unauthorized authority fallback prevents ready publication.

A sweep must complete in one preparation run. Before starting it, the helper
estimates listing/review calls plus reserve and checks remaining core quota.
A skipped or interrupted sweep leaves a pending checkpoint for the next run.
If repository growth makes a complete sweep exceed one run's quota or time,
add resumable sweep collection or revise the quota/cadence before relying on it.
A completed sweep must be at most seven days old at publication.

GitHub CLI must support `gh attestation verify` with signer-workflow, source-ref,
source-digest and deny-self-hosted-runners flags. The workflow uses its scoped
GitHub token. A personal token measurement does not establish that this token
can read collaborators. If the push-permission list is unavailable, the first
publication requires the distinct `allow_badge_fallback` dispatch input. That
consent persists in a signed pending checkpoint bound to the current push-list
release digest, or in a published badge-fallback snapshot. A different newer
push-list release requires new consent. A previously known push-list baseline requires new explicit consent
before downgrade. Inspect the source mode before authorizing it.

For maintainer recovery, use an authenticated account authorized to manage the
upstream workflow and releases. A selected-repository fine-grained token needs
Actions write (enable/disable, dispatch, cancellation and artifact deletion),
Contents write (release/tag changes), and Attestations read for verification.
Workflow jobs use their own narrower permissions, not this recovery token.
The local CLI checked during implementation was gh 2.101.0; use a version with
all listed commands and verification flags. Confirm capabilities without writes:

```sh
gh --version
gh auth status
gh attestation verify --help
gh workflow enable --help
gh workflow disable --help
gh workflow run --help
gh run cancel --help
```

## First build and routine failures

Inspect runs and releases before starting work:

```sh
gh run list --repo sysprog21/codetrial --workflow review-memory.yml
gh api --paginate 'repos/sysprog21/codetrial/releases?per_page=100' --jq '.[].tag_name'
```

After the workflow is installed, a maintainer can dispatch a forced build:

```sh
gh workflow run review-memory.yml --repo sysprog21/codetrial --ref main -f force=true
```

Use `-f allow_badge_fallback=true` only after deliberately accepting badge-based
trust classification. `force=true` bypasses the weekly freshness skip; it still
restores verified progress. `fresh=true` bypasses both checkpoint and release
restoration and starts a full collection. It still verifies the release manifest
metadata to bind authority consent to the existing baseline. A corrupt manifest
must be removed deliberately before rebuilding. It does not bypass source validation,
redaction, authority consent, or completed-pass requirements.

Preparation reports `ready`, `partial`, or `skipped` and a stable machine code.
A collector rate limit or source deadline preserves committed progress in a
fresh public checkpoint. Only the successful checkpoint signing job makes it
restorable; the whole workflow need not succeed. A failed release upload can
therefore retain usable collection progress. The next six-hour run resumes it.
Failures during input verification stop before collection and do not become a
cold rebuild. Investigate `unverified`, `invalid-checkpoint`, network and tooling
errors before deliberately choosing a fresh rebuild.

A disposable index-version change can resume a checkpoint with the same verified
physical schema and source/checkpoint formats. The restored copy rebuilds locally;
raw data, stale queues and sync cursors survive without refetch. Unknown physical
schemas still fail closed before migration.

Checkpoint Actions artifacts expire after seven days. They contain projected
current public payloads, authority, stale queues and sync cursors; they exclude
local history and decisions. Ready handoffs expire after one day. These are
separate from PR caches. If no compatible input exists, start a new collection;
an incompatible attested checkpoint is discarded after checking its provenance,
and collection resumes from a compatible release or a new store. Invalid,
unverified and network failures still stop rather than triggering a cold rebuild. Never edit a cursor or digest to force reuse.

For a bad newest checkpoint, disable and drain the workflow, list artifacts as
shown below, and delete the offending artifact by reviewed ID. For a bad release,
delete the reviewed release by ID, then its exact tag ref only if it remains
(takedown step 2 below). Confirm the corrupt input is gone, then re-enable
and rerun to restore the previous verified input. An explicit `fresh=true` rebuild
bypasses data restoration but still verifies the release-policy baseline; it
does not make a corrupt retained artifact safe. Never
wait seven days for expiry as the only recovery mechanism.

A failed signer can be re-run with the original prepare artifact: verification
uses the producing attempt, and restore accepts successful signing in a later
attempt of that same run. If main moves during upload, promotion reports
`skipped/superseded` and removes its draft. A newer unpublished checkpoint resumes
on the next preparation even if an older public release is still fresh.

The public projection reports redacted-record counts. A credential-bearing key
or structural locator blocks export and identifies the raw key without printing
the value. Correct that source upstream, refresh it, and rebuild. Do not add a
source-specific secret exemption. Publication emits no original secret-derived
fingerprints. A missing patch remains unknown rather than evidence of absence.

## Releases, retries and rollback

Tags are `review-memory-<UTC timestamp>-<run ID>`. No second copy is published
under a fixed tag: the versioned release is the only one, and the one clients
discover.

Attestation and validation precede draft creation. Uploads are checked against exact digests before the
draft is published with `--latest=false`. The product latest pointer stays with
product releases; both discovery paths paginate past more than 100 releases.
An interrupted publication attempts to delete only its still-draft release.
A late main-head check refuses promotion by superseded producer code.

Maintenance keeps the newest verified usable release of the current source
format and removes every older verified one.
A signed manifest alone is insufficient: the database asset must exist and its
size and GitHub digest must match, or its bytes are downloaded and verified.
Unverified or incompatible entries are left for deliberate inspection and do
not count as the retained release. Only exact managed memory
names are candidates. A draft is removed after manifest provenance is established,
or after proving that it is at least an hour old, authored by github-actions[bot],
and tied by its tag run ID and full target SHA to a completed upstream-main run
of this publisher on schedule/manual dispatch. Active runs, foreign authors and
unproven drafts are preserved for manual inspection. This also cleans a killed
upload whose manifest never arrived. Release and tag deletion are separate, so
a missing tag does not make an already-deleted release look like a failed purge.
An interruption between those operations can leave an orphan tag requiring
manual inspection.

Only one release is retained, so an older release to roll back to exists only
until maintenance runs after a newer publication; otherwise use the complete
takedown below. For rollback, first disable and drain the publisher as described
below. Verify a retained older release using the client provenance rules, then remove only
the affected newer releases using the ID/ref commands in takedown step 2. For a content rollback, also delete every checkpoint
and ready handoff newer than the retained release; otherwise the next preparation
can republish the rejected content. Correct upstream source or the projection
policy before resuming: a routine poll deliberately catches up to current source.
Use the complete takedown below if the affected inputs cannot be bounded.

On an isolated database path that does not exist, run `ensure --bootstrap-only --budget 600`
to verify the newest remaining compatible snapshot. Require JSON `outcome: ready`,
`code: ready` and a `status.passes.bootstrap` mark for the intended retained
release. A populated path skips bootstrap and cannot validate a rollback.
Existing populated stores and local decisions remain available for inspection.
After correcting the cause and verifying the retained artifact, explicitly
re-enable the workflow and inspect the next run before normal operation resumes:

```sh
gh workflow enable review-memory.yml --repo sysprog21/codetrial
```

Deleting a release does not repair existing clients or retract downloaded bytes.

## Takedown and clean rebuild

A maintainer performs these remote writes only after choosing the affected
records and releases. Do not leave a publisher running while purging inputs.

1. Disable the workflow, cancel every active or queued run, and wait until all
   runs are completed. List every API page, including queued/manual reruns:

   ```sh
   gh workflow disable review-memory.yml --repo sysprog21/codetrial
   gh api --paginate 'repos/sysprog21/codetrial/actions/workflows/review-memory.yml/runs?per_page=100' --jq '.workflow_runs[] | select(.status != "completed") | .id'
   ```

   For each returned ID, use `gh run cancel RUN_ID --repo sysprog21/codetrial`.
   Repeat the listing until empty before continuing; cancellation is asynchronous.

2. Inventory every Actions artifact page and every managed memory release,
   including drafts, unverified and incompatible releases, and the legacy
   `review-memory-latest` release. For a complete takedown, purge the legacy
   release and the entire exact managed release namespace; maintenance
   deliberately leaves unverifiable inputs alone and cannot perform this step. Delete
   all retained `review-memory-checkpoint-<run>-<attempt>` and
   `review-memory-ready-<run>-<attempt>` artifacts, including unsigned handoffs,
   so a queued rebuild cannot restore the removed source. Recheck after draining.

   ```sh
   gh api --paginate 'repos/sysprog21/codetrial/actions/artifacts?per_page=100' --jq '.artifacts[] | [.id, .name] | @tsv'
   gh api --paginate 'repos/sysprog21/codetrial/releases?per_page=100' --jq '.[] | select(.tag_name | test("^review-memory-([0-9]{8}T[0-9]{6}Z-[0-9]+|latest)$")) | [.id, .tag_name, .draft] | @tsv'
   gh api --method DELETE repos/sysprog21/codetrial/actions/artifacts/ARTIFACT_ID
   gh api --method DELETE repos/sysprog21/codetrial/releases/RELEASE_ID
   gh api repos/sysprog21/codetrial/git/matching-refs/tags/MEMORY_TAG --jq '.[].ref'
   ```

   Only if the exact `refs/tags/MEMORY_TAG` ref remains, delete it separately:

   ```sh
   gh api --method DELETE repos/sysprog21/codetrial/git/refs/tags/MEMORY_TAG
   ```

   Substitute reviewed IDs/tags; never delete product releases or unrelated
   artifacts. Repeat both listings and confirm no matching handoff or managed
   or legacy release remains before re-enabling. A signature proves origin, not revocation.

3. Correct or remove the source upstream. Review and merge any collector or
   projection fix. Re-enable the workflow and immediately dispatch with
   `fresh=true` and `force=true`; use the authority-consent input if required.
   All prior checkpoints must have been purged first. A coincident scheduled
   run has no retained inputs after the complete purge and can collect only from
   corrected upstream source. A fresh collection still spans runs if budgets
   require it. Clients have no official fallback until a replacement publishes.

   ```sh
   gh workflow enable review-memory.yml --repo sysprog21/codetrial
   gh workflow run review-memory.yml --repo sysprog21/codetrial --ref main -f fresh=true -f force=true
   ```

4. Inspect the ready manifest, completed passes, authority mode and projection
   counts. Verify both manifest and database provenance/digest through an empty
   `memory.py --db PATH ensure --bootstrap-only --budget 600` invocation where PATH does not
   exist. Require JSON `outcome: ready`, `code: ready`, and the expected
   `status.passes.bootstrap` release mark. No older official release remains
   after this complete purge.
   Once confirmed, routine scheduled operation can continue. If verification fails,
   disable and drain again before further repair.

Already downloaded local databases and copies cannot be erased remotely.
Tell affected developers to preserve local decisions separately and create a new
store from the corrected release; an attestation does not retract old bytes.

## Credential-free rehearsal

```sh
python3.12 scripts/run-python-tests.py tests/test_review_memory_publish.py
python3.12 scripts/run-python-tests.py tests/test_review_memory.py
python3.12 scripts/check-skill-refs.py
```

Use Python 3.9 or newer for the test harness (the examples use 3.12).
These use local fixtures for interrupted collection, refused verification,
checkpoint projection/corruption, opaque signing handoffs, byte limits,
namespace/pagination and retention. They do not create a GitHub release or
prove a real token's permission, signed-download latency or review quality.

The bootstrap release identity is inside the JSON string at
`.status.passes.bootstrap.mark`: parse it with `fromjson` to inspect `release_id`
and `sha256`. Keep the test path separate from real stores, and remove its
database and adjacent `.lock` file after inspection.

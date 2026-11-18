# Measuring bootstrap without inventing a speedup

`benchmark-memory.py` compares frozen-state backfill with an explicitly
unattested local ensure. Both arms must produce the same projected corpus and
public index. This is a pipeline-consistency experiment with fixture overhead;
it measures neither GitHub network latency nor cryptographic verification.
Keep it separate from live collection and eventual signed-release measurements.

## Inputs and invocation

Use a closed original local collection and its clean snapshot at one corpus
boundary. The original must have no bootstrap/projection markers or journal
sidecars. Copy a live store consistently with SQLite backup before measuring;
do not point the experiment at a store that another process updates. The harness
reads the original in read-only mode and checks its private file hash before
and after. Original source hashes, raw payloads and collaborator identities do
not appear in the report.

The snapshot must have been built by exactly the collector bytes being tested,
with the same source format and index version. Its generator commit must be
available in this checkout. Rebuild a local snapshot at the committed collector
when those differ; never relabel an old artifact's producer or format. The
snapshot builder still requires genuinely completed source passes. A harness
does not make an incomplete store publishable.

From the repository root, with Python 3.9 or newer on a Unix host:

```sh
BENCHMARK_RESULTS_DIR=$(mktemp -d)
python3.12 .claude/skills/codetrial-review/benchmark-memory.py \
  --source-db ORIGINAL_DB --snapshot LOCAL_SNAPSHOT_DIRECTORY --trials 3 \
  > "$BENCHMARK_RESULTS_DIR/results.json"
```

Paths are explicit; the default development store is never a measurement target.
The current harness requires push-list authority in both inputs. It uses private
temporary fixtures and deletes them when finished. Keep the original collection,
raw captures and unsanitized failure logs out of commits and shared reports.

## What the fixture proves

The state fixture derives current API responses from stored original payloads.
It is not a replay of their chronological HTTP capture. Listings are ordered by
updated timestamp and numeric source ID (issue-number fallback for fixtures),
with inclusive `since`, fixed page sizes, and exact query/argv grammar. This is
a stable order by construction; GitHub's real tie behavior is not validated.
Paginated detail responses are emitted as separate JSON arrays and parsed by
the real collector. Unmatched requests are recorded as fatal misses, using a
neutral error rather than pretending an upstream deletion or missing resource.
A skipped detail request cannot become a successful measurement.

The collector's subprocess adapter permits only fixture `gh` calls and invokes
the frozen runner through an absolute interpreter argv. It does not use a
shebang/PATH shim, so interpreter or runner paths containing spaces cannot
fall through to a real GitHub CLI. Git metadata
is read from the script checkout with inherited `GIT_*` overrides removed. The ensure arm reads two exact local artifact paths;
attestation checks are stubbed only inside the benchmark child process. The
production CLI gains no bypass flag. Each arm starts with a new scratch database
and interpreter; trials alternate order. Collector and harness bytes are frozen
into a private runner so concurrent checkout edits cannot change a later arm.
The verified local manifest/database bytes are also frozen into private scratch. Only the collector's `now()` is frozen.
Budgets and timings continue using the real monotonic clock.

After each arm, collection must have no pending detail or listing cursor and
must preserve authority mode. Public raw payloads and projection metadata must
match the snapshot, excluding fetched timestamps. A fresh projected rebuild of
the backfill result must match the snapshot's documents, threads, signals and
FTS contents without depending on insertion-order IDs. The ensure arm checks
its installed index directly, without rebuilding away a bad result. It also
requires zero changed raw records and zero PR-detail requests during catch-up.
This checks an unchanged source state, not the cost of an actual edit.

## Interpreting output

The JSON records every sample with iteration/order and load before/after,
medians/ranges of wall/process/service times, collector and harness hashes,
producer/current commits and dirty status at start/end, Python/SQLite versions,
the observed parent gh version (API not exercised), host conditions,
cache limitations, SQLite journal/synchronous settings, corpus counts and the
projected corpus/index identity. No original credential-bearing fingerprint is
shared. The artifact byte count is the would-be database download size, excluding
manifest and attestation traffic; network bytes are zero and bandwidth is
unmeasured. The report pins the manifest/gzip hashes and frozen source clock.

`wall_s` includes Python fixture process startup/import and local service time.
`gh_process_s` measures that entire subprocess cost; `fixture_service_s` is only
its fixture query/encoding subset. These are simulated gh invocations and
response pages, not HTTP requests. Phase times are inclusive and nested, so do
not sum them. Validation/digest gates after the collector are outside the timed
region. Attestation work is explicitly stubbed, not measured as zero-cost real
verification. OS caches are uncontrolled and generally warm. Do not describe
these samples as a production latency bound or emit an inferred speedup ratio.

Record live backfill/poll/detail/sweep observations separately with immutable
per-invocation responses, actual start/end times and the token type. Distinguish
gh invocations, returned response pages and any separately instrumented HTTP
requests/retries. Missing timing or capture evidence stays unknown. This harness
still uses a state-derived fixture, not a chronological response trace.

After deployment, measure actual signed download, attestation, installation and
catch-up from a fresh clone. Check real workflow-token permissions separately.
Targets such as 80% faster readiness or 90% fewer calls remain hypotheses until
those measurements exist. Bootstrap timing, retrieval scores, repeated agent
flows and blinded review quality are separate experiments.

The first failed arm aborts the entire run and emits no successful result JSON;
previous samples are not reported as a completed experiment. Diagnostics name
the arm and failure class or static gate, without forwarding source tracebacks.
Malformed hashes and nonnumeric, negative or nonfinite timing values are rejected
before aggregation.

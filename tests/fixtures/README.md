# Cross-implementation fixtures

Every data-channel topic has two implementations: a producer in `web/lib.js`
and a consumer in `src/agent.rs`. Each was only ever tested against itself,
which is how the integrity hash diverged. Both sides passed their own tests,
the agent refused every event the browser sent, the gate stayed green, and
every camera interview shipped a report whose evidence section read
"(none captured)".

These files are real producer output fed to the real consumer. They are the
only thing in the repo that crosses that boundary.

| File | Producer | Consumer |
|---|---|---|
| `code-update.json` | `codeUpdatePayload` | `apply_code_update` |
| `control.json` | `timeWarningPayload`, `endInterviewPayload` | `apply_control` |
| `test-results.json` | `testPayload` | `apply_test_results` |
| `integrity-chain.json` | `integrityEventPayload` | `apply_integrity` |

## Regenerating

```sh
node scripts/gen-wire-fixtures.mjs
```

Do not hand-edit them. A hand-edited fixture is a second implementation of the
producer, which is the problem these exist to catch.

The generator is deterministic: timestamps are fixed values rather than clock
reads, so `--check` can tell a stale fixture from a fresh one.
`scripts/test.sh` runs `--check`, so changing a producer without regenerating
fails the gate instead of waiting for someone to notice.

Regenerating on its own proves nothing. After a producer change, run
`cargo test --test agent`, which is where a real divergence shows up as a
failure naming what the two sides now disagree about.

## What the fixtures have to keep covering

- One integrity `detail` at the 80-character bound. That is the case that
  broke: the browser truncated to 160 and the agent normalized to 80 before
  recomputing the hash, so anything longer was unverifiable by construction.
- One case per language tab in `web/interview.js`. A tab the agent's allowlist
  does not know changes the editor and nothing else, and the interviewer never
  acknowledges the click.
- A test run that passed, one that failed, and one that could not start. The
  agent picks its reaction from `passed`, `total`, and `setupError`; a renamed
  field congratulates a candidate whose tests failed.

## Other fixtures here

`visual-shift.css` is not a wire fixture. It is a deliberate layout break used
by `scripts/visual-parity-check.sh` to prove the visual check can fail.

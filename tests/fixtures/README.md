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
| `report-recovery.json` | `reportRecoveryLimits` in `web/report-recovery.js` | `recovery_metadata`, `recovery_notice`, `REPORT_TIMEOUT` |
| `control.json` | `drawingActivityPayload`, `thinkingPayload`, `yieldTurnPayload`, `timeWarningPayload`, `endInterviewPayload`, `retryReportPayload`, `skipReportPayload`, `startupSyncPayload` | `apply_control`, `recovery_request`, `skip_request`, `handle_candidate_packet` in `src/livekit.rs` |
| `startup-receipt.json` | `startupReceipt` in `web/lib.js` | `is_startup_receipt` in `src/livekit.rs` |
| `test-results.json` | `testPayload` | `apply_test_results` |
| `integrity-chain.json` | `integrityEventPayload` | `apply_integrity` |
| `task-action.json`, `task-edit.json`, `task-revision.json`, `task-run.json`, `task-start.json`, `task-end.json`, `task-connected.json`, `task-thinking.json` | `taskActionPayload`, `taskEditPayload`, `taskRevisionPayload`, `taskRunPayload`, `taskStartPayload`, `taskEndPayload`, `taskConnectedPayload`, `taskThinkingPayload` in `web/lib.js` | the typed messages `TaskSession::receive` parses, checked in `tests/unit/tasks/session.rs` |
| `task-state.json`, `task-error.json`, `task-review.json` | written by the generator; the Rust side that must agree is `TaskSession::snapshot` (state), `ErrorMessage` (error) and `tasks::report` (review) | `web/task.js`, driven from these files by `tests/browser/task-workspace.test.js` and `task-controller.test.js`; Rust reads them in `tests/unit/tasks/session.rs` and `report.rs` |

`tests/fixtures/task-mode/` is not wire data: it is the packaged task set, its
encrypted package and the patches both the server and the packaging tool must
refuse, shared by the Rust and Python tests.

## Regenerating

Both startup payloads travel from the browser to the agent on `control`.
`startup_sync` asks the agent to resend its existing readiness state; it does
not request another opening generation. `startup_received` acknowledges the
exact startup failure bytes using their SHA-256 digest as `deliveryId`. The
agent accepts that receipt only from the interview's original candidate.

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

`framework-evaluation-cases.json` is a versioned product-policy corpus rather
than producer output. Each closed-shape scenario names its coverage, production
reaction path, required and forbidden policy phrases, trusted framework evidence,
hint count, round-gate result, and assessable phases. Add a scenario only when its
reaction assertions would fail for the wrong interviewer behavior and its evidence
assertions pass through `record_framework_evidence` and `final_report`; never paste
model-generated scores into this fixture. Run `cargo test --test agent
framework_evaluation_scenarios` to review it locally.

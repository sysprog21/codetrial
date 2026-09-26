#!/bin/sh
set -eu

# The fixture is named by the test, not by this script: the replay test reads it
# with `include_str!`, which resolves at compile time, so an argument here could
# only ever have been ignored. The test replays it twice for determinism and
# compares the result against tests/golden/evidence-ledger.json; regenerate that
# with UPDATE_EVIDENCE_GOLDEN=1 after a deliberate change.
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"
cargo test --lib agent::evidence_tests::evidence_ledger_replay_fixture_is_frozen_and_deterministic -- --exact

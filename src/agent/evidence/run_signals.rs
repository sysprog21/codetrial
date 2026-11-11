//! What one test run reports about itself, and what it does to the ledger: the
//! failure signature, whether the runner reached the cases, the diagnostic it
//! named, and the counts and deltas kept from it.

use serde::{Deserialize, Serialize};

use super::{EvidenceLedger, ObservationFamily, Provenance, TestState};
use crate::sha256_hex;

/// A digest of what failed, used only to tell one failing run from the next.
///
/// The separators are what make it a signature rather than a concatenation.
/// With one delimiter for everything, a run whose label was "a" and error "b"
/// hashed the same as a run with a single label that happened to contain the
/// delimiter, and two runs failing for different reasons then read as the same
/// failure twice. Field name, unit separator between fields, record separator
/// between failures.
pub(super) fn failure_signature(payload: &serde_json::Value) -> Option<String> {
    let failures = payload.get("failures")?.as_array()?;
    let mut material = String::new();
    for failure in failures {
        for key in ["label", "error", "expected", "got"] {
            if let Some(value) = failure.get(key).and_then(serde_json::Value::as_str) {
                material.push_str(key);
                material.push('\u{1f}');
                material.push_str(value);
                material.push('\u{1f}');
            }
        }
        material.push('\u{1e}');
    }
    if failures.is_empty() {
        None
    } else {
        Some(sha256_hex(&[material.as_bytes()]))
    }
}

/// Whether the runner said it never reached the cases.
///
/// The same field `apply_test_results` reads to decide which reaction the
/// interviewer gets, so the ledger and the spoken half agree on what happened.
/// The browser publishes a setup error and case outcomes as alternatives: the
/// paths that fill one return an empty `cases` array, so a run carrying this
/// graded nothing however many cases the judge spec holds.
pub(crate) fn run_failed_to_start(payload: &serde_json::Value) -> bool {
    payload
        .get("setupError")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|text| !text.is_empty())
}

pub(super) fn diagnostic(payload: &serde_json::Value) -> Option<(DiagnosticCategory, String)> {
    let structured_category = payload
        .get("diagnostic")
        .and_then(|value| value.get("category"))
        .and_then(serde_json::Value::as_str)
        .and_then(DiagnosticCategory::from_wire);
    let setup_error = payload
        .get("setupError")
        .and_then(serde_json::Value::as_str)
        .filter(|text| !text.is_empty());
    let category = structured_category.unwrap_or(DiagnosticCategory::Other);
    let material = setup_error
        .map(|text| text.chars().take(400).collect())
        .or_else(|| structured_category.map(|_| format!("structured:{category:?}")))?;
    Some((category, material))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::agent) enum DiagnosticCategory {
    Syntax,
    Type,
    Linker,
    RuntimeSignal,
    Timeout,
    Warning,
    Other,
}

impl DiagnosticCategory {
    /// The closed list a runner may name, in the words the wire spells them.
    ///
    /// One reading for both readers: the sanitizer that decides what survives
    /// ingest and the reducer that counts it. They each spelled the list out,
    /// and they drifted once already, the sanitizer allowing `other` while the
    /// reducer dropped it, so the reducer honored six of the seven categories
    /// the wire allows and nothing said so.
    ///
    /// Read through the derive above rather than a second list of the same
    /// words, which the compiler could not hold to the enum: a variant added
    /// there would have serialized fine and been dropped on the way in.
    pub(crate) fn from_wire(category: &str) -> Option<Self> {
        use serde::de::value::{Error, StrDeserializer};
        Self::deserialize(StrDeserializer::<Error>::new(category)).ok()
    }
}

/// Widened, subtracted, then clamped back. The fields are `i32` because a
/// delta is signed and small; doing the arithmetic in the narrow type is what
/// wraps.
fn delta(current: u32, previous: u32) -> i32 {
    (i64::from(current) - i64::from(previous)).clamp(i64::from(i32::MIN), i64::from(i32::MAX))
        as i32
}

impl EvidenceLedger {
    /// # A run that never ran
    ///
    /// A runner that fails to start produces no case outcomes, and the browser
    /// sends that the same way it sends a real result. Folding it into the
    /// deltas made a transient 503 from the compiler service read as two
    /// failures fixed, and then made the next run of the same untouched code
    /// read as three cases newly passing: a network error became progress. So
    /// the counts, the deltas and the progress flags belong to runs that
    /// executed, and a run that did not still records its attempt, its
    /// diagnostic and its entry.
    ///
    /// `total` alone is not what separates them. The browser fills `total` from
    /// the judge spec before it runs anything, so a compile error arrives as
    /// nought of five with an empty `failures` array, not as nought of nought:
    /// reading the count alone turned a C++ build failure after a green run
    /// into five cases newly failed and a regression, which is the reading this
    /// section exists to refuse. A setup error is the runner saying it never
    /// reached the cases, and the two together are what say a run executed.
    pub(crate) fn record_test(
        &mut self,
        source_timestamp_ms: Option<u64>,
        receipt_timestamp_ms: u64,
        payload: &serde_json::Value,
    ) {
        let total = payload
            .get("total")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
            .min(u64::from(u32::MAX)) as u32;
        let passed = payload
            .get("passed")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
            .min(u64::from(total)) as u32;
        let failed = total.saturating_sub(passed);
        let executed = total > 0 && !run_failed_to_start(payload);
        let signature = failure_signature(payload);
        let previous = self.tests.clone();
        let newly_passed = delta(passed, previous.passed);
        self.progress.runner_attempted = true;
        self.record_sequence("test");
        if executed {
            self.tests = TestState {
                total,
                passed,
                failed,

                // Widened before subtracting. Both operands are clamped to 99
                // by `sanitize_test_run` on every path the browser reaches, but
                // this reducer is public and takes the raw value, so the
                // arithmetic holds the invariant itself rather than borrowing
                // the caller's.
                newly_passed,
                newly_failed: delta(failed, previous.failed),

                // Only executed runs overwrite this state, and an executed run
                // has cases, so a total is what an earlier one leaves behind.
                follows_executed_run: previous.total > 0,
                regression: passed < previous.passed,

                // A streak needs a signature on both sides. Two failing runs
                // that named no failures at all are not evidence of the same
                // failure twice, and counting them as one drove the repeated
                // failure signal that asks the interviewer to step in.
                //
                // Cases that started passing end the streak whatever the
                // signature says. Only the first few failures are reported, so
                // a candidate who fixes six of ten and leaves the reported four
                // untouched hashes the same as before: the signal that says
                // they are stuck on one failure would fire at the moment they
                // made the largest gain of the session.
                consecutive_identical_failures: match (failed > 0, &signature) {
                    (true, Some(_))
                        if newly_passed <= 0 && signature == previous.failure_signature =>
                    {
                        previous.consecutive_identical_failures + 1
                    }
                    (true, _) => 1,
                    (false, _) => 0,
                },
                failure_signature: signature.clone(),
                source_timestamp_ms,
            };
            self.progress.tested = true;
            self.progress.test_progress |= self.tests.newly_passed > 0;
            if self.tests.newly_passed > 0 {
                self.stuck.last_test_progress_ms = Some(receipt_timestamp_ms);
                self.stuck.test_progress_latency_ms = self
                    .stuck
                    .last_meaningful_change_ms
                    .map(|meaningful| receipt_timestamp_ms.saturating_sub(meaningful));
            }
            self.progress.all_tests_passed_once |= passed == total;
            self.stuck.repeated_failure_count = self.tests.consecutive_identical_failures;
        }

        // An edit and then a run is one cycle; the Run button pressed twelve
        // times over an untouched buffer is one cycle and eleven retries, and
        // counting those as cycles told the report the candidate was iterating
        // when they were waiting.
        if self.code.candidate_edits > self.candidate_edits_at_last_test {
            self.stuck.edit_test_cycles += 1;
        }
        self.candidate_edits_at_last_test = self.code.candidate_edits;
        if let Some((category, diagnostic)) = diagnostic(payload) {
            self.record_diagnostic(
                source_timestamp_ms,
                receipt_timestamp_ms,
                category,
                &diagnostic,
            );
        }
        self.append(
            ObservationFamily::Test,
            Provenance::BrowserClaim,
            source_timestamp_ms,
            receipt_timestamp_ms,
            serde_json::json!({
                "executed": executed,
                "total": total,
                "passed": passed,

                // `failed` is the one count here the runner never reported: it
                // is `total` minus `passed`, so a setup error that arrives as
                // nought of five says five cases failed, when the runner is
                // saying it never reached them. `total` and `passed` are what
                // the browser claimed and stay as they are.
                "failed": if executed { failed } else { 0 },
                "newlyPassed": if executed { self.tests.newly_passed } else { 0 },
                "newlyFailed": if executed { self.tests.newly_failed } else { 0 },
                "regression": executed && self.tests.regression,

                // Every count on this entry describes this run, so a run that
                // produced no case outcomes carries none of the running state:
                // a streak printed beside `"executed":false` reads as two
                // identical failures in a run that never failed anything.
                "consecutiveIdenticalFailures": if executed {
                    self.tests.consecutive_identical_failures
                } else {
                    0
                },
                "failureSignature": signature,
            }),
        );
    }
}

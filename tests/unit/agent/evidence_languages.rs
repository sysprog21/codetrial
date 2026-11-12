//! The test deltas the ledger keeps, across a change of language.

use super::*;

fn run(language: &str, passed: u32, total: u32) -> serde_json::Value {
    serde_json::json!({ "language": language, "passed": passed, "total": total })
}

/// Switching tabs and running the starter there is not a regression in the code
/// that passed: the two runs execute different programs, so the second is
/// measured against nothing rather than against the first.
#[test]
fn a_run_in_another_language_is_not_measured_against_the_last() {
    let mut ledger = EvidenceLedger::default();
    ledger.record_test(Some(100), 200, &run("python", 5, 5));
    ledger.record_test(Some(200), 300, &run("java", 0, 5));

    assert!(!ledger.tests.regression, "{:?}", ledger.tests);
    assert!(!ledger.tests.follows_executed_run, "{:?}", ledger.tests);
    assert_eq!(ledger.tests.newly_passed, 0);
    assert_eq!(ledger.tests.newly_failed, 5);

    // Back in the same language the next run is compared to the one before it.
    ledger.record_test(Some(300), 400, &run("java", 2, 5));
    assert!(ledger.tests.follows_executed_run);
    assert_eq!(ledger.tests.newly_passed, 2);
    assert!(!ledger.tests.regression);
    ledger.record_test(Some(400), 500, &run("java", 1, 5));
    assert!(
        ledger.tests.regression,
        "a real drop in one language still is"
    );
}

/// A packet that names no language is the same program as the one before it.
#[test]
fn a_run_without_a_language_keeps_comparing_to_the_last() {
    let mut ledger = EvidenceLedger::default();
    ledger.record_test(
        Some(100),
        200,
        &serde_json::json!({ "passed": 3, "total": 3 }),
    );
    ledger.record_test(Some(200), 300, &run("python", 1, 3));
    assert!(ledger.tests.regression);
}

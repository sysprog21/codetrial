//! Test deltas and progress within one language and across language switches.

use super::super::analyze_code;
use super::*;

fn run(language: &str, passed: u32, total: u32) -> serde_json::Value {
    serde_json::json!({ "language": language, "passed": passed, "total": total })
}

fn edit(
    ledger: &mut EvidenceLedger,
    source_ms: u64,
    receipt_ms: u64,
    language: &str,
    before: &str,
    after: &str,
    candidate_edit: bool,
) {
    ledger.record_code_with_analysis(
        Some(source_ms),
        receipt_ms,
        language,
        after,
        candidate_edit,
        analyze_code(language, before, after),
    );
}

#[test]
fn first_executed_run_after_setup_failure_is_the_baseline() {
    let mut ledger = EvidenceLedger::default();
    ledger.record_test(
        Some(10),
        100,
        &serde_json::json!({
            "passed": 0,
            "total": 5,
            "setupError": "Compiler Explorer returned 503",
        }),
    );
    assert!(!ledger.progress.tested);
    ledger.record_test(Some(20), 200, &run("python", 2, 5));

    assert_eq!((ledger.tests.passed, ledger.tests.total), (2, 5));
    assert!(!ledger.tests.follows_executed_run);
    assert!(!ledger.progress.test_progress);
    assert_eq!(ledger.stuck.last_test_progress_ms, None);
}

#[test]
fn test_progress_without_an_edit_has_no_latency() {
    let mut ledger = EvidenceLedger::default();
    ledger.record_test(Some(10), 100, &run("python", 2, 5));
    ledger.record_test(Some(20), 200, &run("python", 3, 5));

    assert!(ledger.progress.test_progress);
    assert_eq!(ledger.stuck.last_test_progress_ms, Some(200));
    assert_eq!(ledger.stuck.test_progress_latency_ms, None);
}

#[test]
fn zero_pass_run_establishes_a_baseline() {
    let mut ledger = EvidenceLedger::default();
    ledger.record_test(Some(10), 100, &run("python", 0, 5));
    assert!(ledger.progress.tested);
    assert!(!ledger.progress.test_progress);

    ledger.record_test(Some(20), 200, &run("python", 1, 5));
    assert!(ledger.tests.follows_executed_run);
    assert_eq!(ledger.tests.newly_passed, 1);
    assert!(ledger.progress.test_progress);
    assert_eq!(ledger.stuck.last_test_progress_ms, Some(200));
}

/// Switching tabs and running the starter there is not a regression in the code
/// that passed: the two runs execute different programs, so the second is
/// measured against nothing rather than against the first.
#[test]
fn a_run_in_another_language_is_not_measured_against_the_last() {
    let mut ledger = EvidenceLedger::default();
    ledger.record_test(Some(100), 200, &run("python", 5, 5));
    assert!(ledger.progress.tested);
    assert!(ledger.progress.all_tests_passed_once);
    assert!(!ledger.progress.test_progress);

    let starter = "class Solution { int value() { return 1; } }\n";
    let edited = "class Solution { int value() { return 2; } }\n";
    let later = "class Solution { int value() { return 3; } }\n";
    edit(&mut ledger, 150, 250, "java", "", starter, false);
    ledger.record_test(Some(200), 300, &run("java", 1, 5));

    assert!(!ledger.tests.regression, "{:?}", ledger.tests);
    assert!(!ledger.tests.follows_executed_run, "{:?}", ledger.tests);
    assert_eq!(ledger.tests.newly_passed, 1);
    assert_eq!(ledger.tests.newly_failed, 4);
    assert!(!ledger.progress.test_progress);
    assert_eq!(ledger.stuck.last_test_progress_ms, None);

    edit(&mut ledger, 250, 350, "java", starter, edited, true);

    // The next Java run is compared with its baseline.
    ledger.record_test(Some(300), 400, &run("java", 2, 5));
    assert!(ledger.tests.follows_executed_run);
    assert_eq!(ledger.tests.newly_passed, 1);
    assert!(!ledger.tests.regression);

    edit(&mut ledger, 350, 460, "java", edited, later, true);
    assert_eq!(ledger.stuck.last_meaningful_change_ms, Some(460));
    ledger.record_test(Some(400), 500, &run("java", 1, 5));
    assert!(
        ledger.tests.regression,
        "a real drop in one language still is"
    );
    assert_eq!(ledger.stuck.last_test_progress_ms, Some(400));
    assert_eq!(ledger.stuck.test_progress_latency_ms, Some(50));

    ledger.record_test(Some(500), 600, &run("python", 1, 5));
    assert!(!ledger.tests.follows_executed_run);
    assert!(ledger.progress.test_progress);
    assert_eq!(ledger.stuck.last_test_progress_ms, Some(400));
    assert_eq!(ledger.stuck.test_progress_latency_ms, Some(50));
}

/// A packet that names no language is the same program as the one before it.
#[test]
fn a_run_without_a_language_keeps_comparing_to_the_last() {
    let mut ledger = EvidenceLedger::default();
    ledger.record_test(
        Some(10),
        100,
        &serde_json::json!({ "passed": 1, "total": 3 }),
    );
    ledger.record_test(Some(20), 200, &run("python", 2, 3));

    assert!(ledger.tests.follows_executed_run);
    assert!(ledger.progress.test_progress);
    assert_eq!(ledger.stuck.last_test_progress_ms, Some(200));
    ledger.record_test(
        Some(30),
        300,
        &serde_json::json!({ "passed": 1, "total": 3 }),
    );
    assert!(ledger.tests.regression);
}

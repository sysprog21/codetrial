//! Test deltas and progress within one language and across language switches.

use super::super::analyze_code;
use super::*;

fn run(language: &str, passed: u32, total: u32) -> serde_json::Value {
    serde_json::json!({ "language": language, "passed": passed, "total": total })
}

#[test]
fn first_executed_run_after_setup_failure_is_the_baseline() {
    let mut ledger = EvidenceLedger::default();
    ledger.record_test(
        Some(10),
        100,
        &serde_json::json!({
            "passed": 0,
            "total": 0,
            "setupError": "Compiler Explorer returned 503",
        }),
    );
    ledger.record_test(
        Some(20),
        200,
        &serde_json::json!({ "passed": 2, "total": 5 }),
    );

    assert_eq!((ledger.tests.passed, ledger.tests.total), (2, 5));
    assert!(!ledger.progress.test_progress);
    assert_eq!(ledger.stuck.last_test_progress_ms, None);
}

#[test]
fn evidence_ledger_measures_reversal_and_test_progress_latency() {
    let mut ledger = EvidenceLedger::default();
    let starter = "value = 1\n";
    let changed = "value = 2\n";
    ledger.record_code_with_analysis(
        Some(10),
        100,
        "python",
        starter,
        false,
        analyze_code("python", "", starter),
    );
    ledger.record_code_with_analysis(
        Some(20),
        200,
        "python",
        changed,
        true,
        analyze_code("python", starter, changed),
    );
    ledger.record_code_with_analysis(
        Some(30),
        300,
        "python",
        starter,
        true,
        analyze_code("python", changed, starter),
    );
    for (source, receipt) in [(35, 350), (38, 400)] {
        ledger.record_test(Some(source), receipt, &run("python", 2, 5));
        assert!(ledger.progress.tested);
        assert_eq!(ledger.tests.passed, 2);
        assert!(!ledger.progress.test_progress);
        assert_eq!(ledger.stuck.last_test_progress_ms, None);
        assert_eq!(ledger.stuck.test_progress_latency_ms, None);
    }
    ledger.record_test(Some(40), 450, &run("python", 3, 5));
    assert!(ledger.progress.test_progress);
    assert_eq!(ledger.stuck.undo_redo_cycles, 1);
    assert_eq!(ledger.stuck.last_meaningful_change_ms, Some(300));
    assert_eq!(ledger.stuck.last_test_progress_ms, Some(450));
    assert_eq!(ledger.stuck.test_progress_latency_ms, Some(150));
    assert_eq!(ledger.metrics.code_events_emitted, 3);
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

    ledger.record_test(Some(200), 300, &run("java", 1, 5));

    assert!(!ledger.tests.regression, "{:?}", ledger.tests);
    assert!(!ledger.tests.follows_executed_run, "{:?}", ledger.tests);
    assert_eq!(ledger.tests.newly_passed, 1);
    assert_eq!(ledger.tests.newly_failed, 4);
    assert!(!ledger.progress.test_progress);
    assert_eq!(ledger.stuck.last_test_progress_ms, None);

    // Back in the same language the next run is compared to the one before it.
    ledger.stuck.last_meaningful_change_ms = Some(350);
    ledger.record_test(Some(300), 400, &run("java", 2, 5));
    assert!(ledger.tests.follows_executed_run);
    assert_eq!(ledger.tests.newly_passed, 1);
    assert!(!ledger.tests.regression);
    ledger.record_test(Some(400), 500, &run("java", 1, 5));
    assert!(
        ledger.tests.regression,
        "a real drop in one language still is"
    );

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
        Some(100),
        200,
        &serde_json::json!({ "passed": 3, "total": 3 }),
    );
    ledger.record_test(Some(200), 300, &run("python", 1, 3));
    assert!(ledger.tests.regression);
}

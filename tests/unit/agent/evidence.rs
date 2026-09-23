//! Reducer tests for the deterministic evidence ledger.

use super::evidence::{
    comment_kind, grammar_key, input_edit, parse_with, parser_language, parser_unavailable,
};
use super::{
    CodeChangeClass, CodeObservation, EvidenceLedger, LifecycleTransition, ModelInputKind,
    ObservationFamily, Provenance, RuntimeState, analyze_code, apply_data_event,
    apply_data_event_at, apply_server_event_at, record_framework_evidence, record_hint,
};
use super::{ParseCache, ViewFor, analyze_code_cached, observe_code, observe_code_cached};

/// A buffer recorded with no parse behind it, which is all a caller that never
/// saw the prior text can honestly claim. It lived on the ledger until it had
/// no caller but these tests.
fn record_unparsed(
    ledger: &mut EvidenceLedger,
    source_timestamp_ms: Option<u64>,
    receipt_timestamp_ms: u64,
    language: &str,
    code: &str,
    candidate_edit: bool,
) {
    ledger.record_code_with_analysis(
        source_timestamp_ms,
        receipt_timestamp_ms,
        language,
        code,
        candidate_edit,
        parser_unavailable(),
    );
}

fn failed(label: &str) -> serde_json::Value {
    serde_json::json!({
        "passed": 1,
        "total": 3,
        "failures": [{ "label": label, "expected": "2", "got": "1", "error": null }]
    })
}

#[test]
fn evidence_ledger_code_observation_is_deterministic() {
    let mut one = EvidenceLedger::default();
    let mut two = EvidenceLedger::default();
    record_unparsed(&mut one, Some(100), 200, "python", "return value", true);
    record_unparsed(&mut two, Some(100), 200, "python", "return value", true);
    assert_eq!(one, two);
}

#[test]
fn evidence_ledger_starter_code_does_not_advance_semantic_review_state() {
    let mut ledger = EvidenceLedger::default();
    ledger.record_code_with_analysis(
        Some(100),
        200,
        "python",
        "def starter():\n    return 0\n",
        false,
        analyze_code("python", "", "def starter():\n    return 0\n"),
    );
    assert_eq!(ledger.code.semantic_revision, 0);
}

#[test]
fn a_language_switch_after_an_edit_does_not_start_another_edit_test_cycle() {
    let mut state = RuntimeState::default();
    apply_data_event_at(
        &mut state,
        crate::runtime::TOPIC_CODE_UPDATE,
        &serde_json::json!({ "code": "value = 1\n", "language": "python" }),
        0.0,
        100,
    );
    apply_data_event_at(
        &mut state,
        crate::runtime::TOPIC_CODE_UPDATE,
        &serde_json::json!({ "code": "value = 2\n", "language": "python" }),
        0.0,
        200,
    );
    apply_data_event_at(
        &mut state,
        crate::runtime::TOPIC_TEST_RESULTS,
        &serde_json::json!({ "passed": 0, "total": 1 }),
        0.0,
        300,
    );
    apply_data_event_at(
        &mut state,
        crate::runtime::TOPIC_CODE_UPDATE,
        &serde_json::json!({ "code": "class Solution {}\n", "language": "java" }),
        0.0,
        400,
    );
    apply_data_event_at(
        &mut state,
        crate::runtime::TOPIC_TEST_RESULTS,
        &serde_json::json!({ "passed": 0, "total": 1 }),
        0.0,
        500,
    );
    apply_data_event_at(
        &mut state,
        crate::runtime::TOPIC_CONTROL,
        &serde_json::json!({
            "type": "end_interview",
            "code": "value = 1\n",
            "language": "python",
        }),
        0.0,
        600,
    );
    assert!(state.code_edited);
    assert_eq!(state.evidence_ledger.code.candidate_edits, 1);
    assert_eq!(state.evidence_ledger.stuck.edit_test_cycles, 1);
}

#[test]
fn a_completed_edit_keeps_its_semantic_change_after_an_invalid_draft() {
    let mut state = RuntimeState::default();
    for (receipt, code) in [
        (100, "value = 1\n"),
        (200, "if value\n"),
        (300, "if value:\n    value = 2\n"),
    ] {
        apply_data_event_at(
            &mut state,
            crate::runtime::TOPIC_CODE_UPDATE,
            &serde_json::json!({ "code": code }),
            0.0,
            receipt,
        );
    }
    assert_eq!(
        state.evidence_ledger.code.parser_observation,
        Some(CodeObservation::Parsed)
    );
    assert_eq!(state.evidence_ledger.code.semantic_revision, 1);
    assert!(
        state
            .evidence_ledger
            .code
            .last_analysis
            .unwrap()
            .semantic_change
    );
    assert!(state.last_parseable_code.is_empty());
}

#[test]
fn evidence_ledger_code_never_keeps_editor_text() {
    let mut ledger = EvidenceLedger::default();
    record_unparsed(
        &mut ledger,
        Some(100),
        200,
        "python",
        "candidate_secret_name",
        true,
    );
    assert!(
        !serde_json::to_string(&ledger)
            .unwrap()
            .contains("candidate_secret_name")
    );
}

#[test]
fn evidence_ledger_duplicate_code_is_coalesced() {
    let mut ledger = EvidenceLedger::default();
    record_unparsed(&mut ledger, Some(100), 200, "python", "return 1", true);
    record_unparsed(&mut ledger, Some(101), 201, "python", "return 1", true);
    assert_eq!(ledger.entries.len(), 1);
}

#[test]
fn evidence_ledger_marks_parser_unavailable_without_a_diff_guess() {
    let mut ledger = EvidenceLedger::default();
    record_unparsed(&mut ledger, Some(100), 200, "python", "if value:", true);
    assert_eq!(
        ledger.entries[0].observation["parser"],
        "parser_unavailable"
    );
    assert_eq!(ledger.entries[0].provenance, Provenance::Derived);
}

#[test]
fn evidence_ledger_counts_test_delta() {
    let mut ledger = EvidenceLedger::default();
    ledger.record_test(
        Some(100),
        200,
        &serde_json::json!({ "passed": 1, "total": 3 }),
    );
    ledger.record_test(
        Some(200),
        300,
        &serde_json::json!({ "passed": 2, "total": 3 }),
    );
    assert_eq!(ledger.tests.newly_passed, 1);
    assert_eq!(ledger.tests.newly_failed, -1);
}

#[test]
fn evidence_ledger_marks_a_test_regression() {
    let mut ledger = EvidenceLedger::default();
    ledger.record_test(
        Some(100),
        200,
        &serde_json::json!({ "passed": 3, "total": 3 }),
    );
    ledger.record_test(Some(200), 300, &failed("case-a"));
    assert!(ledger.tests.regression);
}

#[test]
fn evidence_ledger_counts_identical_failures() {
    let mut ledger = EvidenceLedger::default();
    ledger.record_test(Some(100), 200, &failed("case-a"));
    ledger.record_test(Some(200), 300, &failed("case-a"));
    assert_eq!(ledger.tests.consecutive_identical_failures, 2);
}

#[test]
fn evidence_ledger_resets_a_failure_streak_when_the_signature_changes() {
    let mut ledger = EvidenceLedger::default();
    ledger.record_test(Some(100), 200, &failed("case-a"));
    ledger.record_test(Some(200), 300, &failed("case-b"));
    assert_eq!(ledger.tests.consecutive_identical_failures, 1);
}

#[test]
fn evidence_ledger_entries_are_monotonic_and_versioned() {
    let mut ledger = EvidenceLedger::default();
    record_unparsed(&mut ledger, Some(100), 200, "python", "return 1", true);
    ledger.record_test(Some(200), 300, &failed("case-a"));
    assert_eq!(ledger.version, 1);
    assert_eq!(ledger.entries[0].sequence, 1);
    assert_eq!(ledger.entries[1].sequence, 2);
    assert_eq!(ledger.entries[1].family, ObservationFamily::Test);
}

#[test]
fn evidence_ledger_keeps_server_receipt_separate_from_browser_time() {
    let mut ledger = EvidenceLedger::default();
    record_unparsed(&mut ledger, Some(100), 300, "python", "return 1", true);
    assert_eq!(ledger.entries[0].source_timestamp_ms, Some(100));
    assert_eq!(ledger.entries[0].receipt_timestamp_ms, 300);
}

#[test]
fn evidence_ledger_keeps_test_source_time_after_sanitizing() {
    let mut state = RuntimeState::default();
    apply_data_event_at(
        &mut state,
        crate::runtime::TOPIC_TEST_RESULTS,
        &serde_json::json!({ "passed": 1, "total": 1, "at": 100 }),
        0.0,
        300,
    );
    assert_eq!(
        state.evidence_ledger.entries[0].source_timestamp_ms,
        Some(100)
    );
    assert_eq!(state.evidence_ledger.entries[0].receipt_timestamp_ms, 300);
    assert!(state.last_test_run.as_ref().unwrap().get("at").is_none());
}

#[test]
fn evidence_ledger_serialization_replays_its_contract() {
    let mut ledger = EvidenceLedger::default();
    ledger.record_test(Some(100), 200, &failed("case-a"));
    let serialized = serde_json::to_string(&ledger).unwrap();
    let restored: EvidenceLedger = serde_json::from_str(&serialized).unwrap();
    assert_eq!(restored, ledger);
}

#[test]
fn evidence_ledger_uses_only_explicit_categories_for_diagnostics() {
    let mut ledger = EvidenceLedger::default();
    ledger.record_test(
        Some(100),
        200,
        &serde_json::json!({
            "passed": 0,
            "total": 1,
            "setupError": "arbitrary compiler prose",
            "diagnostic": { "category": "syntax" },
        }),
    );
    ledger.record_test(
        Some(200),
        300,
        &serde_json::json!({
            "passed": 0,
            "total": 1,
            "setupError": "another unrecognized message",
            "diagnostic": { "category": "algorithm" },
        }),
    );
    assert_eq!(ledger.diagnostics.syntax, 1);
    assert_eq!(ledger.diagnostics.other, 1);
    assert!(
        !serde_json::to_string(&ledger)
            .unwrap()
            .contains("arbitrary compiler prose")
    );
}

#[test]
fn evidence_ledger_progress_and_stuck_signals_are_measurements() {
    let mut ledger = EvidenceLedger::default();
    record_unparsed(&mut ledger, Some(100), 200, "python", "return 1", true);
    ledger.record_test(Some(200), 300, &failed("case-a"));
    ledger.record_test(
        Some(300),
        400,
        &serde_json::json!({ "passed": 3, "total": 3 }),
    );
    assert_eq!(ledger.progress.first_edit_ms, Some(200));
    assert!(ledger.progress.runner_attempted);
    assert!(ledger.progress.test_progress);
    assert!(ledger.progress.all_tests_passed_once);

    // One edit and two runs is one cycle and one retry.
    assert_eq!(ledger.stuck.edit_test_cycles, 1);
    assert_eq!(ledger.stuck.repeated_failure_count, 0);
    assert_eq!(ledger.stuck.last_meaningful_change_ms, None);
}

/// The same text in another tab is another editor state, not a way back to
/// this one. Emptying the Java editor after emptying the Python one is two
/// clean slates, and counting it as a reversal told the model the candidate
/// was going round in circles when they had only changed tabs.
#[test]
fn the_same_text_in_another_language_is_not_a_reversal() {
    let mut ledger = EvidenceLedger::default();
    let written = "x = 1\n";
    for (at, language, before, after) in [
        (100, "python", "", written),
        (200, "python", written, ""),
        (300, "javascript", "", written),
        (400, "javascript", written, ""),
    ] {
        ledger.record_code_with_analysis(
            Some(at),
            at,
            language,
            after,
            true,
            analyze_code(language, before, after),
        );
    }
    assert_eq!(ledger.stuck.undo_redo_cycles, 0);

    // And the same text in the same tab still is.
    ledger.record_code_with_analysis(
        Some(500),
        500,
        "javascript",
        written,
        true,
        analyze_code("javascript", "", written),
    );
    assert_eq!(ledger.stuck.undo_redo_cycles, 1);
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
    ledger.record_test(
        Some(40),
        450,
        &serde_json::json!({ "passed": 1, "total": 1 }),
    );
    assert_eq!(ledger.stuck.undo_redo_cycles, 1);
    assert_eq!(ledger.stuck.last_meaningful_change_ms, Some(300));
    assert_eq!(ledger.stuck.last_test_progress_ms, Some(450));
    assert_eq!(ledger.stuck.test_progress_latency_ms, Some(150));
    assert_eq!(ledger.metrics.code_events_emitted, 3);
}

#[test]
fn evidence_ledger_counts_received_packets_separately_from_evidence() {
    let mut state = RuntimeState::default();
    apply_data_event(
        &mut state,
        crate::runtime::TOPIC_CODE_UPDATE,
        &serde_json::json!({ "code": 42 }),
        0.0,
    );
    assert_eq!(state.evidence_ledger.metrics.raw_events_received, 1);
    assert_eq!(state.evidence_ledger.metrics.code_events_emitted, 0);
}

#[test]
fn evidence_ledger_counts_model_bytes_without_claiming_tokens() {
    let mut ledger = EvidenceLedger::default();
    ledger.record_model_input(ModelInputKind::Watch, "watch");
    ledger.record_model_input(ModelInputKind::Turn, "greeting");
    ledger.record_model_input(ModelInputKind::Turn, "wrap");
    ledger.record_model_input(ModelInputKind::Interim, "interim");
    ledger.record_model_input(ModelInputKind::FinalReport, "report");
    ledger.record_model_input(ModelInputKind::ReadEditor, "editor");
    ledger.record_model_input(ModelInputKind::ToolResponse, "hint");
    assert_eq!(ledger.metrics.watch_prompt_count, 1);
    assert_eq!(ledger.metrics.watch_prompt_bytes, 5);
    assert_eq!(ledger.metrics.interim_prompt_bytes, 7);
    assert_eq!(ledger.metrics.final_report_prompt_bytes, 6);

    // One counter for the four conversational sends, and it accumulates rather
    // than replacing: `Turn` is the only kind a session reaches more than once
    // through more than one call site.
    assert_eq!(ledger.metrics.turn_prompt_count, 2);
    assert_eq!(ledger.metrics.turn_prompt_bytes, 12);

    // The counts as well as the bytes. Asserting only the bytes left every
    // count free to stop incrementing without a test noticing.
    assert_eq!(ledger.metrics.interim_prompt_count, 1);
    assert_eq!(ledger.metrics.final_report_prompt_count, 1);
    assert_eq!(ledger.metrics.read_editor_calls, 1);
    assert_eq!(ledger.metrics.read_editor_bytes, 6);
    assert_eq!(ledger.metrics.tool_response_count, 1);
    assert_eq!(ledger.metrics.tool_response_bytes, 4);

    // Every one of them, not just the first. These are the views a model is
    // handed, and a counter that reached one would be telling the interviewer
    // what its own advice costs.
    let slice = every_view(&ledger);
    for field in [
        "watch_prompt_bytes",
        "turn_prompt_bytes",
        "interim_prompt_bytes",
        "final_report_prompt_bytes",
        "read_editor_bytes",
        "tool_response_bytes",
    ] {
        assert!(!slice.contains(field), "{field} reached the projection");
    }
}

#[test]
fn evidence_ledger_counts_setup_failures_without_a_candidate_judgment() {
    let mut ledger = EvidenceLedger::default();
    ledger.record_test(
        Some(100),
        200,
        &serde_json::json!({ "passed": 0, "total": 0, "setupError": "runner exited" }),
    );
    assert_eq!(ledger.stuck.compile_or_runner_failure_count, 1);
    assert_eq!(ledger.diagnostics.other, 1);
}

#[test]
fn evidence_ledger_receives_only_sanitized_diagnostic_categories() {
    let mut state = RuntimeState::default();
    let event = serde_json::json!({
        "passed": 0,
        "total": 1,
        "language": "python",
        "setupError": "untrusted text",
        "diagnostic": { "category": "syntax", "text": "must not survive" },
    });
    apply_data_event(&mut state, crate::runtime::TOPIC_TEST_RESULTS, &event, 99.0);
    assert_eq!(state.evidence_ledger.diagnostics.syntax, 1);
    assert!(
        !serde_json::to_string(&state.evidence_ledger)
            .unwrap()
            .contains("untrusted text")
    );
}

#[test]
fn evidence_ledger_records_hint_policy_outcomes_without_hint_text() {
    let mut state = RuntimeState {
        hint_ladder: &[
            "private-hint-alpha",
            "private-hint-beta",
            "private-hint-gamma",
        ],
        ..RuntimeState::default()
    };
    record_hint(&mut state, false);
    record_hint(&mut state, true);
    record_hint(&mut state, true);
    record_hint(&mut state, true);
    assert_eq!(state.evidence_ledger.hints.volunteered, 1);
    assert_eq!(state.evidence_ledger.hints.requested, 3);
    assert_eq!(state.evidence_ledger.hints.current_level, 2);
    assert_eq!(state.evidence_ledger.hints.withheld, 1);
    assert!(
        !serde_json::to_string(&state.evidence_ledger)
            .unwrap()
            .contains("private-hint-alpha")
    );
}

#[test]
fn evidence_ledger_keeps_lifecycle_and_coverage_as_closed_facts() {
    let mut ledger = EvidenceLedger::default();
    ledger.set_uncovered_coverage(["repeat", "algorithm"]);
    ledger.record_lifecycle(100, LifecycleTransition::Paused);
    ledger.record_lifecycle(200, LifecycleTransition::Resumed);
    ledger.record_coverage(300, "repeat");
    assert!(!ledger.lifecycle.paused);
    assert_eq!(ledger.lifecycle.transitions, 2);
    assert_eq!(ledger.coverage.covered, ["repeat"]);
    assert_eq!(ledger.coverage.uncovered, ["algorithm"]);
}

#[test]
fn evidence_ledger_only_records_accepted_control_transitions() {
    let mut state = RuntimeState::default();
    apply_data_event(
        &mut state,
        crate::runtime::TOPIC_CONTROL,
        &serde_json::json!({ "type": "pause_interview", "paused": true }),
        99.0,
    );
    apply_data_event(
        &mut state,
        crate::runtime::TOPIC_CONTROL,
        &serde_json::json!({ "type": "pause_interview", "paused": true }),
        99.0,
    );
    apply_data_event(
        &mut state,
        crate::runtime::TOPIC_CONTROL,
        &serde_json::json!({ "type": "end_interview", "reason": "candidate_ended" }),
        99.0,
    );
    assert!(state.evidence_ledger.lifecycle.paused);
    assert!(state.evidence_ledger.lifecycle.ended);
    assert_eq!(state.evidence_ledger.lifecycle.transitions, 2);
}

#[test]
fn evidence_ledger_records_the_final_code_carried_by_end_interview() {
    let mut state = RuntimeState::default();
    apply_data_event_at(
        &mut state,
        crate::runtime::TOPIC_CODE_UPDATE,
        &serde_json::json!({ "code": "value = 1\n", "language": "python", "at": 100 }),
        99.0,
        200,
    );
    apply_data_event_at(
        &mut state,
        crate::runtime::TOPIC_CONTROL,
        &serde_json::json!({
            "type": "end_interview",
            "reason": "candidate_ended",
            "code": "value = 2\n",
            "language": "python"
        }),
        99.0,
        300,
    );
    assert_eq!(state.evidence_ledger.code.revision, 2);
    assert_eq!(
        state
            .evidence_ledger
            .code
            .last_analysis
            .unwrap()
            .classification,
        Some(CodeChangeClass::Expression)
    );
    assert!(state.evidence_ledger.lifecycle.ended);
}

#[test]
fn evidence_ledger_prompt_view_is_bounded_and_raw_free() {
    let mut ledger = EvidenceLedger::default();
    record_unparsed(
        &mut ledger,
        Some(100),
        200,
        "python",
        "candidate_code_must_not_reach_the_prompt_projection",
        true,
    );
    for sequence in 0..40 {
        ledger.record_test(
            Some(sequence),
            sequence,
            &serde_json::json!({ "passed": 0, "total": 1, "setupError": "raw diagnostic" }),
        );
    }
    let slice = every_view(&ledger);
    assert!(slice.len() <= 1_000, "{} bytes", slice.len());
    assert!(!slice.contains("candidate_code_must_not_reach_the_prompt_projection"));
    assert!(!slice.contains("raw diagnostic"));
    assert!(slice.contains("40 runs failed to start"), "{slice}");

    // No digest, which is most of what the JSON view spent its tokens on: the
    // model can compare two 64-character hashes no better than it can read one.
    let hex_run = slice
        .split(|character: char| !character.is_ascii_hexdigit())
        .map(str::len)
        .max()
        .unwrap_or(0);
    assert!(hex_run < 16, "a digest reached the view: {slice}");

    // The operational counters are the session's, never the model's. Every
    // prompt that carries the ledger takes it from this view, and each field is
    // checked by name, so one added later is covered too.
    let metrics = serde_json::to_value(&ledger.metrics).unwrap();
    for field in metrics.as_object().unwrap().keys() {
        assert!(!slice.contains(field.as_str()), "{field} reached the slice");
    }
}

#[test]
fn evidence_ledger_records_closed_turns_and_a_bounded_sequence() {
    let mut ledger = EvidenceLedger::default();
    ledger.record_conversation_turn(100, "interviewer");
    ledger.record_conversation_turn(200, "candidate");
    for receipt in 0..20 {
        ledger.record_test(
            Some(receipt),
            receipt,
            &serde_json::json!({ "passed": 0, "total": 1 }),
        );
    }
    assert_eq!(ledger.conversation.interviewer_turns, 1);
    assert_eq!(ledger.conversation.candidate_turns, 1);
    assert_eq!(ledger.entries[0].family, ObservationFamily::InterviewerTurn);
    assert_eq!(ledger.entries[1].family, ObservationFamily::CandidateTurn);
    assert_eq!(ledger.conversation.recent_sequence.len(), 16);
    assert_eq!(
        ledger.conversation.recent_sequence.last(),
        Some(&"test".to_string())
    );
}

#[test]
fn code_analysis_distinguishes_non_semantic_and_structural_changes() {
    let formatting = analyze_code("python", "value=1\n", "value = 1\n");
    assert_eq!(formatting.observation, CodeObservation::Parsed);
    assert_eq!(
        formatting.classification,
        Some(CodeChangeClass::FormattingOnly)
    );
    assert!(!formatting.semantic_change);

    let comment = analyze_code("python", "value = 1\n", "# explain\nvalue = 1\n");
    assert_eq!(comment.classification, Some(CodeChangeClass::CommentOnly));
    assert!(!comment.semantic_change);

    let identifier = analyze_code("python", "value = 1\n", "total = 1\n");
    assert_eq!(
        identifier.classification,
        Some(CodeChangeClass::IdentifierOnly)
    );
    assert!(identifier.semantic_change);

    let expression = analyze_code("python", "value = 1\n", "value = 2\n");
    assert_eq!(expression.classification, Some(CodeChangeClass::Expression));
    assert!(expression.semantic_change);

    let control = analyze_code("python", "value = 1\n", "if value:\n    return value\n");
    assert_eq!(control.classification, Some(CodeChangeClass::ControlFlow));
    assert!(control.semantic_change);
    assert!(
        control
            .changed_nodes
            .iter()
            .any(|fact| fact == "if_statement:added:1")
    );

    let data_structure = analyze_code("python", "value = 1\n", "value = [1, 2]\n");
    assert_eq!(
        data_structure.classification,
        Some(CodeChangeClass::DataStructure)
    );
    assert!(data_structure.semantic_change);
}

#[test]
fn code_analysis_refuses_to_guess_when_parsing_is_unavailable_or_invalid() {
    assert_eq!(
        analyze_code("rust", "let value = 1;", "let value = 2;").observation,
        CodeObservation::ParserUnavailable
    );
    let invalid = analyze_code("python", "value = 1\n", "if value\n");
    assert_eq!(invalid.observation, CodeObservation::SyntaxInvalid);
    assert_eq!(invalid.classification, None);
    assert!(!invalid.semantic_change);
}

#[test]
fn code_analysis_supports_every_offered_language() {
    for (language, source) in [
        ("c", "int main(void) { return 0; }"),
        ("cpp", "int main() { return 0; }"),
        ("java", "class Main { static int f() { return 0; } }"),
        ("javascript", "function f() { return 0; }"),
        ("python", "def f():\n    return 0\n"),
    ] {
        assert_eq!(
            analyze_code(language, source, source).observation,
            CodeObservation::Parsed,
            "{language}"
        );
    }
}

/// The ledger is a contract, not just a pure function.
///
/// Replaying the fixture twice in one process only proves the reducers do not
/// consult a clock or a hash seed, and every behavior this file pins could
/// change underneath a test that asks no more than that. The frozen ledger is
/// the other half: a diff here is a claim that what the runtime records about
/// an interview changed on purpose. After a deliberate change, regenerate and
/// read the diff:
///
///   UPDATE_EVIDENCE_GOLDEN=1 cargo test --lib evidence_ledger_replay_fixture
#[test]
fn evidence_ledger_replay_fixture_is_frozen_and_deterministic() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../fixtures/evidence-ledger.json")).unwrap();
    let events = fixture["events"].as_array().unwrap();

    // The state the runtime actually builds, not a bare one. `for_problem` is
    // what loads the starter templates a code packet is measured against and
    // what seeds the REACTO phases `coverage.uncovered` reports, so a replay
    // from `default()` froze an empty template map and an empty phase list: the
    // two things the golden is least able to notice going wrong, because their
    // correct value and their missing value both serialize as nothing.
    //
    // The last event is the one the agent decides on rather than one that
    // arrived, which is the distinction `raw_events_received` exists to draw.
    // Replaying it here is what puts a number in the golden smaller than the
    // event count, so a reducer that started counting its own decisions as
    // packets shows up as a diff.
    let replay = || {
        let mut state = RuntimeState::for_problem(crate::agent::get_problem(Some("two-sum")));
        for event in events {
            let apply = if event["serverDecided"].as_bool().unwrap_or(false) {
                apply_server_event_at
            } else {
                apply_data_event_at
            };
            apply(
                &mut state,
                event["topic"].as_str().unwrap(),
                &event["payload"],
                99.0,
                event["receiptMs"].as_u64().unwrap(),
            );
        }
        state.evidence_ledger
    };
    let first = replay();
    assert_eq!(first, replay());

    let path = "tests/golden/evidence-ledger.json";
    if std::env::var_os("UPDATE_EVIDENCE_GOLDEN").is_some() {
        let mut text = serde_json::to_string_pretty(&first).expect("ledger should serialize");
        text.push('\n');
        std::fs::write(path, text).expect("ledger fixture should write");
    }
    let expected: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("ledger fixture should read"))
            .expect("ledger fixture should parse");
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        expected,
        "the replayed ledger drifted from {path}"
    );

    // Named here as well as frozen, because a regenerated golden agrees with
    // whatever the code now does: these are the facts the fixture was built to
    // exercise, and a regeneration that quietly drops one should not pass.
    assert_eq!(first.entries[0].sequence, 1);
    assert!(first.lifecycle.ended);
    assert_eq!(first.code.candidate_edits, 7);
    assert_eq!(first.code.semantic_revision, 6);
    assert_eq!(first.stuck.undo_redo_cycles, 1);
    assert_eq!(first.stuck.edit_test_cycles, 2);

    // One fewer than the fixture holds: the last event is the agent's own
    // decision to end, and the counter is the browser boundary.
    assert_eq!(first.metrics.raw_events_received, events.len() as u64 - 1);

    // Seeded by `for_problem`, and the phases the fixture never reaches stay
    // uncovered. A replay from a bare state reported no phases at all, which
    // looks the same as every phase being covered.
    assert_eq!(first.coverage.covered, Vec::<String>::new());
    assert!(first.coverage.uncovered.contains(&"repeat".to_string()));

    // `<` to `<=`, the edit the fixture exists for.
    assert_eq!(first.entries[6].observation["classification"], "expression");
    assert_eq!(first.entries[6].observation["semanticChange"], true);

    // The tab the candidate consulted mid-error parses; it does not claim the
    // parser was unavailable, which is what kept the recovery below alive.
    assert_eq!(first.entries[8].observation["language"], "cpp");
    assert_eq!(first.entries[8].observation["parser"], "parsed");
    assert_eq!(first.entries[10].observation["semanticChange"], true);

    // The run that never started, and the run after it.
    assert_eq!(first.entries[12].observation["executed"], false);
    assert_eq!(first.entries[12].observation["newlyFailed"], 0);
    assert_eq!(first.tests.total, 2);
    assert_eq!(first.diagnostics.other, 1);
}

#[test]
fn code_analysis_sees_an_operator_the_grammar_leaves_anonymous() {
    // Every one of these is an anonymous token in its grammar, which is the
    // whole reason they were invisible: a walk over named nodes alone reported
    // an off-by-one fix as formatting, left `semantic_revision` where it was,
    // and told the report the candidate had only reindented.
    for (language, before, after) in [
        ("python", "value = a + b\n", "value = a - b\n"),
        (
            "python",
            "while low < high:\n    pass\n",
            "while low <= high:\n    pass\n",
        ),
        ("python", "flag = a and b\n", "flag = a or b\n"),
        (
            "c",
            "int f(int a, int b) { return a + b; }",
            "int f(int a, int b) { return a - b; }",
        ),
        (
            "cpp",
            "int f(int a) { return a == 0; }",
            "int f(int a) { return a != 0; }",
        ),
        (
            "javascript",
            "const f = (a, b) => a * b;",
            "const f = (a, b) => a / b;",
        ),
    ] {
        let analysis = analyze_code(language, before, after);
        assert_eq!(
            analysis.classification,
            Some(CodeChangeClass::Expression),
            "{language}: {before:?} -> {after:?}"
        );
        assert!(analysis.semantic_change, "{language}");
    }
}

#[test]
fn code_analysis_separates_a_rename_from_an_argument_swap() {
    // The same identifiers in a different order is the call changing, not a
    // rename, and the sorted comparison used to make the two indistinguishable
    // in the other direction: a swap read as formatting.
    let swapped = analyze_code("python", "merge(left, right)\n", "merge(right, left)\n");
    assert_eq!(swapped.classification, Some(CodeChangeClass::Expression));
    assert!(swapped.semantic_change);

    let renamed = analyze_code("python", "merge(left, right)\n", "merge(first, right)\n");
    assert_eq!(
        renamed.classification,
        Some(CodeChangeClass::IdentifierOnly)
    );

    // A rename that also changes a literal is no longer only a rename: the tree
    // is the same shape, but the value the call gets is not.
    let retuned = analyze_code("python", "merge(left, 1)\n", "merge(first, 2)\n");
    assert_eq!(retuned.classification, Some(CodeChangeClass::Expression));
}

#[test]
fn code_analysis_does_not_read_a_local_variable_as_a_changed_signature() {
    for (language, before, after) in [
        (
            "c",
            "int main(void) { return 0; }",
            "int main(void) { int count = 0; return count; }",
        ),
        (
            "java",
            "class Main { static int f() { return 0; } }",
            "class Main { static int f() { int n = 0; return n; } }",
        ),
    ] {
        let analysis = analyze_code(language, before, after);
        assert!(analysis.semantic_change, "{language}");
        assert_ne!(
            analysis.classification,
            Some(CodeChangeClass::FunctionInterface),
            "{language} local declaration"
        );
    }

    // The parameter list is what a caller actually sees, and it still counts.
    let signature = analyze_code(
        "c",
        "int f(int a) { return a; }",
        "int f(int a, int b) { return a; }",
    );
    assert_eq!(
        signature.classification,
        Some(CodeChangeClass::FunctionInterface)
    );
}

#[test]
fn a_run_that_never_executed_leaves_the_deltas_where_they_were() {
    let mut ledger = EvidenceLedger::default();
    record_unparsed(&mut ledger, Some(10), 100, "python", "value = 1\n", true);
    ledger.record_test(
        Some(20),
        200,
        &serde_json::json!({ "passed": 3, "total": 5 }),
    );

    // What Compiler Explorer sends when it answers 503: an attempt, no cases.
    ledger.record_test(
        Some(30),
        300,
        &serde_json::json!({
            "passed": 0,
            "total": 0,
            "setupError": "Compiler Explorer returned 503",
        }),
    );
    assert_eq!(ledger.tests.total, 5);
    assert_eq!(ledger.tests.passed, 3);
    assert_eq!(ledger.tests.newly_failed, 2);
    assert!(!ledger.tests.regression);
    assert_eq!(ledger.diagnostics.other, 1);
    assert!(ledger.progress.runner_attempted);

    // The same code run again is not three cases newly passing.
    ledger.record_test(
        Some(40),
        400,
        &serde_json::json!({ "passed": 3, "total": 5 }),
    );
    assert_eq!(ledger.tests.newly_passed, 0);

    // The first run was progress and is still the last one. Counting the
    // unchanged code as three cases newly passing would have moved that mark to
    // the run that only repeated it.
    assert_eq!(ledger.stuck.last_test_progress_ms, Some(200));

    // Entry 2 is the diagnostic the failed setup produced; entry 3 is the run
    // itself, which says in as many words that it did not execute.
    assert_eq!(ledger.entries[3].observation["executed"], false);
}

#[test]
fn evidence_ledger_counts_an_edit_and_a_run_as_one_cycle() {
    let mut ledger = EvidenceLedger::default();
    record_unparsed(&mut ledger, Some(10), 100, "python", "value = 1\n", true);
    for receipt in [200, 300, 400] {
        ledger.record_test(
            Some(receipt),
            receipt,
            &serde_json::json!({ "passed": 1, "total": 2 }),
        );
    }
    assert_eq!(ledger.stuck.edit_test_cycles, 1);

    record_unparsed(&mut ledger, Some(500), 500, "python", "value = 2\n", true);
    ledger.record_test(
        Some(600),
        600,
        &serde_json::json!({ "passed": 2, "total": 2 }),
    );
    assert_eq!(ledger.stuck.edit_test_cycles, 2);
}

#[test]
fn two_failing_runs_are_only_one_failure_when_they_failed_the_same_way() {
    // The signature separates fields and failures with different delimiters, so
    // a label carrying the field separator cannot be confused with two fields.
    let mut collided = EvidenceLedger::default();
    collided.record_test(
        Some(10),
        100,
        &serde_json::json!({
            "passed": 0,
            "total": 1,
            "failures": [{ "label": "case", "error": "boom" }],
        }),
    );
    collided.record_test(
        Some(20),
        200,
        &serde_json::json!({
            "passed": 0,
            "total": 1,
            "failures": [{ "label": "case\u{1f}boom" }],
        }),
    );
    assert_eq!(collided.tests.consecutive_identical_failures, 1);

    // Two runs that named no failure at all are not evidence of the same
    // failure twice either.
    let mut unnamed = EvidenceLedger::default();
    for receipt in [100, 200] {
        unnamed.record_test(
            Some(receipt),
            receipt,
            &serde_json::json!({ "passed": 0, "total": 2 }),
        );
    }
    assert_eq!(unnamed.tests.consecutive_identical_failures, 1);
    assert_eq!(unnamed.stuck.repeated_failure_count, 1);
}

#[test]
fn a_hint_entry_is_stamped_with_the_clock_and_not_the_sequence() {
    let mut ledger = EvidenceLedger::default();
    record_unparsed(
        &mut ledger,
        Some(10),
        1_770_000_000_000,
        "python",
        "value = 1\n",
        true,
    );
    ledger.record_hint(1_770_000_000_500, true, true);
    assert_eq!(ledger.entries[1].receipt_timestamp_ms, 1_770_000_000_500);
    assert_eq!(ledger.hints.last_hint_sequence, Some(2));
}

#[test]
fn unobserved_diagnostic_categories_are_not_reported_as_zero() {
    let mut ledger = EvidenceLedger::default();
    ledger.record_test(
        Some(10),
        100,
        &serde_json::json!({
            "passed": 0,
            "total": 0,
            "setupError": "compilation failed",
        }),
    );
    let slice = every_view(&ledger);
    assert!(
        slice.contains("diagnostics: browser-reported claims (unverified): other 1"),
        "{slice}"
    );

    // The browser half never names these, so a written-out zero would be a
    // claim about something nothing measured.
    assert!(!slice.contains("syntax"));
    assert!(!slice.contains("type "));
    assert!(!slice.contains("linker"));
}

#[test]
fn a_large_paste_does_not_evict_the_session_from_the_prompt() {
    let mut ledger = EvidenceLedger::default();
    for sequence in 0..30 {
        ledger.record_test(
            Some(sequence),
            sequence,
            &serde_json::json!({ "passed": 1, "total": 2 }),
        );
    }

    // A solution pasted over the template changes dozens of node kinds at once.
    // The projection carries none of them now, and the ledger, which keeps them
    // twice (in `code.last_analysis` and in the entry), caps them.
    let pasted = (0..40)
        .map(|index| {
            format!(
                "def f{index}(values):\n    total = {{}}\n    for item in [1, 2]:\n        if item in total:\n            total[item] += 1\n    return sorted(total)\n"
            )
        })
        .collect::<String>();
    ledger.record_code_with_analysis(
        Some(900),
        900,
        "python",
        &pasted,
        true,
        analyze_code("python", "def f(values):\n    return []\n", &pasted),
    );
    let slice = every_view(&ledger);
    assert!(slice.len() <= 1_000, "{} bytes", slice.len());

    // The session before the paste is still what the view reports: it states
    // aggregates, so there is no list of entries for one edit to push out.
    assert!(
        slice.contains("tests: browser-reported claims (unverified): 1 of 2 passing"),
        "{slice}"
    );
    assert!(!slice.contains("changed_nodes") && !slice.contains("changedNodes"));
    assert!(
        !slice.contains("identifier:"),
        "node facts reached the view"
    );
    let facts = &ledger.code.last_analysis.as_ref().unwrap().changed_nodes;
    assert_eq!(facts.len(), 13);

    // The cap keeps twelve and says how many it dropped. The number is the
    // pinned grammars' count of changed kinds for this paste, so a grammar bump
    // that changes it should say so here rather than quietly.
    assert_eq!(facts.last().unwrap(), "+5 more");
}

#[test]
fn a_language_switch_leaves_the_parser_able_to_recover() {
    let mut state = RuntimeState::default();
    for (receipt, language, code) in [
        (100, "python", "value = 1\n"),
        (200, "cpp", "int main() { return 0; }"),
    ] {
        apply_data_event_at(
            &mut state,
            crate::runtime::TOPIC_CODE_UPDATE,
            &serde_json::json!({ "code": code, "language": language }),
            0.0,
            receipt,
        );
    }

    // The switched-to buffer is parsed on its own. Calling the parser
    // unavailable for a language this server parses fine is what used to switch
    // the recovery below off for the rest of the invalid streak.
    assert_eq!(
        state.evidence_ledger.code.parser_observation,
        Some(CodeObservation::Parsed)
    );

    let semantic_before = state.evidence_ledger.code.semantic_revision;
    for (receipt, code) in [
        (300, "int main() { return 0"),
        (400, "int main() { return 1; }"),
    ] {
        apply_data_event_at(
            &mut state,
            crate::runtime::TOPIC_CODE_UPDATE,
            &serde_json::json!({ "code": code, "language": "cpp" }),
            0.0,
            receipt,
        );
    }
    assert_eq!(
        state.evidence_ledger.code.parser_observation,
        Some(CodeObservation::Parsed)
    );
    assert_eq!(
        state.evidence_ledger.code.semantic_revision,
        semantic_before + 1
    );
}

#[test]
fn a_trip_to_another_tab_does_not_strand_an_invalid_draft() {
    let mut state = RuntimeState::default();
    for (receipt, language, code) in [
        (100, "python", "value = 1\n"),
        (200, "python", "if value\n"),
        // Consulting another tab mid-error, then coming back to the draft.
        (300, "cpp", "int main() { return 0; }"),
        (400, "python", "if value\n"),
        (500, "python", "if value:\n    value = 2\n"),
    ] {
        apply_data_event_at(
            &mut state,
            crate::runtime::TOPIC_CODE_UPDATE,
            &serde_json::json!({ "code": code, "language": language }),
            0.0,
            receipt,
        );
    }

    // The baseline carries the language it belongs to and is only consulted for
    // a matching one, so the visit to the C++ tab had no reason to discard it.
    assert_eq!(
        state.evidence_ledger.code.parser_observation,
        Some(CodeObservation::Parsed)
    );
    assert!(
        state
            .evidence_ledger
            .code
            .last_analysis
            .as_ref()
            .unwrap()
            .semantic_change
    );
}

#[test]
fn an_undo_out_of_an_invalid_draft_counts_as_a_reversal() {
    let mut state = RuntimeState::default();
    for (receipt, code) in [
        (100, "value = 1\n"),
        (200, "if value\n"),
        (300, "value = 1\n"),
    ] {
        apply_data_event_at(
            &mut state,
            crate::runtime::TOPIC_CODE_UPDATE,
            &serde_json::json!({ "code": code }),
            0.0,
            receipt,
        );
    }

    // The edit that undoes a draft the parser could not read lands back on a
    // buffer the ledger has already seen, so it reports no semantic change.
    // Counting reversals only inside that branch meant the reversal worth
    // seeing was the one that never counted.
    assert_eq!(state.evidence_ledger.stuck.undo_redo_cycles, 1);
}

#[test]
fn a_browser_timestamp_never_becomes_the_server_receipt() {
    // Read straight from the clock rather than through `current_epoch_millis`,
    // because that function is half of what this is testing: a window measured
    // with the same function it is checking agrees with itself for any value
    // that function returns, including a broken one.
    let wall_clock = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_millis() as u64
    };
    let mut state = RuntimeState::default();
    let before = wall_clock();
    apply_data_event(
        &mut state,
        crate::runtime::TOPIC_CODE_UPDATE,
        &serde_json::json!({ "code": "value = 1\n", "at": 7 }),
        0.0,
    );
    let entry = &state.evidence_ledger.entries[0];
    assert_eq!(entry.source_timestamp_ms, Some(7));

    // The claim stays a claim. The receipt is the server's own clock, read
    // between these two readings, which is what tells it from the browser's
    // `at` and from the ledger sequence the old fallback used: both are small
    // integers far outside the window. Bracketing the call rather than naming a
    // date keeps the test from depending on what year the machine thinks it is.
    assert!(
        (before..=wall_clock()).contains(&entry.receipt_timestamp_ms),
        "receipt {} is outside the window the call ran in",
        entry.receipt_timestamp_ms
    );
}

#[test]
fn clearing_the_editor_and_typing_again_is_still_the_candidate_editing() {
    let mut state = RuntimeState::default();
    for (receipt, code) in [
        (100, "value = 1\n"),
        (200, "value = 2\n"),
        // Select all, delete, and start over. The buffer is empty for exactly
        // one packet, and a packet that follows an empty buffer is not the
        // template the browser publishes on connect: the candidate has been
        // typing since the second packet.
        (300, ""),
        (400, "total = 0\n"),
    ] {
        apply_data_event_at(
            &mut state,
            crate::runtime::TOPIC_CODE_UPDATE,
            &serde_json::json!({ "code": code, "language": "python" }),
            0.0,
            receipt,
        );
    }
    assert_eq!(state.evidence_ledger.code.candidate_edits, 3);
    assert_eq!(state.evidence_ledger.code.semantic_revision, 3);
}

/// The claim `prompt_view` makes: bounded by construction. Every line it
/// writes is a fixed sentence over counts, the coverage lists and the capped
/// conversation sequence, so the largest ledger the caps allow -- every
/// coverage phase, a full sequence, every diagnostic category, hints and a
/// change line -- still renders small. Measured rather than argued, because a
/// field added later could make the argument false.
#[test]
fn the_prompt_view_is_bounded_by_construction() {
    let mut ledger = EvidenceLedger::default();
    ledger.set_uncovered_coverage([
        "repeat",
        "example",
        "approach",
        "code",
        "test",
        "optimize",
        "situation",
        "task",
        "action",
        "result",
    ]);
    for index in 0u64..40 {
        let code = format!("value{index} = [1, 2]\nif value{index}:\n    pass\n");
        let previous = format!("value{} = [1, 2]\n", index.saturating_sub(1));
        ledger.record_code_with_analysis(
            Some(index),
            index,
            "python",
            &code,
            true,
            analyze_code("python", &previous, &code),
        );
        ledger.record_test(
            Some(index),
            index,
            &serde_json::json!({
                "passed": 1,
                "total": 99,
                "setupError": "x",
                "failures": [{ "label": "a", "error": "b", "expected": "c", "got": "d" }],
            }),
        );
        ledger.record_conversation_turn(index, "candidate");
    }
    for category in [
        "syntax",
        "type",
        "linker",
        "runtime_signal",
        "timeout",
        "warning",
        "other",
    ] {
        ledger.record_test(Some(99), 99, &setup_failure(category));
    }
    ledger.record_hint(100, true, true);
    ledger.record_hint(101, false, true);
    ledger.record_hint(102, true, false);
    for purpose in [ViewFor::Watch, ViewFor::Interim, ViewFor::Report] {
        let view = ledger.prompt_view(purpose).join("\n");
        assert!(
            view.len() <= 700,
            "{purpose:?}, {} bytes:\n{view}",
            view.len()
        );
    }
}

fn setup_failure(category: &str) -> serde_json::Value {
    serde_json::json!({
        "passed": 0,
        "total": 1,
        "setupError": "the runner said something",
        "diagnostic": { "category": category },
    })
}

/// Each category a runner can name lands in its own counter.
///
/// One category at a time, because the counters are seven sibling lines: a
/// suite that only ever feeds `syntax` cannot tell them apart, and any one of
/// the other six could be counting into the wrong field, or not counting at
/// all, with nothing to say so.
#[test]
fn every_diagnostic_category_a_runner_can_name_is_counted_on_its_own() {
    let mut ledger = EvidenceLedger::default();
    for (receipt, category) in [
        (100, "syntax"),
        (200, "type"),
        (300, "linker"),
        (400, "runtime_signal"),
        (500, "timeout"),
        (600, "warning"),
        (700, "something_no_runner_names"),
    ] {
        ledger.record_test(Some(receipt), receipt, &setup_failure(category));
    }
    assert_eq!(ledger.diagnostics.syntax, 1);
    assert_eq!(ledger.diagnostics.type_errors, 1);
    assert_eq!(ledger.diagnostics.linker, 1);
    assert_eq!(ledger.diagnostics.runtime_signal, 1);
    assert_eq!(ledger.diagnostics.timeout, 1);
    assert_eq!(ledger.diagnostics.warnings, 1);
    assert_eq!(ledger.diagnostics.other, 1);

    // A warning is the one category that is not a failure to get past.
    assert_eq!(ledger.stuck.compile_or_runner_failure_count, 6);
}

/// A runner that names a category without any prose still reported something.
#[test]
fn a_structured_category_is_recorded_without_setup_text() {
    let mut ledger = EvidenceLedger::default();
    ledger.record_test(
        Some(100),
        200,
        &serde_json::json!({
            "passed": 0,
            "total": 1,
            "diagnostic": { "category": "timeout" },
        }),
    );
    assert_eq!(ledger.diagnostics.timeout, 1);
    assert!(ledger.diagnostics.latest_signature.is_some());

    // Neither a category nor prose is not a diagnostic.
    let mut silent = EvidenceLedger::default();
    silent.record_test(
        Some(100),
        200,
        &serde_json::json!({ "passed": 0, "total": 1 }),
    );
    assert_eq!(silent.diagnostics.other, 0);
}

/// Python has no closing brace to move, so a statement leaving its block moves
/// nothing but its indentation, and that indentation is the program.
#[test]
fn a_statement_dedented_out_of_a_python_block_is_a_semantic_change() {
    let moved = analyze_code(
        "python",
        "def f(a):\n    t = 0\n    for x in a:\n        t += x\n        return t\n",
        "def f(a):\n    t = 0\n    for x in a:\n        t += x\n    return t\n",
    );
    assert_eq!(moved.observation, CodeObservation::Parsed);
    assert!(moved.semantic_change, "{moved:?}");
    assert_ne!(moved.classification, Some(CodeChangeClass::FormattingOnly));
}

/// A catch clause names its exception the way a signature names a parameter,
/// and it is control flow, not the enclosing function's interface.
#[test]
fn a_try_catch_added_inside_a_body_is_not_an_interface_change() {
    for (language, before, after) in [
        (
            "java",
            "class S { int f(int a) { return g(a); } }",
            "class S { int f(int a) { try { return g(a); } catch (Exception e) { return 0; } } }",
        ),
        (
            "cpp",
            "int f(int a) { return g(a); }",
            "int f(int a) { try { return g(a); } catch (const std::exception& e) { return 0; } }",
        ),
    ] {
        let analysis = analyze_code(language, before, after);
        assert_eq!(analysis.observation, CodeObservation::Parsed, "{language}");
        assert_eq!(
            analysis.classification,
            Some(CodeChangeClass::ControlFlow),
            "{language}: {analysis:?}"
        );
    }
}

/// The catch clause shields its own parameter and nothing below it: a function
/// declared in the handler still has a signature of its own.
#[test]
fn a_function_declared_inside_a_catch_keeps_its_interface() {
    let analysis = analyze_code(
        "javascript",
        "try { g(); } catch (e) { function f() { return 1; } }",
        "try { g(); } catch (e) { function f(a) { return 1; } }",
    );
    assert_eq!(analysis.observation, CodeObservation::Parsed);
    assert_eq!(
        analysis.classification,
        Some(CodeChangeClass::FunctionInterface),
        "{analysis:?}"
    );
}

/// Rewriting a comment leaves the count alone, so counting was not enough to
/// tell the edit from a reindent.
#[test]
fn a_comment_rewritten_in_place_is_still_a_comment_edit() {
    let rewritten = analyze_code(
        "python",
        "# first note\nvalue = 1\n",
        "# a different note\nvalue = 1\n",
    );
    assert_eq!(rewritten.classification, Some(CodeChangeClass::CommentOnly));
    assert!(!rewritten.semantic_change);

    // Indenting a comment does not rewrite it: the node starts at the hash.
    let moved = analyze_code(
        "python",
        "if a:\n    # note\n    pass\n",
        "if a:\n        # note\n        pass\n",
    );
    assert_eq!(moved.classification, Some(CodeChangeClass::FormattingOnly));
}

/// Held against the grammars rather than a list of examples: a grammar bump
/// that names a new comment kind should fail here, not read comments as code.
#[test]
fn every_comment_kind_the_grammars_name_is_a_comment() {
    let mut checked = 0;
    for language in ["c", "cpp", "java", "javascript", "python"] {
        let grammar = parser_language(language).unwrap();
        for id in 0..grammar.node_kind_count() as u16 {
            if let Some(kind) = grammar.node_kind_for_id(id)
                && grammar.node_kind_is_named(id)
                && kind.contains("comment")
            {
                assert!(comment_kind(kind), "{language}: {kind}");
                checked += 1;
            }
        }
    }
    assert!(checked >= 6, "{checked} comment kinds found");
}

/// A backslash continuation is where the line breaks, not what the program
/// does; changing the expression across it still is.
#[test]
fn a_python_line_continuation_is_layout() {
    let joined = "def f(a, b):\n    return a + b\n";
    let split = analyze_code(
        "python",
        joined,
        "def f(a, b):\n    return a + \\\n        b\n",
    );
    assert_eq!(split.observation, CodeObservation::Parsed);
    assert_eq!(split.classification, Some(CodeChangeClass::FormattingOnly));
    assert!(!split.semantic_change);

    let changed = analyze_code(
        "python",
        joined,
        "def f(a, b):\n    return a - \\\n        b\n",
    );
    assert!(changed.semantic_change);
}

#[test]
fn a_javascript_html_comment_is_a_comment_edit() {
    let analysis = analyze_code(
        "javascript",
        "function f() { return 1; }\n",
        "<!-- note\nfunction f() { return 1; }\n",
    );
    assert_eq!(analysis.classification, Some(CodeChangeClass::CommentOnly));
    assert!(!analysis.semantic_change);
}

#[test]
fn java_comments_do_not_advance_semantic_review_state() {
    for template in [
        "{comment}\nclass Solution { int solve(int n) { return n; } }",
        "class Solution { int solve({comment}\nint n) { return n; } }",
        "class Solution { int solve(int n) { for (int {comment}\ni = 0; i < n; i++) {} return n; } }",
        "class Solution { void solve() { Runnable r = () -> { {comment}\nreturn; }; } }",
    ] {
        for (before_comment, after_comment) in [
            ("", "// note"),
            ("// note", "// revised"),
            ("// note", ""),
            ("", "/* note */"),
            ("/* note */", "/* revised */"),
            ("/* note */", ""),
            ("// note", "/* note */"),
        ] {
            let before = template.replace("{comment}", before_comment);
            let after = template.replace("{comment}", after_comment);
            let analysis = analyze_code("java", &before, &after);
            assert_eq!(analysis.observation, CodeObservation::Parsed, "{after}");
            assert_eq!(
                analysis.classification,
                Some(CodeChangeClass::CommentOnly),
                "{before} -> {after}"
            );
            assert!(!analysis.semantic_change);
            assert!(analysis.changed_nodes.is_empty());
            let mut ledger = EvidenceLedger::default();
            ledger.record_code_with_analysis(
                None,
                100,
                "java",
                &before,
                false,
                analyze_code("java", "", &before),
            );
            ledger.record_code_with_analysis(None, 200, "java", &after, true, analysis);
            assert_eq!(ledger.code.semantic_revision, 0);
        }
    }
    let mixed = analyze_code(
        "java",
        "class Solution { int solve() { /* before */ return 1; } }",
        "class Solution { int solve() { /* after */ return 2; } }",
    );
    assert!(mixed.semantic_change);
    assert_eq!(mixed.classification, Some(CodeChangeClass::Expression));
}

/// A signature the caller can see, in the grammars where the node counts do
/// not move when it changes.
#[test]
fn a_parameter_list_change_is_an_interface_change() {
    for (language, before, after) in [
        (
            "python",
            "def solve(values):\n    return values\n",
            "def solve(values, target):\n    return values\n",
        ),
        (
            "c",
            "int f(int a) { return a; }",
            "long f(long a) { return a; }",
        ),
        (
            "javascript",
            "function f(a) { return a; }",
            "function f(a, b) { return a; }",
        ),
    ] {
        let analysis = analyze_code(language, before, after);
        assert_eq!(
            analysis.classification,
            Some(CodeChangeClass::FunctionInterface),
            "{language}"
        );
    }
}

/// The kind predicates are lists of alternatives, and a suite that exercises
/// one alternative cannot tell whether the others are wired to anything.
#[test]
fn a_data_structure_is_recognized_in_each_shape_the_grammars_name() {
    for (language, before, after) in [
        ("python", "value = 1\n", "value = [1, 2]\n"),
        ("python", "value = 1\n", "value = {1: 2}\n"),
        ("python", "value = 1\n", "value = {1, 2}\n"),
        ("javascript", "const v = 1;", "const v = [1, 2];"),
        ("javascript", "const v = 1;", "const v = { a: 1 };"),
    ] {
        let analysis = analyze_code(language, before, after);
        assert_eq!(
            analysis.classification,
            Some(CodeChangeClass::DataStructure),
            "{language}: {after}"
        );
    }
}

#[test]
fn an_interface_is_recognized_in_each_shape_the_grammars_name() {
    for (language, before, after) in [
        (
            "python",
            "value = 1\n",
            "def helper():\n    return 1\nvalue = 1\n",
        ),
        (
            "java",
            "class Main { }",
            "class Main { int helper() { return 1; } }",
        ),
        ("java", "class Main { }", "class Main { }\nclass Other { }"),
        (
            "c",
            "int f(int a) { return a; }",
            "int f(int a, int b) { return a; }",
        ),
        // Neither side has a parameter token to compare, so this one is
        // classified from the changed node kind rather than from the parameter
        // list, which is the other half of how an interface change is seen.
        ("java", "class Main { }\nclass Other { }", "class Main { }"),
        // The first parameter of all, which in these two grammars is a bare
        // identifier: the parentheses were already there and the comma only
        // arrives with the second one, so nothing but the name is added.
        (
            "python",
            "def solve():\n    return []\n",
            "def solve(items):\n    return []\n",
        ),
        (
            "javascript",
            "function solve() {}",
            "function solve(items) {}",
        ),
    ] {
        let analysis = analyze_code(language, before, after);
        assert_eq!(
            analysis.classification,
            Some(CodeChangeClass::FunctionInterface),
            "{language}: {after}"
        );
    }
}

/// An argument added to a call changes the identifiers without changing a
/// single other token, which is the one shape where the structure and the
/// tokens disagree: the parentheses are the same, so the leaves match while
/// the node counts do not. The call changed, so this is the expression
/// changing and not something being renamed.
#[test]
fn an_argument_added_to_a_call_is_an_expression_change() {
    let analysis = analyze_code("python", "value = f()\n", "value = f(a)\n");
    assert_eq!(analysis.changed_nodes, ["identifier:added:1"]);
    assert_eq!(analysis.classification, Some(CodeChangeClass::Expression));
    assert!(analysis.semantic_change);
}

/// The starter template is the browser's, and the ledger says so in both
/// places it keeps an analysis.
#[test]
fn a_buffer_the_candidate_did_not_write_carries_no_structural_claim() {
    let mut state = RuntimeState::default();
    apply_data_event_at(
        &mut state,
        crate::runtime::TOPIC_CODE_UPDATE,
        &serde_json::json!({ "code": "def solve(values):\n    return []\n" }),
        0.0,
        100,
    );
    let entry = &state.evidence_ledger.entries[0].observation;
    assert_eq!(entry["classification"], serde_json::Value::Null);
    assert_eq!(entry["semanticChange"], false);
    assert_eq!(entry["changedNodes"], serde_json::json!([]));

    // The projection carries `code` as well as the entries, so masking one and
    // not the other left the claim in front of the model anyway.
    let analysis = state.evidence_ledger.code.last_analysis.as_ref().unwrap();
    assert_eq!(analysis.classification, None);
    assert!(!analysis.semantic_change);
    assert!(analysis.changed_nodes.is_empty());

    // The parse is still an observation about the buffer.
    assert_eq!(analysis.observation, CodeObservation::Parsed);
    assert_eq!(state.evidence_ledger.code.semantic_revision, 0);
}

/// Progress is cases that were not passing before, not a run that happened.
#[test]
fn a_run_with_nothing_newly_passing_is_not_progress() {
    let mut ledger = EvidenceLedger::default();
    ledger.record_test(
        Some(100),
        100,
        &serde_json::json!({ "passed": 0, "total": 2 }),
    );
    assert!(ledger.progress.tested);
    assert!(!ledger.progress.test_progress);
    assert_eq!(ledger.stuck.last_test_progress_ms, None);
}

/// Coverage is a set, and the same phase evidenced twice is still one phase.
#[test]
fn a_phase_evidenced_twice_is_covered_once() {
    let mut ledger = EvidenceLedger::default();
    ledger.set_uncovered_coverage(["repeat", "example"]);
    ledger.record_coverage(100, "repeat");
    ledger.record_coverage(200, "repeat");
    assert_eq!(ledger.coverage.covered, ["repeat"]);
    assert_eq!(ledger.coverage.uncovered, ["example"]);
    assert_eq!(ledger.entries.len(), 1);
}

/// A phase nobody reached is not a phase that was covered.
#[test]
fn a_phase_skipped_when_the_clock_ran_out_is_not_covered() {
    let mut state = RuntimeState::default();
    state
        .evidence_ledger
        .set_uncovered_coverage(["repeat", "situation"]);
    record_framework_evidence(
        &mut state,
        &serde_json::json!({
            "phase": "repeat",
            "source": "candidate_speech",
            "kind": "observed",
            "confidence": 100,
            "summary": "Candidate restated the inputs and the return shape."
        }),
    )
    .unwrap();
    record_framework_evidence(
        &mut state,
        &serde_json::json!({
            "phase": "situation",
            "source": "session_timing",
            "kind": "skipped",
            "confidence": 100,
            "summary": "The interview ended before the behavioral round."
        }),
    )
    .unwrap();
    assert_eq!(state.evidence_ledger.coverage.covered, ["repeat"]);
    assert_eq!(state.evidence_ledger.coverage.uncovered, ["situation"]);
}

/// The received counter is the browser boundary, and the agent ending an
/// interview itself never crossed it.
#[test]
fn an_end_the_agent_decided_on_is_not_a_packet_that_arrived() {
    let mut state = RuntimeState::default();
    apply_server_event_at(
        &mut state,
        crate::runtime::TOPIC_CONTROL,
        &serde_json::json!({ "type": "end_interview", "reason": "time_elapsed" }),
        0.0,
        100,
    );
    assert!(state.evidence_ledger.lifecycle.ended);
    assert_eq!(state.evidence_ledger.metrics.raw_events_received, 0);

    // The same packet from the browser is one that arrived.
    let mut browser = RuntimeState::default();
    apply_data_event_at(
        &mut browser,
        crate::runtime::TOPIC_CONTROL,
        &serde_json::json!({ "type": "end_interview", "reason": "candidate_ended" }),
        0.0,
        100,
    );
    assert_eq!(browser.evidence_ledger.metrics.raw_events_received, 1);
}

/// Another tab's recovery baseline is not this edit's to spend.
#[test]
fn an_edit_in_one_language_leaves_another_tab_its_baseline() {
    let mut state = RuntimeState::default();
    for (receipt, language, code) in [
        (100, "python", "value = 1\n"),
        // An invalid python draft leaves python a baseline to recover from.
        (200, "python", "if value\n"),
        (300, "cpp", "int main() { return 0; }"),
        // A clean edit in C++ must not spend python's baseline.
        (400, "cpp", "int main() { return 1; }"),
        (500, "python", "if value\n"),
        (600, "python", "if value:\n    value = 2\n"),
    ] {
        apply_data_event_at(
            &mut state,
            crate::runtime::TOPIC_CODE_UPDATE,
            &serde_json::json!({ "code": code, "language": language }),
            0.0,
            receipt,
        );
    }
    assert_eq!(
        state.evidence_ledger.code.parser_observation,
        Some(CodeObservation::Parsed)
    );
    assert!(
        state
            .evidence_ledger
            .code
            .last_analysis
            .as_ref()
            .unwrap()
            .semantic_change
    );
}

#[test]
fn invalid_drafts_in_two_languages_keep_both_recovery_baselines() {
    let mut state = RuntimeState::default();
    for (receipt, language, code) in [
        (100, "python", "value = 1\n"),
        (200, "python", "if value\n"),
        (300, "cpp", "int value = 1;"),
        (400, "cpp", "int main() {\n"),
        // Returning to Python must still recover against Python's baseline,
        // even though the C++ invalid draft was recorded afterward.
        (500, "python", "if value\n"),
        (600, "python", "if value:\n    value = 2\n"),
    ] {
        apply_data_event_at(
            &mut state,
            crate::runtime::TOPIC_CODE_UPDATE,
            &serde_json::json!({ "code": code, "language": language }),
            0.0,
            receipt,
        );
    }

    assert_eq!(
        state.evidence_ledger.code.parser_observation,
        Some(CodeObservation::Parsed)
    );
    assert!(
        state
            .evidence_ledger
            .code
            .last_analysis
            .as_ref()
            .unwrap()
            .semantic_change
    );
    assert!(state.last_parseable_code.contains_key("cpp"));
    assert!(!state.last_parseable_code.contains_key("python"));
}

/// What the end packet carries, and what it is allowed to claim.
#[test]
fn the_end_packet_records_a_tab_switch_without_crediting_the_candidate() {
    let mut state = RuntimeState::default();
    apply_data_event_at(
        &mut state,
        crate::runtime::TOPIC_CODE_UPDATE,
        &serde_json::json!({ "code": "value = 1\n", "language": "python" }),
        0.0,
        100,
    );
    let revision = state.evidence_ledger.code.revision;
    let semantic = state.evidence_ledger.code.semantic_revision;

    // The same buffer under a language the candidate switched to on the way out
    // is still an observation worth recording, and still not an edit.
    apply_data_event_at(
        &mut state,
        crate::runtime::TOPIC_CONTROL,
        &serde_json::json!({
            "type": "end_interview",
            "reason": "candidate_ended",
            "code": "value = 1\n",
            "language": "javascript"
        }),
        0.0,
        200,
    );
    assert_eq!(state.evidence_ledger.code.revision, revision + 1);
    assert_eq!(state.evidence_ledger.code.semantic_revision, semantic);
    assert_eq!(state.evidence_ledger.code.language, "javascript");
}

/// An end packet that moved nothing changed no buffer, whatever else it
/// carried.
#[test]
fn an_end_packet_without_code_records_no_code_observation() {
    let mut state = RuntimeState::default();
    apply_data_event_at(
        &mut state,
        crate::runtime::TOPIC_CODE_UPDATE,
        &serde_json::json!({ "code": "value = 1\n", "language": "python" }),
        0.0,
        100,
    );
    let revision = state.evidence_ledger.code.revision;
    apply_data_event_at(
        &mut state,
        crate::runtime::TOPIC_CONTROL,
        &serde_json::json!({ "type": "end_interview", "reason": "candidate_ended" }),
        0.0,
        200,
    );
    assert_eq!(state.evidence_ledger.code.revision, revision);
    assert!(state.evidence_ledger.lifecycle.ended);
}

/// The language read out of an end packet is deliberately independent of the
/// code in it, so that a payload missing one does not discard the other. The
/// ledger is not: it records a buffer only when the payload carried one.
///
/// It used to follow the language regardless, filing the previous tab's buffer
/// under the new tab and parsing one language's code with another's grammar.
/// That reasoned from the code topic recording a switch that brought no new
/// text, but the code topic refuses a packet with no buffer at all, and an end
/// payload with no code has said nothing about the new tab's buffer. Two true
/// statements about different things -- the language the interview ended in,
/// and the last buffer the ledger was given -- beat one false one.
#[test]
fn an_end_packet_that_only_moves_the_language_records_no_buffer() {
    let mut state = RuntimeState::default();
    apply_data_event_at(
        &mut state,
        crate::runtime::TOPIC_CODE_UPDATE,
        &serde_json::json!({ "code": "value = 1\n", "language": "python" }),
        0.0,
        100,
    );
    let before = state.evidence_ledger.code.clone();
    apply_data_event_at(
        &mut state,
        crate::runtime::TOPIC_CONTROL,
        &serde_json::json!({
            "type": "end_interview",
            "reason": "candidate_ended",
            "language": "javascript"
        }),
        0.0,
        200,
    );

    // The interview ended in the tab the payload named.
    assert_eq!(state.language, "javascript");
    assert!(state.evidence_ledger.lifecycle.ended);

    // And the ledger still describes the buffer it was actually given, under
    // the language it was given in, with no observation made up for the new
    // tab.
    assert_eq!(state.evidence_ledger.code, before);
    assert_eq!(state.evidence_ledger.code.language, "python");
}

/// Kinds outside the three categories are an expression edit however many of
/// them moved: `- 1` on a loop bound adds an operator and a literal, and that
/// is one edit, not several.
#[test]
fn uncategorized_kinds_are_an_expression_edit_however_many_moved() {
    let single = analyze_code(
        "python",
        "def f():\n    if a:\n        pass\n",
        "def f():\n    if a:\n        pass\n        pass\n",
    );
    assert_eq!(single.changed_nodes, ["pass_statement:added:1"]);
    assert_eq!(single.classification, Some(CodeChangeClass::Expression));

    let several = analyze_code(
        "python",
        "def f():\n    if a:\n        pass\n",
        "def f():\n    if a:\n        pass\n    return total + 1\n",
    );
    assert!(several.changed_nodes.len() > 1);
    assert_eq!(several.classification, Some(CodeChangeClass::Expression));

    let bound = analyze_code(
        "python",
        "def f(nums):\n    for i in range(len(nums)):\n        pass\n",
        "def f(nums):\n    for i in range(len(nums) - 1):\n        pass\n",
    );
    assert_eq!(bound.classification, Some(CodeChangeClass::Expression));
}

/// The loop was already there, so its count never moved; what it iterates with
/// did. Unpacking builds no data structure, and renaming the loop variable is
/// still only a rename.
#[test]
fn a_loop_that_binds_differently_is_a_control_flow_edit() {
    let base = "def f(nums):\n    for i in range(len(nums)):\n        print(nums[i])\n";
    let unpacked = analyze_code(
        "python",
        base,
        "def f(nums):\n    for i, n in enumerate(nums):\n        print(n)\n",
    );
    assert_eq!(unpacked.classification, Some(CodeChangeClass::ControlFlow));

    let renamed = analyze_code(
        "python",
        base,
        "def f(nums):\n    for j in range(len(nums)):\n        print(nums[j])\n",
    );
    assert_eq!(
        renamed.classification,
        Some(CodeChangeClass::IdentifierOnly)
    );

    // A C-style loop's start value is an expression, as the same off-by-one is
    // in Python's `range`, and a range loop's init-statement is not what it
    // binds.
    for (language, before, after) in [
        (
            "c",
            "void f(int n) { for (int i = 0; i < n; i++) {} }",
            "void f(int n) { for (int i = n - 1; i >= 0; i--) {} }",
        ),
        (
            "javascript",
            "function f(a) { for (let i = 0; i < a.length; i++) {} }",
            "function f(a) { for (let i = a.length - 1; i >= 0; i--) {} }",
        ),
    ] {
        let analysis = analyze_code(language, before, after);
        assert_eq!(
            analysis.classification,
            Some(CodeChangeClass::Expression),
            "{language}"
        );
    }
    let init_statement = analyze_code(
        "cpp",
        "void f(std::vector<int> v) { for (auto w = v; int x : w) {} }",
        "void f(std::vector<int> v) { for (auto w = v; auto [x, y] : w) {} }",
    );
    assert_eq!(init_statement.observation, CodeObservation::Parsed);
    assert_eq!(
        init_statement.classification,
        Some(CodeChangeClass::ControlFlow)
    );

    let destructured = analyze_code(
        "javascript",
        "function f(p) { const x = p[0]; return x; }",
        "function f(p) { const [x] = p; return x; }",
    );
    assert_ne!(
        destructured.classification,
        Some(CodeChangeClass::DataStructure)
    );
}

/// A tab switch is an observation even when the two buffers happen to match.
///
/// Two tabs can hold the same text, by starter or by the candidate typing it,
/// and the packet that carries the switch then changes no digest at all. The
/// ledger still has to move: its `code.language` is what the projection says
/// the candidate is working in, and leaving it on the tab they left describes
/// the wrong language, with the parse of a buffer read under the wrong grammar
/// beside it.
#[test]
fn a_switch_carrying_the_same_text_still_moves_the_ledger_language() {
    let mut state = RuntimeState::default();
    for (receipt, language) in [(100, "python"), (200, "javascript")] {
        apply_data_event_at(
            &mut state,
            crate::runtime::TOPIC_CODE_UPDATE,
            &serde_json::json!({ "code": "value = 1\n", "language": language }),
            0.0,
            receipt,
        );
    }
    assert_eq!(state.language, "javascript");
    assert_eq!(state.evidence_ledger.code.language, "javascript");
    assert_eq!(
        state.evidence_ledger.code.parser_observation,
        Some(CodeObservation::Parsed)
    );

    // The switch is not an edit, whatever it carried.
    assert_eq!(state.evidence_ledger.code.candidate_edits, 0);
    assert_eq!(state.evidence_ledger.code.semantic_revision, 0);
}

/// An interview that ends before the first editor packet lands still carries a
/// buffer out with it, and that buffer is the starter the browser was handed.
/// Comparing it with the runtime's own buffer alone reads it as work, because
/// the runtime has nothing yet: the template is what it has to be measured
/// against.
#[test]
fn an_end_packet_carrying_only_the_starter_is_not_candidate_work() {
    let starter = "def solve(values):\n    return []\n";
    let mut state = RuntimeState::default();
    state
        .code_templates
        .insert("python".to_string(), starter.to_string());
    apply_data_event_at(
        &mut state,
        crate::runtime::TOPIC_CONTROL,
        &serde_json::json!({
            "type": "end_interview",
            "reason": "candidate_ended",
            "code": starter,
            "language": "python"
        }),
        0.0,
        200,
    );
    assert_eq!(state.evidence_ledger.code.candidate_edits, 0);
    assert_eq!(state.evidence_ledger.code.semantic_revision, 0);

    // A final edit that never got its own packet is still the candidate's.
    let mut edited = RuntimeState::default();
    edited
        .code_templates
        .insert("python".to_string(), starter.to_string());
    apply_data_event_at(
        &mut edited,
        crate::runtime::TOPIC_CONTROL,
        &serde_json::json!({
            "type": "end_interview",
            "reason": "candidate_ended",
            "code": "def solve(values):\n    return [0, 1]\n",
            "language": "python"
        }),
        0.0,
        200,
    );
    assert_eq!(edited.evidence_ledger.code.candidate_edits, 1);
}

/// A category the wire allows is a category the reducer counts.
#[test]
fn a_runner_naming_other_without_prose_is_still_a_diagnostic() {
    let mut ledger = EvidenceLedger::default();
    ledger.record_test(
        Some(100),
        200,
        &serde_json::json!({
            "passed": 0,
            "total": 0,
            "diagnostic": { "category": "other" },
        }),
    );
    assert_eq!(ledger.diagnostics.other, 1);
    assert_eq!(ledger.stuck.compile_or_runner_failure_count, 1);

    // A category no runner names is still not classified without prose to hash,
    // which is the rule that keeps compiler text out of the categories.
    let mut unnamed = EvidenceLedger::default();
    unnamed.record_test(
        Some(100),
        200,
        &serde_json::json!({
            "passed": 0,
            "total": 0,
            "diagnostic": { "category": "algorithm" },
        }),
    );
    assert_eq!(unnamed.diagnostics.other, 0);
}

/// The final edit is measured against the starter, not against nothing.
#[test]
fn a_final_only_edit_is_diffed_against_the_starter_it_came_from() {
    let starter = "def solve(values):\n    return []\n";
    let mut state = RuntimeState::default();
    state
        .code_templates
        .insert("python".to_string(), starter.to_string());
    apply_data_event_at(
        &mut state,
        crate::runtime::TOPIC_CONTROL,
        &serde_json::json!({
            "type": "end_interview",
            "reason": "candidate_ended",
            "code": "def solve(values):\n    return [0, 1]\n",
            "language": "python"
        }),
        0.0,
        200,
    );
    let analysis = state.evidence_ledger.code.last_analysis.as_ref().unwrap();
    assert!(analysis.semantic_change);

    // Against an empty buffer every token of the starter reads as added, and
    // the one line the candidate changed is lost in it.
    assert_eq!(analysis.changed_nodes, ["integer:added:2"]);
    assert_eq!(analysis.classification, Some(CodeChangeClass::Expression));
}

/// Calling a function is not changing a data structure.
#[test]
fn extracting_a_helper_is_not_a_data_structure_change() {
    // The call node is spelled with the word "list" in three of these five
    // grammars and without it in the fourth, so a substring match described the
    // same refactor differently depending on which tab it happened in.
    for (language, before, after) in [
        (
            "python",
            "def f(a):\n    return a\n",
            "def f(a):\n    return g(a)\n",
        ),
        (
            "c",
            "int f(int a) { return a; }",
            "int f(int a) { return g(a); }",
        ),
        (
            "java",
            "class M { int f(int a) { return a; } }",
            "class M { int f(int a) { return g(a); } }",
        ),
    ] {
        let analysis = analyze_code(language, before, after);
        assert_ne!(
            analysis.classification,
            Some(CodeChangeClass::DataStructure),
            "{language}"
        );
    }

    // A literal that really is one still reads as one.
    for (language, before, after) in [
        ("python", "value = 1\n", "value = {1: 2}\n"),
        (
            "c",
            "int f(void) { return 0; }",
            "int f(void) { int a[] = {1, 2}; return 0; }",
        ),
    ] {
        let analysis = analyze_code(language, before, after);
        assert_eq!(
            analysis.classification,
            Some(CodeChangeClass::DataStructure),
            "{language}"
        );
    }
}

/// Cases that started passing end a failure streak, whatever the reported
/// failures say.
#[test]
fn a_run_that_fixed_something_is_not_the_same_failure_again() {
    let failures = serde_json::json!([
        { "label": "edge", "expected": "1", "got": "0", "error": null }
    ]);
    let mut ledger = EvidenceLedger::default();
    ledger.record_test(
        Some(100),
        100,
        &serde_json::json!({ "passed": 0, "total": 10, "failures": failures }),
    );
    ledger.record_test(
        Some(200),
        200,
        &serde_json::json!({ "passed": 0, "total": 10, "failures": failures }),
    );
    assert_eq!(ledger.stuck.repeated_failure_count, 2);

    // Six cases fixed, and only the first few failures are ever reported, so
    // the signature is unchanged. The candidate is not stuck on it.
    ledger.record_test(
        Some(300),
        300,
        &serde_json::json!({ "passed": 6, "total": 10, "failures": failures }),
    );
    assert_eq!(ledger.tests.newly_passed, 6);
    assert_eq!(ledger.stuck.repeated_failure_count, 1);
}

/// A run that never reached the cases still carries the judge's case count.
///
/// The browser fills `total` from the spec before it runs anything and only
/// ever sends a setup error in place of case outcomes, so a compile failure
/// arrives as nought of five with no failures listed. Reading the count alone
/// made that a five-case regression the candidate never caused.
#[test]
fn a_setup_failure_carrying_the_case_count_is_still_not_a_run() {
    let mut ledger = EvidenceLedger::default();
    ledger.record_test(
        Some(10),
        100,
        &serde_json::json!({ "passed": 5, "total": 5 }),
    );
    ledger.record_test(
        Some(20),
        200,
        &serde_json::json!({
            "passed": 0,
            "total": 5,
            "setupError": "error: expected ';' before '}' token",
            "diagnostic": { "category": "other" },
            "failures": [],
        }),
    );
    assert_eq!(
        ledger.entries.last().unwrap().observation["executed"],
        false
    );
    assert_eq!(ledger.tests.passed, 5);
    assert_eq!(ledger.tests.newly_failed, 0);
    assert!(!ledger.tests.regression);

    // The attempt and the diagnostic are recorded either way: what the run did
    // not do is move the case counts.
    assert!(ledger.progress.runner_attempted);
    assert_eq!(ledger.diagnostics.other, 1);
    assert_eq!(ledger.stuck.compile_or_runner_failure_count, 1);

    // And the same code, run again once the build is fixed, is not five cases
    // newly passing.
    ledger.record_test(
        Some(30),
        300,
        &serde_json::json!({ "passed": 5, "total": 5 }),
    );
    assert_eq!(ledger.tests.newly_passed, 0);
}

/// A parameter's name is not its interface.
#[test]
fn renaming_a_parameter_is_not_a_signature_change() {
    for (language, before, after) in [
        (
            "python",
            "def solve(n):\n    return n + 1\n",
            "def solve(count):\n    return count + 1\n",
        ),
        (
            "javascript",
            "function solve(n) { return n + 1; }",
            "function solve(count) { return count + 1; }",
        ),
    ] {
        assert_eq!(
            analyze_code(language, before, after).classification,
            Some(CodeChangeClass::IdentifierOnly),
            "{language}"
        );
    }

    // The interface facts are read before the changed node kinds, so a rename
    // that reached them took the classification with it and left the loop the
    // candidate actually wrote unmentioned.
    assert_eq!(
        analyze_code(
            "python",
            "def solve(n):\n    return n\n",
            "def solve(m):\n    total = 0\n    for i in range(10):\n        total += i\n    return total\n",
        )
        .classification,
        Some(CodeChangeClass::ControlFlow)
    );
}

/// A run that reported no cases at all is not a run, and above all it is not a
/// pass: nought of nought satisfies "every case passed" arithmetically, which
/// is how an empty run talks its way into being a green one.
#[test]
fn a_run_reporting_no_cases_at_all_is_not_a_pass() {
    let mut ledger = EvidenceLedger::default();
    ledger.record_test(
        Some(100),
        200,
        &serde_json::json!({ "passed": 0, "total": 0 }),
    );
    assert!(!ledger.progress.all_tests_passed_once);
    assert!(!ledger.progress.tested);
    assert!(ledger.progress.runner_attempted);
    assert_eq!(
        ledger.entries.last().unwrap().observation["executed"],
        false
    );
}

/// What a setup failure is allowed to say about the cases.
#[test]
fn a_run_that_never_started_reports_no_failures() {
    let mut ledger = EvidenceLedger::default();
    ledger.record_test(
        Some(10),
        100,
        &serde_json::json!({ "passed": 5, "total": 5 }),
    );

    // The shape the browser actually sends: the case count comes from the judge
    // before anything ran, so `total` is five and `passed` is nought.
    ledger.record_test(
        Some(20),
        200,
        &serde_json::json!({
            "passed": 0,
            "total": 5,
            "failures": [],
            "setupError": "Compiler Explorer returned 503",
        }),
    );
    let entry = &ledger.entries.last().unwrap().observation;
    assert_eq!(entry["executed"], false);

    // Derived from the two counts, so it is the one that would invent five
    // failures out of a runner that never reached a case.
    assert_eq!(entry["failed"], 0);
    assert_eq!(entry["regression"], false);
    assert_eq!(entry["newlyFailed"], 0);

    // What the browser claimed is still recorded as claimed.
    assert_eq!(entry["total"], 5);
    assert_eq!(entry["passed"], 0);
}

/// A call is not a signature. Every grammar here spells one with a node kind
/// containing "function" or "method", and the predicate that read those as
/// interface changes was matching on substrings, so calling a helper inside a
/// body -- most of what an interview contains -- reported the candidate as
/// having changed the signature of the function they called it from.
#[test]
fn a_call_inside_a_body_is_not_a_changed_signature() {
    for (language, before, after) in [
        (
            "java",
            "class S {\n  int f(int[] a) {\n    int s = 0;\n    return s;\n  }\n}\n",
            "class S {\n  int f(int[] a) {\n    int s = 0;\n    System.out.println(s);\n    return s;\n  }\n}\n",
        ),
        (
            "javascript",
            "function solve(a) { return a; }\n",
            "function solve(a) { return a.map(x => x * 2); }\n",
        ),
        (
            "python",
            "def solve(xs):\n    return xs\n",
            "def solve(xs):\n    return sorted(xs)\n",
        ),
    ] {
        let analysis = analyze_code(language, before, after);
        assert!(analysis.semantic_change, "{language}");
        assert_ne!(
            analysis.classification,
            Some(CodeChangeClass::FunctionInterface),
            "{language}: a call in a body"
        );
    }
}

/// A comparator is a function the candidate wrote to pass somewhere, and its
/// parameters are spelled with the same node kinds a signature's are:
/// `formal_parameters` in Java, `parameter_list` in C++, `lambda_parameters` in
/// Python. Reading the two as one kind reported writing a sort comparator as
/// changing the enclosing function's signature, and, because that branch is
/// taken before the one that reads the changed node kinds, said nothing at all
/// about the loop added beside it.
#[test]
fn a_closures_parameters_are_not_the_enclosing_signature() {
    for (language, before, after, expected) in [
        // The loop is what this edit is about, and naming the comparator a
        // signature change is what kept it from being said.
        (
            "python",
            "def solve(xs):\n    return xs\n",
            "def solve(xs):\n    ys = sorted(xs, key=lambda p: p[1])\n    for y in ys:\n        print(y)\n    return xs\n",
            CodeChangeClass::ControlFlow,
        ),
        (
            "java",
            "class S {\n  int f(java.util.List<Integer> a) { return 0; }\n}\n",
            "class S {\n  int f(java.util.List<Integer> a) {\n    a.sort((Integer x, Integer y) -> x - y);\n    return 0;\n  }\n}\n",
            CodeChangeClass::Expression,
        ),
        (
            "cpp",
            "int f(std::vector<int> v) { return 0; }\n",
            "int f(std::vector<int> v) {\n  std::sort(v.begin(), v.end(), [](int a, int b) { return a < b; });\n  return 0;\n}\n",
            CodeChangeClass::Expression,
        ),
    ] {
        let analysis = analyze_code(language, before, after);
        assert_eq!(analysis.classification, Some(expected), "{language}");

        // The facts still say a closure appeared, and say which parts belong to
        // it, so the model is not left without the edit.
        assert!(
            analysis
                .changed_nodes
                .iter()
                .any(|fact| fact.starts_with("closure.")),
            "{language}: {:?}",
            analysis.changed_nodes
        );
    }
}

/// A closure is not a wall. Its parameter list is not the enclosing signature,
/// which is what the qualification is for, but a loop written inside one is
/// still a loop the candidate wrote. The three predicates used to disagree
/// about this by accident -- two matched kinds exactly and so were blinded by
/// the prefix, one matched on substrings and so was not.
#[test]
fn a_loop_inside_a_closure_is_still_a_loop() {
    let analysis = analyze_code(
        "javascript",
        "function solve(rows) { return rows.map((row) => row); }\n",
        "function solve(rows) { return rows.map((row) => { for (const cell of row) { total += cell; } return row; }); }\n",
    );
    assert!(
        analysis
            .changed_nodes
            .iter()
            .any(|fact| fact.starts_with("closure.for_in_statement")),
        "{:?}",
        analysis.changed_nodes
    );
    assert_eq!(analysis.classification, Some(CodeChangeClass::ControlFlow));
}

/// `for_statement` is what one of these five grammars calls a loop. A list that
/// holds only the shapes C names answers for C, and the most ordinary loop a
/// candidate writes in the other tabs was reported as an unclassifiable edit.
#[test]
fn a_loop_is_recognized_in_each_shape_the_grammars_name() {
    for (language, before, after) in [
        (
            "javascript",
            "function solve(items) { return 0; }\n",
            "function solve(items) { for (const x of items) { } return 0; }\n",
        ),
        (
            "java",
            "class S {\n  int f(int[] a) { return 0; }\n}\n",
            "class S {\n  int f(int[] a) {\n    for (int x : a) { }\n    return 0;\n  }\n}\n",
        ),
        (
            "cpp",
            "int f(std::vector<int> v) { return 0; }\n",
            "int f(std::vector<int> v) {\n  for (auto x : v) { }\n  return 0;\n}\n",
        ),
        (
            "c",
            "int f(int n) { return n; }\n",
            "int f(int n) { int i = 0; do { i++; } while (i < n); return n; }\n",
        ),
    ] {
        let analysis = analyze_code(language, before, after);
        assert_eq!(
            analysis.classification,
            Some(CodeChangeClass::ControlFlow),
            "{language}: {after}"
        );
    }
}

/// The edit that repairs a broken draft is a semantic change, and it was
/// reported as another invalid one whenever the streak did not begin from a
/// buffer this server had seen parse -- an invalid first packet, or a buffer
/// restored into the editor. The pairwise analysis says `syntax_invalid` when
/// either side fails to parse, and with no baseline to diff against there was
/// nothing to recover it with, so `semantic_revision` never moved and the watch
/// loop never saw the moment the code started compiling.
#[test]
fn a_fix_with_no_baseline_is_still_a_buffer_that_parses() {
    let mut state = RuntimeState::default();
    apply_data_event_at(
        &mut state,
        crate::runtime::TOPIC_CODE_UPDATE,
        &serde_json::json!({ "code": "def solve(:\n", "language": "python" }),
        0.0,
        100,
    );
    assert_eq!(
        state.evidence_ledger.code.parser_observation,
        Some(CodeObservation::SyntaxInvalid)
    );
    let semantic = state.evidence_ledger.code.semantic_revision;

    apply_data_event_at(
        &mut state,
        crate::runtime::TOPIC_CODE_UPDATE,
        &serde_json::json!({ "code": "def solve():\n    return 1\n", "language": "python" }),
        0.0,
        200,
    );
    assert_eq!(
        state.evidence_ledger.code.parser_observation,
        Some(CodeObservation::Parsed)
    );
    assert_eq!(state.evidence_ledger.code.semantic_revision, semantic + 1);

    // No classification is invented from a comparison that could not be made.
    // The parse is the observation that was available, and it is the only claim
    // recorded.
    let analysis = state.evidence_ledger.code.last_analysis.clone().unwrap();
    assert_eq!(analysis.classification, None);
    assert!(analysis.changed_nodes.is_empty());
}

/// A baseline still wins where there is one, because a diff says more than a
/// parse does.
#[test]
fn a_fix_with_a_baseline_is_still_classified_against_it() {
    let mut state = RuntimeState::default();
    for (code, receipt) in [
        ("def solve(xs):\n    return xs\n", 100),
        ("def solve(xs):\n    if xs\n", 200),
        ("def solve(xs):\n    if xs:\n        return xs\n", 300),
    ] {
        apply_data_event_at(
            &mut state,
            crate::runtime::TOPIC_CODE_UPDATE,
            &serde_json::json!({ "code": code, "language": "python" }),
            0.0,
            receipt,
        );
    }
    let analysis = state.evidence_ledger.code.last_analysis.clone().unwrap();
    assert_eq!(analysis.observation, CodeObservation::Parsed);
    assert_eq!(analysis.classification, Some(CodeChangeClass::ControlFlow));
}

/// The one line the counters ever reach a person through, and the only part of
/// it nothing else can check: the format string and the order of the twelve
/// fields in it.
#[test]
fn the_session_cost_is_written_somewhere_a_person_can_read_it() {
    let mut ledger = EvidenceLedger::default();

    // A distinct, nonzero size for every counter. A counter left at zero hides
    // its own term in the total, since adding nought and subtracting it agree:
    // the mutation lane found exactly that, two `+` it could flip unseen.
    // Distinct sizes also mean two fields swapped in the line cannot read the
    // same.
    ledger.record_model_input(ModelInputKind::Watch, "watch");
    ledger.record_model_input(ModelInputKind::Turn, "greeting");
    ledger.record_model_input(ModelInputKind::Interim, "interim");
    ledger.record_model_input(ModelInputKind::FinalReport, "finalrept");
    ledger.record_model_input(ModelInputKind::ReadEditor, "editor");
    ledger.record_model_input(ModelInputKind::ToolResponse, "hint");
    ledger.record_received_event();

    let line = ledger.metrics.cost_line();
    assert_eq!(
        line,
        "codetrial model_input_bytes total=39 watch=5/1 turn=8/1 interim=7/1 \
         report=9/1 read_editor=6/1 tool=4/1 events_received=1 code_events=0"
    );
}

/// The ledger projections the prompt samples carry, checked against what
/// `prompt_view` renders for the ledgers they describe.
///
/// The samples live in an integration test, where `prompt_view` is out of
/// reach, so they were frozen strings: regenerating the prompt golden rewrote
/// it from the same strings, and a change to the projection's shape would have
/// left them describing a ledger production no longer sends while every test
/// stayed green. Held here, where the projection is in reach, they fail the
/// moment it moves. Regenerate them with `UPDATE_EVIDENCE_GOLDEN=1`, the same
/// switch as the replayed ledger they sit beside.
#[test]
fn the_prompt_samples_carry_what_prompt_view_renders() {
    let fresh = || RuntimeState::for_problem(crate::agent::get_problem(Some("two-sum")));

    // Nobody has typed yet: two turns and the seeded phases.
    let mut early = fresh();
    early
        .evidence_ledger
        .record_conversation_turn(1_000, "interviewer");
    early
        .evidence_ledger
        .record_conversation_turn(2_000, "candidate");

    // A few minutes in: a phase covered, an edit, and a run.
    let mut working = fresh();
    working.evidence_ledger.record_coverage(1_000, "repeat");
    working
        .evidence_ledger
        .record_conversation_turn(1_500, "candidate");
    let edited = "def two_sum(nums, target):\n    seen = {}\n    return []\n";
    working.evidence_ledger.record_code_with_analysis(
        Some(2_000),
        2_100,
        "python",
        edited,
        true,
        analyze_code(
            "python",
            "def two_sum(nums, target):\n    return []\n",
            edited,
        ),
    );
    working.evidence_ledger.record_test(
        Some(3_000),
        3_100,
        &serde_json::json!({
            "passed": 1, "total": 3, "language": "python", "setupError": null,
            "failures": [{ "label": "duplicates", "expected": "[0,1]", "got": "[]", "error": null }]
        }),
    );

    let watch = |state: &RuntimeState| state.evidence_ledger.prompt_view(ViewFor::Watch);
    let rendered = serde_json::json!({
        "empty": watch(&fresh()).join("\n"),
        "early": watch(&early).join("\n"),
        "working": watch(&working).join("\n"),

        // What a watch prompt sends once an earlier one has shown `early`: the
        // lines that differ from what the session already holds.
        "workingChanged": watch(&working)
            .into_iter()
            .filter(|line| !watch(&early).contains(line))
            .collect::<Vec<_>>()
            .join("\n"),
        "workingInterim": working.evidence_ledger.prompt_view(ViewFor::Interim).join("\n"),
        "workingReport": working.evidence_ledger.prompt_view(ViewFor::Report).join("\n"),
    });
    let path = "tests/fixtures/evidence-projections.json";
    if std::env::var_os("UPDATE_EVIDENCE_GOLDEN").is_some() {
        let mut text = serde_json::to_string_pretty(&rendered).expect("projections serialize");
        text.push('\n');
        std::fs::write(path, text).expect("projection fixture should write");
    }

    // Read back from disk rather than embedded, like the replayed ledger above:
    // an embedded copy is fixed at build time, so a regeneration would compare
    // the fresh file against the stale one it had just replaced and fail.
    let frozen: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(path).expect("projection fixture should read"),
    )
    .expect("projection fixture should parse");
    assert_eq!(
        rendered, frozen,
        "the prompt samples no longer carry what prompt_view renders; regenerate {path}"
    );
}

/// The cache is only a cache if what it produces is what a fresh parse does:
/// every step of a session here is analyzed both ways and must agree, through
/// typing, an edit in the middle of a line, a deleted line, a draft that stops
/// parsing and the fix after it, a multi-byte character ahead of the edits,
/// and a trip to another tab and back, where the cached tree belongs to the
/// other grammar and must not be read.
#[test]
fn a_cached_incremental_parse_analyzes_exactly_as_a_fresh_one() {
    // The Greek letters are there for their byte width: an edit offset that
    // lands inside one is the bug an incremental edit can have and a fresh
    // parse cannot.
    let solution = "def two_sum(nums, target):\n    seen = {}  # \u{3b1}\u{3b2}\n    for i, n in enumerate(nums):\n        if target - n in seen:\n            return [seen[target - n], i]\n        seen[n] = i\n    return []\n";
    let chars = solution.chars().collect::<Vec<_>>();
    let mut steps = chars
        .chunks(5)
        .scan(String::new(), |typed, chunk| {
            typed.extend(chunk);
            Some(("python", typed.clone()))
        })
        .collect::<Vec<_>>();
    let edited = solution.replace("target - n in", "target + n in");
    steps.push(("python", edited.clone()));

    // Longer and shorter in the middle, where an edit that claims too little
    // leaves the old tree's later nodes at the wrong offsets.
    let widened = edited.replace("for i, n in", "for position, value in");
    steps.push(("python", widened.clone()));
    steps.push(("python", widened.replace("position, value", "i, n")));
    steps.push(("python", edited.replace("        seen[n] = i\n", "")));
    steps.push((
        "python",
        edited.replace("    return []\n", "    if (\n    return []\n"),
    ));
    steps.push(("python", edited.clone()));
    steps.push(("javascript", "function f(a) { return a; }\n".to_string()));
    steps.push((
        "javascript",
        "function f(a) { return a + 1; }\n".to_string(),
    ));
    steps.push(("python", edited.clone()));
    steps.push(("python", edited.replace("seen", "index")));

    // The trees themselves, node by node, before the analyses: two trees can
    // count the same kinds while one has its nodes at the wrong offsets, and an
    // edit that claims too little of the buffer produces exactly that.
    fn nodes(tree: &tree_sitter::Tree) -> Vec<(&'static str, usize, usize)> {
        let mut out = Vec::new();
        let mut cursor = tree.walk();
        loop {
            let node = cursor.node();
            out.push((node.kind(), node.start_byte(), node.end_byte()));
            if cursor.goto_first_child() || cursor.goto_next_sibling() {
                continue;
            }
            loop {
                if !cursor.goto_parent() {
                    return out;
                }
                if cursor.goto_next_sibling() {
                    break;
                }
            }
        }
    }
    for pair in steps.windows(2) {
        let [(language, previous), (next, current)] = pair else {
            unreachable!()
        };
        if language != next {
            continue;
        }
        let grammar = grammar_key(language).unwrap();
        let mut edited = parse_with(grammar, previous, None).unwrap();
        edited.edit(&input_edit(previous, current));
        let incremental = parse_with(grammar, current, Some(&edited)).unwrap();
        let fresh = parse_with(grammar, current, None).unwrap();
        assert_eq!(
            nodes(&incremental),
            nodes(&fresh),
            "{previous:?} -> {current:?}"
        );
    }

    let mut cache = ParseCache::default();
    let (mut language, mut previous) = ("python", String::new());
    observe_code_cached(&mut cache, language, &previous);
    for (step, (next_language, current)) in steps.into_iter().enumerate() {
        if next_language != language {
            assert_eq!(
                observe_code_cached(&mut cache, next_language, &current),
                observe_code(next_language, &current),
                "step {step}"
            );
        } else {
            let (analysis, alone) =
                analyze_code_cached(&mut cache, next_language, &previous, &current);
            assert_eq!(
                analysis,
                analyze_code(next_language, &previous, &current),
                "step {step}"
            );
            assert_eq!(
                alone,
                observe_code(next_language, &current).observation,
                "step {step}"
            );
        }
        (language, previous) = (next_language, current);
    }
}

/// The cache holds the last buffer it was given, for that grammar only, and is
/// no part of the state it sits in.
#[test]
fn the_parse_cache_holds_the_last_buffer_for_its_grammar_only() {
    let mut cache = ParseCache::default();
    assert!(cache.tree_for("python", "x = 1\n").is_none());
    observe_code_cached(&mut cache, "python", "x = 1\n");
    assert!(cache.tree_for("python", "x = 1\n").is_some());
    assert!(cache.tree_for("python", "x = 2\n").is_none());
    assert!(cache.tree_for("javascript", "x = 1\n").is_none());

    analyze_code_cached(&mut cache, "python", "x = 1\n", "x = 2\n");
    assert!(cache.tree_for("python", "x = 2\n").is_some());
    assert!(cache.tree_for("python", "x = 1\n").is_none());

    assert_eq!(cache, ParseCache::default());
    assert_eq!(format!("{cache:?}"), "ParseCache");
}

/// The limit is inclusive: a buffer of exactly 64 KiB is parsed, one byte
/// more is not.
#[test]
fn a_buffer_of_exactly_the_parse_limit_is_parsed() {
    let exact = format!("{}#ab\n", "x = 1\n".repeat(10_922));
    assert_eq!(exact.len(), 64 * 1024);
    assert_eq!(
        analyze_code("python", "x = 1\n", &exact).observation,
        CodeObservation::Parsed
    );
    assert_eq!(
        observe_code("python", &exact).observation,
        CodeObservation::Parsed
    );
    let over = format!("{exact}#\n");
    assert_eq!(
        observe_code("python", &over).observation,
        CodeObservation::ParserUnavailable
    );
}

/// The edit itself, field by field. A tree parsed from a wrong edit often
/// still matches a fresh one, because tree-sitter reparses whatever an edit
/// makes it doubt, so these are checked against the bytes and not the tree.
#[test]
fn an_input_edit_names_exactly_the_bytes_that_changed() {
    let point = |row, column| tree_sitter::Point::new(row, column);
    let fields = |edit: tree_sitter::InputEdit| {
        (
            edit.start_byte,
            edit.old_end_byte,
            edit.new_end_byte,
            edit.start_position,
            edit.old_end_position,
            edit.new_end_position,
        )
    };

    // An insertion on the second line.
    assert_eq!(
        fields(input_edit("ab\ncd\n", "ab\ncXd\n")),
        (4, 4, 5, point(1, 1), point(1, 1), point(1, 2))
    );

    // The shared prefix ends inside a two-byte character, so the edit starts at
    // the character.
    assert_eq!(
        fields(input_edit("a\u{3b1}", "a\u{3b2}")),
        (1, 3, 3, point(0, 1), point(0, 3), point(0, 3))
    );

    // The shared suffix starts inside one, so it ends there instead; and a
    // suffix that starts on a character is kept whole.
    assert_eq!(
        fields(input_edit("\u{f1}", "\u{3b1}")),
        (0, 2, 2, point(0, 0), point(0, 2), point(0, 2))
    );
    assert_eq!(
        fields(input_edit("x\u{3b1}", "y\u{3b1}")),
        (0, 1, 1, point(0, 0), point(0, 1), point(0, 1))
    );
}

/// A paste past the size the parser is given is recorded as not parsed, which
/// claims nothing about it, rather than stalling the agent's task on it.
#[test]
fn a_buffer_past_the_parse_limit_is_not_parsed() {
    let huge = "x = 1\n".repeat(20_000);
    assert_eq!(
        analyze_code("python", "x = 1\n", &huge).observation,
        CodeObservation::ParserUnavailable
    );
    assert_eq!(
        observe_code("python", &huge).observation,
        CodeObservation::ParserUnavailable
    );
    assert_eq!(
        analyze_code("python", "x = 1\n", "x = 2\n").observation,
        CodeObservation::Parsed
    );
}

/// The digest moves with every text and depends on its order and kind, so two
/// sessions that sent the same prompts agree on it and no others do.
#[test]
fn the_model_input_digest_tells_identical_sends_from_different_ones() {
    let send = |texts: &[(ModelInputKind, &str)]| {
        let mut ledger = EvidenceLedger::default();
        for (kind, text) in texts {
            ledger.record_model_input(*kind, text);
        }
        ledger.metrics.model_input_digest
    };
    let session = [
        (ModelInputKind::Turn, "hello"),
        (ModelInputKind::Watch, "review"),
    ];
    assert_eq!(send(&session), send(&session));
    assert_eq!(send(&session).len(), 64);
    assert_ne!(send(&session), send(&[session[1], session[0]]));
    assert_ne!(
        send(&session),
        send(&[
            (ModelInputKind::Turn, "hello"),
            (ModelInputKind::Interim, "review")
        ])
    );
    assert_ne!(
        send(&session),
        send(&[
            (ModelInputKind::Turn, "hello"),
            (ModelInputKind::Watch, "reviews")
        ])
    );
    assert_eq!(send(&[]), "");
}

/// A run that failed on the same label, with `passed` of three passing.
fn failing_run(passed: u32) -> serde_json::Value {
    serde_json::json!({
        "passed": passed, "total": 3, "language": "python",
        "failures": [{ "label": "dup", "expected": "[0,1]", "got": "[]", "error": null }],
    })
}

/// Every view a prompt can be given, one per line, for checks that hold for
/// all of them.
fn every_view(ledger: &EvidenceLedger) -> String {
    [ViewFor::Watch, ViewFor::Interim, ViewFor::Report]
        .into_iter()
        .map(|purpose| ledger.prompt_view(purpose).join("\n"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Wherever a view states what the browser reported about its runs, it says
/// the browser reported it: the counts, the differences and the diagnostics
/// are claims, and the report's own test summary is what shows the counts
/// there.
#[test]
fn test_claims_keep_browser_provenance_in_every_prompt_view() {
    let mut ledger = EvidenceLedger::default();
    for (index, passed) in [1, 3, 0].into_iter().enumerate() {
        ledger.record_test(
            None,
            index as u64 + 1,
            &serde_json::json!({
                "passed": passed, "total": 3, "language": "python",
                "diagnostic": { "category": "syntax" }
            }),
        );
        for purpose in [ViewFor::Watch, ViewFor::Interim, ViewFor::Report] {
            let view = ledger.prompt_view(purpose).join("\n");
            let tests = view
                .lines()
                .find(|line| line.starts_with("tests:"))
                .unwrap_or_else(|| panic!("{purpose:?} has no tests line:\n{view}"));
            assert!(
                tests.starts_with("tests: browser-reported claims (unverified): "),
                "{view}"
            );
            assert_eq!(
                tests.contains(&format!("{passed} of 3 passing")),
                purpose != ViewFor::Report,
                "{purpose:?}: {view}"
            );
            assert_eq!(tests.contains("since the run before"), index > 0, "{view}");
            assert!(
                view.contains("diagnostics: browser-reported claims (unverified): syntax"),
                "{view}"
            );
        }
    }
}

/// With nothing recorded, the watch and interim views say only that no test
/// has run, and the report, whose test summary says so already, says nothing.
#[test]
fn an_empty_ledger_is_one_line_or_none() {
    let ledger = EvidenceLedger::default();
    assert_eq!(ledger.prompt_view(ViewFor::Watch), ["tests: not run"]);
    assert_eq!(ledger.prompt_view(ViewFor::Interim), ["tests: not run"]);
    assert!(ledger.prompt_view(ViewFor::Report).is_empty());
}

/// Each prompt gets the lines the rest of it does not state, and all of them
/// whole: a watch prompt carries the code and goes to a session holding the
/// conversation, the interim review carries the code and a transcript window,
/// and the report carries the code, the transcript, the last run and the hints.
#[test]
fn each_prompt_gets_only_the_lines_it_does_not_already_state() {
    let mut ledger = EvidenceLedger::default();
    ledger.set_uncovered_coverage(["repeat", "example"]);
    ledger.record_coverage(500, "repeat");
    let mut previous = String::new();
    for (receipt, code) in [(1_000, "x = 1\n"), (2_000, "x = 2\n"), (3_000, "x = 1\n")] {
        let analysis = analyze_code("python", &previous, code);
        ledger.record_code_with_analysis(None, receipt, "python", code, true, analysis);
        previous = code.to_string();
    }
    ledger.record_test(Some(4), 4_000, &failing_run(1));
    ledger.record_test(Some(5), 5_000, &failing_run(1));
    ledger.record_test(Some(6), 6_000, &setup_failure("syntax"));
    ledger.record_hint(7_000, true, true);
    ledger.record_hint(7_100, false, true);
    ledger.record_hint(7_200, true, false);
    ledger.record_conversation_turn(8_000, "candidate");
    ledger.record_lifecycle(9_000, LifecycleTransition::Paused);

    let code = "code: python, 3 candidate edits, 3 changed the program, parses, last edit code, returned to an earlier buffer 1 times";
    let history = "the same failure 2 runs in a row, 1 runs failed to start, 1 edit-and-run cycles";
    let tests = format!("tests: browser-reported claims (unverified): 1 of 3 passing, {history}");
    let diagnostics = "diagnostics: browser-reported claims (unverified): syntax 1";
    let hints = "hints: 2 requested, 1 volunteered, 1 withheld, ladder at rung 1";
    let session = "session: paused";
    assert_eq!(
        ledger.prompt_view(ViewFor::Watch),
        [
            tests.as_str(),
            diagnostics,
            hints,
            "phases covered: repeat; not yet: example",
            session,
        ]
    );
    assert_eq!(
        ledger.prompt_view(ViewFor::Interim),
        [
            code,
            tests.as_str(),
            diagnostics,
            "last program change: 6 s before the latest event",
            hints,
            session,
        ]
    );
    assert_eq!(
        ledger.prompt_view(ViewFor::Report),
        [
            code,
            &format!("tests: browser-reported claims (unverified): {history}"),
            diagnostics,
            session,
        ]
    );
}

/// The count differences are signed and taken against a run that happened: the
/// first run has none, a repeat has none, and either count moving alone is one.
#[test]
fn the_test_line_states_differences_only_against_an_earlier_run() {
    let tests_line = |ledger: &EvidenceLedger| ledger.prompt_view(ViewFor::Watch)[0].clone();
    let mut ledger = EvidenceLedger::default();
    ledger.record_test(Some(1), 1_000, &failing_run(1));
    assert_eq!(
        tests_line(&ledger),
        "tests: browser-reported claims (unverified): 1 of 3 passing, 0 edit-and-run cycles"
    );

    ledger.record_test(Some(2), 2_000, &failing_run(1));
    let repeated = tests_line(&ledger);
    assert!(!repeated.contains("since the run before"), "{repeated}");
    assert!(
        repeated.contains("the same failure 2 runs in a row"),
        "{repeated}"
    );

    ledger.record_test(Some(3), 3_000, &failing_run(2));
    let better = tests_line(&ledger);
    assert!(
        better.contains("+1 passing and -1 failing since the run before"),
        "{better}"
    );

    let mut grew = EvidenceLedger::default();
    grew.record_test(Some(1), 1_000, &failing_run(1));
    grew.record_test(
        Some(2),
        2_000,
        &serde_json::json!({
            "passed": 2, "total": 4, "language": "python",
            "failures": [
                { "label": "dup", "expected": "[0,1]", "got": "[]", "error": null },
                { "label": "neg", "expected": "[]", "got": "[0]", "error": null },
            ],
        }),
    );
    let grown = tests_line(&grew);
    assert!(
        grown.contains("+1 passing and +0 failing since the run before"),
        "{grown}"
    );

    // A regression, and a run where every case passed.
    let mut ledger = EvidenceLedger::default();
    ledger.record_test(
        Some(1),
        1_000,
        &serde_json::json!({"passed": 3, "total": 3}),
    );
    ledger.record_test(Some(2), 2_000, &failing_run(1));
    let line = tests_line(&ledger);
    assert!(line.contains("a regression"), "{line}");
    assert!(line.contains("all passed at least once"), "{line}");
}

/// A hint line for any hint, whichever counter it moved.
#[test]
fn a_hint_line_appears_for_any_hint() {
    for (requested, delivered, line) in [
        (
            false,
            true,
            "hints: 0 requested, 1 volunteered, 0 withheld, ladder at rung 0",
        ),
        (
            true,
            false,
            "hints: 1 requested, 0 volunteered, 1 withheld, ladder at rung 0",
        ),
    ] {
        let mut hinted = EvidenceLedger::default();
        hinted.record_hint(100, requested, delivered);
        assert!(
            hinted
                .prompt_view(ViewFor::Watch)
                .iter()
                .any(|shown| shown == line)
        );
    }
}

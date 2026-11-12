use super::*;
use crate::agent::RuntimeState;

fn spoke(state: &mut RuntimeState, speech: &str) {
    state.transcript.push(format!("Candidate: {speech}"));
    state
        .evidence_ledger
        .record_conversation_turn(1_000, "candidate");
}

/// A turn the recognizer returned in another script is hidden from the report
/// model, so it is no communication evidence. Counting it would require a
/// numeric communication score the prompt tells the model not to invent.
#[test]
fn communication_is_assessed_only_from_a_turn_the_report_can_read() {
    let mut state = RuntimeState::default();
    assert!(!ReportRounds::of(&state).communication_assessed());
    spoke(
        &mut state,
        "αυτό δεν αναγνωρίστηκε από τον αναγνωριστή ομιλίας καθόλου",
    );
    assert_eq!(state.evidence_ledger.conversation.candidate_turns, 1);
    assert!(
        !ReportRounds::of(&state).communication_assessed(),
        "the only turn is one the report hides"
    );
    spoke(
        &mut state,
        "I would use a hash map to track what I have seen.",
    );
    assert!(ReportRounds::of(&state).communication_assessed());
}

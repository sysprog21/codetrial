use super::*;
use crate::agent::{CommunicationEvidence, RuntimeState};

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
    assert_eq!(
        ReportRounds::of(&state).communication_evidence,
        CommunicationEvidence::NoSpeech
    );
    spoke(
        &mut state,
        "αυτό δεν αναγνωρίστηκε από τον αναγνωριστή ομιλίας καθόλου",
    );
    assert_eq!(state.evidence_ledger.conversation.candidate_turns, 1);
    assert!(
        ReportRounds::of(&state).communication_evidence == CommunicationEvidence::Insufficient,
        "the only turn is one the report hides"
    );
    spoke(
        &mut state,
        "I would store the values I have already seen in a hash map so I can look up each complement in constant time.",
    );
    assert!(ReportRounds::of(&state).communication_assessed());
}

#[test]
fn cancelled_interviewer_turn_expires_after_two_delivered_turns() {
    let question = "Explain why your solution keeps the original indices and how it finds a matching complement without scanning the entire array again.";
    let mut state = RuntimeState {
        transcript: vec![
            format!("Interviewer: {question}"),
            "Interviewer: What constraints should we consider?".into(),
            "Interviewer: How would you test duplicate values?".into(),
        ],
        ..RuntimeState::default()
    };
    state.interviewer_delivery.uncertain.insert(0);
    spoke(&mut state, question);
    assert_eq!(
        ReportRounds::of(&state).communication_evidence,
        CommunicationEvidence::Sufficient
    );
}

#[test]
fn recent_echo_of_cancelled_output_does_not_qualify() {
    let question = "Explain why your solution keeps the original indices and how it finds a matching complement without scanning the entire array again.";
    let mut state = RuntimeState::default();
    state.transcript.push(format!("Interviewer: {question}"));
    state.interviewer_delivery.uncertain.insert(0);
    spoke(&mut state, question);
    assert_eq!(
        ReportRounds::of(&state).communication_evidence,
        CommunicationEvidence::Insufficient
    );
}

#[test]
fn echo_of_cancelled_output_after_behavioral_marker_does_not_qualify() {
    let mut state = RuntimeState {
        behavioral_round_started: true,
        behavioral_round_transcript_start: 1,
        transcript: vec![
            "Interviewer: Describe your approach.".into(),
            "Interviewer: Can you explain how your solution handles duplicate values?".into(),
            "Interviewer: What other constraints matter?".into(),
        ],
        ..RuntimeState::default()
    };
    state.interviewer_delivery.uncertain.insert(1);
    spoke(
        &mut state,
        "I store each value in a hash table and check it before inserting the next one",
    );
    spoke(&mut state, "how your solution handles duplicate");
    assert_eq!(
        ReportRounds::of(&state).communication_evidence,
        CommunicationEvidence::Insufficient
    );
}

#[test]
fn only_sufficient_evidence_with_null_score_logs_unassessed_communication() {
    for (evidence, score, expected) in [
        (
            CommunicationEvidence::Sufficient,
            serde_json::Value::Null,
            true,
        ),
        (
            CommunicationEvidence::Sufficient,
            serde_json::json!(80),
            false,
        ),
        (
            CommunicationEvidence::NoSpeech,
            serde_json::Value::Null,
            false,
        ),
        (
            CommunicationEvidence::Insufficient,
            serde_json::Value::Null,
            false,
        ),
    ] {
        let rounds = ReportRounds {
            communication_evidence: evidence,
            behavioral_opened: false,
            follow_ups_released: false,
        };
        let report = serde_json::json!({ "communicationScore": score });
        assert_eq!(
            communication_left_unassessed(rounds, &report),
            expected,
            "{evidence:?}: {report}"
        );
    }
}

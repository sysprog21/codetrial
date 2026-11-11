use super::*;

#[test]
fn filler_turns_do_not_make_communication_assessable() {
    let mut state = RuntimeState {
        transcript: vec!["Candidate: um".to_owned()],
        ..RuntimeState::default()
    };
    state.evidence_ledger.conversation.candidate_turns = 1;
    assert!(!ReportRounds::of(&state).communication_assessed());
}

fn classify(lines: &[&str]) -> CommunicationEvidence {
    communication_evidence(
        &lines
            .iter()
            .map(|line| (*line).to_owned())
            .collect::<Vec<_>>(),
    )
}

fn turn(count: usize) -> String {
    format!("Candidate: {}", vec!["word"; count].join(" "))
}

#[test]
fn communication_requires_twenty_words_and_one_eight_word_turn() {
    use CommunicationEvidence::{Insufficient, Sufficient};
    for (sizes, expected) in [
        (vec![19], Insufficient),
        (vec![20], Sufficient),
        (vec![7, 7, 6], Insufficient),
        (vec![8, 6, 6], Sufficient),
        (vec![8, 6, 5], Insufficient),
        (vec![21], Sufficient),
    ] {
        let lines: Vec<_> = sizes.into_iter().map(turn).collect();
        assert_eq!(communication_evidence(&lines), expected, "{lines:?}");
    }
}

#[test]
fn noise_fillers_and_acknowledgements_do_not_accumulate_evidence() {
    for noise in [
        "uh",
        "UM",
        "hmm",
        "mhm",
        "yes",
        "no",
        "okay",
        "right",
        "...",
        "[cough]",
        "(NOISE)",
        "[background noise]",
        "[ background noise ]",
    ] {
        let lines = vec![
            format!("{} {noise}", turn(19)),
            format!("Candidate: {}", vec![noise; 20].join(" ")),
        ];
        // An acknowledgement embedded in an explanation is a normal word.
        let expected = if ["yes", "no", "okay", "right"].contains(&noise) {
            CommunicationEvidence::Sufficient
        } else {
            CommunicationEvidence::Insufficient
        };
        assert_eq!(communication_evidence(&lines), expected, "{noise}");
        assert_eq!(
            communication_evidence(&lines[1..]),
            CommunicationEvidence::Insufficient
        );
    }
    for acknowledgement in [
        "OK",
        "Yeah.",
        "Yep.",
        "Sure.",
        "Mm-hmm.",
        "Uh-huh.",
        "Hm.",
        "uhm",
        "Er,",
        "erm",
        "ah",
        "umm",
        "uhh",
        "hmmm",
        "mmm",
        "Got it.",
        "I see.",
        "Thank you.",
        "Makes sense.",
        "Sounds good.",
        "All right.",
        "Alright.",
        "Thanks.",
        "Yup.",
        "Nope.",
    ] {
        let mut lines = vec![turn(8)];
        lines.extend(vec![format!("Candidate: {acknowledgement}"); 12]);
        assert_eq!(
            communication_evidence(&lines),
            CommunicationEvidence::Insufficient,
            "{acknowledgement}"
        );
    }
    assert_eq!(
        classify(&[&turn(17), "Candidate: Got it working."]),
        CommunicationEvidence::Sufficient
    );
    for annotation in [
        "(coughs)",
        "[laughs]",
        "(laughing)",
        "[clears throat]",
        "[music]",
        "[breathing]",
        "[coughs, laughs]",
        "(coughing, laughter)",
        "[clears (throat)]",
        "[background  noise]",
        "(background-noise)",
        "[clears\nthroat]",
    ] {
        let lines = [format!("{} {annotation}", turn(19))];
        assert_eq!(
            communication_evidence(&lines),
            CommunicationEvidence::Insufficient,
            "{annotation}"
        );
    }
    assert_eq!(
        classify(&[&turn(19), "Candidate: music"]),
        CommunicationEvidence::Sufficient
    );
    for mixed in ["cough okay", "sure, noise", "background noise, got it"] {
        assert_eq!(
            classify(&[&turn(19), &format!("Candidate: {mixed}")]),
            CommunicationEvidence::Insufficient,
            "{mixed}"
        );
    }
    assert_eq!(
        classify(&[&turn(13), "Candidate: No, the index is out of bounds."]),
        CommunicationEvidence::Sufficient
    );
    assert_eq!(
        classify(&[
            "Candidate: I can keep each original index in the table for later lookup (so every match points to the original pair)."
        ]),
        CommunicationEvidence::Sufficient
    );
    let bare_noise = format!("Candidate: {}", "cough background noise ".repeat(8));
    assert_eq!(
        classify(&[&bare_noise]),
        CommunicationEvidence::Insufficient
    );
    // Either word alone is insufficient to identify the bare noise marker.
    for phrase in ["background music", "filter noise"] {
        let speech = format!("Candidate: {}", format!("{phrase} ").repeat(10));
        assert_eq!(
            classify(&[&speech]),
            CommunicationEvidence::Sufficient,
            "{phrase}"
        );
    }
    assert_eq!(classify(&[]), CommunicationEvidence::NoSpeech);
    assert_eq!(
        classify(&["Candidate:   ", "Interviewer: explain this"]),
        CommunicationEvidence::NoSpeech
    );
    assert_eq!(
        classify(&[&format!("{} noise", turn(19))]),
        CommunicationEvidence::Sufficient
    );
}

#[test]
fn words_are_case_insensitive_unicode_alphanumeric_runs() {
    // The dotted capital I expands on lowercase but remains within one word.
    for (count, expected) in [
        (18, CommunicationEvidence::Insufficient),
        (19, CommunicationEvidence::Sufficient),
    ] {
        assert_eq!(classify(&[&format!("{} MİX", turn(count))]), expected);
    }
    let words = "one,two;three:four/five_six-seven.eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen";
    assert_eq!(
        classify(&[&format!("Candidate: {words}")]),
        CommunicationEvidence::Insufficient
    );
    assert_eq!(
        classify(&[&format!("Candidate: {words} CAFÉ")]),
        CommunicationEvidence::Sufficient
    );
    assert_eq!(
        classify(&[&format!("{} 123", turn(19))]),
        CommunicationEvidence::Sufficient
    );
}

#[test]
fn echo_uses_five_words_and_only_two_preceding_interviewer_turns() {
    use CommunicationEvidence::{Insufficient, Sufficient};
    let old =
        "Interviewer: First, explain how this approach handles duplicate input values please.";
    let echo = "Candidate: HOW this approach handles duplicate";
    let fifteen = turn(15);
    for (lines, expected) in [
        (vec![old, &fifteen, echo], Insufficient),
        (
            vec![old, "Interviewer: ...", "Interviewer: um", &fifteen, echo],
            Insufficient,
        ),
        (
            vec![
                old,
                &fifteen,
                "Candidate: um HOW this approach handles duplicate",
            ],
            Insufficient,
        ),
        (
            vec![
                "Interviewer: First, explain how this um approach handles duplicate input.",
                &fifteen,
                echo,
            ],
            Insufficient,
        ),
        (
            vec![old, "Interviewer: Next question", &fifteen, echo],
            Insufficient,
        ),
        (
            vec![
                old,
                "Interviewer: Next question",
                "Interviewer: Another question",
                &fifteen,
                echo,
            ],
            Sufficient,
        ),
        (vec![&fifteen, echo, old], Sufficient),
        (
            vec![
                old,
                &fifteen,
                "Candidate: how this different approach handles duplicate",
            ],
            Sufficient,
        ),
    ] {
        assert_eq!(classify(&lines), expected, "{lines:?}");
    }
    assert_eq!(
        classify(&[old, &turn(16), "Candidate: how this approach handles"]),
        Sufficient
    );
    // Genuine repetition is deliberately indistinguishable from this echo.
    let lines = vec![old.to_owned(), fifteen, echo.to_owned()];
    let before = transcript_for_report(&lines);
    assert_eq!(communication_evidence(&lines), Insufficient);
    assert_eq!(transcript_for_report(&lines), before);
}

#[test]
fn masked_and_dropped_words_cannot_qualify_a_report() {
    use CommunicationEvidence::Insufficient;
    // Greek letters exercise the existing non-Latin recognition policy.
    let masked = format!("Candidate: {}", "αλφα ".repeat(20));
    assert_eq!(classify(&[&masked]), Insufficient);
    assert_eq!(classify(&[&turn(19), &masked]), Insufficient);
    let mut lines = vec![turn(20), "Interviewer: x".to_owned()];
    let padding = MAX_TRANSCRIPT_BYTES - lines.iter().map(|line| line.len() + 1).sum::<usize>();
    lines[1].push_str(&"x".repeat(padding));
    assert_eq!(
        communication_evidence(&lines),
        CommunicationEvidence::Sufficient
    );
    lines[1].push('x');
    assert_eq!(communication_evidence(&lines), Insufficient);
    assert!(!transcript_for_report(&lines).contains("Candidate:"));
    // A clipped suffix may spell a speaker prefix without owning that role.
    let clipped = format!(
        "Candidate: {}{}",
        "x".repeat(MAX_TRANSCRIPT_BYTES),
        turn(20)
    );
    assert_eq!(classify(&[&clipped]), Insufficient);
}

#[test]
fn assembled_fragments_count_once_without_changing_live_transcription() {
    let mut state = RuntimeState::default();
    let mut speaker = SpeakerTurn::default();
    speaker.record(&mut state.transcript, "Candidate", &"word ".repeat(7));
    speaker.record(&mut state.transcript, "Candidate", &"word ".repeat(13));
    assert_eq!(state.transcript.len(), 1);
    // Eligibility follows retained speech, not this independent ledger count.
    assert_eq!(state.evidence_ledger.conversation.candidate_turns, 0);
    assert!(ReportRounds::of(&state).communication_assessed());
}

#[test]
fn insufficient_speech_uses_the_null_schema_and_rejects_invented_feedback() {
    let state = RuntimeState {
        transcript: vec!["Candidate: yes".to_owned()],
        ..RuntimeState::default()
    };
    let rounds = ReportRounds::of(&state);
    let schema = report_response_schema(rounds.communication_assessed());
    assert_eq!(
        schema["properties"]["communicationFeedback"]["properties"]["strengths"]["maxItems"],
        0
    );
    let mut report = valid_strict_report();
    let problem = get_problem(Some("two-sum"));
    let validate = |report: &Value| {
        validate_report_for_round(
            report,
            problem,
            ReportRounds {
                behavioral_opened: true,
                ..rounds
            },
            InterviewMode::Coding,
            false,
        )
    };
    assert!(validate(&report).is_err());
    report["communicationScore"] = Value::Null;
    report["communicationFeedback"]["strengths"] = json!([]);
    report["communicationFeedback"]["improvements"] = json!([]);
    report["improvementPlan"]
        .as_array_mut()
        .unwrap()
        .truncate(2);
    assert!(validate(&report).is_ok());
    report["communicationFeedback"]["strengths"] = json!(["Answered clearly"]);
    assert!(
        validate(&report)
            .unwrap_err()
            .iter()
            .any(|error| error.contains("communicationFeedback.strengths"))
    );
}

#[test]
fn brief_distinguishes_missing_and_insufficient_without_persisting_either() {
    for (lines, status) in [
        (vec![], "no_speech"),
        (vec!["Candidate: yes".to_owned()], "insufficient"),
        (vec![turn(20)], "sufficient"),
    ] {
        let state = RuntimeState {
            transcript: lines,
            ..RuntimeState::default()
        };
        let rounds = ReportRounds::of(&state);
        let transcript = transcript_for_report(&state.transcript);
        let problem = get_problem(Some("two-sum"));
        let brief = report_prompt(ReportPromptInput {
            problem,
            interview_mode: InterviewMode::Coding,
            board_attached: false,
            transcript: &transcript,
            rolling_assessment: "",
            final_code: "",
            language: "python",
            hints_used: 0,
            hint_rung: 0,
            volunteered_hints: 0,
            duration_min: 45,
            elapsed_min: 10.0,
            test_summary: "",
            practice_level: None,
            evidence: "",
            behavioral_round: BehavioralRound::Opened,
            follow_ups_released: false,
            communication_evidence: rounds.communication_evidence,
        });
        assert!(brief.contains(&format!("COMMUNICATION EVIDENCE: {status}.")));
        assert_eq!(
            brief.contains("Return `communicationScore` as `null`"),
            status != "sufficient"
        );
        let mut report = valid_strict_report();
        if !rounds.communication_assessed() {
            report["communicationScore"] = Value::Null;
            report["communicationFeedback"]["strengths"] = json!([]);
            report["communicationFeedback"]["improvements"] = json!([]);
            report["improvementPlan"]
                .as_array_mut()
                .unwrap()
                .truncate(2);
        }
        let published = final_report(
            Some(&report),
            0,
            None,
            problem,
            ReportRounds {
                behavioral_opened: true,
                ..rounds
            },
            InterviewMode::Coding,
            false,
        );
        assert!(!published["incomplete"].as_bool().unwrap_or(false));
        assert_eq!(
            published["communicationScore"].is_null(),
            status != "sufficient"
        );
        assert!(published.get("communicationEvidence").is_none());
        assert!(published.get("communication_evidence").is_none());
    }
}

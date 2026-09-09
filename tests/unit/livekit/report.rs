//! The `tests` module of `src/livekit/report.rs`, which declares this file by
//! path.
//! Everything here reaches into `src/livekit/report.rs` through `super`, so it
//! is a unit
//! test and not an integration test: private items are in scope.

use super::*;

use crate::agent::record_framework_evidence;
use crate::config::load_from_pairs;
use crate::runtime::bootstrap;

/// The report says which rounds actually completed, from banked evidence.
///
/// Two gates, and both are the same shape: every phase of the round needs
/// evidence that is not a skip. Loosened to "any phase" or to "including
/// skips", a candidate who ran out of time reads as one who finished.
#[test]
fn the_rounds_a_report_calls_complete_are_the_ones_with_evidence() {
    let rounds = |state: &RuntimeState| {
        report_with_integrity_events(serde_json::json!({}), state, "time_up")["rounds"].clone()
    };
    let bank = |state: &mut RuntimeState, phase: &str, kind: &str| {
        crate::agent::record_framework_evidence(
            state,
            &serde_json::json!({
                "phase": phase, "source": if kind == "skipped" { "session_timing" }
                    else { "candidate_speech" },
                "kind": kind, "confidence": 90,
                "summary": format!("candidate {kind} {phase}"),
            }),
        )
        .expect("evidence should record");
    };

    // Nothing banked: neither round is claimed.
    let bare = RuntimeState {
        interview_loop: crate::agent::InterviewLoop::CodingBehavioral,
        ..RuntimeState::default()
    };
    assert_eq!(rounds(&bare)[0]["status"], "incomplete");
    assert_eq!(rounds(&bare)[1]["status"], "skipped");

    // Evidence for some other phase is evidence for neither of these. Asking
    // whether any banked phase is not Test answers yes for a candidate who only
    // restated the problem, and calls the round done.
    let mut elsewhere = bare.clone();
    bank(&mut elsewhere, "repeat", "observed");
    assert_eq!(
        rounds(&elsewhere)[0]["status"],
        "incomplete",
        "restating the problem is not having tested or optimized it"
    );

    // One of the two coding phases is not both of them.
    let mut half = bare.clone();
    bank(&mut half, "test", "observed");
    assert_eq!(
        rounds(&half)[0]["status"],
        "incomplete",
        "one phase is not the gate"
    );

    // A skip is the record of not reaching it, so it cannot complete it.
    let mut skipped = half.clone();
    bank(&mut skipped, "optimizations", "skipped");
    assert_eq!(
        rounds(&skipped)[0]["status"],
        "incomplete",
        "running out of time is not finishing"
    );

    let mut coding_done = half.clone();
    bank(&mut coding_done, "optimizations", "observed");
    assert_eq!(rounds(&coding_done)[0]["status"], "complete");

    // STAR needs the round to have started and all four phases banked.
    let mut star = coding_done.clone();
    for phase in ["situation", "task", "action", "result"] {
        bank(&mut star, phase, "observed");
    }
    assert_eq!(
        rounds(&star)[1]["status"],
        "skipped",
        "four phases without the round beginning is not a behavioral round"
    );
    star.behavioral_round_started = true;
    assert_eq!(rounds(&star)[1]["status"], "complete");

    // Begun but not finished. Evidence for one STAR phase is not evidence for
    // the other three, and asking whether any banked phase is not Task answers
    // yes as soon as anything else was said, which reports a behavioral round
    // the candidate barely entered as one they completed.
    let mut begun = coding_done.clone();
    begun.behavioral_round_started = true;
    bank(&mut begun, "situation", "observed");
    assert_eq!(
        rounds(&begun)[1]["status"],
        "started",
        "one STAR phase in is started, not complete"
    );

    // A coding-only interview reserves no behavioral round at all.
    let mut coding_only = coding_done.clone();
    coding_only.interview_loop = crate::agent::InterviewLoop::CodingOnly;
    assert_eq!(rounds(&coding_only)[1]["status"], "not_configured");
}

/// The note goes over the data channel into the candidate's report card. A
/// `reqwest` error Displays the URL it was built from, so a credentialed
/// URL
/// anywhere in that chain hands the server's Google key to the candidate.
#[test]
fn report_error_note_never_carries_the_google_api_key() {
    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "AQ.Ab8RN6secret"),
    ])
    .unwrap();
    let boot = bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let leaky = std::io::Error::other(
        "HTTP status server error (503 Service Unavailable) for url \
         (https://generativelanguage.googleapis.com/v1beta/models/x:generateContent?key=AQ.Ab8RN6secret)",
    );

    let note = report_error_note(
        &boot,
        &RuntimeState::default(),
        "candidate_ended",
        &leaky,
        "AQ.Ab8RN6secret",
    );

    assert!(!note.contains("AQ.Ab8RN6secret"), "{note}");
    assert!(note.contains("[REDACTED]"), "{note}");
    assert!(
        note.contains("503 Service Unavailable"),
        "the reason still has to be readable: {note}"
    );
}

#[test]
fn report_helpers_use_report_topic_prompt_state_and_error_note() {
    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "google"),
    ])
    .unwrap();
    let boot = bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let state = RuntimeState {
        code: "return [0, 1]".to_string(),
        language: "python".to_string(),
        transcript: vec![
            "Interviewer: Welcome.".to_string(),
            "Candidate: I will use a hash map.".to_string(),
        ],
        last_test_run: Some(serde_json::json!({
            "language": "python",
            "passed": 1,
            "total": 2,
            "failures": [],
        })),
        test_runs: 1,
        hints_used: 2,
        ..RuntimeState::default()
    };
    let error = std::io::Error::other("model unavailable");
    let report = final_report(
        None,
        state.hints_used,
        Some(&report_error_note(
            &boot,
            &state,
            "candidate_ended",
            &error,
            "google",
        )),
    );
    let packet = report_data_packet(report).unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&packet.payload).unwrap();
    let report = report_with_integrity_events(
        payload.clone(),
        &RuntimeState {
            integrity_events: vec![serde_json::json!({
                "type": "SESSION_START",
                "at": "now",
                "severity": "info",
                "source": "media",
                "durationMs": 0,
                "seq": 1,
                "prevHash": "",
                "hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "detail": null,
            })],
            ..RuntimeState::default()
        },
        "time_up",
    );
    let prompt = report_prompt_text(&boot, &state, 12.4);

    assert!(prompt.contains("Candidate: I will use a hash map."));
    assert!(prompt.contains("Latest test run (run #1, python): 1/2 cases passed."));
    assert!(packet.reliable);
    assert_eq!(packet.topic.as_deref(), Some(TOPIC_REPORT));
    assert!(
        payload["summary"]
            .as_str()
            .unwrap()
            .contains("Final editor state: 13 bytes of python")
    );
    assert_eq!(payload["hintsUsed"], 2);
    assert_eq!(report["integrityEvents"][0]["type"], "SESSION_START");

    // The page can see that a report arrived unasked but not which clock
    // produced it, and was reading its own countdown to guess. This is the
    // answer from the side that made the decision.
    assert_eq!(report["endReason"], "time_up");
}

/// What the interview recorded about itself while it was running reaches the
/// final reviewer, and the transcript still reaches it whole.
///
/// The second half is the part worth pinning. An earlier attempt at issue 31
/// treated the rolling assessment as a replacement and cut the transcript down
/// to a closing window, which traded away the spoken evidence the
/// communication score is almost entirely read from, to save prefill on a call
/// that measures six seconds. The assessment is additional evidence. It is
/// never the record.
#[test]
fn the_report_prompt_carries_both_the_rolling_assessment_and_the_whole_transcript() {
    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "google"),
    ])
    .unwrap();
    let boot = bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let mut state = RuntimeState {
        transcript: vec![
            "Candidate: early reasoning about the hash map".to_string(),
            "Candidate: closing reasoning about the complexity".to_string(),
        ],
        ..RuntimeState::default()
    };
    record_framework_evidence(
        &mut state,
        &serde_json::json!({
            "phase": "algorithm", "source": "candidate_speech", "kind": "observed",
            "confidence": 90, "summary": "Candidate chose a hash map and said why."
        }),
    )
    .unwrap();
    crate::agent::record_interim_notes(
        &mut state,
        "- Candidate enumerated the empty-input case before writing any code.",
    );

    let prompt = report_prompt_text(&boot, &state, 45.0);

    // Delimited, like the transcript and the editor are wherever candidate
    // material reaches a model: the notes are a reading of that material, so an
    // instruction inside one arrived from the candidate by way of a note-taker.
    assert!(prompt.contains("BEGIN UNTRUSTED ROLLING ASSESSMENT"));
    assert!(prompt.contains("END UNTRUSTED ROLLING ASSESSMENT"));
    assert!(prompt.contains("Phase evidence the interviewer recorded"));
    assert!(prompt.contains("Candidate chose a hash map and said why."));
    assert!(prompt.contains("Observations recorded during pauses"));
    assert!(prompt.contains("Candidate enumerated the empty-input case"));
    assert!(prompt.contains("FULL SPOKEN TRANSCRIPT"));
    assert!(
        prompt.contains("early reasoning about the hash map"),
        "the assessment is evidence beside the transcript, never a replacement for it"
    );
    assert!(prompt.contains("closing reasoning about the complexity"));
}

/// A session that recorded nothing about itself is still reportable, and says
/// nothing about a rolling assessment it does not have.
#[test]
fn a_session_with_no_recorded_assessment_keeps_the_plain_report_prompt() {
    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "google"),
    ])
    .unwrap();
    let boot = bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let prompt = report_prompt_text(
        &boot,
        &RuntimeState {
            transcript: vec!["Candidate: only evidence".to_string()],
            ..RuntimeState::default()
        },
        1.0,
    );
    assert!(prompt.contains("FULL SPOKEN TRANSCRIPT"));
    assert!(prompt.contains("Candidate: only evidence"));
    assert!(!prompt.contains("ROLLING ASSESSMENT"));
}

/// The long interview -- the one issue 31 was reported against -- is exactly
/// the one that overruns the evidence cap, and it must still arrive with every
/// phase it reached.
#[test]
fn an_interview_past_the_evidence_cap_still_reports_every_phase_it_reached() {
    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "google"),
    ])
    .unwrap();
    let boot = bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let mut state = RuntimeState::default();
    for phase in [
        "repeat",
        "example",
        "algorithm",
        "coding",
        "test",
        "optimizations",
    ] {
        record_framework_evidence(
            &mut state,
            &serde_json::json!({
                "phase": phase, "source": "candidate_speech", "kind": "observed",
                "confidence": 90, "summary": format!("Candidate completed {phase}.")
            }),
        )
        .unwrap();
    }

    // Well past the cap, not one short of it. A chatty interviewer records this
    // many observations about the phase the candidate spends the most time in,
    // and the old eviction rule answered that by dropping `repeat` first.
    for index in 0..(crate::agent::MAX_FRAMEWORK_EVIDENCE * 2) {
        record_framework_evidence(
            &mut state,
            &serde_json::json!({
                "phase": "coding", "source": "editor_snapshot", "kind": "observed",
                "confidence": 90, "summary": format!("Later coding observation {index}.")
            }),
        )
        .unwrap();
    }

    let prompt = report_prompt_text(&boot, &state, 45.0);
    for phase in ["repeat", "example", "algorithm", "test", "optimizations"] {
        assert!(
            prompt.contains(&format!("Candidate completed {phase}.")),
            "the opening phases must survive an interview that overran the cap"
        );
    }
    assert!(prompt.contains(&format!(
        "Later coding observation {}.",
        crate::agent::MAX_FRAMEWORK_EVIDENCE * 2 - 1
    )));
}

#[test]
fn the_server_overwrites_model_selected_contract_provenance() {
    let mut report = serde_json::json!({
        "decision": "HIRE",
        "interviewContract": {"bundleVersion": 999}
    });
    stamp_report_contract(&mut report);
    assert_eq!(report["interviewContract"], interview_contract_json());

    let mut incomplete = serde_json::json!({"incomplete": true});
    stamp_report_contract(&mut incomplete);
    assert_eq!(incomplete["interviewContract"], interview_contract_json());
}

/// The liveness pair bookends the evidence, and the closing sample is the
/// one that says how the interview ended. Nothing covered this merge, so
/// dropping it, duplicating it, or emitting it out of order was invisible.
#[test]
fn the_report_packet_bookends_the_evidence_with_the_liveness_pair() {
    let event = |seq: u64, kind: &str| {
        serde_json::json!({
            "type": kind,
            "at": "now",
            "severity": "info",
            "source": "media",
            "durationMs": 0,
            "seq": seq,
            "prevHash": "",
            "hash": "a".repeat(64),
            "detail": null,
        })
    };
    let packet = |first, last| {
        report_with_integrity_events(
            serde_json::json!({}),
            &RuntimeState {
                integrity_events: vec![event(5, "CAMERA_STOPPED")],
                integrity_first_heartbeat: first,
                integrity_last_heartbeat: last,
                integrity_chain: Some((9, "b".repeat(64))),
                ..RuntimeState::default()
            },
            "time_up",
        )
    };

    let both = packet(
        Some(event(2, "INTEGRITY_HEARTBEAT")),
        Some(event(9, "INTEGRITY_HEARTBEAT")),
    );
    let seqs: Vec<u64> = both["integrityEvents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["seq"].as_u64().unwrap())
        .collect();
    assert_eq!(seqs, vec![2, 5, 9], "merged in the order the interview ran");
    assert_eq!(both["integrityChainSeq"], 9);

    // Derived, not tallied: nine accepted, one kept as evidence and two held as
    // samples, so six went for space.
    assert_eq!(both["integrityDropped"], 6);

    // A run of one has an opening sample and no closing one.
    let single = packet(Some(event(2, "INTEGRITY_HEARTBEAT")), None);
    let seqs: Vec<u64> = single["integrityEvents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["seq"].as_u64().unwrap())
        .collect();
    assert_eq!(seqs, vec![2, 5], "one sample is one row");
    assert_eq!(single["integrityDropped"], 7);

    // And an interview that produced no heartbeat at all carries neither.
    let none = packet(None, None);
    assert_eq!(none["integrityEvents"].as_array().unwrap().len(), 1);
    assert_eq!(none["integrityDropped"], 8);
}

#[test]
fn complete_and_incomplete_reports_carry_agent_owned_framework_evidence() {
    let mut state = RuntimeState::default();
    record_framework_evidence(
        &mut state,
        &serde_json::json!({
            "phase":"result", "source":"session_timing", "kind":"skipped",
            "confidence":100, "summary":"The cutoff prevented STAR assessment."
        }),
    )
    .unwrap();
    for report in [
        serde_json::json!({"decision":"HIRE"}),
        serde_json::json!({"incomplete":true}),
    ] {
        let report = report_with_integrity_events(report, &state, "time_up");
        assert_eq!(report["frameworkEvidence"][0]["phase"], "result");
        assert_eq!(report["frameworkEvidence"][0]["kind"], "skipped");
        assert_eq!(report["interviewLoop"], "coding_behavioral");
        assert_eq!(report["rounds"][0]["budgetMin"], 37);
        assert_eq!(report["rounds"][1]["status"], "skipped");
        assert_eq!(
            report["frameworkEvidence"][0]["frameworkVersion"],
            crate::agent::FRAMEWORK_VERSION
        );
    }
}

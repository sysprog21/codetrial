//! Building the report packet the browser receives when an interview ends.
//!
//! One region because it is one output. `publish_report` is the only thing the
//! interview loop calls; everything below it is how the packet is assembled,
//! and the pieces are separated so that a failure in one of them is a note in
//! the report rather than no report at all.
//!
//! Split out of `livekit.rs` along the line its module doc already drew.

use ::livekit::prelude::{DataPacket, Room};

use crate::agent::{
    ReportPromptInput, RuntimeState, final_report, format_test_run, framework_evidence_json,
    interview_contract_json, report_prompt, transcript_for_report,
};
use crate::gemini::{generate_report, redact_api_key};
use crate::runtime::{RuntimeBootstrap, TOPIC_REPORT};

use super::{REPORT_TIMEOUT, browser_packet};

pub(super) async fn publish_report(
    room: &Room,
    boot: &RuntimeBootstrap<'_>,
    state: &RuntimeState,
    reason: &str,
    elapsed_min: f64,
    api_key: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    room.local_participant()
        .publish_data(report_packet(boot, state, reason, elapsed_min, api_key).await?)
        .await?;
    Ok(())
}

async fn report_packet(
    boot: &RuntimeBootstrap<'_>,
    state: &RuntimeState,
    reason: &str,
    elapsed_min: f64,
    api_key: &str,
) -> Result<DataPacket, Box<dyn std::error::Error + Send + Sync>> {
    let mut report = match tokio::time::timeout(
        REPORT_TIMEOUT,
        generate_report(
            api_key,
            boot.report_model,
            &report_prompt_text(boot, state, elapsed_min),
        ),
    )
    .await
    {
        Ok(Ok(raw)) => final_report(Some(&raw), state.hints_used, None),
        Ok(Err(error)) => final_report(
            None,
            state.hints_used,
            Some(&report_error_note(
                boot,
                state,
                reason,
                error.as_ref(),
                api_key,
            )),
        ),
        Err(error) => final_report(
            None,
            state.hints_used,
            Some(&report_error_note(boot, state, reason, &error, api_key)),
        ),
    };
    stamp_report_contract(&mut report);
    Ok(report_data_packet(report_with_integrity_events(
        report, state,
    ))?)
}

fn stamp_report_contract(report: &mut serde_json::Value) {
    if let Some(object) = report.as_object_mut() {
        object.insert("interviewContract".to_string(), interview_contract_json());
    }
}

fn report_with_integrity_events(
    mut report: serde_json::Value,
    state: &RuntimeState,
) -> serde_json::Value {
    if let Some(object) = report.as_object_mut() {
        // The evidence, plus the heartbeats that bookend it, merged by sequence
        // rather than appended: a reader goes down this list in the order the
        // interview happened, and a device-state sample out of place reads as a
        // fault rather than as a bookend.
        let kept = state.integrity_events.len() as u64;
        let liveness = state.integrity_first_heartbeat.iter();
        let liveness = liveness.chain(state.integrity_last_heartbeat.iter());
        let samples = liveness.clone().count() as u64;
        let mut events = state.integrity_events.clone();
        events.extend(liveness.cloned());
        events.sort_by_key(|event| event["seq"].as_u64().unwrap_or(0));
        object.insert(
            "integrityEvents".to_string(),
            serde_json::Value::Array(events),
        );

        // How far verification got, and how much of it this report is not
        // showing. The array above is a subsequence, so its links cannot be
        // recomputed by whoever holds the report; without these a reader cannot
        // tell a retention gap from a deleted row, which is the distinction the
        // chain exists to make visible.
        //
        // The dropped count is derived rather than tallied. Every accepted
        // event is in the evidence, held as a sample, or gone, and the cursor
        // counts acceptances, so a counter would have been a fourth place for
        // the same fact to be wrong.
        let verified = state.integrity_chain.as_ref().map(|(seq, _)| *seq);
        object.insert("integrityChainSeq".to_string(), serde_json::json!(verified));
        object.insert(
            "integrityDropped".to_string(),
            serde_json::json!(verified.map(|seq| seq.saturating_sub(kept + samples))),
        );
        object.insert(
            "frameworkEvidence".to_string(),
            serde_json::Value::Array(
                state
                    .framework_evidence
                    .iter()
                    .map(framework_evidence_json)
                    .collect(),
            ),
        );
        let coding_gate = [
            crate::agent::FrameworkPhase::Test,
            crate::agent::FrameworkPhase::Optimizations,
        ]
        .iter()
        .all(|phase| {
            state.framework_evidence.iter().any(|item| {
                item.phase == *phase && item.kind != crate::agent::EvidenceKind::Skipped
            })
        });
        object.insert(
            "interviewLoop".to_string(),
            serde_json::json!(state.interview_loop.as_str()),
        );
        let star_complete = state.behavioral_round_started
            && [
                crate::agent::FrameworkPhase::Situation,
                crate::agent::FrameworkPhase::Task,
                crate::agent::FrameworkPhase::Action,
                crate::agent::FrameworkPhase::Result,
            ]
            .iter()
            .all(|phase| {
                state.framework_evidence.iter().any(|item| {
                    item.phase == *phase && item.kind != crate::agent::EvidenceKind::Skipped
                })
            });
        object.insert("rounds".to_string(), serde_json::json!([
            {"kind":"coding","budgetMin": state.coding_minutes, "status": if coding_gate { "complete" } else { "incomplete" }},
            {"kind":"behavioral","budgetMin": state.behavioral_minutes, "status": if state.interview_loop == crate::agent::InterviewLoop::CodingOnly { "not_configured" } else if star_complete { "complete" } else if state.behavioral_round_started { "started" } else { "skipped" }}
        ]));
    }
    report
}

fn report_prompt_text(
    boot: &RuntimeBootstrap<'_>,
    state: &RuntimeState,
    elapsed_min: f64,
) -> String {
    let transcript = transcript_for_report(&state.transcript);
    let test_summary = format_test_run(state.last_test_run.as_ref(), state.test_runs);
    report_prompt(ReportPromptInput {
        problem: boot.problem,
        transcript: &transcript,
        final_code: &state.code,
        language: &state.language,
        hints_used: state.hints_used,
        duration_min: boot.duration_min,
        elapsed_min,
        test_summary: &test_summary,
    })
}

/// This note is published to the candidate's browser and rendered in the report
/// card, so `api_key` is not decoration: an error carrying a credentialed URL
/// would otherwise hand the server's Google key to whoever is taking the
/// interview.
fn report_error_note(
    boot: &RuntimeBootstrap<'_>,
    state: &RuntimeState,
    reason: &str,
    error: &(dyn std::error::Error + 'static),
    api_key: &str,
) -> String {
    let detail = redact_api_key(&error.to_string(), api_key);
    format!(
        "Rust LiveKit runner ended ({reason}) but Gemini report generation failed for model {} on {}. Final editor state: {} bytes of {}. Error: {detail}",
        boot.report_model,
        boot.problem.id,
        state.code.len(),
        state.language
    )
}

fn report_data_packet(report: serde_json::Value) -> Result<DataPacket, serde_json::Error> {
    browser_packet(TOPIC_REPORT, &report)
}

#[cfg(test)]
mod tests {
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
            report_with_integrity_events(serde_json::json!({}), state)["rounds"].clone()
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

        // Evidence for some other phase is evidence for neither of these.
        // Asking whether any banked phase is not Test answers yes for a
        // candidate who only restated the problem, and calls the round done.
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

        // Begun but not finished. Evidence for one STAR phase is not evidence
        // for the other three, and asking whether any banked phase is not Task
        // answers yes as soon as anything else was said, which reports a
        // behavioral round the candidate barely entered as one they completed.
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

        // Derived, not tallied: nine accepted, one kept as evidence and two
        // held as samples, so six went for space.
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
            let report = report_with_integrity_events(report, &state);
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
}

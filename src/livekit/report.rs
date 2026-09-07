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
#[path = "../../tests/unit/livekit/report.rs"]
mod tests;

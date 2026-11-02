//! Content-free tool context for the observational replay harness.
use std::time::Instant;

use serde_json::Value;

use crate::agent::RuntimeState;
use crate::gemini::GeminiFunctionCall;
use crate::livekit::turn::RuntimeActivity;
use crate::runtime::{
    TOOL_END_INTERVIEW, TOOL_LOG_HINT, TOOL_READ_BOARD, TOOL_READ_CONVERSATION, TOOL_READ_EDITOR,
    TOOL_RECORD_FRAMEWORK_EVIDENCE,
};

/// A declared tool's name, never the model's text for an undeclared one.
pub(super) fn known_tool_name(name: &str) -> &'static str {
    [
        TOOL_READ_EDITOR,
        TOOL_READ_BOARD,
        TOOL_READ_CONVERSATION,
        TOOL_LOG_HINT,
        TOOL_RECORD_FRAMEWORK_EVIDENCE,
        TOOL_END_INTERVIEW,
    ]
    .into_iter()
    .find(|tool| *tool == name)
    .unwrap_or("unknown")
}

pub(super) struct HintCounters {
    rungs: usize,
    volunteered: u32,
}

impl HintCounters {
    pub(super) fn capture(state: &RuntimeState) -> Self {
        Self {
            rungs: state.hint_rungs_given,
            volunteered: state.volunteered_hints,
        }
    }

    fn advanced(&self, state: &RuntimeState) -> bool {
        state.hint_rungs_given > self.rungs || state.volunteered_hints > self.volunteered
    }
}

pub(super) struct ToolTiming {
    pub(super) batch_ms: u64,
    pub(super) execute_ms: u64,
    pub(super) board_ms: u64,
    pub(super) send_ms: u64,
    pub(super) sent: bool,
}

pub(super) fn tool_timing(
    activity: &mut RuntimeActivity,
    state: &RuntimeState,
    answers: &[(GeminiFunctionCall, Value)],
    hints_before: HintCounters,
    timing: ToolTiming,
) -> Value {
    let mut record = activity
        .latency
        .tool_response(Instant::now(), timing.batch_ms, timing.sent);
    record["tool_batch_ms"] = timing.batch_ms.into();
    record["tool_execute_ms"] = timing.execute_ms.into();
    record["board_resend_ms"] = timing.board_ms.into();
    record["tool_send_ms"] = timing.send_ms.into();
    record["tool_count"] = answers.len().into();
    record["tool_names"] = answers
        .iter()
        .map(|(call, _)| known_tool_name(&call.name))
        .collect::<Vec<_>>()
        .into();
    let accepted = answers
        .iter()
        .filter_map(|(call, response)| {
            let name = known_tool_name(&call.name);
            (name != "unknown" && response.get("error").is_none()).then_some(name)
        })
        .collect::<Vec<_>>();
    record["hint_given"] =
        (accepted.contains(&TOOL_LOG_HINT) && hints_before.advanced(state)).into();
    record["accepted_tool_names"] = accepted.into();
    record["sent"] = timing.sent.into();
    record
}

#[cfg(test)]
#[path = "../../../tests/unit/livekit/session/observations.rs"]
mod tests;

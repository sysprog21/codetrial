//! Every string the model is shown.
//!
//! Kept together because the wording is a product decision, frozen by
//! `tests/golden/prompts.json`: a change here is a change to how the
//! interviewer behaves, not a refactor.

mod assessment;
mod instructions;
mod material;
mod nudges;
mod progress;
mod recovery;

pub use assessment::{
    BehavioralRound, InterimReviewInput, PhaseJudgeInput, ReportPromptInput, interim_review_prompt,
    interim_system_instruction, phase_judge_prompt, phase_judge_system_instruction,
    phase_judgment_note, received_test_note, report_prompt, report_system_instruction,
    report_system_instruction_for_examples, rolling_assessment, unvoiced_record_note,
};
pub use instructions::build_instructions_for_plan;
pub use material::{
    MAX_EXCERPT_LINE_CHARS, MAX_NUMBERED_BYTES, changed_excerpt, format_test_run,
    format_test_run_for_reaction, hint_ladder_used_text, hint_rung_text, hint_rung_withheld_text,
    log_hint_text, numbered, numbered_from, read_board_text, read_editor_text,
};
pub(crate) use material::{
    numbered_bounded_line, numbered_from_with_limit, read_editor_text_from_numbered,
    untrusted_block,
};
pub(crate) use nudges::{
    behavioral_round_start, end_interview_refusal, report_transcript_lines, tool_editor_review,
};
pub use nudges::{
    behavioral_silence_nudge, behavioral_time_warning, board_silence_nudge, proactive_review,
    round_skipped, round_started, silence_nudge, time_warning, wrap_up,
};
pub(crate) use progress::evidenced_among;
pub use progress::{
    LanguageChoiceContext, SincePrevious, TestRecord, greeting, greeting_with_language,
    language_choice, released_follow_ups, spoken_language, test_results_reaction,
    test_runner_outage_reaction, test_runner_unavailable_reaction, test_setup_error_reaction,
    uncredited_test_results_reaction, unrecorded_earlier_phases,
};
pub use recovery::{
    cold_restart, compressed_context, owed_reply, resumed_context, with_owed_reply,
};
pub(crate) use recovery::{
    editor_tool_continuity, latest_candidate_continuity, pending_candidate_line,
};

// `agent.rs` re-exports this mark for the other test modules.
#[cfg(test)]
pub(crate) use self::nudges::BEHAVIORAL_ROUND_MARK;
// Read by `tests` through `use super::*`; nothing else needs them here.
#[cfg(test)]
use self::{
    material::{QUOTED_MARKER_PREFIX, quote_fence_markers},
    nudges::proactive_review_policy,
    progress::{coding_only_continuation, coding_progress, coding_progress_without_presence},
    recovery::{
        COMPRESSION_EDITOR_BYTES, COMPRESSION_OPENING_BYTES, COMPRESSION_TEST_REPORT_BYTES,
        compressed_editor_and_test_report, compressed_split_transcript, compression_editor,
        numbered_compression_excerpt, numbered_ending,
    },
};
#[cfg(test)]
use super::RuntimeState;

#[cfg(test)]
#[path = "../../tests/unit/agent/prompts.rs"]
mod tests;

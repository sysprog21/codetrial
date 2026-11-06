//! Interview runtime: session state, timing decisions, and report shaping.
//!
//! What is left here is the state an interview carries and the decisions taken
//! against it: `RuntimeState`, the framework evidence recorded into it, the
//! timing rules, and the interview configuration a room is started with.
//!
//! The separable concerns are submodules, re-exported here so callers keep
//! importing from `crate::agent`. They are separate because each is read on its
//! own: `events` decides what to believe from the candidate's browser, `report`
//! owns the schema Gemini is asked for and whether a response is one, `value`
//! coerces JSON the way the browser and Python actually behave, and `problems`
//! and `prompts` are the data this file used to hold. `framework` records
//! evidence and reads progress back, `phase_judge` checks a judgment against
//! what was said, `code_work` tells what the candidate has written and run,
//! and `transcript` bounds and marks the lines the report reads.

mod events;

mod turn_taking;
#[cfg(test)]
pub(crate) use turn_taking::THINKING_REQUEST_SETTLE;
pub use turn_taking::ThinkingHold;
pub(crate) use turn_taking::{
    owed_context, owed_context_after_briefing, thinking_change, thinking_check_in,
    thinking_owes_context, thinking_resume, with_thinking_debt,
};
mod code_work;
mod evidence;
mod framework;
mod integrity;
mod phase_judge;
mod problem_guides;
mod problem_rubrics;
mod problem_topics;
mod problem_variants;
pub(crate) mod problems;
mod prompts;
pub(crate) mod report;
mod transcript;
mod value;

#[cfg(test)]
#[path = "../tests/unit/agent/evidence.rs"]
mod evidence_tests;

pub use events::ParseBudget;
pub use events::apply_data_event;
pub(crate) use events::apply_data_event_at;
pub(crate) use events::apply_server_event_at;
pub use evidence::ParseCache;
pub use evidence::{
    CodeAnalysis, CodeChangeClass, CodeObservation, EvidenceLedger, LifecycleTransition,
    ObservationFamily, Provenance,
};
// For the `livekit` tests, which size a buffer past the parse limit from it.
pub(crate) use events::round_transition_due;
#[cfg(test)]
pub(crate) use evidence::MAX_PARSED_BYTES;
pub(crate) use evidence::{ModelInputKind, ViewFor};
pub(crate) use evidence::{analyze_code, analyze_code_cached, observe_code, observe_code_cached};
use integrity::integrity_hash;
pub use integrity::{sanitize_integrity_event, sanitize_test_run};
pub(crate) use problems::variant_for;
pub use problems::{DEFAULT_PROBLEM_ID, PROBLEMS, find_problem, get_problem, topics_for};
#[cfg(test)]
pub(crate) use prompts::BEHAVIORAL_ROUND_MARK;
pub use prompts::{
    BehavioralRound, InterimReviewInput, LanguageChoiceContext, MAX_EXCERPT_LINE_CHARS,
    MAX_NUMBERED_BYTES, PhaseJudgeInput, ReportPromptInput, SincePrevious, TestRecord,
    behavioral_silence_nudge, behavioral_time_warning, board_silence_nudge,
    build_instructions_for_plan, changed_excerpt, cold_restart, compressed_context,
    format_test_run, format_test_run_for_reaction, greeting, greeting_with_language,
    hint_ladder_used_text, hint_rung_text, hint_rung_withheld_text, interim_review_prompt,
    interim_system_instruction, language_choice, log_hint_text, numbered, numbered_from,
    owed_reply, phase_judge_prompt, phase_judge_system_instruction, phase_judgment_note,
    proactive_review, read_board_text, read_editor_text, received_test_note, released_follow_ups,
    report_prompt, report_system_instruction, report_system_instruction_for_examples,
    resumed_context, rolling_assessment, round_skipped, round_started, silence_nudge,
    spoken_language, test_results_reaction, test_runner_outage_reaction,
    test_runner_unavailable_reaction, test_setup_error_reaction, time_warning,
    uncredited_test_results_reaction, unrecorded_earlier_phases, unvoiced_record_note,
    with_owed_reply, wrap_up,
};
pub(crate) use prompts::{
    behavioral_round_start, editor_tool_continuity, end_interview_refusal, evidenced_among,
    latest_candidate_continuity, numbered_bounded_line, numbered_from_with_limit,
    pending_candidate_line, read_editor_text_from_numbered, report_transcript_lines,
    tool_editor_review, untrusted_block,
};
pub(crate) use report::{
    MAX_ERROR_CHARS, Sanitized, sanitize_example_drawing_candidate, sanitize_report_candidate,
};
pub use report::{
    MAX_SUMMARY_TEXT, ReportRounds, fallback_report, final_report, names_published_problem,
    report_response_schema, spelled_words, validate_report_candidate, validate_report_for_round,
};

// Only the tests read these, and a report the filter changed is the one place
// they are written, so exporting them unconditionally is an unused name in a
// release build.
#[cfg(test)]
pub(crate) use report::{SELF_REVIEW_REPLACEMENT, SUCCESS_CRITERION_REPLACEMENT};
pub use value::json_number;
pub(crate) use value::{bounded_model_text, json_int, python_truthy, truthy_string, value_string};

use code_work::{
    MAX_WRITTEN_TABLE, anchor_analysis, runner_unavailable_on_screen, split_comments, starter,
};
pub(crate) use code_work::{
    TestSource, covers, editor_update_body, latest_test_report_is_exact,
    latest_test_report_provenance_note, real_test_run, rewritten, test_record_after_run,
    test_recordable, test_source, tested_code_is_current, tested_code_is_exact, written_in,
};
pub use code_work::{code_untouched, code_written};
use framework::record_evidence;
pub use framework::{
    REACTO_PHASE_IDS, RECEIVED_TEST_SUMMARY, STAR_PHASE_IDS, framework_evidence_json,
    framework_progress, record_framework_evidence, record_hint,
};
pub(crate) use framework::{
    RecordOutcome, coding_continues_past_gate, coding_round_complete, evidence_kind_id,
    evidence_source_id, phase_id, phases_evidenced, reconcile_received_test,
    record_framework_evidence_outcome, skip_unassessed_star,
};
pub use phase_judge::{
    PhaseJudgeDue, apply_phase_judgment, phase_judge_due, phase_judge_need, phase_judge_open,
    record_interim_notes, take_phase_judge_window,
};
pub(crate) use transcript::{
    EARLIER_OMITTED, assessed_speech, has_recognized_candidate_turn, is_unrecognized_speech,
    report_transcript_cut, speaker_speech,
};
pub use transcript::{
    MAX_TRANSCRIPT_BYTES, UNRECOGNIZED_TURN, mark_unrecognized_turns, transcript_for_report,
    transcript_tail,
};
use transcript::{is_unrecognized_turn, tail_start, tail_within};
// Read by the test modules; nothing else needs them here.
#[cfg(test)]
pub(crate) use code_work::editor_update_execution_status;
#[cfg(test)]
use {
    code_work::{MIN_WRITTEN_CHARS, edited_within, uncommented_chars},
    phase_judge::{speaker_turns, typed_comments},
};

use crate::config::{DEFAULT_DURATION_MIN, MAX_DURATION_MIN, MIN_DURATION_MIN};

pub const MAX_INTEGRITY_EVENTS: usize = 25;
/// The `detail` bound, and half of a wire contract: `INTEGRITY_DETAIL_MAX` in
/// `web/lib.js` has to be the same number. The agent truncates to this before
/// it recomputes the hash, so a producer emitting anything longer builds a hash
/// the verifier cannot reproduce, the event is refused, sequence continuity
/// stops the chain, and every later event is refused too, silently. Public so
/// `tests/agent.rs` can hold the two numbers against each other: the fixture
/// catches a browser that outgrows this bound, but not a server that outgrows
/// the browser, and only one of those is caught by bytes already on disk.
pub const MAX_INTEGRITY_TEXT: usize = 80;
/// Per-field bound on a test run's free text. Not shared with the browser the
/// way `MAX_INTEGRITY_TEXT` is: nothing hashes these, so truncating past what
/// the producer sent costs a few characters of a failure message rather than
/// breaking a chain. Larger than the integrity bound because a useful line
/// reads "expected [1, 2, 3], got [3, 2, 1]" and 80 characters cuts that in
/// half; small enough that four failures plus a setup error cannot crowd out
/// the code and the transcript in the report prompt.
const MAX_TEST_TEXT: usize = 200;
/// Clamp on the reported pass and case counts. The largest judge in the bank
/// holds seven cases, so this only has to stop a forged count from rendering
/// as a wall of digits; it is not a claim that the number is true.
const MAX_TEST_CASES: i64 = 99;
/// How many failures a run may carry into the prompt. Named because both the
/// sanitizer and the renderer cap the list and the two have to agree: the
/// renderer would otherwise be silently capping a list the sanitizer already
/// bounded, and a change to one would look like it had taken effect.
///
/// `web/lib.js` sends four as well, but that is the producer being tidy rather
/// than a contract. Nothing hashes this list, so a browser that sends more
/// simply has the extra dropped here, and no same-number test is owed.
const MAX_TEST_FAILURES: usize = 4;
pub const MAX_CANDIDATE_CASES: usize = 5;
pub const WATCH_TICK_S: f64 = 2.0;
/// How long the candidate has to be both silent and not typing before the
/// interviewer steps in with a question.
///
/// Sized for a candidate composing an answer in a second language, which is
/// most of them here. At fifteen seconds this fired while people were still
/// thinking, and being asked a new question is the most expensive possible
/// interruption of someone assembling a sentence. The cost of the other
/// mistake is ten more seconds of silence for a candidate who really is stuck,
/// and `SILENCE_COOLDOWN_S` already says they will be asked again.
///
/// Interpolated into the prompt by `silence_nudge`, so the number Jim is told
/// and the number that fires cannot drift apart.
pub const SILENCE_THRESHOLD_S: f64 = 25.0;
pub const TEST_REACTION_COOLDOWN_S: f64 = 20.0;
pub const SILENCE_COOLDOWN_S: f64 = 30.0;
pub const REVIEW_INTERVAL_S: f64 = 30.0;
pub const INTERJECTION_COOLDOWN_S: f64 = 45.0;
pub const SPEECH_SETTLE_S: f64 = 4.0;

/// How far ahead of this process's clock a round transition may legitimately
/// claim to be.
///
/// Both sides now count from the moment the candidate joined, so what is left
/// is the trip the message makes and the second the browser rounds its
/// countdown to, not the setup time this used to have to cover. It cannot be
/// zero: the browser announces the transition exactly once, and refusing it to
/// the second means the behavioral round never begins at all. What the gate is
/// for is a forged jump past the coding round, which is minutes early.
const ROUND_TRANSITION_SKEW: std::time::Duration = std::time::Duration::from_secs(10);

/// Where the browser announces the interview is nearly over, in seconds left.
///
/// The page owns the countdown and decides when to say so; this side owns
/// whether to believe it. `TIME_WARNING_S` in web/lib.js is the same number,
/// and the two are held together by
/// `the_time_warning_threshold_is_the_same_number_on_both_sides`.
pub const TIME_WARNING_S: u64 = 300;

/// How long a declared thinking hold runs in silence before the interviewer
/// checks in once. A hold with no end kept every nudge quiet for as long as the
/// candidate stayed silent, which could be the rest of the interview.
pub const THINKING_CHECK_IN_S: u64 = 120;

/// A Continue this soon after the last one releases the hold without asking
/// the interviewer to say anything. Each release is otherwise a generated
/// turn, and a candidate clicking Thinking on and off would buy one per click.
pub(crate) const THINKING_RELEASE_COOLDOWN: std::time::Duration =
    std::time::Duration::from_secs(10);

pub const INTERVIEW_CONTRACT_BUNDLE_VERSION: u32 = 44;
pub const LIVE_PROMPT_VERSION: u32 = 33;
pub const REPORT_PROMPT_VERSION: u32 = 25;
pub const RUBRIC_VERSION: u32 = 1;
pub const REPORT_SCHEMA_VERSION: u32 = 4;

pub fn interview_contract_json() -> serde_json::Value {
    serde_json::json!({
        "bundleVersion": INTERVIEW_CONTRACT_BUNDLE_VERSION,
        "livePromptVersion": LIVE_PROMPT_VERSION,
        "reportPromptVersion": REPORT_PROMPT_VERSION,
        "rubricVersion": RUBRIC_VERSION,
        "reportSchemaVersion": REPORT_SCHEMA_VERSION,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Problem {
    pub id: &'static str,
    pub title: &'static str,
    /// The published title with its LeetCode number, if imported.
    pub source: Option<&'static str>,
    pub difficulty: &'static str,
    pub summary: &'static str,
    pub optimal: &'static str,
    pub pitfalls: &'static str,
}

/// The exercise as it is posed, beside the problem it is posed from.
///
/// Generated from `problem-bank/variants.json`. The candidate's page carries
/// `title`, `brief` and the worked examples; everything else here is the
/// interviewer's alone, because an answer to a question nobody asked is the
/// specification read out rather than an interview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProblemVariant {
    pub title: &'static str,
    /// The name the browser knows this problem by: page file, judge file, the
    /// token request and saved history. The id is the published slug and stays
    /// on the server.
    pub page: &'static str,
    pub brief: &'static [&'static str],
    /// The worked examples from the same problem-bank entry as the candidate
    /// page.
    pub examples: &'static [(&'static str, &'static str, Option<&'static str>)],
    /// The exact input and output contract in the scenario's words, which the
    /// interviewer judges against in place of the published statement.
    pub contract: &'static str,
    pub constraints: &'static [&'static str],
    /// Question and answer, answered only when the candidate asks.
    pub clarifications: &'static [(&'static str, &'static str)],
    pub follow_ups: &'static [&'static str],
    /// Three rungs, spoken in order: a nudge, a direction, the key step.
    pub hints: &'static [&'static str],
    /// Each language's starter, as the page serves it, by the browser's
    /// language id. The server's own copy, so the baseline written code is
    /// measured against is not something a packet can set.
    pub starters: &'static [(&'static str, &'static str)],
}

impl ProblemVariant {
    /// The brief as one passage, the way the interviewer and the reviewer read
    /// it.
    pub fn brief_text(&self) -> String {
        self.brief.join(" ")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReactoStage {
    Repeat,
    Example,
    Algorithm,
    Coding,
    Test,
    Optimizations,
}

impl ReactoStage {
    pub const ALL: [Self; 6] = [
        Self::Repeat,
        Self::Example,
        Self::Algorithm,
        Self::Coding,
        Self::Test,
        Self::Optimizations,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Repeat => "repeat",
            Self::Example => "example",
            Self::Algorithm => "algorithm",
            Self::Coding => "coding",
            Self::Test => "test",
            Self::Optimizations => "optimizations",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FollowUpDirection {
    pub stage: ReactoStage,
    pub direction: &'static str,
}

pub const NEUTRAL_FOLLOW_UPS: [FollowUpDirection; 6] = [
    FollowUpDirection {
        stage: ReactoStage::Repeat,
        direction: "Ask the candidate to restate the inputs, outputs, constraints, and ambiguities.",
    },
    FollowUpDirection {
        stage: ReactoStage::Example,
        direction: "Ask the candidate to choose and trace an ordinary example and a boundary case.",
    },
    FollowUpDirection {
        stage: ReactoStage::Algorithm,
        direction: "Ask for the candidate's approach, correctness argument, and complexity.",
    },
    FollowUpDirection {
        stage: ReactoStage::Coding,
        direction: "Ask the candidate to implement their stated approach and explain major decisions.",
    },
    FollowUpDirection {
        stage: ReactoStage::Test,
        direction: "Ask the candidate to predict useful cases and expected results before running them.",
    },
    FollowUpDirection {
        stage: ReactoStage::Optimizations,
        direction: "Ask for complexity, an uncovered edge case, and a justified optimization or cleanup.",
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuestionMetadata<'a> {
    pub difficulty: &'static str,
    pub competencies: &'static [&'static str],
    pub reacto_stages: &'static [ReactoStage],
    pub follow_up_directions: &'static [FollowUpDirection],
    /// Private rubric material. This type is constructed only on the server.
    pub expected_discussion_points: [&'a str; 3],
}

impl Problem {
    /// Every problem has one: `scripts/gen-problems.py` refuses a bank entry
    /// without a variant, and `problem_bank_matches_contract` holds this table
    /// to `PROBLEMS`.
    pub fn variant(&self) -> &'static ProblemVariant {
        variant_for(self.id).expect("every problem has a variant")
    }

    /// The published title for an imported exercise. Original exercises use
    /// their scenario title as `title`, so there is no source name to filter.
    pub fn source_title(&self) -> Option<&'static str> {
        (self.title != self.variant().title).then_some(self.title)
    }

    pub fn question_metadata(&self) -> QuestionMetadata<'_> {
        QuestionMetadata {
            difficulty: self.difficulty,
            competencies: topics_for(self.id).unwrap_or(&[]),
            reacto_stages: &ReactoStage::ALL,
            follow_up_directions: &NEUTRAL_FOLLOW_UPS,
            expected_discussion_points: [self.summary, self.optimal, self.pitfalls],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InterviewLoop {
    CodingOnly,
    #[default]
    CodingBehavioral,
}

impl InterviewLoop {
    pub fn parse(value: Option<&str>) -> Self {
        match value {
            Some("coding_only") => Self::CodingOnly,
            _ => Self::CodingBehavioral,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CodingOnly => "coding_only",
            Self::CodingBehavioral => "coding_behavioral",
        }
    }

    pub const fn behavioral_minutes(self) -> u32 {
        match self {
            Self::CodingOnly => 0,
            Self::CodingBehavioral => 8,
        }
    }
}

/// Which surface the candidate works on, and so what the interviewer can read.
///
/// A whiteboard interview takes the same problem bank and the same six REACTO
/// steps; what it does not have is an editor, a starter, or a test runner, so
/// every rule written against one of those needs the mode beside it rather
/// than a second copy of the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InterviewMode {
    #[default]
    Coding,
    Whiteboard,
}

impl InterviewMode {
    pub fn parse(value: Option<&str>) -> Self {
        match value {
            Some("whiteboard") => Self::Whiteboard,
            _ => Self::Coding,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Coding => "coding",
            Self::Whiteboard => "whiteboard",
        }
    }

    pub const fn is_whiteboard(self) -> bool {
        matches!(self, Self::Whiteboard)
    }
}

pub const MAX_PROFILE_TEXT_CHARS: usize = 80;
/// A practice focus is a report's improvement item quoted word for word, and
/// those run to the 400 characters `sanitizeReport` keeps. The profile bound
/// cut one mid-sentence before the interviewer read it.
pub const MAX_PRACTICE_FOCUS_CHARS: usize = 400;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InterviewProfile {
    pub role: String,
    pub seniority: Option<Seniority>,
    pub target_company: String,
    pub practice_focus: String,
}

pub const MAX_GROUNDING_TEXT_CHARS: usize = 240;
pub const MAX_GROUNDING_TEXT_BYTES: usize = 6 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InterviewGrounding {
    pub requirements: Vec<String>,
    pub skills: Vec<String>,
    pub anchors: Vec<String>,
}

impl InterviewGrounding {
    pub fn is_empty(&self) -> bool {
        self.requirements.is_empty() && self.skills.is_empty() && self.anchors.is_empty()
    }
}

pub fn sanitize_interview_grounding(value: Option<&serde_json::Value>) -> InterviewGrounding {
    let Some(object) = value.and_then(serde_json::Value::as_object) else {
        return InterviewGrounding::default();
    };
    if object
        .get("consentVersion")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        return InterviewGrounding::default();
    }
    let Some(requirements) = grounding_array(object.get("requirements"), 8) else {
        return InterviewGrounding::default();
    };
    let Some(skills) = grounding_array(object.get("skills"), 8) else {
        return InterviewGrounding::default();
    };
    let Some(anchors) = grounding_array(object.get("anchors"), 6) else {
        return InterviewGrounding::default();
    };

    // Per-item limits alone still admit 22 items of 240 characters, which is
    // four times this much once they are multi-byte. The budget is what the
    // prompt can afford to carry, so it is a property of the whole packet.
    if requirements
        .iter()
        .chain(&skills)
        .chain(&anchors)
        .map(String::len)
        .sum::<usize>()
        > MAX_GROUNDING_TEXT_BYTES
    {
        return InterviewGrounding::default();
    }
    InterviewGrounding {
        requirements,
        skills,
        anchors,
    }
}

fn grounding_array(value: Option<&serde_json::Value>, max: usize) -> Option<Vec<String>> {
    let values = value?.as_array()?;
    if values.len() > max {
        return None;
    }
    let mut result = Vec::with_capacity(values.len());
    for value in values {
        let raw = value.as_str()?;
        if raw.is_empty() || raw.chars().count() > MAX_GROUNDING_TEXT_CHARS {
            return None;
        }
        let normalized = raw
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if normalized.is_empty() || result.contains(&normalized) {
            return None;
        }
        result.push(normalized);
    }
    Some(result)
}

pub fn interview_grounding_json(grounding: &InterviewGrounding) -> serde_json::Value {
    serde_json::json!({
        "consentVersion": 1,
        "requirements": grounding.requirements,
        "skills": grounding.skills,
        "anchors": grounding.anchors,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seniority {
    Intern,
    Junior,
    Mid,
    Senior,
    Staff,
    Manager,
}

impl Seniority {
    pub fn parse(value: Option<&str>) -> Option<Self> {
        match value {
            Some("intern") => Some(Self::Intern),
            Some("junior") => Some(Self::Junior),
            Some("mid") => Some(Self::Mid),
            Some("senior") => Some(Self::Senior),
            Some("staff") => Some(Self::Staff),
            Some("manager") => Some(Self::Manager),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Intern => "intern",
            Self::Junior => "junior",
            Self::Mid => "mid",
            Self::Senior => "senior",
            Self::Staff => "staff",
            Self::Manager => "manager",
        }
    }
}

pub fn sanitize_interview_profile(value: Option<&serde_json::Value>) -> InterviewProfile {
    let value = value.and_then(serde_json::Value::as_object);
    InterviewProfile {
        role: profile_text(value.and_then(|item| item.get("role"))),
        seniority: Seniority::parse(
            value
                .and_then(|item| item.get("seniority"))
                .and_then(serde_json::Value::as_str),
        ),
        target_company: profile_text(value.and_then(|item| item.get("targetCompany"))),
        practice_focus: bounded_profile_text(
            value.and_then(|item| item.get("practiceFocus")),
            MAX_PRACTICE_FOCUS_CHARS,
        ),
    }
}

pub fn interview_profile_json(profile: &InterviewProfile) -> serde_json::Value {
    serde_json::json!({
        "role": profile.role,
        "seniority": profile.seniority.map(Seniority::as_str),
        "targetCompany": profile.target_company,
        "practiceFocus": profile.practice_focus,
    })
}

fn profile_text(value: Option<&serde_json::Value>) -> String {
    bounded_profile_text(value, MAX_PROFILE_TEXT_CHARS)
}

fn bounded_profile_text(value: Option<&serde_json::Value>, max_chars: usize) -> String {
    let normalized = value
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    normalized.chars().take(max_chars).collect()
}

/// What Gemini's transcription emits where the audio carried no such word.
///
/// One string rather than a list, because one is what has actually been
/// observed and a list invites guesses about what else might show up. It
/// reaches the candidate's own live transcript and the report prompt, so it is
/// dropped at the point the turn is assembled rather than at either reader.
const TRANSCRIPTION_ARTIFACT: &str = "#hashtag";

/// The assembled turn as it should be shown and written down.
///
/// Cleaned here and never in [`SpeakerTurn::text`]'s accumulator: fragments
/// have to concatenate exactly as they arrived, and the artifact can arrive
/// split across two of them, so a per-fragment filter would miss it and a
/// per-fragment trim would glue words together.
///
/// The `#` guard is the whole test for "is there anything here to clean", and
/// it is deliberately loose: "C#" trips it, and so does any other octothorpe a
/// candidate says out loud. All that costs is that runs of spaces in such a
/// line come back as single spaces, which nobody reading an interview
/// transcript can tell. Splicing the token out by byte offset would preserve
/// them exactly and buys nothing for the length it adds.
fn spoken_text(text: &str) -> String {
    let text = text.trim();
    if !text.contains('#') {
        return text.to_string();
    }
    text.split_whitespace()
        .filter(|word| {
            // Trailing punctuation only: the leading `#` is what separates the
            // artifact from "hashtag" said out loud, and from the "hash map"
            // and "hash table" a candidate says all interview.
            !word
                .trim_end_matches(|c: char| c.is_ascii_punctuation())
                .eq_ignore_ascii_case(TRANSCRIPTION_ARTIFACT)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// One speaker's turn, assembled from the fragments Gemini emits as it
/// recognizes speech.
///
/// Publishing each fragment as its own finished segment is what forced the
/// browser to guess where sentences began, and it left the report prompt
/// reading a stack of one-clause lines. The agent is the only place that knows
/// where a turn ends, so it accumulates here: the segment id stays stable for
/// the whole turn, the published text is cumulative, and [`finish`] closes it.
///
/// [`finish`]: SpeakerTurn::finish
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpeakerTurn {
    index: u64,
    text: String,
    line: Option<usize>,
}

impl SpeakerTurn {
    /// Adds a fragment to this turn and to the session transcript, replacing
    /// the line the turn already owns rather than appending another one.
    /// Returns the whole turn so far, which is what gets published.
    pub fn record(
        &mut self,
        transcript: &mut Vec<String>,
        speaker: &str,
        fragment: &str,
    ) -> String {
        // Concatenated exactly as received. Gemini emits transcription as
        // incremental pieces that already carry their own leading spaces and
        // may split mid-word, so inserting a separator turns "Hey, I'm" + ",
        // so" into "Hey, I'm , so" and "Hel" + "lo" into "Hel lo". Only the
        // assembled turn is cleaned.
        //
        // A turn is one transcript line, and every prompt that quotes the
        // transcript relies on each line opening with its speaker. A break
        // inside the recognized text would start a line with whatever the
        // candidate said, which is how a spoken fence marker would close an
        // untrusted block, so breaks become spaces as each fragment arrives.
        self.text.extend(fragment.chars().map(|character| {
            if value::breaks_a_prompt_line(character) {
                ' '
            } else {
                character
            }
        }));
        let spoken = spoken_text(&self.text);

        // A turn that is nothing but artifact has not said anything yet.
        // Opening a line for it puts a speaker with no words into the report
        // and publishes an empty live segment; the line opens when the first
        // real word arrives, which for a split token is the next fragment.
        if spoken.is_empty() {
            return spoken;
        }
        let line = format!("{speaker}: {spoken}");
        match self.line {
            Some(index) => transcript[index] = line,
            None => {
                self.line = Some(transcript.len());
                transcript.push(line);
            }
        }
        spoken
    }

    /// The start of what this speaker has said in the turn so far.
    ///
    /// For the log only, and bounded: a full turn in a log line is not
    /// diagnosable, and the start is where the words that caused a cut are.
    /// Taken from the cleaned turn: room noise arrives as the transcription
    /// artifact, and quoting it would make a cut with no words read as one
    /// with words.
    pub fn head(&self, max: usize) -> String {
        self.text().chars().take(max).collect()
    }

    /// Stable for the life of one turn, so the browser patches one row instead
    /// of appending a new one per fragment.
    pub fn segment_id(&self, speaker: &str) -> String {
        format!("{speaker}-{}", self.index)
    }

    /// Which transcript line this turn owns, which is also when it started
    /// speaking relative to the other speaker. `None` until the turn has said
    /// a real word.
    pub fn transcript_line(&self) -> Option<usize> {
        self.line
    }

    /// Whether this turn has said anything. Asked of the cleaned text, because
    /// a turn holding only an artifact published no segment and owes no line.
    pub fn is_open(&self) -> bool {
        !self.text().is_empty()
    }

    pub fn text(&self) -> String {
        spoken_text(&self.text)
    }

    /// Ends the turn. The next fragment starts a new segment id and a new
    /// transcript line.
    ///
    /// Clearing and advancing are separate questions. Anything at all has to be
    /// cleared or it concatenates into the next turn, but only a turn that
    /// reached the transcript gives up its id: one that was nothing but
    /// artifact never published a segment under it.
    pub fn finish(&mut self) {
        if self.text.trim().is_empty() {
            return;
        }
        if self.is_open() {
            self.index += 1;
        }
        self.text.clear();
        self.line = None;
    }
}

/// Local cancellation evidence is separate from recognition text: raw text
/// still establishes who supplied an idea first, but not whether it was heard.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct InterviewerDelivery {
    pub(crate) uncertain: std::collections::BTreeSet<usize>,
    current: Option<usize>,
    pending_audio: Option<std::time::Instant>,
    queued: Vec<(usize, std::time::Instant)>,
    /// While open, every fragment is marked as possibly cut output.
    window: Option<FragmentWindow>,
    /// The marked line that fragments in the open window may still extend.
    /// Every other marked line is sealed; see `accepts`.
    extendable: Option<usize>,
}

/// How long fragments after a cut stay marked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FragmentWindow {
    /// A running generation was cut: its fragments can still arrive, up to
    /// the next completion.
    UntilCompletion,
    /// Completed output was cut while queued: transcription trails audio, so
    /// a fragment arriving before the next generation's audio may be its tail.
    UntilNextAudio,
}

/// What a local cut stopped, as `cut_off_turn` and the stale playout drop
/// see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeliveryCut {
    /// Nothing was generating or still queued: nothing is marked.
    Idle,
    /// Output that had completed was still queued, or its transcription was
    /// still arriving.
    Playout,
    /// A generation was cut while it was still producing output.
    Generation,
}

impl InterviewerDelivery {
    pub(crate) fn transcript(&mut self, line: usize) {
        self.current = Some(line);
        if self.window.is_some() {
            self.uncertain.insert(line);
            self.extendable = Some(line);
        }
        if let Some(deadline) = self.pending_audio.take() {
            self.audio(deadline);
        }
    }

    /// Whether an open interviewer line may take another fragment. A cut
    /// seals the line it marks, and so does the end of the window that marked
    /// it: words after either belong on a new line, never inside text every
    /// model reader omits.
    pub(crate) fn accepts(&self, line: usize) -> bool {
        !self.is_uncertain(line) || self.extendable == Some(line)
    }

    pub(crate) fn is_uncertain(&self, line: usize) -> bool {
        self.uncertain.contains(&line)
    }

    /// Whether any line from `start` on is marked.
    pub(crate) fn any_uncertain_from(&self, start: usize) -> bool {
        self.uncertain.range(start..).next().is_some()
    }

    pub(crate) fn audio(&mut self, deadline: std::time::Instant) {
        // Completed output has no audio left to send, so audio after a cut of
        // it belongs to the next generation, whose words open their own line.
        if self.window == Some(FragmentWindow::UntilNextAudio) {
            self.window = None;
            self.extendable = None;
        }
        if self.current.is_some_and(|line| !self.accepts(line)) {
            self.current = None;
        }
        self.prune(std::time::Instant::now());
        if let Some(line) = self.current {
            if let Some((_, end)) = self.queued.iter_mut().find(|(index, _)| *index == line) {
                *end = deadline;
            } else {
                self.queued.push((line, deadline));
            }
        } else {
            self.pending_audio = Some(deadline);
        }
    }

    /// Forgets lines whose audio has finished playing by `now`: audio that
    /// ends at a cut was heard.
    fn prune(&mut self, now: std::time::Instant) {
        self.queued.retain(|(_, end)| *end > now);
    }

    /// A generation completed. It ends a generation cut's window, but not a
    /// playout cut's: a completion with no audio says nothing about whether
    /// the cut output's trailing words are still to come.
    pub(crate) fn complete(&mut self) {
        self.current = None;
        self.pending_audio = None;
        if self.window == Some(FragmentWindow::UntilCompletion) {
            self.window = None;
            self.extendable = None;
        }
    }

    /// The provider socket was replaced: nothing the old one generated can
    /// arrive on the new one, so every window ends.
    pub(crate) fn replace(&mut self) {
        self.complete();
        self.window = None;
        self.extendable = None;
    }

    /// Marks every line with audio still queued, and the open line when
    /// something was cut: without word alignment its words may describe the
    /// cut output. Nothing generating and nothing queued marks nothing.
    pub(crate) fn cancel(&mut self, cut: DeliveryCut, now: std::time::Instant) {
        self.prune(now);
        self.uncertain
            .extend(self.queued.drain(..).map(|(line, _)| line));

        if let Some(line) = self.current.take()
            && cut != DeliveryCut::Idle
        {
            self.uncertain.insert(line);
        }
        self.pending_audio = None;
        self.extendable = None;
        match cut {
            DeliveryCut::Idle => {}
            DeliveryCut::Playout => {
                if self.window != Some(FragmentWindow::UntilCompletion) {
                    self.window = Some(FragmentWindow::UntilNextAudio);
                }
            }
            DeliveryCut::Generation => self.window = Some(FragmentWindow::UntilCompletion),
        }
    }
}

impl RuntimeState {
    /// The transcript as model readers see it, borrowed when nothing is
    /// marked.
    pub(crate) fn delivery_transcript(&self) -> std::borrow::Cow<'_, [String]> {
        if self.interviewer_delivery.uncertain.is_empty() {
            return std::borrow::Cow::Borrowed(&self.transcript);
        }
        self.transcript
            .iter()
            .enumerate()
            .map(|(index, line)| {
                if self.interviewer_delivery.is_uncertain(index) {
                    format!("{INTERVIEWER_SPEAKER}: {CANCELLED_INTERVIEWER_OUTPUT}")
                } else {
                    line.clone()
                }
            })
            .collect()
    }
}

/// Defined as a macro so the prompts that describe the marker can `concat!`
/// the same text.
macro_rules! cancelled_interviewer_output {
    () => {
        "[Interviewer output cancelled; delivery uncertain; generated words omitted.]"
    };
}
pub(crate) use cancelled_interviewer_output;

pub(crate) const CANCELLED_INTERVIEWER_OUTPUT: &str = cancelled_interviewer_output!();

/// What a recorded complexity analysis describes; see
/// `RuntimeState::analysis`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Analysis {
    /// Given with no run of the code on screen, so possibly over a draft: the
    /// next credited run's code is what it describes.
    AwaitingRun,
    /// The code it describes.
    Of(TestedCode),
}

/// Code a test run executed; see `RuntimeState::tested_code`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestedCode {
    pub language: String,
    pub code: String,
}

impl TestedCode {
    /// The same source the editor holds now, language included.
    fn is_current(&self, state: &RuntimeState) -> bool {
        self.language == state.language && self.code == state.code
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeState {
    pub started_at: std::time::Instant,
    pub interview_loop: InterviewLoop,
    /// Which surface the candidate works on. A whiteboard interview never
    /// publishes a code update or a test run, so `code`, `code_templates`,
    /// `last_test_run` and `test_runs` below stay at their defaults for its
    /// whole life, and every gate that reads them has to ask this first.
    pub interview_mode: InterviewMode,
    pub code_execution_disabled: bool,
    pub example_drawing_enabled: bool,
    /// The candidate opted to lead the Algorithm and Coding steps; see
    /// `prompts::candidate_led_policy` and the nudge it tells `silence_nudge`
    /// to send instead of naming the next step.
    pub candidate_led_enabled: bool,
    pub coding_minutes: u32,
    pub behavioral_minutes: u32,
    pub round_transition_seen: bool,
    /// The one five-minute warning the interviewer has accepted, so a warning
    /// that arrives twice does not become a second interruption.
    pub time_warning_seen: bool,
    pub behavioral_round_started: bool,
    /// Where the behavioral round begins in `transcript`. A cold restart whose
    /// bounded tail no longer reaches back this far cannot see whether the one
    /// follow-up was used or the candidate declined, so it asks for neither.
    pub behavioral_round_transcript_start: usize,
    /// The last interviewer entry when the round opened. An in-flight turn can
    /// grow in place, even with a candidate entry after it, without adding a
    /// transcript line. Recovery must include that turn if its text changed.
    pub behavioral_round_prior_turn: Option<(usize, String)>,
    /// Thinking keeps media and editor evidence live, but suppresses replies.
    /// Changed only through the methods in `turn_taking`.
    pub thinking_hold: ThinkingHold,
    /// When the Thinking button last ended a hold, so toggling it cannot make
    /// the interviewer answer every click; see `THINKING_RELEASE_COOLDOWN`.
    pub thinking_released_at: Option<std::time::Instant>,
    /// The declared hold state the page has not been told yet; see
    /// `take_thinking_notice`.
    pub thinking_notice: Option<bool>,
    /// The checklist the page was last told, cleared when the candidate
    /// rejoins with an empty one; see `framework_progress_unpublished`.
    pub framework_published: Vec<&'static str>,
    /// A reply the hold dropped is still in the model's history, and the
    /// release has to say so; see `thinking_resume`.
    pub thinking_unheard_reply: bool,
    /// A recovery briefing, sent at once or held for the end of a hold, asked
    /// for a reply that has not ended yet. Its answer proves
    /// nothing about the socket, so it does not reset the restart budget.
    pub recovery_reply_pending: bool,
    pub framework_evidence: Vec<FrameworkEvidence>,
    /// Deterministic, bounded facts derived from the live session.  The ledger
    /// deliberately holds no editor text or runner diagnostics: those remain
    /// at their existing boundary and are available to a model only on demand.
    pub evidence_ledger: EvidenceLedger,
    pub code: String,
    /// Kept only while an editor buffer is syntactically invalid, keyed by
    /// language so switching tabs cannot overwrite another tab's recovery
    /// baseline. These runtime-only recovery baselines never enter the
    /// evidence ledger.
    pub last_parseable_code: std::collections::BTreeMap<String, String>,
    /// The last buffer parsed and its tree, so the next update does not parse
    /// it again. Runtime-only for the same reason as the baselines above.
    pub parse_cache: ParseCache,
    /// The byte rate editor parsing is held to; see `ParseBudget`.
    pub parse_budget: ParseBudget,
    /// Whether the candidate has typed, as opposed to the browser having
    /// published a template. See `apply_code_update`.
    pub code_edited: bool,
    /// Each language's starter, seeded from the problem by
    /// [`RuntimeState::for_problem`] and never overwritten, because the browser
    /// keeps a buffer per tab and switching back restores the candidate's
    /// work, and because an emptied editor followed by a paste is work too.
    /// What the candidate wrote is measured against it; see `code_written`.
    pub code_templates: std::collections::BTreeMap<String, String>,
    pub language: String,
    /// Whether `language` is a choice the candidate made, as opposed to the
    /// default this state starts in. The two are indistinguishable in the field
    /// itself, and a cold-restarted interviewer that reads the default as a
    /// choice tells a candidate who wanted C++ that they picked Python and is
    /// forbidden from asking again.
    pub language_chosen: bool,
    /// How many board snapshots have reached the interviewer, and when the
    /// last one did in milliseconds since the interview started.
    ///
    /// The image itself is not here. It lives beside the Gemini socket in
    /// `livekit::board`, because nothing that can read this state is able to
    /// send one, and a copy here would be a megabyte of JPEG on a struct the
    /// report path clones.
    pub board_snapshots: u32,
    /// Ink strokes on the board the interviewer last saw, as the browser
    /// counted them. The candidate's own claim about their own work, exactly
    /// like the editor contents: it gates nothing that a lie about it would
    /// win. What the interviewer is told about the board now, and so it falls
    /// to zero when the candidate clears it.
    pub board_strokes: u32,
    /// Whether any board so far carried `MIN_BOARD_STROKES`. Sticky, unlike
    /// `board_strokes`, because it is what `written_work` asks: clearing the
    /// board between steps is how a whiteboard is used, and work the
    /// candidate drew and then wiped is still work they did.
    pub board_drawn: bool,
    pub last_board_at_ms: Option<u64>,
    /// `read_board` asking the room loop to put the latest board in front of
    /// the model again. A tool response carries JSON and cannot carry an
    /// image, so what the tool can do is ask, the same way `end_requested`
    /// asks for the interview to be closed.
    pub board_resend_requested: bool,
    pub transcript: Vec<String>,
    /// Local delivery evidence; raw transcript text retains recognition and
    /// independence provenance even when queued output is cancelled.
    pub interviewer_delivery: InterviewerDelivery,
    pub last_test_run: Option<serde_json::Value>,
    /// Submitted source belonging to the displayed report, including runs
    /// that earned no Test credit. Missing submission leaves provenance
    /// unknown.
    pub last_test_run_code: Option<TestedCode>,
    pub test_runs: u32,
    /// The language and code of the latest run that executed cases against
    /// code the candidate wrote, as the browser submitted it to the runner.
    /// Test is judged against this snapshot rather than a flag, so a run of an
    /// early stub, or of another language's buffer, does not vouch for code
    /// written after it.
    pub tested_code: Option<TestedCode>,
    /// Whether every case passed in the run that owns `tested_code`. Later
    /// outages, empty runs or uncredited submissions may replace the displayed
    /// test report without changing this execution's outcome.
    pub tested_passed: Option<bool>,
    /// What the complexity analysis last recorded describes, so a later run is
    /// judged against the solution that analysis described rather than against
    /// whatever ran before it.
    pub analysis: Option<Analysis>,
    /// The language whose latest run said the platform could not run tests at
    /// all: the judge is missing or the execution service could not start a
    /// run. A trace of the written code in that language can complete Test
    /// while it is unavailable; switching to a language whose runner works
    /// does not inherit the exception.
    pub runner_unavailable: Option<String>,
    pub hints_used: u32,
    pub volunteered_hints: u32,
    /// The authored rungs for this problem, handed out one at a time by
    /// `record_hint` rather than held in the live prompt. A model holding all
    /// three answers the first request with the third, and nothing downstream
    /// can tell.
    pub hint_ladder: &'static [&'static str],
    pub hint_rungs_given: usize,
    /// The variant's follow-ups, released by the evidence call that completes
    /// the coding round; see `released_follow_ups`.
    pub follow_ups: &'static [&'static str],
    pub integrity_events: Vec<serde_json::Value>,
    /// The chain cursor, held apart from the evidence above.
    ///
    /// Verification is the producer's contract and eviction is this process's
    /// housekeeping, and they used to share one vector. Dropping an event to
    /// stay under `MAX_INTEGRITY_EVENTS` therefore moved the sequence the next
    /// event had to match, so the chain died on the retention policy rather
    /// than on anything the browser did.
    pub integrity_chain: Option<(u64, String)>,
    /// The ends of the heartbeat run, kept out of the evidence above.
    ///
    /// A heartbeat is the only periodic record of analyzer state, so it is not
    /// disposable, but one arrives every five seconds and evidence does not.
    /// Sharing one capped vector meant either heartbeats crowded the evidence
    /// out or the eviction had to keep pushing them back out again. Two samples
    /// say what the whole run said: what the devices and the analyzer looked
    /// like when the interview started, and when it ended.
    ///
    /// Two fields rather than a pair, so that one heartbeat is one `Some` and
    /// nobody has to ask whether the two ends are the same event.
    pub integrity_first_heartbeat: Option<serde_json::Value>,
    pub integrity_last_heartbeat: Option<serde_json::Value>,
    /// Whether the interviewer owes this candidate a cold-restart briefing.
    ///
    /// Set when a cold briefing failed to send, so the socket that replaces
    /// this one gives it. A socket resumed from the cold one inherits the debt,
    /// since its memory starts from that cold session.
    pub needs_cold_brief: bool,
    /// A replacement socket took over from a session that owed a reply while
    /// a thinking hold kept the interviewer quiet. The reply cannot be asked
    /// for then, since it would be discarded; ending the hold asks for it.
    /// Hold the raw event, not its formatted request, so recovery cannot nest
    /// briefings. Some(None) owes a reply without a known event; None owes no
    /// reply.
    pub owed_reply_on_resume: Option<Option<String>>,
    /// Observations a reviewer recorded in the pauses, while the interview was
    /// still running. Held apart from `framework_evidence`, which is the
    /// interviewer's own bookkeeping about which phase happened: these are the
    /// reading of it, and only the final report consumes them.
    pub interim_notes: Vec<String>,
    /// How many transcript lines an idle-window reviewer has already been
    /// shown. The window it gets is everything after this, so a pause that
    /// arrives with nothing new said costs no call at all.
    pub interim_transcript_lines: usize,
    /// How many transcript lines the phase judge has been shown, and the
    /// editor it last read; a call is due only when one of them moved. See
    /// `phase_judge_due`.
    pub judged_transcript_lines: usize,
    pub judged_last_turn: String,
    /// Where the transcript the judge was last shown began, so a behavioral
    /// round found to open earlier is read again from there.
    pub judged_round_from: usize,
    pub judged_code: String,
    pub judged_language: String,
    /// What the judgment in flight was asked for, so a call that fails opens
    /// again only what it read: an editor-only call that failed is retried as
    /// one, at the editor's pace and against its cap, not as a speech call.
    pub judged_for: PhaseJudgeDue,
    /// Whether the phase judge found the candidate declining the behavioral
    /// question. The steps of that probe stay unassessed rather than skipped
    /// at the end, which in a session without the evidence tool the platform
    /// writes on the interviewer's behalf.
    pub star_declined: bool,
    /// The editor the last idle-window review was sent. A review whose code
    /// has not changed since says so instead of sending up to four kilobytes
    /// a note on record already read; like the transcript cursor, it moves
    /// when the call goes out.
    pub interim_code: String,
    /// The earlier steps an evidence reply has already named as open, so each
    /// is named once; see `unrecorded_earlier_phases`. A reminder written into
    /// a reply waits in `earlier_steps_pending` until that reply is delivered,
    /// so one that never reaches the model does not use up the naming; see
    /// `settle_earlier_steps`.
    pub earlier_steps_named: Vec<&'static str>,
    pub earlier_steps_pending: Vec<&'static str>,
    /// The buffer the Live model was last shown, whole or around a change: by a
    /// watch prompt, a test reaction, `read_editor`, a requested hint or a
    /// cold-restart briefing. The next watch prompt or test reaction shows the
    /// change since this, and says the editor is unchanged when there is none,
    /// rather than sending code the model holds or telling it to read the
    /// editor again. Runtime-only, like the recovery baselines above.
    pub code_shown: String,
    /// The interviewer said the session is over. Read by the room loop, which
    /// ends the interview through the same packet the browser sends, so this is
    /// a request and not the end itself; `ended` is the end itself.
    pub end_requested: bool,
    pub ended: bool,
    /// The Live session's explicit compression window, if it has one: the one
    /// case in which older dialogue can leave the model's context, so tool
    /// answers then carry the pending utterance and the room watches for cuts.
    /// Held here alone, so the checkpoint and the tool answers cannot disagree
    /// about whether compression is on.
    pub context_compression: Option<crate::config::GeminiContextCompression>,
    /// Whether the next input could be the one the provider cuts the context
    /// on, as the room last measured it. Under compression the continuity a
    /// cut needs rides beside every input, and stays in the context billed
    /// again each turn, so it is attached only while this holds. True until
    /// something has been measured.
    pub context_cut_plausible: bool,
}

impl RuntimeState {
    /// The reply carrying a reminder of open earlier steps went out, or did
    /// not: delivered, the steps count as named; lost, they may be named again.
    pub(crate) fn settle_earlier_steps(&mut self, delivered: bool) {
        let pending = std::mem::take(&mut self.earlier_steps_pending);
        if delivered {
            self.earlier_steps_named.extend(pending);
        }
    }

    pub(crate) fn prompt_evidence(&self, purpose: ViewFor) -> Vec<String> {
        let mut lines = self.evidence_ledger.prompt_view(purpose);
        if self.code_execution_disabled {
            lines.push("Code execution: disabled by the candidate for this interview; testing evidence comes from hand traces, not executed cases.".to_string());
        }
        lines
    }

    /// A fresh interview of this problem: its hint ladder and its starters,
    /// which nothing the browser sends may replace. Built here rather than by
    /// whoever starts a room, because a state missing either answers every hint
    /// request with "every rung is used" and measures written code against
    /// nothing.
    pub fn for_problem(problem: &Problem) -> Self {
        let variant = problem.variant();
        let mut state = Self {
            hint_ladder: variant.hints,
            follow_ups: variant.follow_ups,
            code_templates: variant
                .starters
                .iter()
                .map(|(language, code)| ((*language).to_string(), (*code).to_string()))
                .collect(),
            ..Self::default()
        };
        state
            .evidence_ledger
            .set_uncovered_coverage(REACTO_PHASE_IDS);
        state
    }
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            started_at: std::time::Instant::now(),
            interview_loop: InterviewLoop::CodingBehavioral,
            interview_mode: InterviewMode::Coding,
            code_execution_disabled: false,
            example_drawing_enabled: false,
            candidate_led_enabled: false,
            coding_minutes: 37,
            behavioral_minutes: 8,
            round_transition_seen: false,
            time_warning_seen: false,
            behavioral_round_started: false,
            behavioral_round_transcript_start: 0,
            behavioral_round_prior_turn: None,
            thinking_hold: ThinkingHold::Off,
            thinking_released_at: None,
            thinking_notice: None,
            framework_published: Vec::new(),
            thinking_unheard_reply: false,
            recovery_reply_pending: false,
            framework_evidence: Vec::new(),
            evidence_ledger: EvidenceLedger::default(),
            code: String::new(),
            last_parseable_code: std::collections::BTreeMap::new(),
            parse_cache: ParseCache::default(),
            parse_budget: ParseBudget::default(),
            code_edited: false,
            code_templates: std::collections::BTreeMap::new(),
            language: "python".to_string(),
            language_chosen: false,
            board_snapshots: 0,
            board_strokes: 0,
            board_drawn: false,
            last_board_at_ms: None,
            board_resend_requested: false,
            transcript: Vec::new(),
            interviewer_delivery: InterviewerDelivery::default(),
            last_test_run: None,
            last_test_run_code: None,
            test_runs: 0,
            tested_code: None,
            tested_passed: None,
            analysis: None,
            runner_unavailable: None,
            hints_used: 0,
            volunteered_hints: 0,
            hint_ladder: &[],
            hint_rungs_given: 0,
            follow_ups: &[],
            integrity_events: Vec::new(),
            integrity_chain: None,
            integrity_first_heartbeat: None,
            integrity_last_heartbeat: None,
            needs_cold_brief: false,
            owed_reply_on_resume: None,
            interim_notes: Vec::new(),
            interim_transcript_lines: 0,
            judged_transcript_lines: 0,
            judged_last_turn: String::new(),
            judged_round_from: 0,
            judged_code: String::new(),
            judged_language: String::new(),
            judged_for: PhaseJudgeDue::No,
            star_declined: false,
            interim_code: String::new(),
            earlier_steps_named: Vec::new(),
            earlier_steps_pending: Vec::new(),
            code_shown: String::new(),
            end_requested: false,
            ended: false,
            context_compression: None,
            context_cut_plausible: true,
        }
    }
}

pub const FRAMEWORK_VERSION: u32 = 1;
pub const MAX_FRAMEWORK_EVIDENCE: usize = 64;
const MAX_FRAMEWORK_SUMMARY_CHARS: usize = 240;
/// Lines of idle-window assessment one interview keeps. Each is one bounded
/// observation, and the report prompt carries all of them, so this is a size
/// budget rather than a retention policy.
pub const MAX_INTERIM_NOTES: usize = 48;
pub(crate) const MAX_INTERIM_LINE_CHARS: usize = 300;
/// Observations one idle-window review may contribute. The prompt asks for at
/// most four; this is the same number where it can be relied on.
pub const MAX_INTERIM_LINES_PER_REVIEW: usize = 4;
/// How many of those notes a later review is shown, so it does not return one
/// of them reworded.
///
/// The whole list was passed at first, which meant every review re-sent every
/// note taken so far: prefill growing quadratically across a session, to defend
/// against a repeat that `record_interim_notes` already drops. The recent
/// ones are the ones a new note is likely to duplicate, so this is where the
/// defense is worth paying for.
///
/// The notes have no phase to protect, so they cannot be evicted by the rule
/// the evidence uses. They are still ordered in time, though, and a plain
/// oldest-first cap spends the opening of the interview first -- the problem
/// restatement and the clarifying questions, which is REACTO's R and E and the
/// part a reviewer has the least other evidence for. So the opening is
/// reserved and the eviction starts after it.
pub(crate) const INTERIM_CONTEXT_NOTES: usize = 12;
/// Notes from the opening of the interview that the cap may not evict.
pub(crate) const INTERIM_OPENING_KEPT: usize = 8;

// `Vec::remove` panics out of bounds, so the reserve being smaller than the
// store is not a preference: it is what stops an interview crashing on its
// forty-ninth note. Checked at build time rather than left to whoever next
// tunes one of them.
const _: () = assert!(INTERIM_OPENING_KEPT < MAX_INTERIM_NOTES);

/// What `SpeakerTurn::record` is passed for the human in the room, and so the
/// prefix its lines carry.
///
/// Both speakers land in one transcript, so "has anything been said since the
/// last review" has to mean the candidate specifically: four of Jim's own turns
/// are not a stretch of interview worth reading, and counting them spent a call
/// on one. Named here and passed at the call site, so a rename cannot leave
/// `candidate_lines` quietly answering zero forever.
pub(crate) const CANDIDATE_SPEAKER: &str = "Candidate";

/// Shared by the transcript writer and the round-boundary lookup so a speaker
/// rename cannot silently disable recovery of an in-flight interviewer turn.
pub(crate) const INTERVIEWER_SPEAKER: &str = "Interviewer";

/// Where the transcript nobody has reviewed yet begins.
///
/// Clamped rather than indexed directly: the cursor counts lines already shown,
/// and nothing forbids a future caller from clearing the transcript under it. A
/// panic here would take the interview with it.
pub(crate) fn unreviewed_from(state: &RuntimeState) -> usize {
    state.interim_transcript_lines.min(state.transcript.len())
}

/// The head of the editor, within a byte budget, on a character boundary.
///
/// The budget covers the code. The notice that follows a cut is added on top
/// of it, so a truncated head is the budget plus that fixed string: a cap on
/// what is quoted, not on the length of the return value. Reserving its width
/// instead would buy an exact ceiling by dropping a line of code for a
/// constant thirty-odd bytes, which is not the thing being bounded.
///
/// The head and not the tail, unlike a transcript: code is read from the top,
/// and the signature and the approach are what a reviewer needs. Nothing
/// bounds `state.code` on the way in -- `apply_code_update` appends whatever
/// the browser sent -- so a large paste would otherwise be re-sent whole to
/// every idle-window review, each of which has twelve seconds to answer.
pub fn code_head(code: &str, budget: usize) -> String {
    if code.len() <= budget {
        return code.to_string();
    }

    let end = code.floor_char_boundary(budget);
    format!("{}\n(remainder of the editor omitted)", &code[..end])
}

/// The newest line `speaker` wrote, with its index.
pub(crate) fn last_speaker_line<'a>(
    lines: &'a [String],
    speaker: &str,
) -> Option<(usize, &'a String)> {
    let prefix = format!("{speaker}: ");
    lines
        .iter()
        .enumerate()
        .rev()
        .find(|(_, line)| line.starts_with(&prefix))
}

/// The candidate's own turns in a stretch of transcript.
pub(crate) fn candidate_lines(lines: &[String]) -> usize {
    let prefix = format!("{CANDIDATE_SPEAKER}: ");
    lines
        .iter()
        .filter(|line| line.starts_with(&prefix))
        .count()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameworkPhase {
    Repeat,
    Example,
    Algorithm,
    Coding,
    Test,
    Optimizations,
    Situation,
    Task,
    Action,
    Result,
}

impl FrameworkPhase {
    pub(crate) fn is_star(self) -> bool {
        matches!(
            self,
            Self::Situation | Self::Task | Self::Action | Self::Result
        )
    }

    /// Coding, Test and Optimizations are all about code, so none of them is
    /// reached while the editor holds nothing the candidate wrote.
    pub(crate) fn needs_code(self) -> bool {
        matches!(self, Self::Coding | Self::Test | Self::Optimizations)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceSource {
    CandidateSpeech,
    EditorSnapshot,
    /// A whiteboard interview's counterpart to an editor snapshot: the board
    /// image the interviewer was shown. Spelled apart from the editor because
    /// a reviewer reading the report has to be able to tell which surface an
    /// observation was made on.
    BoardSnapshot,
    TestEvent,
    SessionTiming,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceKind {
    Observed,
    Inferred,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameworkEvidence {
    pub at_ms: u64,
    pub phase: FrameworkPhase,
    pub source: EvidenceSource,
    pub kind: EvidenceKind,
    pub confidence: u8,
    pub summary: String,
    pub framework_version: u32,
}

/// Which observation the cap gives up, once one has to go.
///
/// Dropping the oldest is what a bounded log does and it is wrong here. The
/// first rows written are `repeat` and `example`, so the oldest-first rule
/// spent them first, and the interview that overran the cap -- the long one,
/// whose report needs this evidence most -- reached the reviewer having lost
/// the phases it opened with. What is disposable is a phase's second and later
/// observations, because the phase survives them.
///
/// So: the oldest row belonging to a phase that has another one. There are ten
/// phases and the cap is far above that, so at the moment this is called some
/// phase always holds a spare and the fallback below is unreachable; it is the
/// answer to "what if the cap were ever lowered past the phase count", not a
/// case that runs.
fn evict_one_observation(evidence: &mut Vec<FrameworkEvidence>) {
    // "The phase has another row" is not enough on its own. What the report and
    // `phases_evidenced` read is a phase's non-skipped rows, so a phase holding
    // one real observation and one skip has exactly one row that matters, and
    // taking it turns a completed round back into an incomplete one -- which
    // also refuses the interviewer its own ending, because that gate reads the
    // same rows. A skip is expendable whenever anything else covers its phase;
    // an observation only when another observation does.
    let expendable = |index: usize| {
        let item = &evidence[index];
        evidence.iter().enumerate().any(|(other, row)| {
            other != index
                && row.phase == item.phase
                && (item.kind == EvidenceKind::Skipped || row.kind != EvidenceKind::Skipped)
        })
    };
    let doomed = (0..evidence.len()).find(|index| expendable(*index));
    evidence.remove(doomed.unwrap_or(0));
}

/// Whether the live interviewer is given `record_framework_evidence`.
///
/// Every call is a tool round in front of the reply it precedes: the model
/// waits for the answer and then runs a second pass before it speaks. With a
/// typed editor the platform records every step without it, the phase judge
/// from the conversation and the editor and Test from the run or a spoken
/// trace, so the tool only costs time; neither a prompt telling the model not
/// to call it nor leaving steps out of its schema stopped the calls. Only a
/// whiteboard needs it, where Coding and Test are drawn and the judge, which
/// is not shown the board, cannot see them.
///
/// Fixed for the session, since a replaced or resumed socket is set up from
/// the same boot.
pub fn evidence_tool_declared(mode: InterviewMode) -> bool {
    mode.is_whiteboard()
}

/// Strokes a board must carry before the phases about written work are
/// reachable, the board's answer to `MIN_WRITTEN_CHARS`.
///
/// A floor against nothing at all, not a measure of quality: three strokes is
/// a line and two marks, which is less than any real diagram and more than the
/// stray dot a candidate leaves while finding the pen.
pub const MIN_BOARD_STROKES: u32 = 3;

/// How far into the interview it is now, in milliseconds.
///
/// One clock for everything that stamps itself against the session: the phase
/// evidence, the boards, and the age `read_board` reports. They were three
/// copies of the same saturating cast, and the cast is the part worth writing
/// once -- an interview cannot run for 585 million years, but the type says it
/// could and the conversion has to answer for it.
pub fn elapsed_ms(state: &RuntimeState) -> u64 {
    saturating_ms(state.started_at.elapsed())
}

/// Whole milliseconds, saturating rather than wrapping past `u64::MAX`.
pub(crate) fn saturating_ms(duration: std::time::Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

/// How long ago the candidate's newest board arrived, in whole seconds, or
/// `None` before the first one.
///
/// Arrival rather than the last send, because what `read_board` reports with
/// it is how long ago the candidate left the board that way, and the same call
/// puts that board in front of the interviewer again.
pub fn board_age_seconds(state: &RuntimeState) -> Option<u64> {
    Some(elapsed_ms(state).saturating_sub(state.last_board_at_ms?) / 1000)
}

/// Whether the candidate has produced the written work that Coding, Test and
/// Optimizations are about: code in the editor, or a drawing on the board.
///
/// One question with two surfaces under it. Asking `code_written` directly in
/// a whiteboard interview answers about an editor nobody has, which is always
/// no, and that refuses the second half of the interview outright.
pub fn written_work(state: &RuntimeState) -> bool {
    match state.interview_mode {
        InterviewMode::Coding => code_written(state),
        InterviewMode::Whiteboard => state.board_drawn,
    }
}

/// The characters of a piece of code that are content rather than layout.
pub(crate) fn content_chars(code: &str) -> impl Iterator<Item = char> + '_ {
    code.chars().filter(|character| !character.is_whitespace())
}

/// The characters of `code` outside its longest common subsequence with
/// `before`, and of `before` outside it: what was typed and what was deleted,
/// each in order, so neither cancels the other.
///
/// In order because an unordered count lets a deletion cancel an addition. The
/// starters carry a "think out loud" comment, and deleting it used to cancel
/// most of a one-line answer, so `return sqrt(x);` did not count as code. The
/// shared prefix and suffix, the signature and its closing lines, are trimmed
/// first so the quadratic table covers only the body that changed. It runs on
/// an evidence call, a received test run and a prompt that states test
/// progress: every few seconds at most, on code that rarely changes much.
fn changed_characters(before: &[char], code: &[char]) -> Option<(usize, usize)> {
    let prefix = before
        .iter()
        .zip(code)
        .take_while(|(expected, written)| expected == written)
        .count();
    let (before, code) = (&before[prefix..], &code[prefix..]);
    let suffix = before
        .iter()
        .rev()
        .zip(code.iter().rev())
        .take_while(|(expected, written)| expected == written)
        .count();
    let (before, code) = (
        &before[..before.len() - suffix],
        &code[..code.len() - suffix],
    );

    // A bound on the table rather than on the packets: past it there is no
    // count, and the caller decides which way that fails. Against a starter of
    // a few hundred characters a real buffer never gets here. Against a tested
    // snapshot it can, when a long solution was edited near both ends; either
    // way a forged buffer cannot make an evidence call or a test run stall the
    // room.
    if before.len().saturating_mul(code.len()) > MAX_WRITTEN_TABLE {
        return None;
    }
    let mut previous = vec![0usize; before.len() + 1];
    let mut current = previous.clone();
    for written in code {
        for (at, expected) in before.iter().enumerate() {
            current[at + 1] = if written == expected {
                previous[at] + 1
            } else {
                current[at].max(previous[at + 1])
            };
        }
        std::mem::swap(&mut previous, &mut current);
    }
    let common = previous[before.len()];
    Some((code.len() - common, before.len() - common))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataConfig {
    pub voice: Option<&'static str>,
    pub accent: Option<&'static str>,
    pub problem: &'static Problem,
    pub duration_min: u32,
    pub interview_loop: InterviewLoop,
    pub interview_mode: InterviewMode,
    pub profile: InterviewProfile,
    pub grounding: InterviewGrounding,
    /// The candidate hid the worked examples in the preflight.
    pub examples_hidden: bool,
    pub code_execution_disabled: bool,
    pub example_drawing_enabled: bool,
    pub candidate_led_enabled: bool,
    /// The language the candidate chose before joining, an id
    /// `spoken_language` accepts, and never set at a whiteboard.
    pub language: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TimingInput {
    pub agent_busy: bool,
    pub user_talking: bool,
    pub idle_seconds: f64,
    pub since_last_nudge_seconds: f64,
    pub since_last_review_seconds: f64,
    pub since_last_interjection_seconds: f64,
    pub speech_gap_seconds: f64,
    pub significant_change: bool,
}

/// All-false is "do nothing this tick", which is the overwhelmingly common
/// outcome, so every field defaults to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TimingDecision {
    pub silence_nudge: bool,
    pub proactive_review: bool,
    pub update_last_nudge: bool,
    pub update_last_review: bool,
    pub update_last_interjection: bool,
    pub sync_revision_at_last_review: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TestReactionDecision {
    pub react: bool,
    pub update_last_test_reaction: bool,
    pub update_last_interjection: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DataEventResult {
    pub update_last_code_change: bool,
    pub update_last_test_reaction: bool,
    pub update_last_interjection: bool,
    pub generate_reply: Option<String>,
    pub finish_interview: Option<String>,
    pub thinking_changed: Option<bool>,
    pub yield_turn: bool,
    /// `generate_reply` carries what a hold left owed (see `thinking_resume`),
    /// so delivering it pays that debt.
    pub carries_thinking_debt: bool,
    /// The clock outranks a thinking hold: the reply ends it rather than being
    /// suppressed by it. Held until the candidate spoke again, the five-minute
    /// warning reached a candidate thinking in silence only at the deadline,
    /// and a new round cannot wait on thinking about the last one.
    pub preempts_hold: bool,
    /// A reply the thinking hold suppressed, to be given to the model as
    /// context that asks for no answer.
    pub held_context: Option<String>,
    /// Agent-owned round transition result: `started` or `skipped`.
    pub round_changed: Option<&'static str>,
    /// How a test result was judged, for the room log; see `TestRunNote`.
    /// `None` is a packet dropped before it was judged. Nothing reads it to
    /// decide anything.
    pub test_run: Option<TestRunNote>,
}

/// The one fact about a test result only its judging knows: whether the run
/// earned credit, and if so how its code compares with the previous credited
/// run's. Whether a reaction followed, and whether the credited run still
/// covers the editor, the log reads from the result and the state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TestRunNote {
    /// `None` when the run earned no credit, so there is nothing to compare.
    pub credited: Option<SincePrevious>,
}

pub fn spoken_minutes_from_remaining_seconds(remaining_seconds: i64) -> i64 {
    ((remaining_seconds as f64) / 60.0)
        .round_ties_even()
        .max(1.0) as i64
}

/// What the candidate's countdown reads, in whole minutes.
///
/// Wall time, because the page's deadline is wall time too, and nothing stops
/// either clock before it.
pub fn minutes_left(state: &RuntimeState) -> i64 {
    let planned = i64::from(state.coding_minutes + state.behavioral_minutes) * 60;
    let elapsed = i64::try_from(state.started_at.elapsed().as_secs()).unwrap_or(i64::MAX);
    let remaining = planned.saturating_sub(elapsed);

    // Not `spoken_minutes_from_remaining_seconds`, whose floor of one is what
    // the spoken warning wants and the opposite of what a reading wants. The
    // deadline grace runs two minutes past zero, and through all of it that
    // floor would report a minute the candidate's screen does not have.
    if remaining <= 0 {
        return 0;
    }
    spoken_minutes_from_remaining_seconds(remaining)
}

/// The two halves of `timer_line`, shared with `unstamped_event` so a reworded
/// clock is still the one it strips.
const TIMER_PREFIX: &str = "TIMER: about ";
const TIMER_SUFFIX: &str = " minutes remain on the candidate's countdown.";

/// The one sentence any prompt says the time in, so a model told these are the
/// only readings it has can recognize every one of them.
pub fn timer_line(minutes: i64) -> String {
    format!("{TIMER_PREFIX}{minutes}{TIMER_SUFFIX}")
}

/// Removes the final clock from a trusted platform event or tool result before
/// it is moved beside a fresh reading. Interior clocks can be candidate text
/// and stay.
pub(crate) fn unstamped_event(prompt: &str) -> &str {
    let Some((prefix, clock)) = prompt.rsplit_once(TIMER_PREFIX) else {
        return prompt;
    };
    let Some(event) = prefix
        .strip_suffix(' ')
        .or_else(|| prefix.strip_suffix('\n'))
    else {
        return prompt;
    };
    let Some(minutes) = clock.strip_suffix(TIMER_SUFFIX) else {
        return prompt;
    };
    if minutes
        .parse::<i64>()
        .is_ok_and(|value| value.to_string() == minutes)
    {
        event
    } else {
        prompt
    }
}

/// A stage direction, with the clock the model does not have.
///
/// Told the duration once and warned once at five minutes, it guessed at
/// everything in between, and it guessed the one number its instructions name:
/// five minutes remaining with fifteen still on the timer, and a candidate
/// hurried into wrapping up.
pub fn with_timer(state: &RuntimeState, prompt: String) -> String {
    // Stamped whatever the prompt already says. Most of these carry the
    // candidate's own code, so skipping a prompt that reads as stamped would
    // let anyone who types the sentence into the editor decide that this event
    // carries no reading at all -- or, worse, that the only one it carries is
    // theirs. The reading is always the last sentence, and the live prompt says
    // so, which is the rule the model needs when a stamp it cannot trust sits
    // somewhere above.
    format!("{prompt} {}", timer_line(minutes_left(state)))
}

pub const INTERVIEWER_VOICES: &[&str] = &["Puck", "Charon", "Kore", "Orus"];
pub const INTERVIEWER_ACCENTS: &[&str] =
    &["American", "British", "Australian", "Canadian", "Indian"];

pub(crate) fn sanitize_interviewer_choice(
    value: Option<&serde_json::Value>,
    choices: &[&'static str],
) -> Option<&'static str> {
    let value = value?.as_str()?;
    choices.iter().copied().find(|choice| *choice == value)
}

/// Fixed speaking directions, never candidate-authored prompt text.
pub fn interviewer_accent_instruction(accent: &str) -> Option<String> {
    INTERVIEWER_ACCENTS.contains(&accent).then(|| {
        format!(
            "\n\n[INTERVIEWER SPEAKING STYLE]\nSpeak English with a natural {accent} accent. Apply this only to pronunciation; keep the interview content, assessment rules, and professional tone unchanged."
        )
    })
}

pub fn parse_participant_metadata(metadata: Option<&str>) -> MetadataConfig {
    let value = metadata
        .and_then(|metadata| serde_json::from_str::<serde_json::Value>(metadata).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let problem = get_problem(value.get("problemId").and_then(serde_json::Value::as_str));
    let duration_min = duration_from_metadata(value.get("durationMin"));
    let interview_loop = InterviewLoop::parse(
        value
            .get("interviewLoop")
            .and_then(serde_json::Value::as_str),
    );
    let interview_mode = InterviewMode::parse(
        value
            .get("interviewMode")
            .and_then(serde_json::Value::as_str),
    );
    let profile = sanitize_interview_profile(value.get("interviewProfile"));
    let grounding = sanitize_interview_grounding(value.get("interviewGrounding"));
    let examples_hidden = value.get("hideExamples") == Some(&serde_json::Value::Bool(true));
    let code_execution_disabled =
        value.get("codeExecution") == Some(&serde_json::Value::Bool(false));

    let example_drawing_enabled = !interview_mode.is_whiteboard()
        && value.get("exampleDrawing") == Some(&serde_json::Value::Bool(true));
    let candidate_led_enabled = value.get("candidateLed") == Some(&serde_json::Value::Bool(true));

    // Checked again here although the token handler checked it: this string
    // reaches the greeting and the runtime state, and the metadata is the only
    // record of it the agent has.
    let language = value
        .get("language")
        .and_then(serde_json::Value::as_str)
        .filter(|language| !interview_mode.is_whiteboard() && spoken_language(language).is_some())
        .map(str::to_string);
    MetadataConfig {
        voice: sanitize_interviewer_choice(value.get("interviewerVoice"), INTERVIEWER_VOICES),
        accent: sanitize_interviewer_choice(value.get("interviewerAccent"), INTERVIEWER_ACCENTS),
        problem,
        duration_min,
        interview_loop,
        interview_mode,
        profile,
        grounding,
        examples_hidden,
        code_execution_disabled,
        example_drawing_enabled,
        candidate_led_enabled,
        language,
    }
}

pub fn timing_decision(input: &TimingInput) -> TimingDecision {
    if input.agent_busy || input.user_talking {
        return TimingDecision::default();
    }

    if input.idle_seconds >= SILENCE_THRESHOLD_S
        && input.since_last_nudge_seconds >= SILENCE_COOLDOWN_S
    {
        return TimingDecision {
            silence_nudge: true,
            proactive_review: false,
            update_last_nudge: true,
            update_last_review: false,
            update_last_interjection: true,
            sync_revision_at_last_review: false,
        };
    }

    if input.since_last_review_seconds >= REVIEW_INTERVAL_S
        && input.since_last_interjection_seconds >= INTERJECTION_COOLDOWN_S
        && input.speech_gap_seconds >= SPEECH_SETTLE_S
        && input.significant_change
    {
        return TimingDecision {
            silence_nudge: false,
            proactive_review: true,
            update_last_nudge: false,
            update_last_review: true,
            update_last_interjection: true,
            sync_revision_at_last_review: true,
        };
    }

    TimingDecision::default()
}

/// `must_react` is a run the model has to hear about whatever the cooldown
/// says, because nothing else would tell it: one that makes Test recordable,
/// or the first to find the runner missing for the code on screen.
pub fn test_reaction_decision(
    ended: bool,
    since_last_test_reaction_seconds: f64,
    must_react: bool,
) -> TestReactionDecision {
    if ended || (since_last_test_reaction_seconds < TEST_REACTION_COOLDOWN_S && !must_react) {
        return TestReactionDecision::default();
    }

    TestReactionDecision {
        react: true,
        update_last_test_reaction: true,
        update_last_interjection: true,
    }
}

pub(crate) fn duration_from_metadata(value: Option<&serde_json::Value>) -> u32 {
    value
        .filter(|value| python_truthy(value))
        .and_then(json_int)
        .map_or(DEFAULT_DURATION_MIN, |duration| {
            u32::try_from(duration.clamp(MIN_DURATION_MIN.into(), MAX_DURATION_MIN.into()))
                .expect("a duration clamped to u32 bounds fits in u32")
        })
}

#[cfg(test)]
#[path = "../tests/unit/agent.rs"]
mod tests;

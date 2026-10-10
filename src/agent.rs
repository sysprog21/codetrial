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
//! and `prompts` are the data this file used to hold.

mod events;

mod turn_taking;
#[cfg(test)]
pub(crate) use turn_taking::THINKING_REQUEST_SETTLE;
pub use turn_taking::ThinkingHold;
pub(crate) use turn_taking::{
    owed_context, thinking_change, thinking_check_in, thinking_owes_context, thinking_resume,
    with_thinking_debt,
};
mod evidence;
mod integrity;
mod problem_guides;
mod problem_rubrics;
mod problem_topics;
mod problem_variants;
mod problems;
mod prompts;
mod report;
mod value;

#[cfg(test)]
#[path = "../tests/unit/agent/evidence.rs"]
mod evidence_tests;

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
use problems::variant_for;
pub use problems::{DEFAULT_PROBLEM_ID, PROBLEMS, find_problem, get_problem, topics_for};
#[cfg(test)]
pub(crate) use prompts::BEHAVIORAL_ROUND_MARK;
pub use prompts::{
    BehavioralRound, InterimReviewInput, LanguageChoiceContext, MAX_EXCERPT_LINE_CHARS,
    MAX_NUMBERED_BYTES, PhaseJudgeInput, ReportPromptInput, SincePrevious, TestRecord,
    behavioral_silence_nudge, behavioral_time_warning, board_silence_nudge,
    build_instructions_for_plan, changed_excerpt, cold_restart, compressed_context,
    format_test_run, format_test_run_for_reaction, greeting, hint_ladder_used_text, hint_rung_text,
    hint_rung_withheld_text, interim_review_prompt, interim_system_instruction, language_choice,
    log_hint_text, numbered, numbered_from, owed_reply, phase_judge_prompt,
    phase_judge_system_instruction, phase_judgment_note, proactive_review, read_board_text,
    read_editor_text, received_test_note, released_follow_ups, report_prompt,
    report_system_instruction, resume, resumed_context, rolling_assessment, round_skipped,
    round_started, silence_nudge, spoken_language, test_results_reaction,
    test_runner_unavailable_reaction, test_setup_error_reaction, time_warning,
    uncredited_test_results_reaction, unrecorded_earlier_phases, with_owed_reply, wrap_up,
};
pub(crate) use prompts::{editor_tool_continuity, end_interview_refusal, report_transcript_lines};
pub(crate) use report::{MAX_ERROR_CHARS, Sanitized, sanitize_report_candidate};
pub use report::{
    MAX_SUMMARY_TEXT, fallback_report, final_report, names_published_problem,
    report_response_schema, spelled_words, validate_report, validate_report_candidate,
    validate_report_for_round,
};

// Only the tests read these, and a report the filter changed is the one place
// they are written, so exporting them unconditionally is an unused name in a
// release build.
#[cfg(test)]
pub(crate) use report::{SELF_REVIEW_REPLACEMENT, SUCCESS_CRITERION_REPLACEMENT};
pub use value::json_number;
pub(crate) use value::{bounded_model_text, json_int, python_truthy, truthy_string, value_string};

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

pub const INTERVIEW_CONTRACT_BUNDLE_VERSION: u32 = 32;
pub const LIVE_PROMPT_VERSION: u32 = 24;
pub const REPORT_PROMPT_VERSION: u32 = 19;
pub const RUBRIC_VERSION: u32 = 1;
pub const REPORT_SCHEMA_VERSION: u32 = 2;

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
        self.text.push_str(fragment);
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

    /// Stable for the life of one turn, so the browser patches one row instead
    /// of appending a new one per fragment.
    /// The tail of what this speaker has said in the turn so far.
    ///
    /// For the log only, and bounded: a cut turn is diagnosable from what the
    /// candidate was heard saying at the moment it was cut, and a full turn in
    /// a log line is not.
    pub fn tail(&self, max: usize) -> &str {
        let text = self.text.trim();

        // No length test beside this. `nth_back` already answers it: it yields
        // nothing when there are fewer than `max` characters, and when there
        // are exactly `max` it lands on the first one, so the slice is the
        // whole string either way. The condition that used to be here could be
        // written as `>` or `>=` without changing a single result.
        match text.char_indices().nth_back(max.saturating_sub(1)) {
            Some((start, _)) => &text[start..],
            None => text,
        }
    }

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
    pub coding_minutes: u32,
    pub behavioral_minutes: u32,
    pub round_transition_seen: bool,
    /// The one five-minute warning the interviewer has accepted. The browser
    /// re-sends its level after a pause in case the first packet arrived while
    /// the server was paused, so this is the acknowledgement that prevents a
    /// delivered warning from becoming a second interruption.
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
    pub paused: bool,
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
    /// A recovery briefing, sent at once or held for an unpause or the end of
    /// a hold, asked for a reply that has not ended yet. Its answer proves
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
    pub last_test_run: Option<serde_json::Value>,
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
    /// Set when a Gemini socket is replaced by a session that remembers nothing
    /// while the interview is paused, which is the one moment the briefing
    /// cannot simply be spoken: the reply would be discarded on the way out,
    /// and when a cold briefing failed to send. Resuming is what clears it,
    /// because that is when Jim speaks again, and the line resuming sends
    /// otherwise assumes an interviewer who was here for the whole interview.
    /// A socket resumed from the cold one inherits the debt, since its memory
    /// starts from that cold session.
    pub needs_cold_brief: bool,
    /// A resumed socket replaced a session that owed a reply while the
    /// interview was paused. The reply cannot be asked for then, since it
    /// would be discarded. Hold the raw event, not its formatted request, so
    /// recovery cannot nest briefings. Some(None) owes a reply without a known
    /// event; None owes no reply.
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
    pub judged_code: String,
    pub judged_language: String,
    /// What the judgment in flight was asked for, so a call that fails opens
    /// again only what it read: an editor-only call that failed is retried as
    /// one, at the editor's pace and against its cap, not as a speech call.
    pub judged_for: PhaseJudgeDue,
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
            coding_minutes: 37,
            behavioral_minutes: 8,
            round_transition_seen: false,
            time_warning_seen: false,
            behavioral_round_started: false,
            behavioral_round_transcript_start: 0,
            behavioral_round_prior_turn: None,
            paused: false,
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
            last_test_run: None,
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
            judged_code: String::new(),
            judged_language: String::new(),
            judged_for: PhaseJudgeDue::No,
            interim_code: String::new(),
            earlier_steps_named: Vec::new(),
            earlier_steps_pending: Vec::new(),
            code_shown: String::new(),
            end_requested: false,
            ended: false,
            context_compression: None,
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

/// The REACTO steps the phase judge records. Test is the platform's from the
/// run itself; these are the ones only understanding can tick.
const JUDGED_PHASES: [FrameworkPhase; 5] = [
    FrameworkPhase::Repeat,
    FrameworkPhase::Example,
    FrameworkPhase::Algorithm,
    FrameworkPhase::Coding,
    FrameworkPhase::Optimizations,
];

/// Transcript the judge reads, the latest stretch within the bound. A tail
/// rather than only what is new, so a step an earlier call missed can still be
/// quoted while it is in reach.
const PHASE_JUDGE_WINDOW_BYTES: usize = 6 * 1024;
const PHASE_JUDGE_CODE_BYTES: usize = 6 * 1024;

/// The fewest words a quote may have. A restatement, a worked case or an
/// approach takes a sentence, so six; complexity is short ("it is O of n
/// time") and Coding quotes code, so four. Either way a quote cannot be a
/// stray phrase that happens to appear in a candidate line.
const MIN_SPOKEN_QUOTE_WORDS: usize = 6;
const MIN_QUOTE_WORDS: usize = 4;
/// What the row keeps of the judge's summary and of the quote it was checked
/// against. The quote goes into the row, so the report reads the words that
/// were verified rather than only the judge's account of them.
const MAX_PHASE_JUDGE_SUMMARY_CHARS: usize = 120;
const MAX_PHASE_JUDGE_QUOTE_CHARS: usize = 110;

/// The steps the judge may still record, in checklist spelling. None in the
/// behavioral round, and none about code before there is code.
///
/// Not closed by the end of the interview: the end waits for a judgment of
/// the candidate's last answer, and the report is written after it.
///
/// Repeat, Example and Algorithm are no longer asked about once Coding is
/// recorded. A candidate who skipped one would otherwise keep a call due after
/// every turn for the rest of the session, spending the budget before
/// Optimizations, and what was said before or while the code was written was
/// judged while it was. The interviewer can still record them.
pub fn phase_judge_open(state: &RuntimeState) -> Vec<&'static str> {
    if state.behavioral_round_started {
        return Vec::new();
    }
    let coded = phases_evidenced(state, &[FrameworkPhase::Coding]);
    let unrecorded = JUDGED_PHASES
        .into_iter()
        .filter(|phase| !phases_evidenced(state, &[*phase]))
        .filter(|phase| phase.needs_code() || !coded)
        .collect::<Vec<_>>();

    // Asked last and once: whether there is written work compares the editor
    // with the starter, and nothing about code may still be open. Coding is
    // grounded in typed code, so a whiteboard, where the work is a drawing,
    // leaves it to the interviewer; Optimizations is grounded in what they say
    // about the work, drawn or typed.
    let has_work = unrecorded.iter().any(|phase| phase.needs_code()) && written_work(state);
    let typed = !state.interview_mode.is_whiteboard();
    unrecorded
        .into_iter()
        .filter(|phase| match phase {
            FrameworkPhase::Coding => has_work && typed,
            phase if phase.needs_code() => has_work,
            _ => true,
        })
        .map(phase_id)
        .collect()
}

/// The candidate's latest turn, which the recognizer keeps rewriting in place
/// while they speak, so a judge that read it halfway knows to read it again.
fn last_candidate_line(state: &RuntimeState) -> &str {
    last_speaker_line(&state.transcript, CANDIDATE_SPEAKER).map_or("", |(_, line)| line)
}

/// Why a judgment is due, if one is: for something the candidate said, or for
/// the editor alone. Typing changes the editor on every pause, so a call for it
/// alone is paced and capped apart from the others; see `claim_phase_judge`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseJudgeDue {
    No,
    Spoken,
    EditorOnly,
}

/// Whether there is anything new for the judge: a candidate turn it has not
/// been shown, the latest one grown since, or for an open Coding step, an
/// editor it has not read. The cursors are compared before the open steps are
/// worked out, so an idle tick with nothing new costs two comparisons.
pub fn phase_judge_need(state: &RuntimeState) -> PhaseJudgeDue {
    let from = state.judged_transcript_lines.min(state.transcript.len());
    let spoke = candidate_lines(&state.transcript[from..]) > 0
        || last_candidate_line(state) != state.judged_last_turn;
    let typed = state.code != state.judged_code || state.language != state.judged_language;
    if !spoke && !typed {
        return PhaseJudgeDue::No;
    }
    let open = phase_judge_open(state);
    if spoke && !open.is_empty() {
        PhaseJudgeDue::Spoken
    } else if typed && open.contains(&"coding") {
        PhaseJudgeDue::EditorOnly
    } else {
        PhaseJudgeDue::No
    }
}

/// Whether a judgment is due for any reason.
pub fn phase_judge_due(state: &RuntimeState) -> bool {
    phase_judge_need(state) != PhaseJudgeDue::No
}

/// The judge's prompt, with its cursors moved past what it shows.
///
/// Moved when the call goes out, as the interim window is: a call that fails
/// costs nothing, because the next one reads the same tail.
pub fn take_phase_judge_window(state: &mut RuntimeState, problem: &Problem) -> String {
    // Only the tail is marked, since only the tail is sent.
    let tail = &state.transcript[tail_start(&state.transcript, PHASE_JUDGE_WINDOW_BYTES)..];
    let window = transcript_tail(&mark_unrecognized_turns(tail), PHASE_JUDGE_WINDOW_BYTES);
    state.judged_for = phase_judge_need(state);
    state.judged_transcript_lines = state.transcript.len();
    state.judged_last_turn = last_candidate_line(state).to_string();
    state.judged_code = state.code.clone();
    state.judged_language = state.language.clone();
    let open = phase_judge_open(state);
    let code = if open
        .iter()
        .any(|id| matches!(*id, "coding" | "optimizations"))
    {
        code_head(&state.code, PHASE_JUDGE_CODE_BYTES)
    } else {
        String::new()
    };
    let prompt = phase_judge_prompt(&PhaseJudgeInput {
        problem,
        open: &open,
        transcript_window: &window,
        code: &code,
        language: &state.language,
        interview_mode: state.interview_mode,
    });
    state.evidence_ledger.record_model_input(
        ModelInputKind::Interim,
        &format!("{}\n\n{prompt}", phase_judge_system_instruction()),
    );
    prompt
}

/// Lowercase words, punctuation dropped: a quote is checked for the words the
/// candidate said, not for how the recognizer punctuated them.
fn quote_words(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_lowercase().next().unwrap_or(c)
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// One speaker's turn, as `speaker_turns` reads the transcript.
struct Turn {
    speaker: &'static str,
    /// Its normalized words, padded with a space at each end so a quote is
    /// found as whole words with one `contains`.
    words: String,
}

/// The transcript as turns: adjacent lines of one speaker joined, since the
/// recognizer can split one answer into two lines with nothing between them.
/// An unrecognized line ends a turn without joining one, as it carries no
/// words.
fn speaker_turns(state: &RuntimeState) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    let mut joinable = false;
    for line in &state.transcript {
        let spoken = [CANDIDATE_SPEAKER, INTERVIEWER_SPEAKER]
            .into_iter()
            .find_map(|speaker| speaker_speech(line, speaker).map(|text| (speaker, text)));
        let Some((speaker, text)) = spoken.filter(|_| !is_unrecognized_turn(line)) else {
            joinable = false;
            continue;
        };
        let words = quote_words(text);
        match turns.last_mut() {
            Some(turn) if joinable && turn.speaker == speaker => {
                turn.words.push_str(&words);
                turn.words.push(' ');
            }
            _ => turns.push(Turn {
                speaker,
                words: format!(" {words} "),
            }),
        }
        joinable = true;
    }
    turns
}

/// Whether `quote` is the candidate's own: words from one recognized candidate
/// turn, or for Coding, from what they typed into the editor.
///
/// Proof of who said it, not that the step was done; that judgment stays with
/// the model and its prompt. What can be checked mechanically is checked: the
/// starter is not the candidate's code, and an approach or an analysis the
/// interviewer said before the candidate did is the candidate agreeing, not
/// explaining. The interviewer saying it back afterwards, which is how an
/// interviewer confirms they followed, leaves the candidate's words theirs.
fn quote_is_grounded(
    state: &RuntimeState,
    turns: &[Turn],
    phase: FrameworkPhase,
    quote: &str,
) -> bool {
    let words = quote_words(quote);
    let least = if phase.needs_code() {
        MIN_QUOTE_WORDS
    } else {
        MIN_SPOKEN_QUOTE_WORDS
    };
    if words.split(' ').count() < least {
        return false;
    }
    let needle = format!(" {words} ");
    if phase == FrameworkPhase::Coding {
        let typed = |code: &str| format!(" {} ", quote_words(code)).contains(&needle);
        return typed(&state.code) && !typed(starter(state, &state.language));
    }
    let mut said = turns.iter().filter(|turn| turn.words.contains(&needle));
    let Some(first) = said.clone().next() else {
        return false;
    };
    if !said.any(|turn| turn.speaker == CANDIDATE_SPEAKER) {
        return false;
    }
    let echo = matches!(
        phase,
        FrameworkPhase::Algorithm | FrameworkPhase::Optimizations
    ) && first.speaker == INTERVIEWER_SPEAKER;
    !echo
}

/// Records what the judge found, through the same gate the interviewer's calls
/// pass, and returns the rows it added.
///
/// The judge is a model too, so nothing it says is taken on its word: the step
/// has to be one still open when the answer lands, and the quote has to be the
/// candidate's own words, or for Coding the editor's. A step the interviewer
/// recorded while the call was out is no longer open and is left alone, and a
/// judgment about code the candidate has since rewritten is dropped, since it
/// would anchor an analysis of the old code to the new. That check reads the
/// editor, so it holds for typed code only: a board arrives after every
/// stroke, and refusing a judgment for each would refuse nearly all of them.
pub fn apply_phase_judgment(state: &mut RuntimeState, text: &str) -> Vec<FrameworkEvidence> {
    // A call that failed is answered with nothing, where a judgment is always
    // JSON. Its window was marked read when it went out, so it is opened again
    // here: otherwise the candidate's last answer, read by a call that failed,
    // would wait for another turn that may never come.
    if text.trim().is_empty() {
        if state.judged_for != PhaseJudgeDue::EditorOnly {
            state.judged_transcript_lines = 0;
            state.judged_last_turn.clear();
        }
        state.judged_code.clear();
        return Vec::new();
    }
    let Ok(answer) = serde_json::from_str::<serde_json::Value>(text.trim()) else {
        return Vec::new();
    };
    let Some(steps) = answer
        .get("steps")
        .and_then(serde_json::Value::as_array)
        .filter(|steps| !steps.is_empty())
    else {
        return Vec::new();
    };

    // Read once, as the call was asked: Coding recorded from this answer closes
    // the spoken steps to later calls, not to a restatement listed after it.
    // The transcript and the editor comparison are worked out only when a step
    // needs them, since most answers find nothing.
    let open = phase_judge_open(state);
    let mut turns = None;
    let mut code_current = None;
    let mut recorded = Vec::new();
    for step in steps.iter().take(JUDGED_PHASES.len()) {
        let (Some(id), Some(quote)) = (
            step.get("step").and_then(serde_json::Value::as_str),
            step.get("quote").and_then(serde_json::Value::as_str),
        ) else {
            continue;
        };
        let Some(phase) = JUDGED_PHASES
            .into_iter()
            .find(|phase| phase_id(*phase) == id)
        else {
            continue;
        };
        if !open.contains(&id) || phases_evidenced(state, &[phase]) {
            continue;
        }
        if phase.needs_code()
            && !*code_current.get_or_insert_with(|| {
                let judged = TestedCode {
                    language: state.judged_language.clone(),
                    code: state.judged_code.clone(),
                };
                covers(&judged, &state.language, &state.code)
            })
        {
            continue;
        }
        let turns = turns.get_or_insert_with(|| speaker_turns(state));
        if !quote_is_grounded(state, turns, phase, quote) {
            continue;
        }
        let summary = step
            .get("summary")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|summary| !summary.is_empty());
        let quoted = format!(
            "\"{}\"",
            bounded_model_text(quote.trim(), MAX_PHASE_JUDGE_QUOTE_CHARS)
        );
        let summary = match summary {
            Some(summary) => format!(
                "{} Quote: {quoted}",
                bounded_model_text(summary, MAX_PHASE_JUDGE_SUMMARY_CHARS)
            ),
            None => format!("Quote: {quoted}"),
        };
        let source = if phase == FrameworkPhase::Coding {
            "editor_snapshot"
        } else {
            "candidate_speech"
        };
        if let Ok((evidence, RecordOutcome::Added)) = record_evidence(
            state,
            &serde_json::json!({
                "phase": id,
                "source": source,
                "kind": "observed",
                "confidence": 70,
                "summary": summary,
            }),
            true,
        ) {
            recorded.push(evidence);
        }
    }
    recorded
}

/// One line of what a pause-time reviewer saw, bounded the way a summary is.
///
/// The text is model output and reaches the report prompt, so it is trimmed to
/// one line per observation and cut to a length: an idle-window call that
/// returns a paragraph, or a hundred of them, must not be able to crowd out the
/// transcript it sits beside.
pub fn record_interim_notes(state: &mut RuntimeState, text: &str) {
    let mut taken = 0usize;
    for line in text.lines() {
        // The prompt's own ceiling, enforced rather than trusted. Without it a
        // single degenerate response -- the answer is prose, so nothing but the
        // token cap bounds its line count -- walks the whole session's notes
        // out of the list one `remove(0)` at a time, and the report is then
        // written from one bad pause instead of the interview.
        if taken == MAX_INTERIM_LINES_PER_REVIEW {
            break;
        }

        // A bullet followed by a space, not every leading dash: the prompt asks
        // for "- ", and stripping the character on its own would edit "-1 is
        // the case they missed" down to a different claim.
        let line = line.trim();
        let line = line.strip_prefix("- ").unwrap_or(line).trim();
        if line.is_empty() {
            continue;
        }
        let line = bounded_model_text(line, MAX_INTERIM_LINE_CHARS);
        if state.interim_notes.iter().any(|held| held == &line) {
            continue;
        }
        taken += 1;
        if state.interim_notes.len() == MAX_INTERIM_NOTES {
            state.interim_notes.remove(INTERIM_OPENING_KEPT);
        }
        state.interim_notes.push(line);
    }
}

/// How much the candidate must have added to the template before the editor
/// counts as holding their code: a short expression, not a keystroke. Counted
/// in non-whitespace characters, in order; see `changed_characters`. The same
/// bound, added or removed, is how far the editor may move from a tested
/// snapshot before that run stops covering it, so tuning one tunes both.
const MIN_WRITTEN_CHARS: usize = 5;

/// Cells in the table `changed_characters` fills, a few milliseconds of work.
const MAX_WRITTEN_TABLE: usize = 4_000_000;

/// Whether the editor holds code the candidate wrote.
///
/// The interviewer's say-so is not enough for the phases that are about code.
/// A session once ticked Coding with nothing typed at all, because the model
/// recorded it from what the candidate said they would write.
pub fn code_written(state: &RuntimeState) -> bool {
    written_in(state, &state.language, &state.code)
}

/// The starter the page served for `language`, or nothing for one it did not.
fn starter<'a>(state: &'a RuntimeState, language: &str) -> &'a str {
    state
        .code_templates
        .get(language)
        .map_or("", String::as_str)
}

/// Whether `code` holds code the candidate wrote over the starter of
/// `language`.
pub(crate) fn written_in(state: &RuntimeState, language: &str, code: &str) -> bool {
    let template = starter(state, language);

    // Too long to compare is not written: the shortcut below already credits
    // anything that grew by the threshold, which is all a length can prove.
    written_beyond(template, code).unwrap_or(false)
}

/// Whether `code` adds at least `MIN_WRITTEN_CHARS` of content to `before`, or
/// `None` when the two differ over a stretch too long to compare. Each caller
/// decides which way an unknown answer fails, because the two readings of
/// "written" fail in opposite directions.
fn written_beyond(before: &str, code: &str) -> Option<bool> {
    let before = content_chars(before).collect::<Vec<_>>();
    let code = content_chars(code).collect::<Vec<_>>();

    // A common subsequence is never longer than the starter, so code that
    // outgrows it by the threshold has written that much whatever it kept, and
    // a large paste needs no table.
    if code.len() >= before.len() + MIN_WRITTEN_CHARS {
        return Some(true);
    }
    changed_characters(&before, &code).map(|(added, _)| added >= MIN_WRITTEN_CHARS)
}

/// Whether the runner reported itself unavailable for the language on screen.
fn runner_unavailable_on_screen(state: &RuntimeState) -> bool {
    state.runner_unavailable.as_deref() == Some(state.language.as_str())
}

/// What Test can be recorded from right now, decided once for the evidence
/// gate, the test reactions and the wrap-up refusal, so none of them can tell
/// the interviewer to record something another refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TestSource {
    /// A run of the code on screen: recorded as `test_event`. It wins over an
    /// outage reported after it, because the run happened.
    Run,
    /// No such run, and the runner is reported missing for this language:
    /// the candidate's hand trace, recorded as `candidate_speech`.
    Trace,
    /// Neither: the candidate has to click Run.
    Neither,
    /// A whiteboard, where nothing runs at all: the cases the candidate names
    /// against the drawing are the testing, recorded as `candidate_speech` or
    /// `board_snapshot`. Not `Trace`, which is an outage in an interview that
    /// had a runner, and whose every sentence names one.
    Board,
}

pub(crate) fn test_source(state: &RuntimeState) -> TestSource {
    if state.interview_mode.is_whiteboard() {
        TestSource::Board
    } else if state.code_execution_disabled {
        TestSource::Trace
    } else if tested_code_is_current(state) {
        TestSource::Run
    } else if runner_unavailable_on_screen(state) {
        TestSource::Trace
    } else {
        TestSource::Neither
    }
}

/// Whether Test can be recorded now: it has no row yet, a received run still
/// covers the code on screen, and that code is the candidate's. The one
/// definition both the platform's own recording and the test reaction read.
///
/// Cheapest first, since a code update asks after every keystroke burst until
/// Test is recorded: the row check, then the run's language and code, and only
/// then whether the candidate wrote anything, which compares against the
/// starter.
pub(crate) fn test_recordable(state: &RuntimeState) -> bool {
    // `test_source` answers Board for a whiteboard before it reads any run, so
    // a page that sends editor packets there anyway records no execution: the
    // interview has no test runner, as the evidence gate also holds.
    !phases_evidenced(state, &[FrameworkPhase::Test])
        && test_source(state) == TestSource::Run
        && code_written(state)
}

/// What a test reaction should say about recording Test after a run, given
/// whether that run earned execution credit. RunAgain is only for a run that
/// earned credit the candidate has already typed past. A run that earned
/// nothing is answered by `uncredited_test_results_reaction` instead, so it is
/// Settled here whatever an earlier run left recordable.
pub(crate) fn test_record_after_run(state: &RuntimeState, credited: bool) -> TestRecord {
    if !credited {
        TestRecord::Settled
    } else if test_recordable(state) {
        TestRecord::Record
    } else if !phases_evidenced(state, &[FrameworkPhase::Test]) && code_written(state) {
        TestRecord::RunAgain
    } else {
        TestRecord::Settled
    }
}

/// Whether the editor still holds the code the latest executed run covered:
/// the same language, with less than a short expression added or removed
/// since. Fixing a failing case in place keeps the run; writing the solution
/// after running a stub does not, and neither does deleting a guard the run
/// exercised, which changes what runs without typing anything. A change too
/// long to compare is not current: a same-length rewrite of a long solution
/// would otherwise count as the code that ran.
pub(crate) fn tested_code_is_current(state: &RuntimeState) -> bool {
    state
        .tested_code
        .as_ref()
        .is_some_and(|tested| covers(tested, &state.language, &state.code))
}

/// Outcome claims require identical source and language. The Test gate keeps
/// credit through small edits, but even whitespace or comments can affect
/// literals, indentation, token boundaries or compiler directives. Comparing
/// bytes avoids claiming semantic equivalence without a language parser.
pub(crate) fn tested_code_is_exact(state: &RuntimeState) -> bool {
    state
        .tested_code
        .as_ref()
        .is_some_and(|tested| tested.language == state.language && tested.code == state.code)
}

/// Whether `code` in `language` is still the code `tested` holds, by the Test
/// gate's rule: the same language, with less than a short expression added or
/// removed. The one definition every comparison of two runs, or of a run and
/// the editor, goes through.
pub(crate) fn covers(tested: &TestedCode, language: &str, code: &str) -> bool {
    tested.language == language
        && (tested.code == code || edited_within(language, &tested.code, code))
}

/// Whether fewer than `MIN_WRITTEN_CHARS` were added and fewer removed, from
/// one table: the common subsequence is the same either way round. Comments
/// are left out, so noting the complexity after a run, or deleting the
/// starter's prompt line, does not void a run of code that did not change.
fn edited_within(language: &str, before: &str, code: &str) -> bool {
    let before = uncommented_chars(language, before);
    let code = uncommented_chars(language, code);
    changed_characters(&before, &code)
        .is_some_and(|(added, removed)| added < MIN_WRITTEN_CHARS && removed < MIN_WRITTEN_CHARS)
}

/// The content characters of `code` with its comments left out: `#` to the
/// end of the line in Python, `//` to the end of the line and `/* */` in the
/// other tabs. A quote opens a string until the same quote closes it, past a
/// backslash escape, so a `#` or `//` inside a literal is code; in Python a
/// tripled quote opens a string only the same three close, so a lone quote
/// inside a docstring does not end it. An unterminated
/// string runs to the end and keeps everything after it counted, which errs
/// toward asking for a rerun; an unterminated block comment hides the rest,
/// and what it hid is counted as removed.
fn uncommented_chars(language: &str, code: &str) -> Vec<char> {
    let hash_comments = language == "python";
    let mut kept = Vec::with_capacity(code.len());
    let mut chars = code.chars().peekable();
    // The quote that opened the current string, and whether it was tripled.
    let mut quote: Option<(char, bool)> = None;
    let tripled = |chars: &std::iter::Peekable<std::str::Chars<'_>>, quote: char| {
        let mut ahead = chars.clone();
        ahead.next() == Some(quote) && ahead.next() == Some(quote)
    };
    while let Some(character) = chars.next() {
        if let Some((open, triple)) = quote {
            kept.push(character);
            if character == '\\' {
                kept.extend(chars.next());
            } else if character == open && (!triple || tripled(&chars, open)) {
                if triple {
                    kept.extend(chars.by_ref().take(2));
                }
                quote = None;
            }
            continue;
        }
        match character {
            '"' | '\'' | '`' => {
                let triple = hash_comments && tripled(&chars, character);
                kept.push(character);
                if triple {
                    kept.extend(chars.by_ref().take(2));
                }
                quote = Some((character, triple));
            }
            '#' if hash_comments => while chars.next_if(|&next| next != '\n').is_some() {},
            '/' if !hash_comments && chars.peek() == Some(&'/') => {
                while chars.next_if(|&next| next != '\n').is_some() {}
            }
            '/' if !hash_comments && chars.peek() == Some(&'*') => {
                chars.next();
                let mut previous = '\0';
                for next in chars.by_ref() {
                    if previous == '*' && next == '/' {
                        break;
                    }
                    previous = next;
                }
            }
            _ => kept.push(character),
        }
    }
    kept.retain(|character| !character.is_whitespace());
    kept
}

/// Changed by enough that an analysis of `before` may not describe `code`:
/// another language, more than 80 characters of code added and removed
/// together, comments aside, or a change too long to compare. Counting what
/// changed rather than the net size is what catches a new algorithm of about
/// the same length, which an unchanged size would pass as the same solution.
pub(crate) fn rewritten(before: &TestedCode, language: &str, code: &str) -> bool {
    if before.language != language {
        return true;
    }
    let old = uncommented_chars(language, &before.code);
    let new = uncommented_chars(language, code);
    changed_characters(&old, &new).is_none_or(|(added, removed)| added + removed > 80)
}

/// Records which code a complexity analysis just given describes. One given
/// over a run of the code on screen describes that code. One given ahead of
/// any such run may describe a draft, and finishing the same algorithm is not
/// a rewrite, so the first credited run after it is what it describes.
fn anchor_analysis(state: &mut RuntimeState) {
    state.analysis = Some(if tested_code_is_current(state) {
        Analysis::Of(TestedCode {
            language: state.language.clone(),
            code: state.code.clone(),
        })
    } else {
        Analysis::AwaitingRun
    });
}

/// A test run that ran something: not a runner setup error, and not the 0/0
/// record a packet with no counts is sanitized into.
pub(crate) fn real_test_run(run: &serde_json::Value) -> bool {
    !evidence::run_failed_to_start(run)
        && run
            .get("total")
            .and_then(serde_json::Value::as_i64)
            .is_some_and(|total| total > 0)
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
    state
        .started_at
        .elapsed()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
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

pub fn record_framework_evidence(
    state: &mut RuntimeState,
    args: &serde_json::Value,
) -> Result<FrameworkEvidence, &'static str> {
    record_framework_evidence_outcome(state, args).map(|(evidence, _)| evidence)
}

/// What an accepted evidence call did. A call answered with a row already
/// held added nothing, and telling the model "Recorded" then has it believe a
/// note landed that was dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordOutcome {
    Added,
    /// The same note again.
    AlreadyHeld,
    /// A Test row from a received run, which the platform records itself.
    TestAlreadyRecorded,
}

/// `record_framework_evidence`, also saying what the call did.
pub(crate) fn record_framework_evidence_outcome(
    state: &mut RuntimeState,
    args: &serde_json::Value,
) -> Result<(FrameworkEvidence, RecordOutcome), &'static str> {
    record_evidence(state, args, false)
}

/// The gate both callers pass. `quote_verified` is the phase judge's: its
/// speech row carries a quote the server found in a recognized candidate turn,
/// so a later turn the recognizer garbled says nothing about it.
fn record_evidence(
    state: &mut RuntimeState,
    args: &serde_json::Value,
    quote_verified: bool,
) -> Result<(FrameworkEvidence, RecordOutcome), &'static str> {
    let phase = match args.get("phase").and_then(serde_json::Value::as_str) {
        Some("repeat") => FrameworkPhase::Repeat,
        Some("example") => FrameworkPhase::Example,
        Some("algorithm") => FrameworkPhase::Algorithm,
        Some("coding") => FrameworkPhase::Coding,
        Some("test") => FrameworkPhase::Test,
        Some("optimizations") => FrameworkPhase::Optimizations,
        Some("situation") => FrameworkPhase::Situation,
        Some("task") => FrameworkPhase::Task,
        Some("action") => FrameworkPhase::Action,
        Some("result") => FrameworkPhase::Result,
        _ => return Err("invalid phase"),
    };
    let source = match args.get("source").and_then(serde_json::Value::as_str) {
        Some("candidate_speech") => EvidenceSource::CandidateSpeech,
        Some("editor_snapshot") => EvidenceSource::EditorSnapshot,
        Some("board_snapshot") => EvidenceSource::BoardSnapshot,
        Some("test_event") => EvidenceSource::TestEvent,
        Some("session_timing") => EvidenceSource::SessionTiming,
        _ => return Err("invalid source"),
    };
    let kind = match args.get("kind").and_then(serde_json::Value::as_str) {
        Some("observed") => EvidenceKind::Observed,
        Some("inferred") => EvidenceKind::Inferred,
        Some("skipped") => EvidenceKind::Skipped,
        _ => return Err("invalid kind"),
    };
    if (source == EvidenceSource::SessionTiming) != (kind == EvidenceKind::Skipped) {
        return Err("session_timing is only valid for skipped evidence");
    }

    // A model can invent a round-start event in its own speech. Only the
    // platform transition opens STAR; platform-owned skips bypass this tool.
    if phase.is_star() && !state.behavioral_round_started {
        return Err(
            "the behavioral round has not started; do not ask behavioral questions or record STAR evidence before the trusted round-start event, and return to the coding round",
        );
    }

    // The evidence declaration offers only assessable work: code and tests in
    // coding, drawn work in whiteboard. Coding example drawings remain optional
    // explanation aids even after an image arrives.
    match state.interview_mode {
        InterviewMode::Coding if source == EvidenceSource::BoardSnapshot => {
            return Err("example drawings are explanation aids, not coding assessment evidence");
        }
        InterviewMode::Whiteboard
            if matches!(
                source,
                EvidenceSource::EditorSnapshot | EvidenceSource::TestEvent
            ) =>
        {
            return Err(
                "this interview has no editor and no test runner; record what you saw on the board as board_snapshot",
            );
        }
        _ => {}
    }
    let confidence = args
        .get("confidence")
        .and_then(json_int)
        .ok_or("invalid confidence")?;
    if !(0..=100).contains(&confidence) {
        return Err("invalid confidence");
    }
    let summary = args
        .get("summary")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
        .ok_or("invalid summary")
        .map(|summary| bounded_model_text(summary, MAX_FRAMEWORK_SUMMARY_CHARS))?;

    // The interviewer heard the audio the recognizer garbled, so a summary it
    // writes from that turn ("did not give the indices") would reach the report
    // as the candidate's speech even though the report never reads the turn
    // itself. Speech evidence waits for a turn the report can read, unless its
    // words were found in one.
    if source == EvidenceSource::CandidateSpeech
        && kind != EvidenceKind::Skipped
        && !quote_verified
        && state
            .transcript
            .iter()
            .rev()
            .find(|line| line.starts_with(&format!("{CANDIDATE_SPEAKER}: ")))
            .is_some_and(|line| is_unrecognized_turn(line))
    {
        return Err(
            "the candidate's latest turn was not recognized as English; ask them to repeat it, and record speech evidence from their clarified answer",
        );
    }

    // Execution is already banked at receipt. A delayed model call must not
    // replace its provenance or move its report timestamp to the reply.
    if phase == FrameworkPhase::Test
        && source == EvidenceSource::TestEvent
        && kind != EvidenceKind::Skipped
        && let Some(existing) = state.framework_evidence.iter().find(|item| {
            item.phase == FrameworkPhase::Test
                && item.source == EvidenceSource::TestEvent
                && item.kind != EvidenceKind::Skipped
        })
    {
        return Ok((existing.clone(), RecordOutcome::TestAlreadyRecorded));
    }

    // Coding, Test and Optimizations are all about code, so none of them is
    // reached while the editor holds nothing the candidate wrote: a plan spoken
    // aloud is the Algorithm phase, and testing or improving it comes after
    // there is something to run.
    if phase.needs_code() && kind != EvidenceKind::Skipped && !written_work(state) {
        return Err(match state.interview_mode {
            InterviewMode::Coding => {
                "coding, test and optimizations need code the candidate has written in the editor; read_editor shows none yet"
            }
            InterviewMode::Whiteboard => {
                "coding, test and optimizations need work the candidate has drawn on the board; read_board shows none yet"
            }
        });
    }

    // Before the duplicate check, since a repeat is still the analysis given
    // again, now: "still O(n log n)" for rewritten code comes back under the
    // summary it had before. Nothing below refuses an Optimizations record.
    if phase == FrameworkPhase::Optimizations && kind != EvidenceKind::Skipped {
        anchor_analysis(state);
    }
    if let Some(index) = state.framework_evidence.iter().position(|item| {
        item.phase == phase && item.source == source && item.kind == kind && item.summary == summary
    }) {
        return Ok((
            state.framework_evidence[index].clone(),
            RecordOutcome::AlreadyHeld,
        ));
    }

    // After the duplicate check, so a resumed interviewer repeating a Test it
    // already recorded is answered with that row rather than refused because
    // the candidate has typed since.
    //
    // A verbal trace is useful preparation, but cannot stand in for running the
    // implementation. Test is the run's evidence, so it is recorded from the
    // test event, and only while the editor still holds the code that run
    // executed. This also covers model-inferred completion and a model claiming
    // a test_event the server never received. When execution is disabled or a
    // platform cannot run tests, the candidate's hand trace is the only testing
    // left, and it is recorded as what it is: speech. That outage is the
    // browser's claim, as its pass counts are; a Test row from
    // `candidate_speech` identifies tracing, while the session setting tells it
    // apart from an outage. A run of the code on screen still wins over an
    // outage reported after it.
    if phase == FrameworkPhase::Test && kind != EvidenceKind::Skipped {
        match test_source(state) {
            TestSource::Run if source != EvidenceSource::TestEvent => {
                return Err("record Test with source test_event: it is the run's evidence");
            }
            TestSource::Trace if source != EvidenceSource::CandidateSpeech => {
                return Err(if state.code_execution_disabled {
                    "code execution is disabled, so record Test from a hand trace with source candidate_speech"
                } else {
                    "the runner cannot provide tests, so Test is recorded from the candidate's hand trace, with source candidate_speech"
                });
            }
            TestSource::Neither => {
                return Err(
                    "test evidence requires a received run with executed cases of the code now in the editor; ask the candidate to click Run; the platform records Test when qualifying results arrive",
                );
            }
            // The source was already held to the board's two above.
            TestSource::Run | TestSource::Trace | TestSource::Board => {}
        }
    }

    Ok((
        append_framework_evidence(
            state,
            phase,
            source,
            kind,
            confidence as u8,
            summary,
            crate::current_epoch_millis(),
        ),
        RecordOutcome::Added,
    ))
}

fn append_framework_evidence(
    state: &mut RuntimeState,
    phase: FrameworkPhase,
    source: EvidenceSource,
    kind: EvidenceKind,
    confidence: u8,
    summary: String,
    receipt_timestamp_ms: u64,
) -> FrameworkEvidence {
    if state.framework_evidence.len() == MAX_FRAMEWORK_EVIDENCE {
        evict_one_observation(&mut state.framework_evidence);
    }
    let evidence = FrameworkEvidence {
        at_ms: elapsed_ms(state),
        phase,
        source,
        kind,
        confidence,
        summary,
        framework_version: FRAMEWORK_VERSION,
    };
    state.framework_evidence.push(evidence.clone());
    if kind != EvidenceKind::Skipped {
        state
            .evidence_ledger
            .record_coverage(receipt_timestamp_ms, phase_id(phase));
    }
    evidence
}

/// The summary of the Test row the platform records from a received run.
pub const RECEIVED_TEST_SUMMARY: &str = "Browser reported executed test cases for the candidate's current code; results are unverified.";

/// A received execution needs no model judgment to establish that testing
/// happened. Counts remain browser claims and grant no correctness credit.
///
/// Recorded whenever Test becomes recordable, not only on the run that earned
/// the credit: a run credited while the editor showed another language's
/// starter becomes the code on screen again when the candidate switches back,
/// and leaving that one to the model put Test back in the hands that missed it.
/// Every event that can make it recordable asks here, so no prompt has to tell
/// the model to record Test itself.
pub(crate) fn reconcile_received_test(
    state: &mut RuntimeState,
    receipt_timestamp_ms: u64,
) -> Option<FrameworkEvidence> {
    (!state.ended && test_recordable(state)).then(|| {
        append_framework_evidence(
            state,
            FrameworkPhase::Test,
            EvidenceSource::TestEvent,
            EvidenceKind::Observed,
            100,
            RECEIVED_TEST_SUMMARY.to_string(),
            receipt_timestamp_ms,
        )
    })
}

/// Closes every STAR step that holds no row as skipped for session timing.
///
/// The platform's own bookkeeping, done here rather than asked of the model.
/// The five-minute warning and the wrap-up used to tell the interviewer to call
/// `record_framework_evidence` once per missing step before speaking, which is
/// up to four tool round trips in front of the two moments the candidate is
/// listening hardest, and a skip only when the model complied. A candidate
/// who ended the session themselves got no wrap-up and so no skips at all.
/// Any row closes a step, a skip included, so the warning and the end never
/// write two skips for one step. Skips never reach the ledger's coverage, the
/// rule `record_framework_evidence` applies to them as well.
///
/// A coding-only interview has no STAR steps to close. Skipping them there
/// would list a round it never had and, with the ledger full, evict a coding
/// observation to make room.
pub(crate) fn skip_unassessed_star(state: &mut RuntimeState, summary: &str) {
    if state.interview_loop == InterviewLoop::CodingOnly {
        return;
    }
    let at_ms = state
        .started_at
        .elapsed()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64;
    for phase in [
        FrameworkPhase::Situation,
        FrameworkPhase::Task,
        FrameworkPhase::Action,
        FrameworkPhase::Result,
    ] {
        if state
            .framework_evidence
            .iter()
            .any(|item| item.phase == phase)
        {
            continue;
        }
        if state.framework_evidence.len() == MAX_FRAMEWORK_EVIDENCE {
            evict_one_observation(&mut state.framework_evidence);
        }
        state.framework_evidence.push(FrameworkEvidence {
            at_ms,
            phase,
            source: EvidenceSource::SessionTiming,
            kind: EvidenceKind::Skipped,
            confidence: 100,
            summary: summary.to_string(),
            framework_version: FRAMEWORK_VERSION,
        });
    }
}

/// Whether the coding round is finished on evidence rather than on the clock.
///
/// Test and Optimizations, both observed or inferred and neither skipped: a
/// candidate who ran their cases and justified their complexity has reached the
/// end of REACTO, and one who did not has not, whatever the timer says. Three
/// callers ask this same question -- whether to open the behavioral round,
/// whether a report may call the coding round complete, and whether the
/// interviewer may close a two-round session -- and they were three copies of
/// the same closure, which is three chances for the gate to mean something
/// slightly different in each.
pub(crate) fn coding_round_complete(state: &RuntimeState) -> bool {
    phases_evidenced(
        state,
        &[FrameworkPhase::Test, FrameworkPhase::Optimizations],
    )
}

/// Past the coding gate in a session that keeps going anyway: a coding-only
/// session's time belongs to the candidate until the timer or their End, so
/// every prompt that would wrap up a two-round coding round asks this instead.
pub(crate) fn coding_continues_past_gate(state: &RuntimeState) -> bool {
    state.interview_loop == InterviewLoop::CodingOnly && coding_round_complete(state)
}

/// Every one of these phases observed or inferred, none of them skipped.
///
/// The phase list is the parameter because the rule is not: the coding gate and
/// the behavioral one differ only in which phases they name, and writing the
/// `any`-inside-`all` out per caller is how there came to be four readings of
/// "this round is finished" across three files.
pub(crate) fn phases_evidenced(state: &RuntimeState, phases: &[FrameworkPhase]) -> bool {
    phases.iter().all(|phase| {
        state
            .framework_evidence
            .iter()
            .any(|item| item.phase == *phase && item.kind != EvidenceKind::Skipped)
    })
}

/// The phases this interview has evidence for, in the id spelling the browser
/// ticks off.
///
/// Derived rather than accumulated: the evidence list is already the record,
/// and a second counter beside it would be one restart away from disagreeing
/// with the report built from the same list. Carries no summary, confidence or
/// source, because this is the only framework state the candidate is allowed to
/// see and any of those would leak how they are being read.
pub fn framework_progress(state: &RuntimeState) -> Vec<&'static str> {
    let mut phases = Vec::new();
    for evidence in &state.framework_evidence {
        // Skipped is the record of a phase the candidate never reached, which a
        // session that times out writes for everything still outstanding.
        // Ticking those would hand out a full checklist for running out of
        // time, which is the opposite of what the marks are for.
        if evidence.kind == EvidenceKind::Skipped {
            continue;
        }
        let phase = phase_id(evidence.phase);
        if !phases.contains(&phase) {
            phases.push(phase);
        }
    }
    phases
}

/// The behavioral half of the phase ids, named here beside `phase_id` because
/// that is where a reader renaming one of them will be standing.
/// `star_complete`
/// in livekit.rs asks the same question of the enum, which the compiler checks;
/// this is for the callers that already hold the ids as strings.
pub const STAR_PHASE_IDS: [&str; 4] = ["situation", "task", "action", "result"];

/// The coding half, for the callers that need the same question the other way
/// round.
pub const REACTO_PHASE_IDS: [&str; 6] = [
    "repeat",
    "example",
    "algorithm",
    "coding",
    "test",
    "optimizations",
];

pub(crate) const fn phase_id(phase: FrameworkPhase) -> &'static str {
    match phase {
        FrameworkPhase::Repeat => "repeat",
        FrameworkPhase::Example => "example",
        FrameworkPhase::Algorithm => "algorithm",
        FrameworkPhase::Coding => "coding",
        FrameworkPhase::Test => "test",
        FrameworkPhase::Optimizations => "optimizations",
        FrameworkPhase::Situation => "situation",
        FrameworkPhase::Task => "task",
        FrameworkPhase::Action => "action",
        FrameworkPhase::Result => "result",
    }
}

/// The wire spelling of a source, shared by the browser's row and the report
/// prompt's line so the two cannot come to disagree about what to call one.
pub(crate) const fn evidence_source_id(source: EvidenceSource) -> &'static str {
    match source {
        EvidenceSource::CandidateSpeech => "candidate_speech",
        EvidenceSource::EditorSnapshot => "editor_snapshot",
        EvidenceSource::BoardSnapshot => "board_snapshot",
        EvidenceSource::TestEvent => "test_event",
        EvidenceSource::SessionTiming => "session_timing",
    }
}

pub(crate) const fn evidence_kind_id(kind: EvidenceKind) -> &'static str {
    match kind {
        EvidenceKind::Observed => "observed",
        EvidenceKind::Inferred => "inferred",
        EvidenceKind::Skipped => "skipped",
    }
}

pub fn framework_evidence_json(evidence: &FrameworkEvidence) -> serde_json::Value {
    let phase = phase_id(evidence.phase);
    let source = evidence_source_id(evidence.source);
    let kind = evidence_kind_id(evidence.kind);
    serde_json::json!({
        "atMs": evidence.at_ms,
        "phase": phase,
        "source": source,
        "kind": kind,
        "confidence": evidence.confidence,
        "summary": evidence.summary,
        "frameworkVersion": evidence.framework_version,
    })
}

/// Count a hint and, when the candidate asked for it, hand out the next rung.
///
/// The last rung names the key step, so it waits until the candidate has an
/// approach of their own: an Algorithm phase observed from what they said, or
/// Coding evidence, which the server already refuses without code they wrote.
/// An Algorithm phase the interviewer only inferred does not count, since that
/// is the model's word alone. Until then the interviewer gets no clue and is
/// told to ask what the candidate would try; the request is not counted,
/// because the candidate was given nothing.
pub fn record_hint(state: &mut RuntimeState, requested: bool) -> String {
    // One reading for the call, whichever of the branches below answers it.
    let receipt_timestamp_ms = crate::current_epoch_millis();
    let clue = requested
        .then(|| state.hint_ladder.get(state.hint_rungs_given))
        .flatten();
    if clue.is_some() {
        let last = state.hint_rungs_given + 1 == state.hint_ladder.len();
        let approach_stated = state.framework_evidence.iter().any(|item| {
            item.phase == FrameworkPhase::Algorithm && item.kind == EvidenceKind::Observed
        }) || phases_evidenced(state, &[FrameworkPhase::Coding]);
        if last && !approach_stated {
            state
                .evidence_ledger
                .record_hint(receipt_timestamp_ms, requested, false);
            return hint_rung_withheld_text(state.hints_used);
        }
    }
    if !requested {
        state.volunteered_hints = state.volunteered_hints.saturating_add(1);
    }
    state.hints_used = state.hints_used.saturating_add(1);
    match clue {
        Some(clue) => {
            state.hint_rungs_given += 1;
            state
                .evidence_ledger
                .record_hint(receipt_timestamp_ms, requested, true);
            hint_rung_text(state.hints_used, state.hint_rungs_given, clue)
        }
        None if requested => {
            state
                .evidence_ledger
                .record_hint(receipt_timestamp_ms, requested, false);
            hint_ladder_used_text(state.hints_used)
        }
        None => {
            state
                .evidence_ledger
                .record_hint(receipt_timestamp_ms, requested, true);
            log_hint_text(state.hints_used)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataConfig {
    pub problem: &'static Problem,
    pub duration_min: u32,
    pub interview_loop: InterviewLoop,
    pub interview_mode: InterviewMode,
    pub profile: InterviewProfile,
    pub grounding: InterviewGrounding,
    /// The candidate hid the worked examples in the preflight.
    pub examples_hidden: bool,
    pub code_execution_disabled: bool,
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
    /// Some only when the pause state genuinely changed, so a browser asking
    /// twice for what it already has publishes nothing.
    pub pause_changed: Option<bool>,
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

/// How much conversation the report prompt may carry. A 90-minute interview
/// runs to roughly 40k characters of speech, so this never fires for a real
/// session; it exists so a pathological one cannot grow the prompt without a
/// ceiling. The tail is what matters to a grader: the closing reasoning, not
/// the opening pleasantries.
///
/// Bytes, because that is what bounds the request. A transcript in a language
/// that spends three bytes a character therefore carries fewer characters, not
/// more bytes than it can afford.
pub const MAX_TRANSCRIPT_BYTES: usize = 60_000;

/// Joins the session transcript for the report prompt, keeping the most recent
/// entries when the whole thing would not fit.
pub fn transcript_for_report(lines: &[String]) -> String {
    transcript_tail(&mark_unrecognized_turns(lines), MAX_TRANSCRIPT_BYTES)
}

/// What an assessment reads in place of a candidate turn the recognizer did
/// not return as English. The report prompt names it, so a change here that
/// left the prompt describing the old marker fails
/// `prompts_match_frozen_fixture`.
pub const UNRECOGNIZED_TURN: &str = "(this turn was not recognized as English and is left out)";

/// `lines` with every candidate turn written mostly in a non-Latin script
/// replaced by `UNRECOGNIZED_TURN`, for the passes that assess the candidate.
///
/// The interview is in English, so such a turn is the recognizer's output
/// rather than what the candidate said: fluent Mandarin about clothing sizes
/// came back for an English answer about delimiters. Shown the text, the
/// report model cited it, and once told not to, still rewrote it into an
/// improvement about staying on topic, however each wording was refused. A
/// turn it never reads cannot be coached. The live interviewer, the replay and
/// the stored transcript keep what the recognizer produced; only assessment is
/// spared it.
///
/// Most of the letters, not any: a sentence that quotes a string in another
/// script is still English, and accented Latin is still Latin. English that
/// the recognizer turned into Spanish or into unrelated English stays, and the
/// prompts' uncertainty rules are what cover it.
pub fn mark_unrecognized_turns(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            if is_unrecognized_turn(line) {
                format!("{CANDIDATE_SPEAKER}: {UNRECOGNIZED_TURN}")
            } else {
                line.clone()
            }
        })
        .collect()
}

/// Whether the candidate has said anything the recognizer did not return
/// mostly in a non-Latin script. A turn `mark_unrecognized_turns` hides is not
/// evidence that the discussion of the exercise has begun.
pub(crate) fn has_recognized_candidate_turn(lines: &[String]) -> bool {
    lines
        .iter()
        .any(|line| candidate_speech(line).is_some_and(|speech| !mostly_non_latin(speech)))
}

/// Whether `line` is a candidate turn `mark_unrecognized_turns` hides.
fn is_unrecognized_turn(line: &str) -> bool {
    candidate_speech(line).is_some_and(mostly_non_latin)
}

/// What the candidate said in `line`, if it is their turn.
fn candidate_speech(line: &str) -> Option<&str> {
    speaker_speech(line, CANDIDATE_SPEAKER)
}

/// What `speaker` said in `line`, if it is their turn.
fn speaker_speech<'a>(line: &'a str, speaker: &str) -> Option<&'a str> {
    line.strip_prefix(speaker)
        .and_then(|rest| rest.strip_prefix(": "))
}

/// More than two non-Latin letters as well as a majority, so a turn that is
/// only a symbol the question is about, "theta" come back as a Greek letter,
/// survives: the prompts call that Unicode, not a recognition error, and a
/// hidden turn gives them nothing to apply it to. The sentence-length turns the
/// recognizer invents carry many more.
fn mostly_non_latin(speech: &str) -> bool {
    let (latin, other) = speech
        .chars()
        .filter(|character| character.is_alphabetic())
        .fold((0usize, 0usize), |(latin, other), character| {
            // ASCII, Latin-1 and Latin Extended-A/B, and Latin Extended
            // Additional, where Vietnamese lives.
            if character.is_ascii_alphabetic()
                || ('\u{00C0}'..='\u{024F}').contains(&character)
                || ('\u{1E00}'..='\u{1EFF}').contains(&character)
            {
                (latin + 1, other)
            } else {
                (latin, other + 1)
            }
        });
    other > latin && other > 2
}

/// The newest entries that fit in `budget` bytes, joined.
///
/// `budget` bounds the entries, and `EARLIER_OMITTED` is added above them when
/// any were dropped, so a trimmed tail is that much longer than the budget.
/// The same trade as `code_head`: the notice is what stops the reader taking
/// the opening as missing, and paying for it in dropped conversation would be
/// the wrong economy.
///
/// Two callers want the same tail against different ceilings: the report prompt
/// above, and the briefing a cold-restarted interviewer is rebuilt from. The
/// beginning is the least useful part to lose in both, and the notice below is
/// what stops either reader from taking the opening as missing rather than
/// dropped. Empty in, empty out: naming that case is the caller's, because a
/// report says nothing about it and a restart has to.
pub fn transcript_tail(lines: &[String], budget: usize) -> String {
    let start = tail_start(lines, budget);
    let mut kept = lines[start..]
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    if start > 0 {
        // One line can outrun the whole budget on its own. Dropping it would
        // answer a cold restart with nothing but the notice, so keep the end of
        // it: that is the part the conversation stopped in the middle of.
        if kept.is_empty() {
            kept.push(tail_within(&lines[start - 1], budget));
        }
        kept.insert(0, EARLIER_OMITTED);
    }
    kept.join("\n")
}

/// What stands in for the lines a bounded tail dropped, so a reader takes the
/// opening as cut rather than missing.
pub(crate) const EARLIER_OMITTED: &str = "(earlier conversation omitted)";

/// The first of the whole lines `transcript_tail` keeps, each paid for with
/// its separator. A cold restart asks this rather than counting for itself,
/// so what it believes the replacement can see is what the replacement is
/// shown.
fn tail_start(lines: &[String], mut budget: usize) -> usize {
    for (index, line) in lines.iter().enumerate().rev() {
        match budget.checked_sub(line.len() + 1) {
            Some(remaining) => budget = remaining,
            None => return index + 1,
        }
    }
    0
}

/// The longest suffix of `line` that starts on a character boundary and fits in
/// `budget` bytes.
///
/// Written as that sentence rather than as a walk from `len - budget`, which is
/// the same answer arrived at by arithmetic that has two ways to be wrong: a
/// step in the other direction also lands on a boundary and quietly returns
/// more than `budget`, and a step that fails to advance never terminates. The
/// suffix a candidate speaking a three-byte-per-character language gets is up
/// to two bytes shorter than one speaking ASCII, which is the cost of the slice
/// being valid.
fn tail_within(line: &str, budget: usize) -> &str {
    line.char_indices()
        .map(|(start, _)| &line[start..])
        .find(|suffix| suffix.len() <= budget)
        .unwrap_or("")
}

pub fn spoken_minutes_from_remaining_seconds(remaining_seconds: i64) -> i64 {
    ((remaining_seconds as f64) / 60.0)
        .round_ties_even()
        .max(1.0) as i64
}

/// What the candidate's countdown reads, in whole minutes.
///
/// Wall time, because the page's deadline is wall time too: a pause stops the
/// conversation and not the clock, as `tickTimer` in `web/interview.js` says
/// at the other end of the same decision.
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

/// The one sentence any prompt says the time in, so a model told these are the
/// only readings it has can recognize every one of them.
pub fn timer_line(minutes: i64) -> String {
    format!("TIMER: about {minutes} minutes remain on the candidate's countdown.")
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

    MetadataConfig {
        problem,
        duration_min,
        interview_loop,
        interview_mode,
        profile,
        grounding,
        examples_hidden,
        code_execution_disabled,
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
            duration.clamp(MIN_DURATION_MIN.into(), MAX_DURATION_MIN.into()) as u32
        })
}

#[cfg(test)]
#[path = "../tests/unit/agent.rs"]
mod tests;

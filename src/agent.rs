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
#[cfg(test)]
pub(crate) use evidence::MAX_PARSED_BYTES;
pub(crate) use evidence::{ModelInputKind, ViewFor};
pub(crate) use evidence::{analyze_code, analyze_code_cached, observe_code, observe_code_cached};
use integrity::integrity_hash;
pub use integrity::{sanitize_integrity_event, sanitize_test_run};
use problems::variant_for;
pub use problems::{DEFAULT_PROBLEM_ID, PROBLEMS, find_problem, get_problem, topics_for};
pub use prompts::{
    InterimReviewInput, LanguageChoiceContext, MAX_EXCERPT_LINE_CHARS, MAX_NUMBERED_BYTES,
    ReportPromptInput, behavioral_silence_nudge, behavioral_time_warning,
    build_instructions_for_plan, changed_excerpt, cold_restart, format_test_run, greeting,
    hint_ladder_used_text, hint_rung_text, hint_rung_withheld_text, interim_review_prompt,
    interim_system_instruction, language_choice, log_hint_text, numbered, numbered_from,
    proactive_review, read_editor_text, released_follow_ups, report_prompt,
    report_system_instruction, resume, rolling_assessment, round_skipped, round_started,
    silence_nudge, spoken_language, test_results_reaction, test_setup_error_reaction, time_warning,
    unrecorded_earlier_phases, wrap_up,
};
pub(crate) use report::sanitize_report_candidate;
pub use report::{
    MAX_SUMMARY_TEXT, fallback_report, final_report, names_published_problem,
    report_response_schema, spelled_words, validate_report, validate_report_candidate,
};

// Only the tests read this, and a report the filter emptied is the one place it
// is written, so exporting it unconditionally is an unused name in a release
// build.
#[cfg(test)]
pub(crate) use report::SELF_REVIEW_REPLACEMENT;
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

pub const INTERVIEW_CONTRACT_BUNDLE_VERSION: u32 = 16;
pub const LIVE_PROMPT_VERSION: u32 = 8;
pub const REPORT_PROMPT_VERSION: u32 = 13;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptItem<'a> {
    pub item_type: &'a str,
    pub role: &'a str,
    pub text: &'a str,
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

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeState {
    pub started_at: std::time::Instant,
    pub interview_loop: InterviewLoop,
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
    pub transcript: Vec<String>,
    pub last_test_run: Option<serde_json::Value>,
    pub test_runs: u32,
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
    /// cannot simply be spoken: the reply would be discarded on the way out.
    /// Resuming is what clears it, because that is when Jim speaks again, and
    /// the line resuming sends otherwise assumes an interviewer who was here
    /// for the whole interview.
    pub needs_cold_brief: bool,
    /// Observations a reviewer recorded in the pauses, while the interview was
    /// still running. Held apart from `framework_evidence`, which is the
    /// interviewer's own bookkeeping about which phase happened: these are the
    /// reading of it, and only the final report consumes them.
    pub interim_notes: Vec<String>,
    /// How many transcript lines an idle-window reviewer has already been
    /// shown. The window it gets is everything after this, so a pause that
    /// arrives with nothing new said costs no call at all.
    pub interim_transcript_lines: usize,
    /// The editor the last idle-window review was sent. A review whose code
    /// has not changed since says so instead of sending up to four kilobytes
    /// a note on record already read; like the transcript cursor, it moves
    /// when the call goes out.
    pub interim_code: String,
    /// The earlier steps an evidence reply has already named as open, so each
    /// is named once; see `unrecorded_earlier_phases`.
    pub earlier_steps_named: Vec<&'static str>,
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
}

impl RuntimeState {
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
            coding_minutes: 37,
            behavioral_minutes: 8,
            round_transition_seen: false,
            time_warning_seen: false,
            behavioral_round_started: false,
            behavioral_round_transcript_start: 0,
            behavioral_round_prior_turn: None,
            paused: false,
            framework_evidence: Vec::new(),
            evidence_ledger: EvidenceLedger::default(),
            code: String::new(),
            last_parseable_code: std::collections::BTreeMap::new(),
            parse_cache: ParseCache::default(),
            code_edited: false,
            code_templates: std::collections::BTreeMap::new(),
            language: "python".to_string(),
            language_chosen: false,
            transcript: Vec::new(),
            last_test_run: None,
            test_runs: 0,
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
            interim_notes: Vec::new(),
            interim_transcript_lines: 0,
            interim_code: String::new(),
            earlier_steps_named: Vec::new(),
            code_shown: String::new(),
            end_requested: false,
            ended: false,
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
/// The head and not the tail, unlike a transcript: code is read from the top,
/// and the signature and the approach are what a reviewer needs. Nothing
/// bounds `state.code` on the way in -- `apply_code_update` appends whatever
/// the browser sent -- so a large paste would otherwise be re-sent whole to
/// every idle-window review, each of which has twelve seconds to answer.
pub fn code_head(code: &str, budget: usize) -> String {
    if code.len() <= budget {
        return code.to_string();
    }

    // A bounded search rather than a decrementing loop. Both walk back to the
    // same byte -- index 0 is always a character boundary, so neither can run
    // off the front, and `budget` indexes the string by the early return above
    // -- but a loop that advances by hand can be made not to advance, and that
    // is a hang rather than a wrong answer. The mutation gate reports a hang as
    // a timeout, which is neither a pass nor a finding; a search over a range
    // cannot be turned into one.
    let end = (0..=budget)
        .rev()
        .find(|end| code.is_char_boundary(*end))
        .unwrap_or_default();
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceSource {
    CandidateSpeech,
    EditorSnapshot,
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
/// in non-whitespace characters, in order; see `added_characters`.
const MIN_WRITTEN_CHARS: usize = 5;

/// Cells in the table `added_characters` fills, a few milliseconds of work.
const MAX_WRITTEN_TABLE: usize = 4_000_000;

/// Whether the editor holds code the candidate wrote.
///
/// The interviewer's say-so is not enough for the phases that are about code.
/// A session once ticked Coding with nothing typed at all, because the model
/// recorded it from what the candidate said they would write.
pub fn code_written(state: &RuntimeState) -> bool {
    let template = state
        .code_templates
        .get(&state.language)
        .map_or("", String::as_str);
    let template = content_chars(template).collect::<Vec<_>>();
    let code = content_chars(&state.code).collect::<Vec<_>>();

    // A common subsequence is never longer than the starter, so code that
    // outgrows it by the threshold has written that much whatever it kept, and
    // a large paste needs no table.
    code.len() >= template.len() + MIN_WRITTEN_CHARS
        || added_characters(&template, &code) >= MIN_WRITTEN_CHARS
}

/// The characters of a piece of code that are content rather than layout.
pub(crate) fn content_chars(code: &str) -> impl Iterator<Item = char> + '_ {
    code.chars().filter(|character| !character.is_whitespace())
}

/// The characters of `code` outside its longest common subsequence with
/// `template`: what was typed, in order, with nothing credited for deletions.
///
/// In order because an unordered count lets a deletion cancel an addition. The
/// starters carry a "think out loud" comment, and deleting it used to cancel
/// most of a one-line answer, so `return sqrt(x);` did not count as code. The
/// shared prefix and suffix, the signature and its closing lines, are trimmed
/// first so the quadratic table covers only the body that changed; this runs
/// on an evidence call, a few times an interview.
fn added_characters(template: &[char], code: &[char]) -> usize {
    let prefix = template
        .iter()
        .zip(code)
        .take_while(|(expected, written)| expected == written)
        .count();
    let (template, code) = (&template[prefix..], &code[prefix..]);
    let suffix = template
        .iter()
        .rev()
        .zip(code.iter().rev())
        .take_while(|(expected, written)| expected == written)
        .count();
    let (template, code) = (
        &template[..template.len() - suffix],
        &code[..code.len() - suffix],
    );

    // A bound on the table rather than on the packets: past it, count only what
    // the length alone proves was added. Server starters are a few hundred
    // characters, so a real buffer never gets here; a forged one cannot make an
    // evidence call stall the room.
    if template.len().saturating_mul(code.len()) > MAX_WRITTEN_TABLE {
        return code.len().saturating_sub(template.len());
    }
    let mut previous = vec![0usize; template.len() + 1];
    let mut current = previous.clone();
    for written in code {
        for (at, expected) in template.iter().enumerate() {
            current[at + 1] = if written == expected {
                previous[at] + 1
            } else {
                current[at].max(previous[at + 1])
            };
        }
        std::mem::swap(&mut previous, &mut current);
    }
    code.len() - previous[template.len()]
}

pub fn record_framework_evidence(
    state: &mut RuntimeState,
    args: &serde_json::Value,
) -> Result<FrameworkEvidence, &'static str> {
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

    // Coding, Test and Optimizations are all about code, so none of them is
    // reached while the editor holds nothing the candidate wrote: a plan spoken
    // aloud is the Algorithm phase, and testing or improving it comes after
    // there is something to run.
    let about_code = matches!(
        phase,
        FrameworkPhase::Coding | FrameworkPhase::Test | FrameworkPhase::Optimizations
    );
    if about_code && kind != EvidenceKind::Skipped && !code_written(state) {
        return Err(
            "coding, test and optimizations need code the candidate has written in the editor; read_editor shows none yet",
        );
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
    if let Some(index) = state.framework_evidence.iter().position(|item| {
        item.phase == phase && item.source == source && item.kind == kind && item.summary == summary
    }) {
        return Ok(state.framework_evidence[index].clone());
    }
    if state.framework_evidence.len() == MAX_FRAMEWORK_EVIDENCE {
        evict_one_observation(&mut state.framework_evidence);
    }
    state.framework_evidence.push(FrameworkEvidence {
        at_ms: state
            .started_at
            .elapsed()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64,
        phase,
        source,
        kind,
        confidence: confidence as u8,
        summary,
        framework_version: FRAMEWORK_VERSION,
    });
    let evidence = state
        .framework_evidence
        .last()
        .expect("just appended evidence")
        .clone();

    // `at_ms` is milliseconds since the interview started, which is what the
    // report renders. The ledger stamps receipts with the epoch clock every
    // other entry uses, so passing the elapsed value here put a third scale in
    // a field that is read as one timeline. Read once for the call, so a second
    // entry added here later shares this one's reading rather than taking its
    // own, which is the rule the packet path already follows.
    let receipt_timestamp_ms = crate::current_epoch_millis();
    if evidence.kind != EvidenceKind::Skipped {
        state
            .evidence_ledger
            .record_coverage(receipt_timestamp_ms, phase_id(evidence.phase));
    }
    Ok(evidence)
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
pub(crate) fn skip_unassessed_star(state: &mut RuntimeState, summary: &str) {
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
/// interviewer may close the session -- and they were three copies of the same
/// closure, which is three chances for the gate to mean something slightly
/// different in each.
pub(crate) fn coding_round_complete(state: &RuntimeState) -> bool {
    phases_evidenced(
        state,
        &[FrameworkPhase::Test, FrameworkPhase::Optimizations],
    )
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
    pub profile: InterviewProfile,
    pub grounding: InterviewGrounding,
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
    /// Agent-owned round transition result: `started` or `skipped`.
    pub round_changed: Option<&'static str>,
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
    transcript_tail(lines, MAX_TRANSCRIPT_BYTES)
}

/// The newest entries that fit in `budget` bytes, joined.
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

pub fn format_transcript(items: &[TranscriptItem<'_>]) -> String {
    items
        .iter()
        .filter(|item| item.item_type == "message")
        .filter_map(|item| {
            if item.text.trim().is_empty() || item.text.starts_with("[SYSTEM EVENT]") {
                return None;
            }
            let text = item.text.trim();
            let speaker = if item.role == "assistant" {
                INTERVIEWER_SPEAKER
            } else {
                CANDIDATE_SPEAKER
            };
            Some(format!("{speaker}: {text}"))
        })
        .collect::<Vec<_>>()
        .join("\n")
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
    let profile = sanitize_interview_profile(value.get("interviewProfile"));
    let grounding = sanitize_interview_grounding(value.get("interviewGrounding"));

    MetadataConfig {
        problem,
        duration_min,
        interview_loop,
        profile,
        grounding,
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

pub fn test_reaction_decision(
    ended: bool,
    since_last_test_reaction_seconds: f64,
) -> TestReactionDecision {
    if ended || since_last_test_reaction_seconds < TEST_REACTION_COOLDOWN_S {
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

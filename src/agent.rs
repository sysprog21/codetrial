//! Interview runtime: session state, timing decisions, and report shaping.
//!
//! The bulky, separable concerns live in submodules and are re-exported here,
//! so callers keep importing from `crate::agent` and this file stays about
//! behavior rather than data.

mod integrity;
mod problems;
mod prompts;

use integrity::integrity_hash;
pub use integrity::{sanitize_integrity_event, sanitize_test_run};
pub use problems::{DEFAULT_PROBLEM_ID, PROBLEMS, get_problem};
pub use prompts::{
    LanguageChoiceContext, ReportPromptInput, build_instructions, build_instructions_for_mode,
    format_test_run, greeting, language_choice, log_hint_text, numbered, proactive_review,
    read_editor_text, report_prompt, significant_change, silence_nudge, spoken_language,
    test_results_reaction, time_warning, wrap_up,
};

use crate::config::{DEFAULT_DURATION_MIN, MAX_DURATION_MIN, MIN_DURATION_MIN};
use crate::runtime::{TOPIC_CODE_UPDATE, TOPIC_CONTROL, TOPIC_INTEGRITY, TOPIC_TEST_RESULTS};

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
pub const WATCH_TICK_S: f64 = 2.0;
pub const SILENCE_THRESHOLD_S: f64 = 15.0;
pub const TEST_REACTION_COOLDOWN_S: f64 = 20.0;
pub const SILENCE_COOLDOWN_S: f64 = 30.0;
pub const REVIEW_INTERVAL_S: f64 = 30.0;
pub const INTERJECTION_COOLDOWN_S: f64 = 45.0;
pub const SPEECH_SETTLE_S: f64 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Problem {
    pub id: &'static str,
    pub title: &'static str,
    pub difficulty: &'static str,
    pub summary: &'static str,
    pub optimal: &'static str,
    pub pitfalls: &'static str,
    pub hint_ladder: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InterviewMode {
    Practice,
    #[default]
    Scored,
}

impl InterviewMode {
    pub fn parse(value: Option<&str>) -> Self {
        match value {
            Some("practice") => Self::Practice,
            _ => Self::Scored,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Practice => "practice",
            Self::Scored => "scored",
        }
    }
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
    pub fn segment_id(&self, speaker: &str) -> String {
        format!("{speaker}-{}", self.index)
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
    pub mode: InterviewMode,
    pub paused: bool,
    pub code: String,
    pub language: String,
    pub transcript: Vec<String>,
    pub last_test_run: Option<serde_json::Value>,
    pub test_runs: u32,
    pub hints_used: u32,
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
    pub ended: bool,
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            mode: InterviewMode::Scored,
            paused: false,
            code: String::new(),
            language: "python".to_string(),
            transcript: Vec::new(),
            last_test_run: None,
            test_runs: 0,
            hints_used: 0,
            integrity_events: Vec::new(),
            integrity_chain: None,
            integrity_first_heartbeat: None,
            integrity_last_heartbeat: None,
            ended: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetadataConfig {
    pub problem: &'static Problem,
    pub duration_min: u32,
    pub mode: InterviewMode,
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
    pub sync_code_at_last_review: bool,
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
    /// Some only when a practice-mode pause state genuinely changed.
    pub pause_changed: Option<bool>,
}

/// How much conversation the report prompt may carry. A 90-minute interview
/// runs to roughly 40k characters of speech, so this never fires for a real
/// session; it exists so a pathological one cannot grow the prompt without a
/// ceiling. The tail is what matters to a grader: the closing reasoning, not
/// the opening pleasantries.
pub const MAX_TRANSCRIPT_CHARS: usize = 60_000;

/// Joins the session transcript for the report prompt, keeping the most recent
/// entries when the whole thing would not fit.
pub fn transcript_for_report(lines: &[String]) -> String {
    let total = lines.iter().map(|line| line.len() + 1).sum::<usize>();
    if total <= MAX_TRANSCRIPT_CHARS {
        return lines.join("\n");
    }
    let mut kept = Vec::new();
    let mut budget = MAX_TRANSCRIPT_CHARS;
    for line in lines.iter().rev() {
        let Some(remaining) = budget.checked_sub(line.len() + 1) else {
            break;
        };
        budget = remaining;
        kept.push(line.as_str());
    }
    kept.reverse();
    // Said out loud so the grader does not read the opening as missing.
    format!("(earlier conversation omitted)\n{}", kept.join("\n"))
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
                "Interviewer"
            } else {
                "Candidate"
            };
            Some(format!("{speaker}: {text}"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// An evaluation that could not be produced is not an evaluation of zero.
///
/// This used to emit `codingScore: 0`, `communicationScore: 0` and
/// `decision: "NO_HIRE"`, so a Gemini outage reached the candidate as a
/// rejection with 0/100 on both axes, and was then saved to their history and
/// synced to `/api/reports`. The `error: true` flag it set alongside was
/// dropped by the browser's own sanitizer, so nothing downstream could tell the
/// difference between a failed provider and a failed candidate. `incomplete`
/// is the shape the browser already renders as "No evaluation", with no
/// verdict, no scores and no badge, and it is the only flag: an `error: true`
/// that travelled beside it reached no consumer and was a second name for the
/// same fact.
pub fn fallback_report(hints_used: u32, note: &str) -> serde_json::Value {
    serde_json::json!({
        "incomplete": true,
        "summary": format!(
            "The automatic evaluation could not be completed: {note}. Your session ran end-to-end, but no scores were produced, so nothing here is an assessment of your work. Check the agent logs and GOOGLE_API_KEY, then try again."
        ),
        "hintsUsed": hints_used,
    })
}

pub fn sanitize_report(raw: &serde_json::Value, hints_used: u32) -> serde_json::Value {
    serde_json::json!({
        "codingScore": clamp_score(raw.get("codingScore")),
        "communicationScore": clamp_score(raw.get("communicationScore")),
        "decision": if raw.get("decision").and_then(serde_json::Value::as_str) == Some("HIRE") {
            "HIRE"
        } else {
            "NO_HIRE"
        },
        "summary": value_string(raw.get("summary")).unwrap_or_default(),
        "codingFeedback": feedback(raw.get("codingFeedback")),
        "communicationFeedback": feedback(raw.get("communicationFeedback")),
        "hintsUsed": hints_used,
    })
}

pub fn spoken_minutes_from_remaining_seconds(remaining_seconds: i64) -> i64 {
    ((remaining_seconds as f64) / 60.0)
        .round_ties_even()
        .max(1.0) as i64
}

pub fn parse_participant_metadata(metadata: Option<&str>) -> MetadataConfig {
    let value = metadata
        .and_then(|metadata| serde_json::from_str::<serde_json::Value>(metadata).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let problem = get_problem(value.get("problemId").and_then(serde_json::Value::as_str));
    let duration_min = duration_from_metadata(value.get("durationMin"));
    let mode = InterviewMode::parse(value.get("mode").and_then(serde_json::Value::as_str));

    MetadataConfig {
        problem,
        duration_min,
        mode,
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
            sync_code_at_last_review: false,
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
            sync_code_at_last_review: true,
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

pub fn apply_data_event(
    state: &mut RuntimeState,
    topic: &str,
    payload: &serde_json::Value,
    since_last_test_reaction_seconds: f64,
) -> DataEventResult {
    if state.paused && matches!(topic, TOPIC_CODE_UPDATE | TOPIC_TEST_RESULTS) {
        return DataEventResult::default();
    }
    match topic {
        TOPIC_CODE_UPDATE => apply_code_update(state, payload),
        TOPIC_TEST_RESULTS => apply_test_results(state, payload, since_last_test_reaction_seconds),
        TOPIC_CONTROL => apply_control(state, payload),
        TOPIC_INTEGRITY => apply_integrity(state, payload),
        _ => DataEventResult::default(),
    }
}

pub fn final_report(
    raw_report: Option<&serde_json::Value>,
    hints_used: u32,
    error_note: Option<&str>,
) -> serde_json::Value {
    match (raw_report, error_note) {
        (Some(raw_report), None) => sanitize_report(raw_report, hints_used),
        _ => fallback_report(hints_used, error_note.unwrap_or("report generation failed")),
    }
}

/// The id and its spoken form, or nothing at all when the browser sent
/// something the tabs do not offer.
fn offered_language(language: &str) -> Option<(&str, &'static str)> {
    spoken_language(language).map(|spoken| (language, spoken))
}

fn apply_code_update(state: &mut RuntimeState, payload: &serde_json::Value) -> DataEventResult {
    // A packet with no string `code` is a malformed packet, not an empty
    // editor. Defaulting to "" meant one of those wiped the authoritative
    // buffer, and the buffer is what the report is written from.
    let new_code = payload.get("code").and_then(serde_json::Value::as_str);

    // Editor packets arrive several times a second and are usually identical to
    // the last one, so only touch the buffer when the text actually moved.
    let update_last_code_change = new_code.is_some_and(|code| code != state.code);
    if let Some(code) = new_code.filter(|_| update_last_code_change) {
        state.code.clear();
        state.code.push_str(code);
    }

    // A language switch is silent from the interviewer's side: the click swaps
    // the editor buffer and publishes the same code topic as any keystroke. The
    // greeting asks the candidate to pick one, so leaving the pick
    // unacknowledged makes the interviewer look like they missed it.
    //
    // Only a real change speaks. The browser publishes its starting language on
    // connect, which matches the default and must not produce a confirmation
    // for a choice nobody made. Only a language the tabs actually offer may
    // change the state or reach a prompt. The id comes from the candidate's
    // browser and is interpolated into Gemini's instructions, so an unvalidated
    // one is a way to write into them.
    let mut language_changed = None;
    if let Some(spoken) = payload
        .get("language")
        .and_then(serde_json::Value::as_str)
        .filter(|language| *language != state.language)
        .and_then(offered_language)
    {
        state.language = spoken.0.to_string();
        let context = if state.code.trim().is_empty() {
            LanguageChoiceContext::Start
        } else {
            LanguageChoiceContext::SwitchWithCode
        };
        language_changed = Some(language_choice(spoken.1, context));
    }

    DataEventResult {
        update_last_code_change,

        // Counts as an interjection: it is the interviewer speaking unprompted,
        // and without this a candidate trying two tabs in a row is talked over
        // twice while the cooldown thinks nothing has been said.
        update_last_interjection: language_changed.is_some(),
        generate_reply: language_changed,
        ..DataEventResult::default()
    }
}

/// # What a passing run means
///
/// Nothing, on its own. `passed` and `total` are counted in the candidate's
/// browser against a judge the candidate's browser holds, and
/// `agent/integrity.rs` already says why that makes them an account rather than
/// evidence: the reporter is the adversary. Believing the count here is a
/// deliberate product choice, not an oversight. The interviewer reacts to what
/// the candidate says happened, the same way a human interviewer reacts to
/// "that one passes" without rerunning it.
///
/// So the numbers are safe to narrate and unsafe to score. What carries weight
/// is the code itself, which the agent receives over the code topic and can
/// read, and the room state it observes for itself. Anything that grades rather
/// than converses has to judge server-side, against code the server holds.
fn apply_test_results(
    state: &mut RuntimeState,
    payload: &serde_json::Value,
    since_last_test_reaction_seconds: f64,
) -> DataEventResult {
    // Bounded before it is stored, not before it is rendered. Both readers of
    // `last_test_run` put it in front of a model, so the sanitized value has to
    // be the only one that exists past this line.
    let payload = &sanitize_test_run(payload);

    // The sanitizer reports "no run happened" as null, and the caller has to
    // honor that or the refusal to invent a run is undone one line later:
    // counting it and reacting to it is what telling the report a run occurred
    // looks like from here.
    if payload.is_null() {
        return DataEventResult::default();
    }
    state.last_test_run = Some(payload.clone());
    state.test_runs += 1;

    let decision = test_reaction_decision(state.ended, since_last_test_reaction_seconds);
    if !decision.react {
        return DataEventResult::default();
    }

    let all_passed = payload
        .get("setupError")
        .is_none_or(|value| !python_truthy(value))
        && payload
            .get("total")
            .and_then(serde_json::Value::as_i64)
            .is_some_and(|total| total > 0)
        && payload.get("passed").and_then(serde_json::Value::as_i64)
            == payload.get("total").and_then(serde_json::Value::as_i64);
    let summary = format_test_run(Some(payload), state.test_runs);

    DataEventResult {
        update_last_code_change: false,
        update_last_test_reaction: true,
        update_last_interjection: true,
        generate_reply: Some(test_results_reaction(&summary, all_passed)),
        finish_interview: None,
        ..DataEventResult::default()
    }
}

fn apply_control(state: &mut RuntimeState, payload: &serde_json::Value) -> DataEventResult {
    match payload.get("type").and_then(serde_json::Value::as_str) {
        Some("pause_interview") if state.mode == InterviewMode::Practice && !state.ended => {
            let paused = payload
                .get("paused")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            if paused == state.paused {
                return DataEventResult::default();
            }
            state.paused = paused;
            DataEventResult {
                pause_changed: Some(paused),
                generate_reply: Some(if paused {
                    "The practice interview is paused. I will wait until you resume.".to_string()
                } else {
                    "The practice interview has resumed. Continue with your REACTO step."
                        .to_string()
                }),
                ..DataEventResult::default()
            }
        }
        Some("time_warning") if !state.ended && !state.paused => {
            let remaining_seconds = payload
                .get("remainingSeconds")
                .and_then(json_int)
                .unwrap_or(300);
            DataEventResult {
                generate_reply: Some(time_warning(spoken_minutes_from_remaining_seconds(
                    remaining_seconds,
                ) as u32)),
                ..DataEventResult::default()
            }
        }
        Some("end_interview") if !state.ended => {
            if !payload.get("code").is_some_and(serde_json::Value::is_null)
                && let Some(code) = payload.get("code").and_then(serde_json::Value::as_str)
            {
                state.code = code.to_string();
            }

            // Read independently of the code. Nesting it meant a payload whose
            // code was absent or null also discarded the language, and the
            // report prompt was then written against whatever language the
            // session last happened to record. Validated for the same reason as
            // the code path: this string reaches the report prompt.
            if let Some((id, _)) = payload
                .get("language")
                .and_then(serde_json::Value::as_str)
                .and_then(offered_language)
            {
                state.language = id.to_string();
            }
            state.ended = true;
            DataEventResult {
                finish_interview: Some(
                    payload
                        .get("reason")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("candidate_ended")
                        .to_string(),
                ),
                ..DataEventResult::default()
            }
        }
        _ => DataEventResult::default(),
    }
}

fn apply_integrity(state: &mut RuntimeState, payload: &serde_json::Value) -> DataEventResult {
    let Some(event) = sanitize_integrity_event(payload) else {
        return DataEventResult::default();
    };
    let seq = event.get("seq").and_then(serde_json::Value::as_u64);
    let prev_hash = event.get("prevHash").and_then(serde_json::Value::as_str);
    let (expected_seq, expected_prev_hash) = state
        .integrity_chain
        .as_ref()
        .map_or((1, ""), |(seq, hash)| {
            (seq.saturating_add(1), hash.as_str())
        });

    let computed_hash = integrity_hash(&event);

    // Named, because a rejection here is permanent. Sequence continuity is
    // required, so `expected_seq` never advances past a refused event and every
    // later one is refused for the same reason. Four different conditions used
    // to return the same silent default, and a hash mismatch caused by this
    // process normalizing `detail` before rehashing it emptied the evidence
    // section of every camera interview without a line anywhere saying so.
    //
    // Only tampering belongs here. A retention decision is handled below, after
    // the cursor has moved, because punishing the producer for this process
    // running out of room is how a full buffer used to end the chain.
    let refusal = if seq != Some(expected_seq) {
        Some(format!("sequence expected {expected_seq}, got {seq:?}"))
    } else if prev_hash != Some(expected_prev_hash) {
        Some("previous hash does not match the last accepted event".to_string())
    } else if event.get("hash").and_then(serde_json::Value::as_str) != Some(computed_hash.as_str())
    {
        Some("hash mismatch; the producer and this verifier disagree".to_string())
    } else {
        None
    };
    if let Some(reason) = refusal {
        eprintln!(
            "WARNING: integrity event refused, chain stops here: {reason} (seq={seq:?}, type={})",
            event
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("?")
        );
        return DataEventResult::default();
    }

    // The chain is satisfied, so the cursor moves whatever happens to the event
    // itself. Everything past this line is about what is worth keeping, and
    // none of it can refuse the next event.
    //
    // Built from the values the checks above already proved equal, rather than
    // re-read from the event. A conditional here would have a branch that
    // silently leaves the cursor behind, which is the failure this separation
    // exists to remove.
    state.integrity_chain = Some((expected_seq, computed_hash));

    // Liveness, not evidence, and now stored as such. Keeping only the ends of
    // the run is what lets the evidence buffer be a plain queue again.
    if is_heartbeat(&event) {
        if state.integrity_first_heartbeat.is_none() {
            state.integrity_first_heartbeat = Some(event);
        } else {
            state.integrity_last_heartbeat = Some(event);
        }
        return DataEventResult::default();
    }

    if !valid_review_sources(state, &event) {
        eprintln!(
            "WARNING: integrity event dropped from the evidence, chain continues: review event \
             references an event no longer retained (seq={seq:?})"
        );
        return DataEventResult::default();
    }

    state.integrity_events.push(event);

    // Plain oldest-out, because nothing periodic lands here any more. This used
    // to hold heartbeats too, and at one every five seconds they filled all 25
    // slots in about two minutes: a camera that stopped at minute three was
    // gone from a thirty minute report by minute five.
    if state.integrity_events.len() > MAX_INTEGRITY_EVENTS {
        let excess = state.integrity_events.len() - MAX_INTEGRITY_EVENTS;
        state.integrity_events.drain(0..excess);
    }
    DataEventResult::default()
}

/// Liveness rather than evidence, which is what sends it to its own field
/// instead of competing for a slot in the buffer.
fn is_heartbeat(event: &serde_json::Value) -> bool {
    event.get("type").and_then(serde_json::Value::as_str) == Some("INTEGRITY_HEARTBEAT")
}

pub(crate) fn valid_review_sources(state: &RuntimeState, event: &serde_json::Value) -> bool {
    if event.get("type").and_then(serde_json::Value::as_str) != Some("REVIEW_EVENT") {
        return true;
    }
    let Some(ids) = event
        .get("sourceEventIds")
        .and_then(serde_json::Value::as_array)
    else {
        return false;
    };
    if ids.is_empty() {
        return false;
    }
    ids.iter().all(|id| {
        let Some(seq) = id.as_str().and_then(|value| value.parse::<u64>().ok()) else {
            return false;
        };
        state
            .integrity_events
            .iter()
            .any(|stored| stored.get("seq").and_then(serde_json::Value::as_u64) == Some(seq))
    })
}

pub(crate) fn duration_from_metadata(value: Option<&serde_json::Value>) -> u32 {
    value
        .filter(|value| python_truthy(value))
        .and_then(json_int)
        .map_or(DEFAULT_DURATION_MIN, |duration| {
            duration.clamp(MIN_DURATION_MIN.into(), MAX_DURATION_MIN.into()) as u32
        })
}

/// Python-compatible coercion of a JSON scalar to a number. The browser has
/// historically sent durations and scores as numbers, numeric strings, and
/// booleans, so both the token endpoint and the agent decode them this way.
pub fn json_number(value: &serde_json::Value) -> Option<f64> {
    match value {
        serde_json::Value::Number(number) => number.as_f64(),
        serde_json::Value::String(text) => text.trim().parse().ok(),
        serde_json::Value::Bool(value) => Some(f64::from(*value)),
        _ => None,
    }
}

pub(crate) fn json_int(value: &serde_json::Value) -> Option<i64> {
    json_number(value).map(|number| number as i64)
}

pub(crate) fn feedback(value: Option<&serde_json::Value>) -> serde_json::Value {
    let object = value.and_then(serde_json::Value::as_object);
    serde_json::json!({
        "strengths": string_list(object.and_then(|object| object.get("strengths"))),
        "improvements": string_list(object.and_then(|object| object.get("improvements"))),
    })
}

pub(crate) fn string_list(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(serde_json::Value::as_array)
        .map(|items| items.iter().take(4).map(python_string).collect())
        .unwrap_or_default()
}

pub(crate) fn clamp_score(value: Option<&serde_json::Value>) -> i64 {
    value.and_then(json_int).unwrap_or(0).clamp(0, 100)
}

pub(crate) fn truthy_string(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .filter(|value| python_truthy(value))
        .map(python_string)
}

pub(crate) fn value_string(value: Option<&serde_json::Value>) -> Option<String> {
    value.map(python_string)
}

pub(crate) fn python_truthy(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::Number(number) => number.as_f64() != Some(0.0),
        serde_json::Value::String(text) => !text.is_empty(),
        serde_json::Value::Array(items) => !items.is_empty(),
        serde_json::Value::Object(fields) => !fields.is_empty(),
    }
}

pub(crate) fn python_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "None".to_string(),
        serde_json::Value::Bool(true) => "True".to_string(),
        serde_json::Value::Bool(false) => "False".to_string(),
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(items) => format!(
            "[{}]",
            items.iter().map(python_repr).collect::<Vec<_>>().join(", ")
        ),
        serde_json::Value::Object(fields) => format!(
            "{{{}}}",
            fields
                .iter()
                .map(|(key, value)| format!("'{key}': {}", python_repr(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

pub(crate) fn python_repr(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => format!("'{text}'"),
        value => python_string(value),
    }
}

#[cfg(test)]
mod tests {
    use super::{python_repr, python_string, python_truthy};
    use serde_json::json;

    /// These render the values a candidate's Python assertion failed on, so an
    /// empty list has to read as `[]` and false, not as `None` or true. Kept
    /// here rather than in `tests/`, which would mean making three internal
    /// helpers public to reach them.
    #[test]
    fn python_coercion_matches_the_interpreter() {
        for falsy in [
            json!(null),
            json!(false),
            json!(0),
            json!(""),
            json!([]),
            json!({}),
        ] {
            assert!(!python_truthy(&falsy), "{falsy} should be falsy");
        }
        for truthy in [
            json!(true),
            json!(42.5),
            json!("hello"),
            json!([1]),
            json!({"k": 1}),
        ] {
            assert!(python_truthy(&truthy), "{truthy} should be truthy");
        }

        assert_eq!(python_string(&json!(null)), "None");
        assert_eq!(python_string(&json!(true)), "True");
        assert_eq!(python_string(&json!(false)), "False");
        assert_eq!(python_string(&json!(42.5)), "42.5");
        assert_eq!(python_string(&json!("hello")), "hello");
        assert_eq!(
            python_string(&json!([1, "a", null, true])),
            "[1, 'a', None, True]"
        );

        // The difference that matters: `repr` quotes a string, `str` does not.
        assert_eq!(python_repr(&json!("hello")), "'hello'");
        assert_eq!(python_repr(&json!(42.5)), "42.5");
    }
}

//! Deterministic evidence reducers for the interview runtime.
//!
//! This module intentionally records facts, not conclusions about a candidate.
//! It stores hashes and bounded counts instead of code, diagnostics, or speech,
//! so the ledger can be replayed without becoming another prompt transcript.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const LEDGER_VERSION: u32 = 1;
const MAX_LEDGER_ENTRIES: usize = 256;
const MAX_CONVERSATION_SEQUENCE: usize = 16;
const MAX_CODE_DIGEST_HISTORY: usize = 16;
/// How many changed node kinds one edit may record. A paste over the template
/// changes dozens of kinds at once, and the list is kept twice, in
/// `code.last_analysis` and in the entry. No prompt carries it; the ledger and
/// its replay do.
const MAX_CHANGED_NODE_FACTS: usize = 12;
/// Qualifies a node kind that was found inside a closure, so that the facts can
/// tell a lambda's parameter list from the enclosing function's. Both are
/// spelled `formal_parameters` in Java and `parameter_list` in C++, and reading
/// them as one kind made a sort comparator a signature change.
const CLOSURE_PREFIX: &str = "closure.";
/// The largest buffer the parser is asked to read. The analysis runs inline on
/// the agent's task, and with the tree cache an edit costs about 1 ms at fifty
/// lines and 12 ms at five hundred, measured optimized; an interview's code is
/// well inside that, so the work stays inline rather than behind a worker that
/// would have to put results back in event order. What stays unbounded is a
/// paste, and past this size one is recorded as not parsed instead.
const MAX_PARSED_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    BrowserClaim,
    ServerObservation,
    Derived,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ObservationFamily {
    Code,
    Test,
    Diagnostic,

    // Who held the floor, which is the fact. Filing every interviewer turn as a
    // question and every candidate turn as an answer read a role into the
    // exchange that the runtime never observed: a candidate asking what an
    // empty input should return was recorded as answering, and the
    // interviewer's reply to them as asking.
    InterviewerTurn,
    CandidateTurn,
    Hint,
    Lifecycle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeObservation {
    ParserUnavailable,
    SyntaxInvalid,
    Parsed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeChangeClass {
    FormattingOnly,
    CommentOnly,
    IdentifierOnly,
    Expression,
    ControlFlow,
    DataStructure,
    FunctionInterface,
}

impl CodeChangeClass {
    /// What the model is told: whether the edit was layout, a comment, only
    /// identifiers, or the code itself: distinctions the review gate and the
    /// parse can back, where the finer class is a heuristic.
    pub(crate) fn coarse(&self) -> &'static str {
        match self {
            Self::FormattingOnly => "formatting",
            Self::CommentOnly => "comment",
            Self::IdentifierOnly => "identifier",
            Self::Expression
            | Self::ControlFlow
            | Self::DataStructure
            | Self::FunctionInterface => "code",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeAnalysis {
    pub observation: CodeObservation,
    pub classification: Option<CodeChangeClass>,
    pub semantic_change: bool,
    pub changed_nodes: Vec<String>,
}

impl CodeAnalysis {
    /// A parse result with nothing diffed behind it, so no classification and
    /// no semantic change to claim.
    pub(crate) fn observed(observation: CodeObservation) -> Self {
        Self {
            observation,
            classification: None,
            semantic_change: false,
            changed_nodes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceEntry {
    pub sequence: u64,
    pub family: ObservationFamily,
    pub provenance: Provenance,
    /// The browser timestamp when supplied. It is a claim, not an ordering key.
    pub source_timestamp_ms: Option<u64>,
    /// When the server received it, read from the server's clock. The one
    /// timestamp here that is not the candidate's word for it, which is why it
    /// is never taken from the packet; a replay supplies the recorded value and
    /// gets the recorded ledger back.
    pub receipt_timestamp_ms: u64,
    pub observation: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CodeState {
    pub language: String,
    pub revision: u64,
    /// Buffers the candidate changed, as opposed to the templates the browser
    /// publishes on connect and on every tab switch.
    pub candidate_edits: u64,
    pub semantic_revision: u64,
    pub parser_observation: Option<CodeObservation>,
    pub latest_digest: String,
    /// Bounded raw-free history used only to make revisiting an earlier editor
    /// state observable as a reversal. Entries are digests, never source, and
    /// each covers the language as well as the buffer: the history is shared
    /// by every tab, so a digest of the text alone read an emptied Java editor
    /// as the candidate undoing back to the Python one they had emptied
    /// before. They are therefore not `latest_digest`, which is the buffer's.
    pub digest_history: Vec<String>,
    pub last_analysis: Option<CodeAnalysis>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TestState {
    pub total: u32,
    pub passed: u32,
    pub failed: u32,
    pub newly_passed: i32,
    pub newly_failed: i32,
    /// Whether an executed run came before this one, without which the two
    /// differences above are measured against nothing.
    pub follows_executed_run: bool,
    pub regression: bool,
    pub consecutive_identical_failures: u32,
    pub failure_signature: Option<String>,
    pub source_timestamp_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCategory {
    Syntax,
    Type,
    Linker,
    RuntimeSignal,
    Timeout,
    Warning,
    Other,
}

impl DiagnosticCategory {
    /// The closed list a runner may name, in the words the wire spells them.
    ///
    /// One reading for both readers: the sanitizer that decides what survives
    /// ingest and the reducer that counts it. They each spelled the list out,
    /// and they drifted once already, the sanitizer allowing `other` while the
    /// reducer dropped it, so the reducer honored six of the seven categories
    /// the wire allows and nothing said so.
    ///
    /// Read through the derive above rather than a second list of the same
    /// words, which the compiler could not hold to the enum: a variant added
    /// there would have serialized fine and been dropped on the way in.
    pub(crate) fn from_wire(category: &str) -> Option<Self> {
        use serde::de::value::{Error, StrDeserializer};
        Self::deserialize(StrDeserializer::<Error>::new(category)).ok()
    }
}

/// # Why a zero is not written down
///
/// A category is counted only when a runner named it from structured execution
/// state, and the browser half names three of these: `timeout`,
/// `runtime_signal` and `other`. A compile failure carries no structure that
/// says whether it was a syntax error, a type error or a link error, so it is
/// counted as `other` rather than guessed at.
///
/// That makes a written-out `"syntax":0` a claim nothing measured. A model
/// reading the projection took it as one, and a C session that failed to
/// compile five times in a row was described as having made no syntax errors.
/// Skipping the zeros leaves the model with the counts that were observed and
/// no sentence about the ones that were not.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DiagnosticState {
    #[serde(skip_serializing_if = "is_zero")]
    pub syntax: u32,
    #[serde(skip_serializing_if = "is_zero")]
    pub type_errors: u32,
    #[serde(skip_serializing_if = "is_zero")]
    pub linker: u32,
    #[serde(skip_serializing_if = "is_zero")]
    pub runtime_signal: u32,
    #[serde(skip_serializing_if = "is_zero")]
    pub timeout: u32,
    #[serde(skip_serializing_if = "is_zero")]
    pub warnings: u32,
    #[serde(skip_serializing_if = "is_zero")]
    pub other: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_signature: Option<String>,
}

fn is_zero(count: &u32) -> bool {
    *count == 0
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct InterviewProgress {
    /// Milestones, each latching once reached: this struct answers what has
    /// happened at some point in the interview, never what is true of the
    /// buffer or the last run right now. `tests` is where the current state
    /// lives, and a reader that takes a mark here for the present will read a
    /// solution that passed once and has since regressed as passing.
    pub first_edit_ms: Option<u64>,
    pub runner_attempted: bool,
    pub tested: bool,
    pub test_progress: bool,
    pub all_tests_passed_once: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StuckSignals {
    pub repeated_failure_count: u32,
    pub compile_or_runner_failure_count: u32,
    pub edit_test_cycles: u32,
    pub undo_redo_cycles: u32,
    pub last_meaningful_change_ms: Option<u64>,
    pub last_test_progress_ms: Option<u64>,
    pub test_progress_latency_ms: Option<u64>,
}

/// Operational measurements retained with the session, never evidence sent
/// back to a model. They measure logical input construction, not provider
/// billing tokens or transport retries.
///
/// Every text this server hands a model is counted in exactly one of these. A
/// set that covered the watch loop and left the greeting, the cold restart,
/// the replies a data event produces and the wrap-up uncounted was not a
/// measurement of what the session costs, it was a measurement of the one send
/// site that happened to be instrumented, and it read low by whatever the
/// interviewer actually said.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EvidenceMetrics {
    pub raw_events_received: u64,
    pub code_events_emitted: u64,
    pub watch_prompt_count: u32,
    pub watch_prompt_bytes: u64,
    /// The conversational sends that are not the watch loop: the greeting, a
    /// cold restart, the reply a data event asks for, and the wrap-up. One
    /// counter rather than four, because they are one channel and the split
    /// nobody would use is a split nobody can keep right.
    pub turn_prompt_count: u32,
    pub turn_prompt_bytes: u64,
    pub interim_prompt_count: u32,
    pub interim_prompt_bytes: u64,
    pub final_report_prompt_count: u32,
    pub final_report_prompt_bytes: u64,
    pub read_editor_calls: u32,
    pub read_editor_bytes: u64,
    /// Every other tool answer: the hint, the recorded evidence, the
    /// end-of-interview reply and a refusal of any of them. Kept apart from
    /// `read_editor`, which carries the candidate's whole buffer and is the one
    /// worth watching on its own.
    pub tool_response_count: u32,
    pub tool_response_bytes: u64,
    /// Every text above folded into one SHA-256, in the order it was handed
    /// over, kind by kind. Two runs of the same session send the same prompts
    /// exactly when these match, which is the check a replay needs and the
    /// byte counts cannot give: two different prompts of one length count
    /// the same. Empty until something is sent.
    pub model_input_digest: String,
}

impl EvidenceMetrics {
    /// What the session spent, for an operator, in the `codetrial <event>
    /// key=value` shape the rest of the runtime writes its machine-readable
    /// lines in.
    ///
    /// A counter nothing reads is not a measurement. Every model-bound byte was
    /// counted, carried through the session and then dropped with the state
    /// that held it, so the numbers existed for the tests that asserted on them
    /// and for nothing else.
    ///
    /// Built as a value rather than printed, so a test can read it: the one
    /// part of a line written with `eprintln!` alone that nothing can check is
    /// the format string, which is the whole of what this is. Destructured
    /// rather than passed positionally for the same reason -- twelve `{}` in
    /// bytes/count pairs put the correctness of the line in the argument order,
    /// where swapping two adjacent ones compiles and reads wrong.
    ///
    /// Stderr and never the report or the projection, for the reason the
    /// counters exist under: a model handed its own byte count is being told
    /// something no interview should turn on.
    pub(crate) fn cost_line(&self) -> String {
        let Self {
            raw_events_received,
            code_events_emitted,
            watch_prompt_count,
            watch_prompt_bytes,
            turn_prompt_count,
            turn_prompt_bytes,
            interim_prompt_count,
            interim_prompt_bytes,
            final_report_prompt_count,
            final_report_prompt_bytes,
            read_editor_calls,
            read_editor_bytes,
            tool_response_count,
            tool_response_bytes,
            model_input_digest: _,
        } = self;
        let total = watch_prompt_bytes
            + turn_prompt_bytes
            + interim_prompt_bytes
            + final_report_prompt_bytes
            + read_editor_bytes
            + tool_response_bytes;
        format!(
            "codetrial model_input_bytes total={total} \
             watch={watch_prompt_bytes}/{watch_prompt_count} \
             turn={turn_prompt_bytes}/{turn_prompt_count} \
             interim={interim_prompt_bytes}/{interim_prompt_count} \
             report={final_report_prompt_bytes}/{final_report_prompt_count} \
             read_editor={read_editor_bytes}/{read_editor_calls} \
             tool={tool_response_bytes}/{tool_response_count} \
             events_received={raw_events_received} code_events={code_events_emitted}"
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelInputKind {
    Watch,
    Turn,
    Interim,
    FinalReport,
    ReadEditor,
    ToolResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HintState {
    pub current_level: u32,
    pub requested: u32,
    pub volunteered: u32,
    pub withheld: u32,
    pub last_hint_sequence: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LifecycleState {
    pub paused: bool,
    pub ended: bool,
    pub behavioral_round_started: bool,
    pub transitions: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleTransition {
    Paused,
    Resumed,
    BehavioralStarted,
    Ended,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CoverageState {
    pub covered: Vec<String>,
    pub uncovered: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ConversationState {
    pub interviewer_turns: u32,
    pub candidate_turns: u32,
    pub recent_sequence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceLedger {
    pub version: u32,
    pub next_sequence: u64,
    /// Where `stuck.edit_test_cycles` was last counted from. Ledger state
    /// rather than prompt state: it is bookkeeping for one counter and says
    /// nothing a model should read.
    pub candidate_edits_at_last_test: u64,
    pub entries: Vec<EvidenceEntry>,
    pub code: CodeState,
    pub tests: TestState,
    pub diagnostics: DiagnosticState,
    pub progress: InterviewProgress,
    pub stuck: StuckSignals,
    pub hints: HintState,
    pub lifecycle: LifecycleState,
    pub coverage: CoverageState,
    pub conversation: ConversationState,
    pub metrics: EvidenceMetrics,
}

impl Default for EvidenceLedger {
    fn default() -> Self {
        Self {
            version: LEDGER_VERSION,
            next_sequence: 1,
            candidate_edits_at_last_test: 0,
            entries: Vec::new(),
            code: CodeState::default(),
            tests: TestState::default(),
            diagnostics: DiagnosticState::default(),
            progress: InterviewProgress::default(),
            stuck: StuckSignals::default(),
            hints: HintState::default(),
            lifecycle: LifecycleState::default(),
            coverage: CoverageState::default(),
            conversation: ConversationState::default(),
            metrics: EvidenceMetrics::default(),
        }
    }
}

impl EvidenceLedger {
    /// The sequence of the newest entry, which a caller keeps to ask
    /// [`Self::prompt_view`] for what arrived after it.
    pub(crate) fn last_sequence(&self) -> u64 {
        self.next_sequence.saturating_sub(1)
    }

    /// What the model is told about the session: a few lines of plain text,
    /// raw-free and bounded by construction rather than by truncation.
    ///
    /// It used to be the ledger itself as JSON, capped at 6,000 bytes. Measured
    /// with Gemini's own tokenizer that was 1,000 to 2,700 tokens a prompt,
    /// more than half of it SHA-256 digests the model can do nothing with, and
    /// the rest per-entry bookkeeping that the aggregates below already state.
    /// A watch prompt carried six times what pasting the code had. The ledger
    /// keeps every entry, digest and timestamp for replay; the model gets the
    /// facts those entries add up to, with elapsed time relative to the latest
    /// event so the text depends on nothing but the ledger.
    ///
    /// `since` is the last sequence an earlier prompt of the same kind showed.
    /// A Live session keeps its earlier turns, so what changed since then is a
    /// line of its own; the state lines are always complete, because the
    /// session compresses old context away and a view that leaned on an
    /// evicted one would describe nothing.
    ///
    /// A code change is named by [`CodeChangeClass::coarse`] alone. The fine
    /// class is a heuristic over grammar node kinds, and a wrong one here is
    /// wrong evidence the interviewer reads.
    pub(crate) fn prompt_view(&self, since: Option<u64>) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let code = &self.code;
        if code.revision == 0 {
            out.push_str("code: nothing received yet");
        } else {
            let parse = match code.parser_observation {
                Some(CodeObservation::Parsed) => "parses",
                Some(CodeObservation::SyntaxInvalid) => "does not parse",
                Some(CodeObservation::ParserUnavailable) | None => "not parsed",
            };
            let _ = write!(
                out,
                "code: {}, {} candidate edits, {} changed the program, {parse}",
                code.language, code.candidate_edits, code.semantic_revision
            );
            if let Some(class) = code
                .last_analysis
                .as_ref()
                .and_then(|analysis| analysis.classification.as_ref())
            {
                let _ = write!(out, ", last edit {}", class.coarse());
            }
            if self.stuck.undo_redo_cycles > 0 {
                let _ = write!(
                    out,
                    ", returned to an earlier buffer {} times",
                    self.stuck.undo_redo_cycles
                );
            }
        }

        out.push_str("\ntests: ");
        if !self.progress.runner_attempted {
            out.push_str("not run");
        } else {
            let tests = &self.tests;
            let _ = write!(out, "{} of {} passing", tests.passed, tests.total);

            // Differences in the counts, which can be negative, and only
            // against a run that happened: the first run's differences are
            // against nothing. Read as "newly passing" they said a run with one
            // more pass and one fewer failure had -1 newly failing.
            if tests.follows_executed_run && (tests.newly_passed != 0 || tests.newly_failed != 0) {
                let _ = write!(
                    out,
                    ", {:+} passing and {:+} failing since the run before",
                    tests.newly_passed, tests.newly_failed
                );
            }
            if tests.regression {
                out.push_str(", a regression");
            }
            if tests.consecutive_identical_failures > 1 {
                let _ = write!(
                    out,
                    ", the same failure {} runs in a row",
                    tests.consecutive_identical_failures
                );
            }
            if self.progress.all_tests_passed_once {
                out.push_str(", all passed at least once");
            }
            if self.stuck.compile_or_runner_failure_count > 0 {
                let _ = write!(
                    out,
                    ", {} runs failed to start",
                    self.stuck.compile_or_runner_failure_count
                );
            }
            let _ = write!(out, ", {} edit-and-run cycles", self.stuck.edit_test_cycles);
        }

        let diagnostics = &self.diagnostics;
        let counted = [
            ("syntax", diagnostics.syntax),
            ("type", diagnostics.type_errors),
            ("linker", diagnostics.linker),
            ("runtime signal", diagnostics.runtime_signal),
            ("timeout", diagnostics.timeout),
            ("warning", diagnostics.warnings),
            ("other", diagnostics.other),
        ]
        .into_iter()
        .filter(|(_, count)| *count > 0)
        .map(|(name, count)| format!("{name} {count}"))
        .collect::<Vec<_>>();
        if !counted.is_empty() {
            let _ = write!(out, "\ndiagnostics: {}", counted.join(", "));
        }

        if let (Some(meaningful), Some(latest)) = (
            self.stuck.last_meaningful_change_ms,
            self.entries.last().map(|entry| entry.receipt_timestamp_ms),
        ) {
            let _ = write!(
                out,
                "\nlast program change: {} s before the latest event",
                latest.saturating_sub(meaningful) / 1000
            );
        }

        let hints = &self.hints;
        if hints.requested + hints.volunteered + hints.withheld > 0 {
            let _ = write!(
                out,
                "\nhints: {} requested, {} volunteered, {} withheld, ladder at rung {}",
                hints.requested, hints.volunteered, hints.withheld, hints.current_level
            );
        }

        if !self.coverage.covered.is_empty() || !self.coverage.uncovered.is_empty() {
            let list = |phases: &[String]| {
                if phases.is_empty() {
                    "none".to_string()
                } else {
                    phases.join(", ")
                }
            };
            let _ = write!(
                out,
                "\nphases covered: {}; not yet: {}",
                list(&self.coverage.covered),
                list(&self.coverage.uncovered)
            );
        }

        let conversation = &self.conversation;
        let _ = write!(
            out,
            "\nturns: {} interviewer, {} candidate",
            conversation.interviewer_turns, conversation.candidate_turns
        );
        if !conversation.recent_sequence.is_empty() {
            let _ = write!(out, "; recent: {}", conversation.recent_sequence.join(" "));
        }

        let lifecycle = &self.lifecycle;
        let flags = [
            (lifecycle.paused, "paused"),
            (lifecycle.ended, "ended"),
            (
                lifecycle.behavioral_round_started,
                "behavioral round started",
            ),
        ]
        .into_iter()
        .filter_map(|(set, name)| set.then_some(name))
        .collect::<Vec<_>>();
        if !flags.is_empty() {
            let _ = write!(out, "\nsession: {}", flags.join(", "));
        }

        if let Some(since) = since {
            let (mut updates, mut changes, mut runs, mut hints, mut turns) = (0, 0, 0, 0, 0);
            for entry in self.entries.iter().filter(|entry| entry.sequence > since) {
                match entry.family {
                    ObservationFamily::Code => {
                        updates += 1;
                        if entry.observation["semanticChange"] == true {
                            changes += 1;
                        }
                    }
                    ObservationFamily::Test => runs += 1,
                    ObservationFamily::Hint => hints += 1,
                    ObservationFamily::InterviewerTurn | ObservationFamily::CandidateTurn => {
                        turns += 1;
                    }
                    ObservationFamily::Diagnostic | ObservationFamily::Lifecycle => {}
                }
            }
            let _ = write!(
                out,
                "\nsince the last event like this: {updates} editor updates ({changes} changed the program), {runs} test runs, {hints} hints, {turns} turns"
            );
        }
        out
    }

    fn append(
        &mut self,
        family: ObservationFamily,
        provenance: Provenance,
        source_timestamp_ms: Option<u64>,
        receipt_timestamp_ms: u64,
        observation: serde_json::Value,
    ) {
        let entry = EvidenceEntry {
            sequence: self.next_sequence,
            family,
            provenance,
            source_timestamp_ms,
            receipt_timestamp_ms,
            observation,
        };
        self.next_sequence += 1;
        if self.entries.len() >= MAX_LEDGER_ENTRIES {
            self.entries.remove(0);
        }
        self.entries.push(entry);
    }

    /// Counts an event at the browser boundary before reducers accept or
    /// discard it. This is intentionally separate from ledger entries: a
    /// malformed or paused packet is still received, but is not evidence.
    pub(crate) fn record_received_event(&mut self) {
        self.metrics.raw_events_received += 1;
    }

    pub(crate) fn record_model_input(&mut self, kind: ModelInputKind, text: &str) {
        let bytes = text.len() as u64;
        match kind {
            ModelInputKind::Watch => {
                self.metrics.watch_prompt_count += 1;
                self.metrics.watch_prompt_bytes += bytes;
            }
            ModelInputKind::Turn => {
                self.metrics.turn_prompt_count += 1;
                self.metrics.turn_prompt_bytes += bytes;
            }
            ModelInputKind::Interim => {
                self.metrics.interim_prompt_count += 1;
                self.metrics.interim_prompt_bytes += bytes;
            }
            ModelInputKind::FinalReport => {
                self.metrics.final_report_prompt_count += 1;
                self.metrics.final_report_prompt_bytes += bytes;
            }
            ModelInputKind::ReadEditor => {
                self.metrics.read_editor_calls += 1;
                self.metrics.read_editor_bytes += bytes;
            }
            ModelInputKind::ToolResponse => {
                self.metrics.tool_response_count += 1;
                self.metrics.tool_response_bytes += bytes;
            }
        }
        let mut hasher = Sha256::new();
        hasher.update(self.metrics.model_input_digest.as_bytes());
        hasher.update(format!("{kind:?}").as_bytes());
        hasher.update(bytes.to_le_bytes());
        hasher.update(text.as_bytes());
        self.metrics.model_input_digest = format!("{:x}", hasher.finalize());
    }

    /// Records a code observation whose structural classification was produced
    /// by the server's language parser. A caller that never saw the prior
    /// buffer has no structural claim to make and passes `parser_unavailable`,
    /// which says so rather than guessing.
    pub(crate) fn record_code_with_analysis(
        &mut self,
        source_timestamp_ms: Option<u64>,
        receipt_timestamp_ms: u64,
        language: &str,
        code: &str,
        candidate_edit: bool,
        analysis: CodeAnalysis,
    ) {
        let digest = hex_digest(code.as_bytes());
        if self.code.latest_digest == digest && self.code.language == language {
            return;
        }
        let state = state_digest(language, code);
        let revisits_prior_state = candidate_edit
            && digest != self.code.latest_digest
            && self.code.digest_history.iter().any(|prior| prior == &state);
        self.code.language.clear();
        self.code.language.push_str(language);
        self.code.revision += 1;
        if candidate_edit {
            self.code.candidate_edits += 1;
        }
        if candidate_edit && analysis.semantic_change {
            self.code.semantic_revision += 1;
            self.stuck.last_meaningful_change_ms = Some(receipt_timestamp_ms);
        }

        // Outside the branch above on purpose. The reversal that matters most
        // is the one that undoes a draft the parser could not read, and that
        // edit lands back on a buffer the ledger has already seen, so the
        // analysis reports no semantic change and the count would never move.
        if revisits_prior_state {
            self.stuck.undo_redo_cycles += 1;
        }
        self.code.parser_observation = Some(analysis.observation.clone());
        self.code.latest_digest = digest.clone();
        if self.code.digest_history.len() >= MAX_CODE_DIGEST_HISTORY {
            self.code.digest_history.remove(0);
        }
        self.code.digest_history.push(state);

        // A template the browser published on connect, or the buffer a tab
        // switch carried with it, is not the candidate's work, so the
        // structural claim that came with it is not theirs to answer for
        // either. Masking it in the entry alone left the same claim reachable
        // through `code.last_analysis`, which the projection carries, so the
        // opening of an interview still told the model the candidate had made a
        // semantic change before they had typed anything. The parse itself
        // survives: a buffer either parses or it does not, whoever wrote it.
        let observed = if candidate_edit {
            analysis.clone()
        } else {
            CodeAnalysis::observed(analysis.observation.clone())
        };
        self.code.last_analysis = Some(observed.clone());
        self.metrics.code_events_emitted += 1;
        if candidate_edit {
            self.record_sequence("edit");
        }
        if candidate_edit && self.progress.first_edit_ms.is_none() {
            self.progress.first_edit_ms = Some(receipt_timestamp_ms);
        }
        self.append(
            ObservationFamily::Code,
            Provenance::Derived,
            source_timestamp_ms,
            receipt_timestamp_ms,
            serde_json::json!({
                "language": language,
                "revision": self.code.revision,
                "semanticRevision": self.code.semantic_revision,
                "digest": digest,
                "parser": analysis.observation,
                "classification": observed.classification,
                "semanticChange": observed.semantic_change,
                "changedNodes": observed.changed_nodes,
            }),
        );
    }

    /// # A run that never ran
    ///
    /// A runner that fails to start produces no case outcomes, and the browser
    /// sends that the same way it sends a real result. Folding it into the
    /// deltas made a transient 503 from the compiler service read as two
    /// failures fixed, and then made the next run of the same untouched code
    /// read as three cases newly passing: a network error became progress. So
    /// the counts, the deltas and the progress flags belong to runs that
    /// executed, and a run that did not still records its attempt, its
    /// diagnostic and its entry.
    ///
    /// `total` alone is not what separates them. The browser fills `total` from
    /// the judge spec before it runs anything, so a compile error arrives as
    /// nought of five with an empty `failures` array, not as nought of nought:
    /// reading the count alone turned a C++ build failure after a green run
    /// into five cases newly failed and a regression, which is the reading this
    /// section exists to refuse. A setup error is the runner saying it never
    /// reached the cases, and the two together are what say a run executed.
    pub(crate) fn record_test(
        &mut self,
        source_timestamp_ms: Option<u64>,
        receipt_timestamp_ms: u64,
        payload: &serde_json::Value,
    ) {
        let total = payload
            .get("total")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
            .min(u64::from(u32::MAX)) as u32;
        let passed = payload
            .get("passed")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
            .min(u64::from(total)) as u32;
        let failed = total.saturating_sub(passed);
        let executed = total > 0 && !run_failed_to_start(payload);
        let signature = failure_signature(payload);
        let previous = self.tests.clone();
        let newly_passed = delta(passed, previous.passed);
        self.progress.runner_attempted = true;
        self.record_sequence("test");
        if executed {
            self.tests = TestState {
                total,
                passed,
                failed,

                // Widened before subtracting. Both operands are clamped to 99
                // by `sanitize_test_run` on every path the browser reaches, but
                // this reducer is public and takes the raw value, so the
                // arithmetic holds the invariant itself rather than borrowing
                // the caller's.
                newly_passed,
                newly_failed: delta(failed, previous.failed),

                // Only executed runs overwrite this state, and an executed run
                // has cases, so a total is what an earlier one leaves behind.
                follows_executed_run: previous.total > 0,
                regression: passed < previous.passed,

                // A streak needs a signature on both sides. Two failing runs
                // that named no failures at all are not evidence of the same
                // failure twice, and counting them as one drove the repeated
                // failure signal that asks the interviewer to step in.
                //
                // Cases that started passing end the streak whatever the
                // signature says. Only the first few failures are reported, so
                // a candidate who fixes six of ten and leaves the reported four
                // untouched hashes the same as before: the signal that says
                // they are stuck on one failure would fire at the moment they
                // made the largest gain of the session.
                consecutive_identical_failures: match (failed > 0, &signature) {
                    (true, Some(_))
                        if newly_passed <= 0 && signature == previous.failure_signature =>
                    {
                        previous.consecutive_identical_failures + 1
                    }
                    (true, _) => 1,
                    (false, _) => 0,
                },
                failure_signature: signature.clone(),
                source_timestamp_ms,
            };
            self.progress.tested = true;
            self.progress.test_progress |= self.tests.newly_passed > 0;
            if self.tests.newly_passed > 0 {
                self.stuck.last_test_progress_ms = Some(receipt_timestamp_ms);
                self.stuck.test_progress_latency_ms = self
                    .stuck
                    .last_meaningful_change_ms
                    .map(|meaningful| receipt_timestamp_ms.saturating_sub(meaningful));
            }
            self.progress.all_tests_passed_once |= passed == total;
            self.stuck.repeated_failure_count = self.tests.consecutive_identical_failures;
        }

        // An edit and then a run is one cycle; the Run button pressed twelve
        // times over an untouched buffer is one cycle and eleven retries, and
        // counting those as cycles told the report the candidate was iterating
        // when they were waiting.
        if self.code.candidate_edits > self.candidate_edits_at_last_test {
            self.stuck.edit_test_cycles += 1;
        }
        self.candidate_edits_at_last_test = self.code.candidate_edits;
        if let Some((category, diagnostic)) = diagnostic(payload) {
            self.record_diagnostic(
                source_timestamp_ms,
                receipt_timestamp_ms,
                category,
                &diagnostic,
            );
        }
        self.append(
            ObservationFamily::Test,
            Provenance::BrowserClaim,
            source_timestamp_ms,
            receipt_timestamp_ms,
            serde_json::json!({
                "executed": executed,
                "total": total,
                "passed": passed,

                // `failed` is the one count here the runner never reported: it
                // is `total` minus `passed`, so a setup error that arrives as
                // nought of five says five cases failed, when the runner is
                // saying it never reached them. `total` and `passed` are what
                // the browser claimed and stay as they are.
                "failed": if executed { failed } else { 0 },
                "newlyPassed": if executed { self.tests.newly_passed } else { 0 },
                "newlyFailed": if executed { self.tests.newly_failed } else { 0 },
                "regression": executed && self.tests.regression,

                // Every count on this entry describes this run, so a run that
                // produced no case outcomes carries none of the running state:
                // a streak printed beside `"executed":false` reads as two
                // identical failures in a run that never failed anything.
                "consecutiveIdenticalFailures": if executed {
                    self.tests.consecutive_identical_failures
                } else {
                    0
                },
                "failureSignature": signature,
            }),
        );
    }

    fn record_diagnostic(
        &mut self,
        source_timestamp_ms: Option<u64>,
        receipt_timestamp_ms: u64,
        category: DiagnosticCategory,
        diagnostic: &str,
    ) {
        let signature = hex_digest(diagnostic.as_bytes());
        match category {
            DiagnosticCategory::Syntax => self.diagnostics.syntax += 1,
            DiagnosticCategory::Type => self.diagnostics.type_errors += 1,
            DiagnosticCategory::Linker => self.diagnostics.linker += 1,
            DiagnosticCategory::RuntimeSignal => self.diagnostics.runtime_signal += 1,
            DiagnosticCategory::Timeout => self.diagnostics.timeout += 1,
            DiagnosticCategory::Warning => self.diagnostics.warnings += 1,
            DiagnosticCategory::Other => self.diagnostics.other += 1,
        }
        if !matches!(category, DiagnosticCategory::Warning) {
            self.stuck.compile_or_runner_failure_count += 1;
        }
        self.diagnostics.latest_signature = Some(signature.clone());
        self.append(
            ObservationFamily::Diagnostic,
            Provenance::BrowserClaim,
            source_timestamp_ms,
            receipt_timestamp_ms,
            serde_json::json!({
                "category": category,
                "signature": signature,
            }),
        );
    }

    pub(crate) fn record_hint(
        &mut self,
        receipt_timestamp_ms: u64,
        requested: bool,
        delivered: bool,
    ) {
        if requested {
            self.record_sequence("hint_request");
        }
        if requested {
            self.hints.requested += 1;
        } else {
            self.hints.volunteered += 1;
        }
        if delivered {
            self.hints.current_level += u32::from(requested);
            self.hints.last_hint_sequence = Some(self.next_sequence);
        } else {
            self.hints.withheld += 1;
        }
        self.append(
            ObservationFamily::Hint,
            Provenance::ServerObservation,
            None,
            receipt_timestamp_ms,
            serde_json::json!({
                "requested": requested,
                "delivered": delivered,
                "level": self.hints.current_level,
            }),
        );
    }

    pub(crate) fn record_lifecycle(
        &mut self,
        receipt_timestamp_ms: u64,
        transition: LifecycleTransition,
    ) {
        match transition {
            LifecycleTransition::Paused => self.lifecycle.paused = true,
            LifecycleTransition::Resumed => self.lifecycle.paused = false,
            LifecycleTransition::BehavioralStarted => {
                self.lifecycle.behavioral_round_started = true
            }
            LifecycleTransition::Ended => self.lifecycle.ended = true,
        }
        self.lifecycle.transitions += 1;
        self.append(
            ObservationFamily::Lifecycle,
            Provenance::ServerObservation,
            None,
            receipt_timestamp_ms,
            serde_json::json!({ "transition": transition }),
        );
    }

    pub(crate) fn record_coverage(&mut self, receipt_timestamp_ms: u64, phase: &str) {
        if self.coverage.covered.iter().any(|covered| covered == phase) {
            return;
        }
        self.coverage.covered.push(phase.to_string());
        self.coverage.covered.sort();
        self.coverage
            .uncovered
            .retain(|uncovered| uncovered != phase);
        self.append(
            ObservationFamily::Lifecycle,
            Provenance::Derived,
            None,
            receipt_timestamp_ms,
            serde_json::json!({ "coverage": phase }),
        );
    }

    pub(crate) fn set_uncovered_coverage(
        &mut self,
        phases: impl IntoIterator<Item = &'static str>,
    ) {
        self.coverage.uncovered = phases.into_iter().map(str::to_string).collect();
        self.coverage.uncovered.sort();
    }

    pub(crate) fn record_conversation_turn(&mut self, receipt_timestamp_ms: u64, speaker: &str) {
        let family = match speaker {
            "interviewer" => {
                self.conversation.interviewer_turns += 1;
                ObservationFamily::InterviewerTurn
            }
            "candidate" => {
                self.conversation.candidate_turns += 1;
                ObservationFamily::CandidateTurn
            }
            _ => return,
        };
        self.record_sequence(speaker);
        self.append(
            family,
            Provenance::ServerObservation,
            None,
            receipt_timestamp_ms,
            serde_json::json!({ "turn": speaker }),
        );
    }

    fn record_sequence(&mut self, event: &str) {
        if self.conversation.recent_sequence.len() >= MAX_CONVERSATION_SEQUENCE {
            self.conversation.recent_sequence.remove(0);
        }
        self.conversation.recent_sequence.push(event.to_string());
    }
}

/// Widened, subtracted, then clamped back. The fields are `i32` because a
/// delta is signed and small; doing the arithmetic in the narrow type is what
/// wraps.
fn delta(current: u32, previous: u32) -> i32 {
    (i64::from(current) - i64::from(previous)).clamp(i64::from(i32::MIN), i64::from(i32::MAX))
        as i32
}

fn hex_digest(input: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input);
    format!("{:x}", hasher.finalize())
}

/// One editor state: the language and the buffer together. The language is
/// length-prefixed, since the browser names it and nothing stops a name from
/// ending in bytes the buffer could begin with.
fn state_digest(language: &str, code: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update((language.len() as u64).to_le_bytes());
    hasher.update(language.as_bytes());
    hasher.update(code.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub(crate) fn analyze_code(language: &str, previous: &str, current: &str) -> CodeAnalysis {
    analyze_sides(None, language, previous, current).0
}

/// The same analysis, and beside it what the current buffer is on its own.
///
/// `analyze_code` collapses the two sides into one `syntax_invalid`, which is
/// the right answer to "what does this edit show" and the wrong one to "is the
/// candidate's buffer a program now". A caller that needs the second was
/// re-parsing `current` with [`observe_code`] to ask it, which is a third parse
/// of a buffer this function has already parsed once and diffed.
///
/// The second half is a `CodeObservation` rather than a bool because there are
/// three answers, not two: the buffer parsed, the buffer did not, or no parser
/// ever ran. A bool folded the last two together and left its own meaning
/// resting on the caller checking the first half first.
///
/// It reads the previous buffer's tree from `cache` when it holds it, and
/// leaves the current one there for the next update.
///
/// Every update used to parse both buffers from nothing, and the previous one
/// is exactly the buffer the update before had just parsed: measured with the
/// crate optimized, two parses were most of the 3.6 ms an analysis took on a
/// fifty-line buffer and of the 39 ms on five hundred lines, run inline on the
/// agent's task up to three times a second. The current buffer is parsed
/// incrementally from the previous tree edited to match it, which tree-sitter
/// guarantees produces the tree a fresh parse would.
pub(crate) fn analyze_code_cached(
    cache: &mut ParseCache,
    language: &str,
    previous: &str,
    current: &str,
) -> (CodeAnalysis, CodeObservation) {
    analyze_sides(Some(cache), language, previous, current)
}

fn analyze_sides(
    cache: Option<&mut ParseCache>,
    language: &str,
    previous: &str,
    current: &str,
) -> (CodeAnalysis, CodeObservation) {
    let unavailable = || (parser_unavailable(), CodeObservation::ParserUnavailable);
    let Some(grammar) = grammar_key(language) else {
        return unavailable();
    };
    if previous.len().max(current.len()) > MAX_PARSED_BYTES {
        return unavailable();
    }
    let cached = cache
        .as_deref()
        .and_then(|cache| cache.tree_for(grammar, previous));
    let Some(previous_tree) = cached.or_else(|| parse_with(grammar, previous, None)) else {
        return unavailable();
    };
    let mut edited = previous_tree.clone();
    edited.edit(&input_edit(previous, current));
    let Some(current_tree) = parse_with(grammar, current, Some(&edited)) else {
        return unavailable();
    };
    if let Some(cache) = cache {
        cache.store(grammar, current, &current_tree);
    }
    let current_alone = if current_tree.root_node().has_error() {
        CodeObservation::SyntaxInvalid
    } else {
        CodeObservation::Parsed
    };
    if previous_tree.root_node().has_error() || current_alone != CodeObservation::Parsed {
        return (
            CodeAnalysis::observed(CodeObservation::SyntaxInvalid),
            current_alone,
        );
    }
    let before = syntax_facts(previous_tree.root_node(), previous);
    let after = syntax_facts(current_tree.root_node(), current);
    let changed_nodes = changed_node_facts(&before.node_counts, &after.node_counts);

    // Read once, and read whole. The predicates below disagree on purpose about
    // what a closure's insides mean: a loop written inside a lambda is still a
    // loop the candidate wrote, and a dict built in one is still a dict, but a
    // closure's parameter list is not the enclosing function's signature. So
    // the first two are asked about the kind with its nesting stripped and the
    // last about the kind as recorded. Spelled out here because it used to be
    // an accident -- two predicates matched exactly and one on substrings, so
    // the prefix blinded two of the three and not the third, and nothing said
    // which behaviour was meant.
    let changed_kinds = || {
        changed_nodes
            .iter()
            .map(|fact| fact.split(':').next().unwrap_or_default())
    };
    let classification = if before.node_counts == after.node_counts
        && before.identifiers == after.identifiers
        && before.leaves == after.leaves
        && before.shape == after.shape
    {
        if before.comments != after.comments {
            CodeChangeClass::CommentOnly
        } else {
            CodeChangeClass::FormattingOnly
        }
    } else if before.interface_leaves != after.interface_leaves {
        CodeChangeClass::FunctionInterface
    } else if before.loop_bindings != after.loop_bindings {
        CodeChangeClass::ControlFlow
    } else if before.node_counts == after.node_counts && before.leaves == after.leaves {
        // Same identifiers in a different order is not a rename, it is an
        // argument swap, and calling it `identifier_only` told the report the
        // candidate had renamed something while the call they actually changed
        // went unmentioned. The multiset separates the two.
        if before.shape == after.shape
            && before.identifiers != after.identifiers
            && sorted(&before.identifiers) != sorted(&after.identifiers)
        {
            CodeChangeClass::IdentifierOnly
        } else {
            CodeChangeClass::Expression
        }
    } else if before.node_counts == after.node_counts {
        CodeChangeClass::Expression
    } else if changed_kinds().any(|kind| control_kind(unnest(kind))) {
        CodeChangeClass::ControlFlow
    } else if changed_kinds().any(|kind| data_structure_kind(unnest(kind))) {
        CodeChangeClass::DataStructure
    } else if changed_kinds().any(interface_kind) {
        CodeChangeClass::FunctionInterface
    } else {
        // Every kind outside the three categories, however many moved. Adding
        // `- 1` to a loop bound adds an operator and a literal, two kinds, and
        // counting them as a mixed edit described one expression as several.
        CodeChangeClass::Expression
    };
    let semantic_change = !matches!(
        classification,
        CodeChangeClass::FormattingOnly | CodeChangeClass::CommentOnly
    );
    (
        CodeAnalysis {
            observation: CodeObservation::Parsed,
            classification: Some(classification),
            semantic_change,

            // Bounded after classifying, never before: the facts are ordered by
            // node kind, so a cap applied first would decide control flow
            // against whatever the alphabet happened to keep.
            changed_nodes: bounded_node_facts(changed_nodes),
        },
        CodeObservation::Parsed,
    )
}

/// An observation about one buffer on its own, with no claim about what
/// changed. A language switch swaps the editor and the tab in one packet, so
/// the buffer that preceded it belongs to the tab the candidate left and a diff
/// against it would be a comparison between two languages. Recording the parse
/// anyway is what lets the next completed edit recover: `analyze_code_update`
/// only keeps a recovery baseline when the buffer before the invalid draft was
/// seen to parse, and claiming the parser was unavailable for a language the
/// server parses fine left that condition permanently false.
pub(crate) fn observe_code(language: &str, code: &str) -> CodeAnalysis {
    observe(None, language, code)
}

/// [`observe_code`], leaving the tree in `cache` so the next edit in the tab
/// just switched to diffs against it without parsing it again.
pub(crate) fn observe_code_cached(
    cache: &mut ParseCache,
    language: &str,
    code: &str,
) -> CodeAnalysis {
    observe(Some(cache), language, code)
}

fn observe(cache: Option<&mut ParseCache>, language: &str, code: &str) -> CodeAnalysis {
    let Some(grammar) = grammar_key(language) else {
        return parser_unavailable();
    };
    if code.len() > MAX_PARSED_BYTES {
        return parser_unavailable();
    }
    let Some(tree) = parse_with(grammar, code, None) else {
        return parser_unavailable();
    };
    if let Some(cache) = cache {
        cache.store(grammar, code, &tree);
    }
    CodeAnalysis::observed(if tree.root_node().has_error() {
        CodeObservation::SyntaxInvalid
    } else {
        CodeObservation::Parsed
    })
}

/// The last buffer this runtime parsed, with its tree. Runtime-only like the
/// recovery baselines: it holds source, so it never enters the ledger, and it
/// changes nothing a reducer records. A tree parsed afresh and one read from
/// here are the same tree, which is what makes it a cache and not state; it
/// compares equal to any other for that reason, and a clone may keep or drop
/// it without changing a result.
#[derive(Clone, Default)]
pub struct ParseCache {
    last: Option<(&'static str, String, tree_sitter::Tree)>,
}

impl ParseCache {
    pub(super) fn tree_for(
        &self,
        grammar: &'static str,
        source: &str,
    ) -> Option<tree_sitter::Tree> {
        self.last
            .as_ref()
            .filter(|(cached, text, _)| *cached == grammar && text == source)
            .map(|(_, _, tree)| tree.clone())
    }

    fn store(&mut self, grammar: &'static str, source: &str, tree: &tree_sitter::Tree) {
        self.last = Some((grammar, source.to_string(), tree.clone()));
    }
}

impl PartialEq for ParseCache {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for ParseCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ParseCache")
    }
}

thread_local! {
    /// One parser per grammar per thread. Building a parser and loading its
    /// language was paid on every analysis before, twice.
    static PARSERS: std::cell::RefCell<std::collections::HashMap<&'static str, tree_sitter::Parser>> =
        std::cell::RefCell::default();
}

pub(super) fn parse_with(
    grammar: &'static str,
    source: &str,
    old: Option<&tree_sitter::Tree>,
) -> Option<tree_sitter::Tree> {
    PARSERS.with(|parsers| {
        let mut parsers = parsers.borrow_mut();
        if !parsers.contains_key(grammar) {
            let mut parser = tree_sitter::Parser::new();
            parser.set_language(&parser_language(grammar)?).ok()?;
            parsers.insert(grammar, parser);
        }
        parsers.get_mut(grammar)?.parse(source, old)
    })
}

/// The one edit that turns `old` into `new`: the bytes both share at the start
/// and at the end stay, and everything between is replaced. Cut at character
/// boundaries, so a point never lands inside a multi-byte character. One
/// string's boundary is the other's: past a shared prefix both hold the same
/// lead byte, and valid UTF-8 gives both the same continuation bytes after it,
/// and a shared suffix starts on the same byte in both.
pub(super) fn input_edit(old: &str, new: &str) -> tree_sitter::InputEdit {
    let prefix = old.floor_char_boundary(
        old.bytes()
            .zip(new.bytes())
            .take_while(|(left, right)| left == right)
            .count(),
    );
    let suffix = old.as_bytes()[prefix..]
        .iter()
        .rev()
        .zip(new.as_bytes()[prefix..].iter().rev())
        .take_while(|(left, right)| left == right)
        .count();
    let suffix = old.len() - old.ceil_char_boundary(old.len() - suffix);
    let point = |text: &str, byte: usize| {
        let before = &text.as_bytes()[..byte];
        let row = before.iter().filter(|&&byte| byte == b'\n').count();
        let column = before
            .iter()
            .rposition(|&byte| byte == b'\n')
            .map_or(byte, |newline| byte - newline - 1);
        tree_sitter::Point::new(row, column)
    };
    tree_sitter::InputEdit {
        start_byte: prefix,
        old_end_byte: old.len() - suffix,
        new_end_byte: new.len() - suffix,
        start_position: point(old, prefix),
        old_end_position: point(old, old.len() - suffix),
        new_end_position: point(new, new.len() - suffix),
    }
}

/// The grammar a language is parsed with, spelled as the static name the parser
/// pool is keyed by.
pub(super) fn grammar_key(language: &str) -> Option<&'static str> {
    Some(match language {
        "c" => "c",
        "cpp" => "cpp",
        "java" => "java",
        "javascript" => "javascript",
        "python" => "python",
        _ => return None,
    })
}

/// Keeps the count of what was dropped rather than the facts themselves, so a
/// truncated list still says it is one.
fn bounded_node_facts(mut facts: Vec<String>) -> Vec<String> {
    if facts.len() <= MAX_CHANGED_NODE_FACTS {
        return facts;
    }
    let dropped = facts.len() - MAX_CHANGED_NODE_FACTS;
    facts.truncate(MAX_CHANGED_NODE_FACTS);
    facts.push(format!("+{dropped} more"));
    facts
}

fn sorted<'a>(identifiers: &[&'a str]) -> Vec<&'a str> {
    let mut sorted = identifiers.to_vec();
    sorted.sort_unstable();
    sorted
}

pub(super) fn parser_language(language: &str) -> Option<tree_sitter::Language> {
    Some(match language {
        "c" => tree_sitter_c::LANGUAGE.into(),
        "cpp" => tree_sitter_cpp::LANGUAGE.into(),
        "java" => tree_sitter_java::LANGUAGE.into(),
        "javascript" => tree_sitter_javascript::LANGUAGE.into(),
        "python" => tree_sitter_python::LANGUAGE.into(),
        _ => return None,
    })
}

pub(crate) fn parser_unavailable() -> CodeAnalysis {
    CodeAnalysis::observed(CodeObservation::ParserUnavailable)
}

/// Borrowed from the two buffers being compared, which both outlive it: the
/// facts are only ever compared with each other inside one analysis and never
/// stored, so hashing each token (and formatting each leaf first) bought
/// nothing but a SHA-256 and an allocation per token per keystroke.
struct SyntaxFacts<'a> {
    node_counts: std::collections::BTreeMap<String, u32>,
    identifiers: Vec<&'a str>,
    /// Kind and text, since `+` and a string holding "+" are different leaves.
    leaves: Vec<(&'static str, &'a str)>,
    comments: Vec<&'a str>,
    /// `None` stands in for a parameter's name, so that gaining or losing a
    /// parameter moves these and renaming one does not. Not a leaf, so it
    /// cannot collide with the leaves it sits beside.
    interface_leaves: Vec<Option<(&'static str, &'a str)>>,
    /// Every named node but a comment, with its depth, in document order.
    /// Python spells a block in whitespace alone, so dedenting a `return` out
    /// of a loop moves no node, no identifier and no token: the counts and the
    /// leaves above are identical, and without the nesting the edit read as a
    /// reindent. A brace language moves a `}` for the same edit; this is the
    /// same fact for the grammar that has none.
    shape: Vec<(&'static str, u32)>,
    /// Each loop's kind followed by the named kinds of what it binds, the
    /// target of a `for ... in`, `for ... of` or `for (... : ...)`. Kinds
    /// alone, so renaming the loop variable is still a rename, while `for i in`
    /// becoming `for i, n in` changes what the loop iterates with: the loop
    /// was already there, so its count never moved, and the new tuple pattern
    /// was all the edit showed.
    ///
    /// Not a C-style loop's initializer, which holds the start value as well
    /// as the variable: `int i = 0` becoming `int i = n - 1` is the same
    /// off-by-one Python spells in `range`, and it read as control flow in C
    /// and JavaScript while Python read it as an expression. C++ puts an
    /// optional init-statement under the same field name ahead of a range
    /// loop's declarator, which it would have shadowed.
    loop_bindings: Vec<&'static str>,
}

/// Walks the tree in document order and records what a change to the code can
/// move: the named structure, the identifiers, and every leaf token.
///
/// Both halves of that are load-bearing. Operators are anonymous tokens in
/// every grammar here, so a walk that keeps only named nodes sees `a + b` and
/// `a - b` as the same program and reports the edit as formatting; they are
/// leaves, so collecting anonymous leaves is what makes an operator visible.
/// Order is the other half: the facts are compared as sequences rather than
/// sets, so swapping two arguments is a change even though the multiset of
/// identifiers is untouched. Document order is what makes that comparison
/// stable across runs, which is why the children go on the stack backwards.
///
/// A closure is walked like anything else, but everything inside one is
/// qualified as a closure's. A comparator passed to a sort is spelled with the
/// same node kinds a function signature is - `formal_parameters` in Java,
/// `parameter_list` in C++ - so counting the two as one kind reported writing a
/// comparator as changing the enclosing function's signature, and, because that
/// branch is taken early, said nothing about the loop the candidate added
/// beside it. The closure's own node keeps its kind, because gaining a lambda
/// is a fact about the code the candidate wrote; only what is nested inside it
/// is qualified.
fn syntax_facts<'a>(root: tree_sitter::Node<'_>, source: &'a str) -> SyntaxFacts<'a> {
    struct Frame<'tree> {
        node: tree_sitter::Node<'tree>,
        in_parameters: bool,
        in_closure: bool,
        /// Parameters here belong to no signature: a closure's or a catch's.
        opaque_parameters: bool,
        in_binding: bool,
        depth: u32,
    }
    let mut stack = vec![Frame {
        node: root,
        in_parameters: false,
        in_closure: false,
        opaque_parameters: false,
        in_binding: false,
        depth: 0,
    }];
    let mut node_counts = std::collections::BTreeMap::new();
    let mut identifiers = Vec::new();
    let mut leaves = Vec::new();
    let mut comments = Vec::new();
    let mut interface_leaves = Vec::new();
    let mut shape = Vec::new();
    let mut loop_bindings = Vec::new();

    // `Node::kind` is an FFI call and a UTF-8 validation scan every time, and
    // this loop asked for it seven times a node over a walk that runs twice per
    // analysis.
    let mut scratch = String::new();
    let mut cursor = root.walk();
    while let Some(Frame {
        node,
        in_parameters,
        in_closure,
        opaque_parameters,
        in_binding,
        depth,
    }) = stack.pop()
    {
        let kind = node.kind();

        // A Python `\` joins two lines into one, which is layout like the
        // newline it hides, but the grammar makes it a named token: breaking a
        // long expression across lines read as an expression edit and moved the
        // review gate. It is a leaf, so there is nothing beneath to walk.
        if kind == "line_continuation" {
            continue;
        }
        let is_comment = comment_kind(kind);
        if node.is_named() && !is_comment {
            shape.push((kind, depth));
            if in_binding {
                loop_bindings.push(kind);
            }
        }
        let mut binding = None;
        if loop_kind(kind) {
            loop_bindings.push(kind);
            binding = ["left", "declarator", "name"]
                .into_iter()
                .find_map(|field| node.child_by_field_name(field))
                .map(|child| child.id());
        }
        if node.is_named() {
            if is_comment {
                if let Ok(text) = node.utf8_text(source.as_bytes()) {
                    comments.push(text);
                }
            } else {
                // Keyed by borrow, so the allocation is one per distinct kind
                // rather than one per node: a buffer holds tens of kinds and
                // thousands of nodes, and every one of those strings was
                // allocated to be dropped by `entry`.
                let counted = if in_closure {
                    scratch.clear();
                    scratch.push_str(CLOSURE_PREFIX);
                    scratch.push_str(kind);
                    scratch.as_str()
                } else {
                    kind
                };
                if let Some(count) = node_counts.get_mut(counted) {
                    *count += 1;
                } else {
                    node_counts.insert(counted.to_string(), 1);
                }
            }
        }
        if node.is_named()
            && kind == "identifier"
            && let Ok(text) = node.utf8_text(source.as_bytes())
        {
            // A parameter is a bare identifier in Python and JavaScript, so a
            // function that gains its first one gains no token at all: the
            // parentheses were already there and the comma only arrives with
            // the second. Leaving parameters out of the interface facts read
            // that as an expression edit, while the same change in C, which
            // spells a type beside the name, read as the signature change it
            // is.
            //
            // A placeholder rather than the name, because the name is not the
            // interface: positional callers cannot see it. Recording the name
            // here made renaming `n` to `count` a signature change, and, since
            // this branch is taken before the one that reads the changed node
            // kinds, made any rewrite that renamed a parameter along the way
            // report itself as a signature change and nothing else: the new
            // loop in it went unmentioned.
            if in_parameters {
                interface_leaves.push(None);
            }
            identifiers.push(text);
        }
        if node.child_count() == 0
            && kind != "identifier"
            && !is_comment
            && let Ok(text) = node.utf8_text(source.as_bytes())
        {
            if in_parameters {
                interface_leaves.push(Some((kind, text)));
            }
            leaves.push((kind, text));
        }

        // A closure's own parameters are not the enclosing function's, so the
        // closure flag is resolved first and then suppresses the parameter one
        // for the whole subtree beneath it.
        //
        // A catch clause's parameter is the same: Java spells it
        // `catch_formal_parameter` and C++ a `parameter_list`, and reading it
        // as the enclosing function's made adding a try/catch a signature
        // change, a branch taken before the one that would have called it
        // control flow. Only the parameter: shielding the handler too hid the
        // signature of a function declared inside it.
        let child_in_closure = in_closure || closure_kind(kind);
        let child_opaque_parameters = opaque_parameters || child_in_closure;
        let catch_body = (kind == "catch_clause")
            .then(|| node.child_by_field_name("body").map(|body| body.id()));

        // Through a cursor, not `child(index)`, which walks the siblings from
        // the first on every call: a module of a few hundred statements made
        // this loop quadratic in them.
        let pushed_from = stack.len();
        stack.extend(node.children(&mut cursor).map(|child| {
            let opaque =
                child_opaque_parameters || catch_body.is_some_and(|body| body != Some(child.id()));
            Frame {
                node: child,
                in_parameters: !opaque && (in_parameters || kind.contains("parameter")),
                in_closure: child_in_closure,
                opaque_parameters: opaque,
                in_binding: in_binding || binding == Some(child.id()),
                depth: depth + 1,
            }
        }));
        stack[pushed_from..].reverse();
    }
    SyntaxFacts {
        node_counts,
        identifiers,
        leaves,
        comments,
        interface_leaves,
        shape,
        loop_bindings,
    }
}

fn changed_node_facts(
    before: &std::collections::BTreeMap<String, u32>,
    after: &std::collections::BTreeMap<String, u32>,
) -> Vec<String> {
    let kinds = before
        .keys()
        .chain(after.keys())
        .collect::<std::collections::BTreeSet<_>>();
    kinds
        .into_iter()
        .filter_map(|kind| {
            let old = before.get(kind).copied().unwrap_or(0);
            let new = after.get(kind).copied().unwrap_or(0);
            match new.cmp(&old) {
                std::cmp::Ordering::Greater => Some(format!("{kind}:added:{}", new - old)),
                std::cmp::Ordering::Less => Some(format!("{kind}:removed:{}", old - new)),
                std::cmp::Ordering::Equal => None,
            }
        })
        .collect()
}

/// Every spelling of a branch or a loop the five grammars use, because a list
/// that holds only the shapes C happens to name is a list that answers for one
/// language. `for (const x of items)` is a `for_in_statement` in JavaScript,
/// `for (int x : a)` is an `enhanced_for_statement` in Java and `for (auto x :
/// v)` is a `for_range_loop` in C++, and none of them is a `for_statement`: the
/// most ordinary loop a candidate writes in three of these tabs was reported as
/// an edit of no particular kind. A `do`/`while` is a `do_statement` in all
/// four C-family grammars and was missing for the same reason.
fn control_kind(kind: &str) -> bool {
    loop_kind(kind)
        || matches!(
            kind,
            "if_statement"
                | "elif_clause"
                | "else_clause"
                | "while_statement"
                | "do_statement"
                | "switch_statement"
                | "switch_expression"
                | "case_statement"
                | "match_statement"
                | "try_statement"
                | "catch_clause"
                | "finally_clause"
                | "with_statement"
                | "break_statement"
                | "continue_statement"
        )
}

/// Every comment kind the pinned grammars have. Java has no `comment`, only
/// `line_comment` and `block_comment`, so matching the one word read every
/// Java comment edit as code and moved the review gate on it; JavaScript adds
/// `html_comment` for a `<!--` line.
pub(super) fn comment_kind(kind: &str) -> bool {
    matches!(
        kind,
        "comment" | "line_comment" | "block_comment" | "html_comment"
    )
}

/// The loops that bind something, a subset of [`control_kind`]: `while` and
/// `do` iterate with nothing of their own.
fn loop_kind(kind: &str) -> bool {
    matches!(
        kind,
        "for_statement" | "for_in_statement" | "for_range_loop" | "enhanced_for_statement"
    )
}

/// A node kind with its closure nesting stripped, for the questions that do not
/// turn on it.
fn unnest(kind: &str) -> &str {
    kind.strip_prefix(CLOSURE_PREFIX).unwrap_or(kind)
}

/// A function the candidate wrote inline to pass somewhere, as opposed to one a
/// caller can find by name. What is inside one is qualified by
/// [`syntax_facts`], and the kinds here are what that walk keys off.
fn closure_kind(kind: &str) -> bool {
    matches!(
        kind,
        "lambda" | "lambda_expression" | "arrow_function" | "function_expression"
    )
}

/// A list rather than a chain of alternatives, because the alternatives
/// overlap: every kind containing "dictionary" contains "dict" too, so one of
/// the two terms could never change the answer and nothing could be written to
/// tell whether it was wired up at all.
///
/// The exclusions are the price of matching on substrings. Several grammars
/// spell a syntactic grouping as a list: `argument_list` is what a call puts
/// its arguments in, so extracting a helper and calling it, which is the most
/// ordinary refactor an interview contains, was reported as the candidate
/// changing a data structure. It read as something else again in JavaScript,
/// where the same node is named `arguments`, so the same edit was described
/// differently
/// depending on the tab. A literal that really is a data structure keeps its
/// own name, C's `initializer_list` included.
fn data_structure_kind(kind: &str) -> bool {
    const GROUPING: [&str; 6] = [
        "argument_list",
        "parameter_list",
        "expression_list",
        "declaration_list",
        "enumerator_list",
        "type_list",
    ];

    // A destructuring target is spelled as a list or an object in several
    // grammars, `pattern_list` in Python and `array_pattern` in JavaScript, and
    // unpacking `for i, n in enumerate(nums)` builds no structure at all.
    if kind.contains("pattern") || GROUPING.iter().any(|grouping| kind.contains(grouping)) {
        return false;
    }
    ["array", "list", "dict", "object", "map", "set"]
        .iter()
        .any(|word| kind.contains(word))
}

/// A change to something a caller can see: the definition itself, or the
/// parameter list it carries.
///
/// Named in full rather than matched on substrings, which is what the words
/// "function", "method", "class" and "parameter" were. Every call is spelled
/// with one of them - `method_invocation` in Java, `call_expression` and
/// `arrow_function` in JavaScript - so calling a method inside a function body,
/// which is most of what an interview contains, reported the candidate as
/// having changed the signature of the function they called it from.
///
/// Deliberately not "any declaration" either: C spells a local variable
/// `declaration` and Java spells it `local_variable_declaration`, so matching
/// that word reported `int count = 0;` inside a body as a changed signature.
fn interface_kind(kind: &str) -> bool {
    matches!(
        kind,
        // The definitions.
        "function_definition"
            | "function_declaration"
            | "function_declarator"
            | "method_declaration"
            | "method_definition"
            | "constructor_declaration"
            | "class_definition"
            | "class_declaration"
            | "class_specifier"
            | "struct_specifier"
            | "interface_declaration"
            | "enum_declaration"
            // The parameter lists they carry, and the parameters in them.
            | "parameters"
            | "parameter_list"
            | "parameter_declaration"
            | "optional_parameter_declaration"
            | "variadic_parameter_declaration"
            | "formal_parameters"
            | "formal_parameter"
            | "spread_parameter"
            | "default_parameter"
            | "typed_parameter"
            | "typed_default_parameter"
    )
}

/// A digest of what failed, used only to tell one failing run from the next.
///
/// The separators are what make it a signature rather than a concatenation.
/// With one delimiter for everything, a run whose label was "a" and error "b"
/// hashed the same as a run with a single label that happened to contain the
/// delimiter, and two runs failing for different reasons then read as the same
/// failure twice. Field name, unit separator between fields, record separator
/// between failures.
fn failure_signature(payload: &serde_json::Value) -> Option<String> {
    let failures = payload.get("failures")?.as_array()?;
    let mut material = String::new();
    for failure in failures {
        for key in ["label", "error", "expected", "got"] {
            if let Some(value) = failure.get(key).and_then(serde_json::Value::as_str) {
                material.push_str(key);
                material.push('\u{1f}');
                material.push_str(value);
                material.push('\u{1f}');
            }
        }
        material.push('\u{1e}');
    }
    if failures.is_empty() {
        None
    } else {
        Some(hex_digest(material.as_bytes()))
    }
}

/// Whether the runner said it never reached the cases.
///
/// The same field `apply_test_results` reads to decide which reaction the
/// interviewer gets, so the ledger and the spoken half agree on what happened.
/// The browser publishes a setup error and case outcomes as alternatives: the
/// paths that fill one return an empty `cases` array, so a run carrying this
/// graded nothing however many cases the judge spec holds.
fn run_failed_to_start(payload: &serde_json::Value) -> bool {
    payload
        .get("setupError")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|text| !text.is_empty())
}

fn diagnostic(payload: &serde_json::Value) -> Option<(DiagnosticCategory, String)> {
    let structured_category = payload
        .get("diagnostic")
        .and_then(|value| value.get("category"))
        .and_then(serde_json::Value::as_str)
        .and_then(DiagnosticCategory::from_wire);
    let setup_error = payload
        .get("setupError")
        .and_then(serde_json::Value::as_str)
        .filter(|text| !text.is_empty());
    let category = structured_category.unwrap_or(DiagnosticCategory::Other);
    let material = setup_error
        .map(|text| text.chars().take(400).collect())
        .or_else(|| structured_category.map(|_| format!("structured:{category:?}")))?;
    Some((category, material))
}

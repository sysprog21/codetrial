//! Recording evidence into the runtime state, and reading the framework
//! progress back out of it.

// This module reads the hub's imports and types through `use super::*`.
#[cfg_attr(
    not(test),
    expect(
        clippy::wildcard_imports,
        reason = "the module reads its hub through super::*"
    )
)]
use super::*;

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
pub(super) fn record_evidence(
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
    let confidence = u8::try_from(confidence)
        .ok()
        .filter(|confidence| *confidence <= 100)
        .ok_or("invalid confidence")?;
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
            confidence,
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
    let at_ms = elapsed_ms(state);
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

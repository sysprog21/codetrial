//! Answering a hint request: counting it, handing out the next rung or
//! holding it back, and the question of whether the candidate has acted on the
//! last one.

// This module reads the hub's imports and types through `use super::*`.
use super::framework::HintAttemptMark;
#[cfg_attr(
    not(test),
    expect(
        clippy::wildcard_imports,
        reason = "the module reads its hub through super::*"
    )
)]
use super::*;

impl RuntimeState {
    /// Where the work stands now, for the next rung to be measured against.
    fn hint_mark(&self) -> HintAttemptMark {
        let open = self.interviewer_delivery.current;
        HintAttemptMark {
            candidate_edits: self.evidence_ledger.code.candidate_edits,
            real_test_runs: self.hint_attempt.real_test_runs,
            board_revisions: self.hint_attempt.board_revisions,
            clue_from: open.unwrap_or(self.transcript.len()),
            cut: false,
        }
    }

    /// The first candidate line after the clue's first interviewer line.
    fn hint_reply(&self, mark: HintAttemptMark) -> Option<usize> {
        let said = |line: &usize, who| speaker_speech(&self.transcript[*line], who).is_some();
        (mark.clue_from..self.transcript.len())
            .skip_while(|line| !said(line, INTERVIEWER_SPEAKER))
            .find(|line| said(line, CANDIDATE_SPEAKER))
    }

    /// Speech counts from the first candidate turn after the clue began, so a
    /// request transcribed after a spoken opening counts. A wiped board tried
    /// nothing. A cut clue may never have been heard, so it holds nothing: a
    /// cut counts on the clue until the interviewer speaks again after the
    /// first answer, a held reply with no words yet included, erring toward a
    /// rung over a question about a clue they may not have heard.
    fn acted_since_hint(&self, mark: HintAttemptMark) -> bool {
        let (attempt, reply) = (&self.hint_attempt, self.hint_reply(mark));
        self.evidence_ledger.code.candidate_edits > mark.candidate_edits
            || attempt.real_test_runs > mark.real_test_runs
            || (attempt.board_revisions > mark.board_revisions && self.board_strokes > 0)
            || reply.is_some_and(|reply| attempt.quoted_turn_start >= Some(reply))
            || mark.cut
    }

    /// Cuts the interviewer's output, and marks the clue cut when no
    /// interviewer line follows the candidate's first reply to it yet.
    pub(crate) fn cut_interviewer(&mut self, cut: DeliveryCut) {
        self.interviewer_delivery
            .cancel(cut, std::time::Instant::now());
        let interviewer = |line: &String| speaker_speech(line, INTERVIEWER_SPEAKER).is_some();
        if let Some(mark) = self.hint_attempt.mark
            && cut != DeliveryCut::Idle
            && !self
                .hint_reply(mark)
                .is_some_and(|reply| self.transcript[reply..].iter().any(interviewer))
        {
            self.hint_attempt.mark = Some(HintAttemptMark { cut: true, ..mark });
        }
    }
}

/// Count a hint and, when the candidate asked for it, hand out the next rung.
///
/// The last rung names the key step, so it waits until the candidate has an
/// approach of their own: an Algorithm phase observed from what they said, or
/// Coding evidence, which the server already refuses without code they wrote.
/// An Algorithm phase the interviewer only inferred does not count, since that
/// is the model's word alone. Until then the interviewer gets no clue and is
/// told to ask what the candidate would try; the request is not counted,
/// because the candidate was given nothing. When the candidate leads, a rung
/// after the first is held that way too until they act on the one before.
pub fn record_hint(state: &mut RuntimeState, requested: bool) -> String {
    // One reading for the call, whichever of the branches below answers it.
    let receipt_timestamp_ms = crate::current_epoch_millis();
    let clue = requested
        .then(|| state.hint_ladder.get(state.hint_rungs_given))
        .flatten();
    if clue.is_some() {
        let mark = state.hint_attempt.mark;
        if state.candidate_led_enabled && mark.is_some_and(|mark| !state.acted_since_hint(mark)) {
            state
                .evidence_ledger
                .record_hint(receipt_timestamp_ms, requested, false);
            let spoken = phase_judge_open(state)
                .into_iter()
                .any(|id| id != phase_id(FrameworkPhase::Coding));
            return prompts::hint_rung_held_text(state.hints_used, spoken);
        }
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
            if state.candidate_led_enabled {
                state.hint_attempt.mark = Some(state.hint_mark());
            }
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

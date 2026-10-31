//! Output transcripts that trail the turn they belong to: told apart from the
//! next generation's first words, and settled without starting a generation.

use super::{Instant, LATE_TRANSCRIPT_GRACE, RuntimeActivity};

impl RuntimeActivity {
    /// Whether `now` is still inside the grace after the last completed turn,
    /// where the provider may yet send that turn's transcription.
    pub(super) fn is_late_transcript(&self, now: Instant) -> bool {
        self.turn_completed_at
            .is_some_and(|at| now.saturating_duration_since(at) < LATE_TRANSCRIPT_GRACE)
    }

    /// An output transcript with nothing generating and nothing owed, just
    /// after a completion: the provider can send a generation's transcription
    /// after its `TurnComplete`, and no completion follows it. Counting it as
    /// output would leave `generating` set for good, and a replacement would
    /// ask again for a reply that had already finished.
    ///
    /// Only with nothing owed, and the candidate silent since the completion.
    /// Nothing tells the transcription from the next generation's first words,
    /// so with a reply owed, or speech heard after the completion (which
    /// `note_candidate_finished` leaves unarmed inside the window), it is taken
    /// as the answer it may be, as before, rather than dropped on a guess; a
    /// generation that really began is marked generating by its audio, text
    /// or tool call, and is still owed when it is cut off.
    pub(in crate::livekit) fn is_trailing_transcript(&self, now: Instant) -> bool {
        !self.generating
            && !self.owes_reply()
            && self.is_late_transcript(now)
            && self
                .turn_completed_at
                .is_some_and(|completed| self.last_user_speech <= completed)
    }

    /// What `note_output` settles that a trailing transcript still settles:
    /// the turn it trails was answered, so a continuation's fallback and a
    /// handoff's wait for a reply are over. It starts no generation.
    pub(in crate::livekit) fn note_trailing_transcript(&mut self) {
        self.drop_handoff_reply();
        self.socket.thinking_reply_fallback = None;
        self.tool_response_at = None;
    }
}

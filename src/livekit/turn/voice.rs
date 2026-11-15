//! The candidate's voice as the provider and the microphone report it: when
//! speech started and ended, whether fresh microphone evidence says it is still
//! under way, and the transcription tail a socket replacement waits out.

use super::{CHECKPOINT_VOICE_QUIET, Duration, Instant, RuntimeActivity, STALE_VOICE_ACTIVITY};

impl RuntimeActivity {
    pub(in crate::livekit) fn note_candidate_voice_activity(&mut self, active: bool, now: Instant) {
        if active || self.candidate_voice_active {
            self.reset_completed_coding_idle();
        }
        if active {
            self.note_rotation_spoken_over(now);
        }
        self.candidate_voice_active = active;
        self.candidate_voice_ended_at = (!active).then_some(now);
        self.last_user_speech = now;
        self.candidate_voice_at = Some(now);
        if active && self.recent_voiced_pcm(now) && self.voiced_pcm_at != self.rebased_voiced_pcm_at
        {
            self.incoming_speech_owed |= self.reply_unfinished();
        } else if !active && std::mem::take(&mut self.incoming_speech_owed) {
            self.rebased_voiced_pcm_at = self.voiced_pcm_at;
            self.resume_reply_wait(now);
        }
    }

    /// Ends a speech-start whose end never came, as if it had come when the
    /// evidence went stale, so the transcription grace still applies.
    pub(in crate::livekit) fn expire_stale_voice_activity(&mut self, now: Instant) {
        if self.candidate_voice_active
            && self
                .candidate_voice_at
                .is_none_or(|at| now.saturating_duration_since(at) >= STALE_VOICE_ACTIVITY)
        {
            self.candidate_voice_active = false;
            self.candidate_voice_ended_at = Some(now);
        }
    }

    pub(in crate::livekit) fn clear_candidate_voice_activity(&mut self) {
        self.candidate_voice_active = false;
        self.candidate_voice_ended_at = None;
        self.voiced_pcm_at = None;
        self.rebased_voiced_pcm_at = None;
        self.incoming_speech_owed = false;
    }

    pub(super) fn recent_voiced_pcm(&self, now: Instant) -> bool {
        self.voiced_pcm_at
            .is_some_and(|at| now.saturating_duration_since(at) < CHECKPOINT_VOICE_QUIET)
    }

    pub(in crate::livekit) fn incoming_speech_active(&self, now: Instant) -> bool {
        (self.candidate_voice_active || self.incoming_speech_owed) && self.recent_voiced_pcm(now)
    }

    // A stale VAD edge cannot renew the wait without microphone evidence.
    // Rebase once at the acoustic quiet boundary, not at every watch tick.
    pub(super) fn settle_incoming_speech(&mut self, now: Instant) {
        if self.incoming_speech_owed && !self.incoming_speech_active(now) {
            self.incoming_speech_owed = false;
            self.rebased_voiced_pcm_at = self.voiced_pcm_at;
            if let Some(at) = self.voiced_pcm_at {
                self.resume_reply_wait(at + CHECKPOINT_VOICE_QUIET);
            }
        }
    }

    /// The provider reported the end of candidate speech that no later start
    /// has superseded.
    pub(in crate::livekit) fn candidate_voice_ended(&self) -> bool {
        self.candidate_voice_ended_at.is_some()
    }

    pub(in crate::livekit) fn candidate_transcription_pending(&self, now: Instant) -> bool {
        self.candidate_voice_ended_at
            .is_some_and(|at| now.saturating_duration_since(at) < CHECKPOINT_VOICE_QUIET)
            || (self.candidate_voice_active
                && self.candidate_voice_at.is_some_and(|at| {
                    now.saturating_duration_since(at) >= Duration::from_millis(500)
                }))
    }

    /// The candidate's microphone carried more than room noise. Until the
    /// socket has transcribed them, this is also the idle timers' evidence of
    /// speech: a candidate talking to a socket that cannot hear them would
    /// otherwise be counted silent and nudged. Only until then, because room
    /// noise above `VOICE_RMS` would hold every nudge back for good.
    pub(in crate::livekit) fn note_candidate_voice(&mut self, now: Instant) {
        self.note_rotation_spoken_over(now);
        self.voiced_pcm_at = Some(now);
        self.incoming_speech_owed |= self.candidate_voice_active && self.reply_unfinished();
        self.candidate_voice_at = Some(now);
        if !self.transcribed_on_socket {
            self.reset_completed_coding_idle();
            self.last_user_speech = now;
        }
    }
}

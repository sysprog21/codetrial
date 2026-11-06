//! When a Gemini socket may be restarted, what a restart that has to wait for
//! the model to finish speaking leaves pending, and how a session's end is
//! named in its summary.

// This module reads the hub's imports and types through `use super::*`.
#[cfg_attr(
    not(test),
    expect(
        clippy::wildcard_imports,
        reason = "the module reads its hub through super::*"
    )
)]
use super::*;

/// Decides whether the interview may be put back on a fresh Gemini socket, and
/// spends one of the budgeted attempts when it may.
///
/// The failure budget must be testable even when no replacement can open, so
/// its counter and age policy stays independent of socket I/O.
///
/// The budget counts restarts, not resumptions, and says nothing about whether
/// a handle exists. Without one the caller rebuilds from bounded local state;
/// the same budget guards both recovery paths rather than blocking fallback.
///
/// The counter resets for a socket that lived a while, because the ceiling is
/// there to stop a socket that fails instantly from spinning, and a socket that
/// carried seven minutes of interview did not fail instantly. Counting those
/// for the life of the session made the ceiling a limit on interview length: at
/// Gemini's own connection cap a ninety-minute interview spends most of the
/// budget before anything goes wrong, and whatever is left is what a network
/// blip gets to consume.
pub(super) fn take_restart_attempt(restarts: &mut usize, socket_age: Duration) -> bool {
    if socket_age >= HEALTHY_GEMINI_SOCKET {
        *restarts = 0;
    }
    if *restarts >= GEMINI_RESTART_LIMIT {
        return false;
    }

    // Spent before the reconnect rather than after it. A socket that fails to
    // come back is the case the ceiling exists for, and counting only the
    // successes would let an endpoint failing instantly be retried without
    // bound.
    *restarts += 1;
    true
}

/// A checkpoint can be accepted yet restore a conversation that fails again.
/// Once that happens, rebuild locally until output proves recovery worked.
///
/// `resumed` describes the socket now open and only a connect writes it: a
/// resumed socket that answers once and then dies young is still a failed
/// resumption, so completed output lifts the suppression without forgetting
/// where the open socket came from.
///
/// The answer to a recovery briefing proves nothing either way. Every
/// replacement asks for one, so counting it would let a run of sockets that
/// each answer their briefing and die reset the budget forever. It is the
/// first turn to end after the briefing (`recovery_reply_pending`) rather than
/// a reply tagged with the briefing's cause, because a tool call in that answer
/// retags the continuation that completes it.
///
/// Judged by how long the socket stayed up, not by the debt-adjusted age the
/// restart budget uses: a resumed socket that carried minutes of interview and
/// was replaced owing a reply did not fail at its checkpoint.
#[derive(Default)]
pub(super) struct ResumeRecovery {
    pub(super) resumed: bool,
    pub(super) suppressed: bool,
}

impl ResumeRecovery {
    /// Judged on every replacement, whether or not a handle exists to use.
    pub(super) fn allow_resume(&mut self, age: Duration) -> bool {
        if age >= HEALTHY_GEMINI_SOCKET {
            self.suppressed = false;
        } else if self.resumed {
            self.suppressed = true;
        }
        !self.suppressed
    }

    pub(super) fn connected(&mut self, resumed: bool) {
        self.resumed = resumed;
    }

    pub(super) fn note_event(
        &mut self,
        restarts: &mut usize,
        state: &mut RuntimeState,
        activity: &RuntimeActivity,
        event: &GeminiEvent,
    ) {
        // Whichever way the briefing's turn ends, silent or cut off by the
        // candidate, it is over, and the next reply is not its answer.
        if session::ends_turn(event) && std::mem::take(&mut state.recovery_reply_pending) {
            return;
        }
        if completed_live_reply(activity, event) {
            *restarts = 0;
            self.suppressed = false;
        }
    }
}

/// The agent's output has settled: nothing generating, and nothing left in the
/// LiveKit playout queue. The floor alone is not enough, because it is stamped
/// once when a turn completes and the queue drains on its own afterwards.
///
/// Two things wait on this. The goodbye is over when it is true, and a `GoAway`
/// may replace the transport when it is true: replacing Gemini interrupts its
/// output queue, so anything still in flight would be cut.
pub(super) fn output_settled(floor: Floor, audio_playing: bool) -> bool {
    floor != Floor::Speaking && !audio_playing
}

/// The held half of a `GoAway`: the advisory arrives whenever Gemini decides,
/// and the socket may only be replaced once the output has settled, so the two
/// are separated by however long the current turn has left.
///
/// Kept as a type rather than a bare `bool` because four places in the room
/// loop touch the same advisory, and the one that forgets a rule is how it ends
/// up either cutting a reply or never being spent at all.
#[derive(Default)]
pub(super) struct DeferredRestart(pub(super) bool);

impl DeferredRestart {
    /// An advisory is held and not yet spent.
    pub(super) fn is_armed(&self) -> bool {
        self.0
    }

    pub(super) fn can_spend(&self, activity: &RuntimeActivity, now: Instant) -> bool {
        self.is_armed() && !activity.incoming_speech_active(now)
    }

    /// The candidate is mid-utterance with fresh evidence, which is the case a
    /// speech handoff exists for. Kept apart from [`Self::can_spend`], whose
    /// speech guard holds an ordinary restart back for exactly this case.
    pub(super) fn handoff_due(&self, activity: &RuntimeActivity, now: Instant) -> bool {
        self.is_armed()
            && activity.candidate_voice_active
            && !activity.candidate_transcription_pending(now)
    }

    /// After speech ends the spend waits out the transcription tail, and no
    /// provider event need follow it, so the watch keeps retrying until then.
    pub(super) fn watch_due(
        &self,
        activity: &RuntimeActivity,
        stalled: bool,
        now: Instant,
    ) -> bool {
        self.handoff_due(activity, now)
            || (self.can_spend(activity, now)
                && (stalled || activity.candidate_voice_active || activity.candidate_voice_ended()))
    }

    /// Some other path replaced the socket. Leaving the advisory armed would
    /// make the first completed turn on the new socket pay for a second
    /// replacement.
    pub(super) fn cancel(&mut self) {
        self.0 = false;
    }

    /// A `GoAway` arrived. `true` is the caller's cue to replace the socket
    /// now; otherwise the advisory is held for `take_if_due`.
    ///
    /// Armed first and then spent through `take_if_due`, so the settled rule
    /// lives in one place and this method holds only what it adds to it.
    ///
    /// `reply_pending` is that addition, and the one signal `output_settled`
    /// cannot carry. `Floor::Speaking` is stamped on the first audio chunk, so
    /// a turn that has issued a tool call and produced no audio is still
    /// generating under a `Listening` floor. Without it a `GoAway` landing
    /// there throws the generation away after its tool response has already
    /// gone out on the old socket.
    pub(super) fn request(
        &mut self,
        floor: Floor,
        audio_playing: bool,
        reply_pending: bool,
        tool_owed: bool,
    ) -> bool {
        self.0 = true;
        !reply_pending && self.take_if_due(floor, audio_playing, tool_owed)
    }

    /// Spends a held advisory at the first settled moment after it.
    ///
    /// Weighs `tool_response_outstanding` and deliberately not the pending
    /// reply that `request` also reads. The two are not the same debt. A
    /// candidate talking over the queued reply ends the turn this was deferred
    /// behind without Gemini ever calling it an interruption, and that same
    /// event stamps a pending reply; holding on it would carry the advisory
    /// across another whole turn until `timeLeft` was spent and the socket
    /// dropped mid-generation, which is the outcome deferring exists to avoid.
    /// A tool response is the opposite case: it is generation already paid for
    /// on this socket, it leaves the floor `Listening` with nothing playing, so
    /// every term above reads settled while the answer is still owed.
    pub(super) fn take_if_due(
        &mut self,
        floor: Floor,
        audio_playing: bool,
        tool_owed: bool,
    ) -> bool {
        if !self.0 || tool_owed || !output_settled(floor, audio_playing) {
            return false;
        }
        self.0 = false;
        true
    }

    /// `take_if_due` read off the room's own state, which is how every caller
    /// asks it, so the settled rule is read the same way at each.
    pub(super) fn take_if_settled(
        &mut self,
        activity: &RuntimeActivity,
        audio_playing: bool,
        now: Instant,
    ) -> bool {
        if activity.candidate_transcription_pending(now) {
            return false;
        }
        self.take_if_due(
            activity.floor,
            audio_playing,
            activity.socket.tool_response_outstanding,
        )
    }
}

/// The age a replaced socket is judged by for the restart budget. One that
/// left a reply unanswered is not healthy just because it stayed connected:
/// the watchdog found it silent, or it still owed a reply or was partway
/// through one when a `GoAway` or the server closed it. A provider that closes
/// silent sockets a minute in would otherwise reset the run on every one and
/// never let the budget end the interview.
pub(super) fn replaced_socket_age(
    age: Duration,
    reply_timeout: bool,
    activity: &RuntimeActivity,
) -> Duration {
    if reply_timeout || activity.reply_unfinished() {
        Duration::ZERO
    } else {
        age
    }
}

/// How an interview's Live session ended, as its summary line spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LiveOutcome {
    Ok,
    Error,
    GeminiUnreachable,
    Billing,
}

impl LiveOutcome {
    /// A project out of credit is named wherever the failure surfaced, since
    /// it is the one an operator fixes by paying rather than by waiting, and
    /// the one that looks like any other silence from the candidate's side.
    pub(super) fn from_error(error: &(dyn std::error::Error + 'static), otherwise: Self) -> Self {
        if crate::gemini::is_billing_failure(error) {
            Self::Billing
        } else {
            otherwise
        }
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::GeminiUnreachable => "gemini_unreachable",
            Self::Billing => "billing",
        }
    }
}

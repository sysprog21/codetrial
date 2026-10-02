//! Whose turn it is: the state an interview is a sequence of.
//!
//! Data and the rules over it, nothing that drives the room. Who holds the
//! floor, when the candidate last typed or spoke, which prompt is due, and
//! whether a wrap-up is owed at all.
//!
//! The procedures that end a turn stayed in the room loop, because every one of
//! them borrows its `GeminiEventContext`. They read as turn logic, but a module
//! that has to reach back into its parent for the type it operates on is not a
//! seam; it is the same code in two files. This depends on `crate::agent` and
//! on nothing above it.

use std::time::{Duration, Instant};

#[cfg(test)]
use crate::config::DEFAULT_MAX_INTERIM_REVIEWS;

use crate::agent::{
    RuntimeState, SpeakerTurn, TEST_REACTION_COOLDOWN_S, TimingInput, ViewFor,
    behavioral_silence_nudge, board_silence_nudge, candidate_lines, changed_excerpt,
    proactive_review, silence_nudge, timing_decision, unreviewed_from, with_timer,
};

/// How long the room has to be quiet before a pause is worth reading into.
///
/// Well under `SILENCE_THRESHOLD_S`, and that is the point: this is meant to
/// land in the ordinary gaps of an interview -- someone reading the problem,
/// typing, composing a sentence -- rather than in the stuck silences the
/// interviewer already steps into. It costs the candidate nothing either way,
/// because the call it starts runs beside the room instead of inside it.
pub(super) const INTERIM_IDLE: Duration = Duration::from_secs(8);
/// Between two idle-window reviews. A pause every eight seconds would spend a
/// call on every breath. At seventy-five seconds a review read little more
/// than the one before it, and the final report reads the whole transcript
/// regardless, so the stretch between two is longer.
pub(super) const INTERIM_COOLDOWN: Duration = Duration::from_secs(150);
/// What one review may read, in bytes.
///
/// The pause it runs in is the budget: `INTERIM_ATTEMPT_TIMEOUT` gives the call
/// twelve seconds, and a window too large to summarize in that spends its
/// prefill and returns nothing. Far below `MAX_TRANSCRIPT_BYTES`, which bounds
/// a whole interview rather than one stretch of it.
pub(super) const INTERIM_WINDOW_BYTES: usize = 8 * 1024;
/// What one review may read of the editor. Smaller than the transcript budget:
/// the code is re-sent in full on every review, where the transcript is only
/// the stretch since the last one.
pub(super) const INTERIM_CODE_BYTES: usize = 4 * 1024;
/// Candidate turns a pause has to have produced before one is read. Below this,
/// the window is an "mm-hm" and the note would be about nothing; at four, a
/// short exchange of acknowledgements still qualified.
pub(super) const INTERIM_MIN_NEW_TURNS: usize = 6;
/// What an idle-window review is sent in place of an editor it was already
/// sent.
pub(super) const INTERIM_CODE_UNCHANGED: &str =
    "(unchanged since the notes on record were written; judge this stretch's speech)";

/// A candidate who has just typed is still working, even if their speech has
/// paused. Give them a beat before a periodic review tries to take the floor.
pub(super) const CODE_SETTLE: Duration = Duration::from_secs(10);

/// How long a prompt may go without Gemini producing anything for it before the
/// floor goes back to the candidate. Gemini starts a reply within a couple of
/// seconds; a prompt still unanswered after this is one it will not answer.
/// Holding `Floor::Speaking` for it keeps every silence nudge quiet, and holds
/// back the replacement a `GoAway` asked for until the server closes the socket
/// itself, from an older checkpoint than an orderly replacement resumes.
pub(super) const PROMPT_STALL: Duration = Duration::from_secs(20);

/// How long after Thinking is chosen a transcript is still taken as the tail
/// of what came before it. With the audio stream ended at the click, Gemini
/// delivered that transcript about 0.3s later in a measured session.
pub(super) const THINKING_TRANSCRIPT_GRACE: Duration = Duration::from_millis(1_500);

/// How long after a hold's release, sent as context into the candidate's open
/// turn, the reply may take before it is asked for outright. Gemini's reply to
/// a closed audio stream began about 0.8s after it in a measured session.
pub(super) const THINKING_REPLY_FALLBACK: Duration = Duration::from_secs(4);

/// The window for a room whose silence window is `silence_ms`: never as long
/// as half of it, so the transcript of speech begun after the click, which
/// Gemini sends only once that silence window has passed behind it, always
/// lands outside.
pub(super) fn thinking_transcript_grace(silence_ms: u32) -> Duration {
    THINKING_TRANSCRIPT_GRACE.min(Duration::from_millis(u64::from(silence_ms) / 2))
}

/// A live socket can acknowledge pings while never answering a turn. Allow
/// slower provider replies before replacing it independently of `GoAway`. The
/// default of `CODETRIAL_GEMINI_REPLY_TIMEOUT_S`, which an interview reads once
/// into `RuntimeActivity::reply_timeout`.
pub(super) const REPLY_TIMEOUT: Duration =
    Duration::from_secs(crate::config::DEFAULT_GEMINI_REPLY_TIMEOUT_S as u64);

/// How long an owed reply goes unstarted before the room is told the
/// interviewer is thinking. Gemini usually starts within a couple of seconds;
/// past this the candidate is looking at `Listening` with nothing coming, which
/// is the state issue 95 could not tell apart from a stall.
pub(super) const REPLY_WAIT_SHOWN: Duration = Duration::from_secs(4);

/// How long after Gemini completes a turn a candidate transcript is still
/// taken as the lagging tail of the speech that turn answered. Input
/// transcription trails the audio, so that tail can land after the reply is
/// complete, and arming a reply deadline on it asks for a second answer to a
/// question already answered once the candidate goes quiet.
pub(super) const LATE_TRANSCRIPT_GRACE: Duration = Duration::from_secs(2);

pub(super) struct RuntimeActivity {
    pub(super) last_code_change: Instant,
    pub(super) last_user_speech: Instant,
    /// When the candidate stopped talking and started waiting, if they have and
    /// the reply has not begun. Separate from `last_user_speech`, which the
    /// nudge logic needs seeded at the interview start and never absent.
    ///
    /// Sharing one field is what made the reply-latency line lie. `Interrupted`
    /// hands the floor back without ever saying when the candidate finished, so
    /// a reply that followed one measured from whatever the seed was: on the
    /// first turn that is the start of the interview, which is how a candidate
    /// who waited under a second was reported as having waited fifteen.
    ///
    /// Read as liveness as well as latency; see `reply_in_flight`.
    pub(super) awaiting_reply_since: Option<Instant>,
    /// When a prompt went out that Gemini has produced nothing for yet. The
    /// other half of what the interviewer owes: `awaiting_reply_since` covers a
    /// candidate who finished speaking, this covers a test result, nudge or
    /// briefing sent in text. Outlives the stall release in `settle_stalls`,
    /// which gives the floor back without pretending the reply arrived.
    pub(super) prompted_at: Option<Instant>,
    /// The prompt behind `prompted_at` accepts silence as an answer, as an
    /// editor review does. It still holds the floor until output or a stall,
    /// but a replacement socket does not owe the candidate anything for it.
    pub(super) prompt_allows_silence: bool,
    /// The prompt behind `prompted_at` went out while an earlier generation
    /// was still producing, so the output that follows belongs to that one
    /// until its turn ends. Without this, the tail of Jim's previous sentence
    /// settles a test result he has not reacted to yet.
    pub(super) prompt_behind_turn: bool,
    /// The text of the prompt behind `prompted_at`, so a replacement asked to
    /// give the reply it owed is told which one. Kept past the teardown that
    /// clears `prompted_at`, since that is exactly when it is read.
    pub(super) prompt_text: Option<String>,
    /// Gemini has produced output in the turn now under way. Set by output and
    /// cleared at a turn boundary, which is what "an earlier generation is
    /// still producing" means; the floor alone cannot say it, since a prompt
    /// takes the floor before anything is produced.
    pub(super) generating: bool,
    /// Progress within a turn that has not completed, distinct from its debt.
    pub(super) last_output_at: Option<Instant>,
    /// When the tool response behind `tool_response_outstanding` went out, so
    /// a continuation Gemini never produces stalls like a prompt does.
    pub(super) tool_response_at: Option<Instant>,
    /// How many prompts have gone out in this interview, so a replacement's
    /// log line can name the one it found still owed. The briefing that asks a
    /// new socket for that reply is a prompt too and gets its own number.
    pub(super) prompt_sequence: u64,
    pub(super) last_agent_speech: Instant,
    pub(super) last_nudge: Instant,
    pub(super) last_review: Instant,
    pub(super) last_interjection: Instant,
    pub(super) last_test_reaction: Instant,
    pub(super) substantive_revision_at_last_review: u64,
    /// The evidence lines the last watch prompt of this Live session left the
    /// model holding, so the next sends only the lines that differ. The session
    /// keeps its earlier turns, and a line repeated unchanged every prompt was
    /// paid for every prompt. `None` on a session shown none, which a cold
    /// replacement is: it has heard nothing before, so it gets them all.
    pub(super) evidence_shown: Option<Vec<String>>,
    /// What the last watch prompt moved. See [`Self::unsend_watch_prompt`].
    pub(super) unsent_watch: Option<UnsentWatch>,
    pub(super) floor: Floor,
    /// A pause can arrive between Gemini producing a reply and this loop
    /// receiving its final event. Drop that old turn after resume too.
    pub(super) discarding_output: bool,
    /// Transcription from before Thinking was chosen, arriving after it, is
    /// not the candidate speaking again; see `ignore_input_before_hold`.
    pub(super) thinking_ignore_input_until: Option<Instant>,
    /// A reply the candidate is owed was dropped with a discarded turn when a
    /// provisional hold ended; see `defer_thinking_reply`.
    pub(super) reply_after_thinking_discard: bool,
    /// A hold's release sent as context, for Gemini to answer natively when
    /// the candidate's open turn ends, and when to ask for it outright if it
    /// has produced nothing by then; see `claim_thinking_reply_fallback`.
    pub(super) thinking_reply_fallback: Option<(Instant, String)>,
    /// Gemini ends an interrupted turn with `interrupted` and then
    /// `turnComplete`, skipping `generationComplete` (the Live API reference,
    /// under `BidiGenerateContentServerContent`). That second, older boundary
    /// cannot settle candidate speech or a prompt sent between the two events.
    /// Output from a newer generation also disarms it, so a server that ever
    /// skipped the second event costs one spurious recovery, not a lost turn.
    pub(super) interrupted_turn_pending: bool,
    /// A pause cut off a generation that had stalled, so no discard was armed
    /// for it, but it may yet end. Its `Interrupted` or `TurnComplete` belongs
    /// to that old turn and must not settle the resume prompt sent since:
    /// swallowed here, the way `interrupted_turn_pending` swallows the second
    /// ending of an interrupted turn. New output disarms it.
    pub(super) stale_turn_pending: bool,
    /// When Gemini last completed a turn that said something, which
    /// `LATE_TRANSCRIPT_GRACE` is measured from. Not stamped by an
    /// interruption, where speech is the barge-in itself, nor by a turn that
    /// produced nothing: a candidate pausing on "um," gets a silent completion,
    /// and what they say next is the answer still owed a reply.
    pub(super) turn_completed_at: Option<Instant>,
    /// When a pause was last read into. Sized against `INTERIM_COOLDOWN`.
    pub(super) last_interim: Instant,
    /// The quota is fixed when the interview starts. A later config reload
    /// must not change how much of this interview may spend the report model.
    pub(super) max_interim_reviews: usize,
    pub(super) interim_reviews: usize,
    /// Fixed when the interview starts, like the review quota.
    pub(super) reply_timeout: Duration,
    /// A tool response went out on this socket and its generation has not come
    /// back. Distinct from `awaiting_reply_since`, which a barge-in also stamps
    /// while Gemini owes nothing: this is generation already paid for, and
    /// replacing the transport under it throws the answer away on a socket
    /// nothing will ever read.
    pub(super) tool_response_outstanding: bool,
    /// The behavioral round gets one silence nudge. Repeating it every
    /// cooldown would keep inviting a candidate who has declined or finished.
    pub(super) behavioral_nudged: bool,
    /// Observed Live usage summed over every socket in this interview. Provider
    /// counters are not an invoice. Logged at the end; never model input.
    pub(super) live_usage: crate::gemini::TokenUsage,
    /// Which of this interview's sockets the next usage line belongs to.
    pub(super) live_socket: u64,
    /// The epoch second this room loop started. On every usage line, so two
    /// runs that shared a room name are not summed as one.
    pub(super) live_session_id: u64,
    /// Why the room loop let the interview go when that was not an error
    /// return, for the summary line's `outcome`.
    pub(super) live_exit: super::LiveOutcome,
    /// The latest prompt count observed since the last completed turn, and the
    /// largest since then, the completed turn's own count included. A turn's
    /// usage may arrive on frames other than the one completing it, so the
    /// drop is judged at completion.
    pub(super) latest_prompt_tokens: Option<u64>,
    pub(super) peak_prompt_tokens: u64,
    pub(super) context_refresh_pending: bool,
    /// When the candidate's microphone last carried more than room noise. The
    /// transcript is no guide here: it arrives after the speech it transcribes,
    /// and a checkpoint sent in that gap lands in the middle of an utterance.
    pub(super) candidate_voice_at: Option<Instant>,
    /// Whether the open socket has transcribed the candidate yet. Until it
    /// has, it may not be hearing them at all, as a socket on its way to a
    /// 1011 does not, so their voice on the track stamps `last_user_speech`.
    pub(super) transcribed_on_socket: bool,
}

/// How long the microphone has to stay quiet before a checkpoint may go out:
/// above the default one-second end-of-speech silence, so a pause between two
/// sentences is not taken for the end of the turn.
pub(super) const CHECKPOINT_VOICE_QUIET: Duration = Duration::from_secs(2);

/// A prompt the watcher wants spoken, and whether delivering it spends the
/// behavioral round's one silence nudge. Carried with the text so the sender
/// records the prompt it sent rather than inferring it from the round.
pub(super) struct WatchPrompt {
    pub(super) text: String,
    pub(super) behavioral_nudge: bool,
    /// An editor review, which may rightly get no answer at all.
    pub(super) allows_silence: bool,
}

/// What a watch tick found stalled. See `RuntimeActivity::settle_stalls`.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Stalls {
    /// A prompt got no output in time and the floor went back to the
    /// candidate.
    pub(super) prompt_released: bool,
    /// A held `GoAway` may be spent now rather than waiting on output.
    pub(super) spend_restart: bool,
}

/// Whether a pause landing now leaves output still on its way.
///
/// Only a turn still being produced can deliver more events. Once it is
/// complete and merely draining, its `TurnComplete` has already been and gone,
/// so arming the discard on that state leaves it armed: nothing arrives to
/// disarm it, and the first reply after the resume is swallowed whole.
///
/// A generation or tool continuation that has produced nothing for
/// `PROMPT_STALL` is not on its way either, the way `settle_stalls` stops
/// counting a prompt then. A live one ends when the resume prompt reaches
/// Gemini, which interrupts it and so ends the discard; a dead one never ends,
/// and a discard armed on it dropped the answer to the resume prompt until the
/// watchdog asked again. That covers the floor too: audio a stalled generation
/// already delivered took `Floor::Speaking`, and only its `TurnComplete` would
/// have handed the floor back.
pub(super) fn pause_leaves_output_in_flight(activity: &RuntimeActivity, now: Instant) -> bool {
    let continuation_live = activity.tool_response_outstanding
        && activity
            .tool_response_at
            .is_some_and(|at| now.saturating_duration_since(at) < PROMPT_STALL);
    ((activity.floor == Floor::Speaking || activity.generating)
        && !activity.generation_stalled(now))
        || continuation_live
}

/// Who holds the conversation. `agent_busy` + `agent_turn_complete` encoded
/// this as two bools, but only three of the four combinations were reachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Floor {
    /// The candidate has the floor.
    Listening,
    /// A prompt is in flight; Gemini is still producing the turn.
    Speaking,
    /// The turn is complete but queued audio is still draining.
    AwaitingPlayout,
}

impl RuntimeActivity {
    /// Whether a pending checkpoint may go out now, for an interview still
    /// running: one that has ended, or asked to, owes the model no context.
    pub(super) fn checkpoint_due(
        &self,
        state: &RuntimeState,
        audio_playing: bool,
        candidate_open: bool,
        now: Instant,
    ) -> bool {
        !state.ended
            && !state.end_requested
            && self.can_refresh_context(state.paused, audio_playing, candidate_open, now)
    }

    /// A pending checkpoint implies a compression window: only
    /// `observe_turn_complete` sets it, and only under one.
    pub(super) fn can_refresh_context(
        &self,
        paused: bool,
        audio_playing: bool,
        candidate_open: bool,
        now: Instant,
    ) -> bool {
        self.context_refresh_pending
            && !paused
            && !candidate_open
            && self
                .candidate_voice_at
                .is_none_or(|at| now.saturating_duration_since(at) >= CHECKPOINT_VOICE_QUIET)
            && !self.generating
            && !self.owes_reply()
            && super::output_settled(self.floor, audio_playing)
    }

    /// Adds one usage observation to the interview's sum and the compression
    /// watch, and returns its log line. `cause` is the last platform input
    /// that asked for a reply, or `None` when nothing did since the previous
    /// completed turn, which is the candidate's speech. A label for where turns
    /// come from, not a causal record: speech overlapping a platform input is
    /// credited to the input.
    pub(super) fn account_live_usage(
        &mut self,
        room: &str,
        clock: &str,
        cause: Option<&str>,
        compression: Option<crate::config::GeminiContextCompression>,
        usage: crate::gemini::TokenUsage,
    ) -> String {
        let line = format!(
            "codetrial live_turn_usage room={room} session={} at={clock} socket={} usage_event={} cause={} {}",
            self.live_session_id,
            self.live_socket,
            self.live_usage.samples + 1,
            cause.unwrap_or("candidate"),
            usage.log_fields()
        );
        self.observe_prompt_tokens(compression, usage.prompt);
        self.live_usage.add(usage);
        line
    }

    /// Every observation, periodic or not; a zero says nothing about the
    /// context and is ignored.
    pub(super) fn observe_prompt_tokens(
        &mut self,
        compression: Option<crate::config::GeminiContextCompression>,
        prompt: u64,
    ) {
        if compression.is_none() || prompt == 0 {
            return;
        }
        self.latest_prompt_tokens = Some(prompt);
        self.peak_prompt_tokens = self.peak_prompt_tokens.max(prompt);
    }

    /// Schedules a checkpoint when the turn just completed ran on a context the
    /// provider had cut. The reference is the larger of the last completed
    /// count and any observation since, so usage reported on a frame of its
    /// own still counts. A drop of the threshold is a cut, and so is any drop
    /// from a context that had reached the trigger: new input in the same turn
    /// can make up most of what was cut, but a context only shrinks when the
    /// provider cuts it. A heuristic, not an API compression event.
    pub(super) fn observe_turn_complete(
        &mut self,
        compression: Option<crate::config::GeminiContextCompression>,
    ) {
        let Some(compression) = compression else {
            return;
        };
        let Some(latest) = self.latest_prompt_tokens.take() else {
            return;
        };
        let threshold = u64::from(
            compression
                .trigger_tokens
                .saturating_sub(compression.target_tokens),
        )
        .saturating_div(2)
        .clamp(1, 2048);
        let reference = self.peak_prompt_tokens;
        let cut_at_trigger =
            reference >= u64::from(compression.trigger_tokens) && latest < reference;
        if reference.saturating_sub(latest) >= threshold || cut_at_trigger {
            self.context_refresh_pending = true;
        }
        self.peak_prompt_tokens = latest;
    }

    /// A replacement's briefing already carries the local state a checkpoint
    /// would, so a pending one is dropped. A resumed socket keeps the
    /// provider's context and so its baseline, or a cut in its first turn
    /// would have nothing to be measured against; a cold one starts over.
    pub(super) fn reset_context_observations(&mut self, resumed: bool) {
        if !resumed {
            self.peak_prompt_tokens = 0;
        }
        self.latest_prompt_tokens = None;
        self.context_refresh_pending = false;
        self.transcribed_on_socket = false;
    }

    /// The candidate's microphone carried more than room noise. Until the
    /// socket has transcribed them, this is also the idle timers' evidence of
    /// speech: a candidate talking to a socket that cannot hear them would
    /// otherwise be counted silent and nudged. Only until then, because room
    /// noise above `VOICE_RMS` would hold every nudge back for good.
    pub(super) fn note_candidate_voice(&mut self, now: Instant) {
        self.candidate_voice_at = Some(now);
        if !self.transcribed_on_socket {
            self.last_user_speech = now;
        }
    }

    #[cfg(test)]
    pub(super) fn new(now: Instant) -> Self {
        Self::with_interim_review_cap(now, DEFAULT_MAX_INTERIM_REVIEWS)
    }

    /// The activity a room's interview starts with, named on every usage line
    /// by `session_id`.
    pub(super) fn for_interview(now: Instant, max_interim_reviews: usize, session_id: u64) -> Self {
        Self {
            live_session_id: session_id,
            ..Self::with_interim_review_cap(now, max_interim_reviews)
        }
    }

    pub(super) fn with_interim_review_cap(now: Instant, max_interim_reviews: usize) -> Self {
        Self {
            last_code_change: now,
            last_user_speech: now,
            awaiting_reply_since: None,
            prompted_at: None,
            prompt_allows_silence: false,
            prompt_behind_turn: false,
            prompt_text: None,
            generating: false,
            last_output_at: None,
            tool_response_at: None,
            prompt_sequence: 0,
            last_agent_speech: now,
            last_nudge: now,
            last_review: now,
            last_interjection: now,

            // Seeded in the past so the first test run reacts immediately; see
            // `CandidateMedia::new` for why the subtraction is checked.
            last_test_reaction: now
                .checked_sub(Duration::from_secs_f64(TEST_REACTION_COOLDOWN_S))
                .unwrap_or(now),
            substantive_revision_at_last_review: 0,
            evidence_shown: None,
            unsent_watch: None,
            floor: Floor::Listening,
            discarding_output: false,
            thinking_ignore_input_until: None,
            thinking_reply_fallback: None,
            reply_after_thinking_discard: false,
            interrupted_turn_pending: false,
            turn_completed_at: None,
            stale_turn_pending: false,
            tool_response_outstanding: false,
            behavioral_nudged: false,
            live_usage: crate::gemini::TokenUsage::default(),
            live_socket: 1,
            live_session_id: 0,
            live_exit: super::LiveOutcome::Ok,
            latest_prompt_tokens: None,
            peak_prompt_tokens: 0,
            context_refresh_pending: false,
            candidate_voice_at: None,
            transcribed_on_socket: false,

            // Seeded at `now` rather than in the past: the first minutes of an
            // interview are the greeting and the problem statement, and there
            // is nothing to assess in them.
            last_interim: now,
            max_interim_reviews,
            interim_reviews: 0,
            reply_timeout: REPLY_TIMEOUT,
        }
    }

    /// The operator's reply timeout, in place of the default.
    pub(super) fn with_reply_timeout(mut self, reply_timeout: Duration) -> Self {
        self.reply_timeout = reply_timeout;
        self
    }

    /// The agent has the floor: it was just handed a prompt and owns the
    /// conversation until Gemini reports the turn complete.
    pub(super) fn mark_speaking(&mut self) {
        self.floor = Floor::Speaking;
    }

    /// A prompt just went out: the floor is the agent's, and a reply is owed
    /// until Gemini produces something for it. `allows_silence` is a prompt,
    /// like an editor review, that may rightly be answered with nothing.
    ///
    /// `text` is what a replacement would be told is owed if this goes
    /// unanswered: the prompt itself, or `None` for one with nothing of its own
    /// to name.
    pub(super) fn mark_prompted(&mut self, now: Instant, text: Option<&str>, allows_silence: bool) {
        self.prompt_sequence += 1;
        self.prompt_behind_turn = self.generating;
        self.mark_speaking();
        self.prompted_at = Some(now);
        self.prompt_allows_silence = allows_silence;
        self.prompt_text = text.map(str::to_string);
    }

    /// Provider events can create new debt after the pause cut off the turn.
    /// Preserve that debt on resume, but exclude paused time from its deadline.
    pub(super) fn resume_reply_wait(&mut self, now: Instant) {
        for at in [&mut self.awaiting_reply_since, &mut self.prompted_at] {
            if at.is_some() {
                *at = Some(now);
            }
        }
        self.tool_response_at = self
            .tool_response_at
            .filter(|_| self.tool_response_outstanding)
            .map(|_| now);
        self.last_output_at = self.last_output_at.filter(|_| self.generating).map(|_| now);
    }

    /// A tool response just went out, and Gemini owes its continuation.
    pub(super) fn note_tool_response(&mut self, now: Instant) {
        if self.discarding_output {
            return;
        }
        self.tool_response_outstanding = true;
        self.tool_response_at = Some(now);
        if self.generating {
            // Keep progress after settle_stalls converts the tool debt into
            // prompt debt and clears its separate timestamp.
            self.last_output_at = Some(now);
        }
    }

    /// A reply is owed for a prompt that is not on the wire any more, as when
    /// a recovery briefing failed to send. The floor is left alone: nothing is
    /// generating, and holding it would only silence the watcher until the
    /// dead socket's close is reported.
    ///
    /// `text` is what is owed, when known. It replaces whatever an earlier,
    /// answered prompt left, which the next briefing would otherwise present
    /// as unanswered.
    pub(super) fn owe_prompt(&mut self, now: Instant, text: Option<String>) {
        self.prompt_text = text;
        self.prompted_at = Some(now);
        self.prompt_allows_silence = false;
        self.prompt_behind_turn = false;
    }

    /// A prompt is still owed an answer: one went out, got no output, and was
    /// not one silence answers.
    pub(super) fn owes_prompt(&self) -> bool {
        self.prompted_at.is_some() && !self.prompt_allows_silence
    }

    /// Gemini produced output, so whatever it was prompted with is answered,
    /// unless the output still belongs to the generation before the prompt.
    ///
    /// Output also disarms a tool continuation's stall: the continuation has
    /// begun, and releasing it mid-turn would let a pending close start the
    /// wrap-up before the continuation finishes.
    pub(super) fn note_output(&mut self, now: Instant) {
        self.generating = true;
        self.thinking_reply_fallback = None;
        self.last_output_at = Some(now);
        self.tool_response_at = None;
        if !self.prompt_behind_turn {
            self.prompted_at = None;
        }
    }

    /// A generation ended. One the prompt went out behind is simply over, and
    /// the next is the prompt's own. The prompt's own ending with nothing said
    /// is Gemini answering with silence: no reply is owed for it, and replaying
    /// it on a replacement would repeat the question it chose not to ask.
    pub(super) fn note_turn_boundary(&mut self, now: Instant) {
        self.generating = false;
        self.last_output_at = None;
        if std::mem::take(&mut self.prompt_behind_turn) {
            if let Some(at) = &mut self.prompted_at {
                *at = now;
            }
            return;
        }
        self.prompted_at = None;

        // A turn Gemini completes with nothing in it is the model choosing
        // silence, which the system instruction allows, on a socket that has
        // just proved it answers. Reconnecting would force the speech it chose
        // not to give, so this settles the debt; the idle nudges, not the
        // watchdog, answer a silence that goes on. `handle_gemini_event` logs
        // it, so a report of Jim going quiet can tell this from a stall.
        self.awaiting_reply_since = None;
    }

    /// Whether a replaced socket leaves the interviewer owing a reply: the
    /// candidate finished and heard nothing back, a prompt got no output, or
    /// a tool response still needs its continuation.
    pub(super) fn owes_reply(&self) -> bool {
        self.reply_in_flight()
            || self.owes_prompt()
            || self.tool_response_outstanding
            || self.reply_after_thinking_discard
    }

    /// Hands the floor back when a prompt has gone `PROMPT_STALL` without any
    /// output, and says what the watch tick may do about it. The reply stays
    /// owed either way: a replacement socket answers it. Not while earlier
    /// audio still plays, which is the floor's business, not the prompt's.
    ///
    /// The candidate's own turn stalls the same way without any prompt: they
    /// finish, Gemini sends nothing, and a `GoAway` deferred on that pending
    /// reply has no later event to be spent on. So does a tool response Gemini
    /// never continues from, which `take_if_due` would otherwise wait on until
    /// the socket drops; it becomes an owed prompt, so the replacement still
    /// answers. All of them let a held advisory go, which is what keeps the
    /// socket from being dropped by the server from an older checkpoint.
    pub(super) fn settle_stalls(&mut self, now: Instant, audio_playing: bool) -> Stalls {
        let stalled = |at: Option<Instant>| {
            at.is_some_and(|at| now.saturating_duration_since(at) >= PROMPT_STALL)
        };

        // A candidate turn that superseded the prompt is owed the same way:
        // with nothing generating, the floor the prompt took is held for it,
        // and only this releases it.
        let owed_at = self
            .prompted_at
            .or(self.awaiting_reply_since.filter(|_| !self.generating));
        let prompt_released = self.floor == Floor::Speaking && !audio_playing && stalled(owed_at);
        if prompt_released {
            self.mark_listening();
        }

        // A continuation that began and then produced nothing for as long is
        // released too, once what it did say has played. Held, it kept a
        // requested close waiting on an acknowledgement that would never
        // finish, with the watchdog standing aside for the close and nothing
        // left to provoke the event that acts on it.
        let started_and_stalled = !audio_playing && self.generation_stalled(now);
        let tool_released = self.tool_response_outstanding
            && (stalled(self.tool_response_at) || started_and_stalled);
        if tool_released {
            self.tool_response_outstanding = false;
            if let Some(at) = self.tool_response_at.take()
                && !self.owes_prompt()
            {
                self.owe_prompt(at, None);
            }
        }
        Stalls {
            prompt_released,
            spend_restart: prompt_released || tool_released || stalled(self.awaiting_reply_since),
        }
    }

    /// Silence is a valid editor-review answer. Queued audio must finish;
    /// generation that stops making progress still needs recovery.
    pub(super) fn reply_timed_out(&self, now: Instant, audio_playing: bool) -> bool {
        if audio_playing {
            return false;
        }
        if self.generating {
            let progress = self
                .last_output_at
                .into_iter()
                .chain(self.tool_response_at)
                .max();
            return progress
                .is_some_and(|at| now.saturating_duration_since(at) >= self.reply_timeout);
        }
        self.unstarted_reply_since()
            .is_some_and(|at| now.saturating_duration_since(at) >= self.reply_timeout)
    }

    /// The candidate has waited `REPLY_WAIT_SHOWN` in silence: for a reply
    /// nothing has started on, or for one that started, played what it had and
    /// then stopped producing. The same debt the watchdog recovers, read
    /// earlier, so the room says the interviewer is working on it rather than
    /// listening, or still speaking after the audio ran out.
    pub(super) fn reply_visibly_late(&self, now: Instant, audio_playing: bool) -> bool {
        let waited = if self.generating {
            self.last_output_at
        } else {
            self.unstarted_reply_since()
        };
        !audio_playing
            && waited.is_some_and(|at| now.saturating_duration_since(at) >= REPLY_WAIT_SHOWN)
    }

    /// A generation under way has produced nothing for `PROMPT_STALL`: the
    /// same rule `settle_stalls` holds a prompt to, applied to output that
    /// began and stopped.
    pub(super) fn generation_stalled(&self, now: Instant) -> bool {
        self.generating
            && self
                .last_output_at
                .is_some_and(|at| now.saturating_duration_since(at) >= PROMPT_STALL)
    }

    /// A reply is owed, or one under way has not finished: what a replaced
    /// socket leaves the next one to give, and what keeps the nudges out.
    pub(super) fn reply_unfinished(&self) -> bool {
        self.owes_reply() || self.generating
    }

    /// The oldest reply still owed with no output toward it: a candidate turn,
    /// a prompt silence does not answer, or a tool continuation.
    fn unstarted_reply_since(&self) -> Option<Instant> {
        [
            self.awaiting_reply_since,
            self.prompted_at.filter(|_| !self.prompt_allows_silence),
            self.tool_response_at
                .filter(|_| self.tool_response_outstanding),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Gemini owes a reply it has not begun to deliver.
    ///
    /// The second reader of `awaiting_reply_since`, and the reason its
    /// lifecycle is no longer only a metric's business: a `GoAway` weighs this
    /// before replacing the transport, because `Floor::Speaking` is stamped on
    /// the first audio chunk and so does not cover a turn that has issued a
    /// tool call and produced nothing yet. Tightening when the stamp is cleared
    /// now also changes when a socket is replaced.
    pub(super) fn reply_in_flight(&self) -> bool {
        self.awaiting_reply_since.is_some()
    }

    /// The first audible chunk of a reply went out: the wait it answered is
    /// over, and the floor is the agent's until its turn completes.
    pub(super) fn note_reply_audible(&mut self) {
        self.awaiting_reply_since = None;
        self.mark_speaking();
    }

    /// Gemini finished the turn, so whatever a tool response was owed has
    /// arrived; the floor waits on the queue, or is the candidate's already
    /// when nothing is left to play.
    pub(super) fn settle_completed_turn(&mut self, audio_playing: bool) {
        self.tool_response_outstanding = false;
        self.floor = Floor::AwaitingPlayout;
        if !audio_playing {
            self.mark_listening();
        }
    }

    /// Turn finished and audio drained: hand the floor back to the candidate.
    pub(super) fn mark_listening(&mut self) {
        self.floor = Floor::Listening;
        self.last_agent_speech = Instant::now();
    }

    /// Gemini has transcribed something the candidate said, so they have
    /// stopped talking and started waiting.
    ///
    /// The two stamps are not the same fact, which is why they are separate
    /// fields. `last_user_speech` feeds the idle timers and has to move on
    /// every fragment. `awaiting_reply_since` is the start of a measurable
    /// wait, and it exists only while there is a wait to measure.
    ///
    /// Not armed while Gemini is producing. Input transcription lags the
    /// audio it describes, so a fragment covering the tail of what the
    /// candidate said can land after the reply has already begun. Arming on
    /// that one made the next chunk of a turn already in progress announce
    /// itself as the reply starting, measured from a moment nobody waited from.
    /// A prompt that has produced nothing yet is not a reply under way, so the
    /// candidate speaking over it is their turn to answer.
    ///
    /// Speech during a generation is never a new turn left unarmed. The session
    /// is configured with `START_OF_ACTIVITY_INTERRUPTS`: a candidate who
    /// starts
    /// talking while Jim generates makes Gemini send `Interrupted`, ending the
    /// generation before their words are transcribed. Only the tail of speech
    /// Jim is already answering reaches here while `generating`.
    ///
    /// The same tail can outlive the generation by a moment, so a fragment
    /// within `LATE_TRANSCRIPT_GRACE` of a completed turn is not armed either.
    /// The drop of stale playout ahead of this call already handed the floor
    /// back, so this is the only place that can tell it from a new answer.
    pub(super) fn note_candidate_finished(&mut self, now: Instant, audio_playing: bool) {
        self.last_user_speech = now;
        self.transcribed_on_socket = true;

        // More speech gets its own native reply; asking as well would answer
        // twice, or over the candidate.
        self.thinking_reply_fallback = None;
        if self.floor == Floor::AwaitingPlayout && !audio_playing {
            self.mark_listening();
        }
        let late_tail = self
            .turn_completed_at
            .is_some_and(|at| now.saturating_duration_since(at) < LATE_TRANSCRIPT_GRACE);
        if !self.generating
            && !late_tail
            && !(self.floor == Floor::AwaitingPlayout && audio_playing)
        {
            self.awaiting_reply_since = Some(now);

            // The candidate has moved on past a prompt that went unanswered, so
            // their turn is what is owed now. Left set, a replacement minutes
            // later is told to answer a prompt nobody is waiting on.
            self.prompted_at = None;
        }
    }

    /// One fragment of the candidate's current utterance, against the hold.
    /// A provisional request can reverse as more of the same utterance
    /// arrives; the state methods decide what is recorded and told.
    pub(super) fn observe_thinking_fragment(
        &mut self,
        state: &mut RuntimeState,
        text: &str,
        at: Instant,
        receipt: u64,
    ) {
        if state.ended || state.paused || self.predates_hold(at) {
            return;
        }
        let Some(next) = crate::agent::thinking_change(state.thinking_hold, text) else {
            return;
        };
        if next {
            state.request_thinking();
            return;
        }
        if !state.withdraw_thinking_request() {
            state.end_thinking(receipt);
        }

        // Prompting here would answer before the candidate has finished this
        // utterance. But Gemini may already be answering it, and the hold has
        // been dropping that answer: its discard runs to the turn's end, so
        // nothing of it would be heard. The reply is asked for once it ends.
        self.reply_after_thinking_discard |= self.discarding_output;
    }

    /// Thinking was chosen at `at`. Gemini transcribes an utterance when it
    /// decides the utterance is over, and in a measured session sent the whole
    /// transcript in one message at that point, so what the candidate said just
    /// before the click arrives after it and must not read as them speaking
    /// again. The click also ends the audio stream, which brought that
    /// transcript about 0.3s later. The Live API documents an
    /// `inputTranscription.finished` that would mark the utterance's end, but
    /// did not send it in a measured session, so the window's own end is it.
    ///
    /// Not extended by what arrives inside it. Speech that starts after the
    /// click is transcribed only once Gemini's silence window has run out
    /// behind it, so its transcript cannot land inside a `window` shorter than
    /// that one; see `thinking_transcript_grace`. Extending on arrival is what
    /// let an answer given straight after the click be swallowed.
    pub(super) fn ignore_input_before_hold(&mut self, at: Instant, window: Duration) {
        self.thinking_ignore_input_until = Some(at + window);
    }

    /// Monotonic, unlike the epoch receipt: a wall-clock step inside the
    /// window would otherwise end it early or stretch it.
    fn predates_hold(&self, at: Instant) -> bool {
        self.thinking_ignore_input_until
            .is_some_and(|until| at < until)
    }

    /// The utterance behind a provisional request has ended, so the request
    /// stands: declare it.
    pub(super) fn confirm_thinking_request(
        &mut self,
        state: &mut RuntimeState,
        now: Instant,
        receipt: u64,
    ) {
        if !state.thinking_hold.is_requested() || state.ended {
            return;
        }
        self.declared_request(state, now, receipt);
    }

    /// `settle_stale_request`, for the watch tick: a request its turn's end
    /// never confirmed is confirmed here instead.
    pub(super) fn settle_stale_request(
        &mut self,
        state: &mut RuntimeState,
        now: Instant,
        receipt: u64,
    ) {
        if state.settle_stale_request(now, receipt) {
            self.reply_after_thinking_discard = false;
        }
    }

    /// A confirmed request replaces debt caused only by a tentative hold.
    fn declared_request(&mut self, state: &mut RuntimeState, now: Instant, receipt: u64) {
        self.reply_after_thinking_discard = false;
        state.declare_thinking(now, receipt);
    }

    /// What a control packet did to a provisional request, read off the
    /// reducer's result rather than the packet. A Thinking click that declared
    /// it supersedes the reply debt its tentative discard left; a yield or an
    /// ending that abandoned it owes a reply its discard dropped, once that
    /// discard's turn ends.
    pub(super) fn settle_thinking_request(
        &mut self,
        was_requested: bool,
        state: &RuntimeState,
        result: &crate::agent::DataEventResult,
    ) {
        if !was_requested {
            return;
        }
        if state.thinking_hold.is_declared() {
            self.reply_after_thinking_discard = false;
        } else if result.yield_turn || result.finish_interview.is_some() {
            self.reply_after_thinking_discard |= self.discarding_output;
        }
    }

    /// Continuing ended a provisional hold while its discard was still
    /// dropping the reply Gemini started for the request. The candidate is
    /// owed that reply, so it is asked for once the discarded turn ends; see
    /// `claim_thinking_reply`.
    pub(super) fn defer_thinking_reply(&mut self, prompt: &str) {
        if self.discarding_output {
            self.reply_after_thinking_discard = true;
            self.owe_prompt(Instant::now(), Some(prompt.to_string()));
        }
    }

    /// See `thinking_reply_fallback`.
    pub(super) fn arm_thinking_reply_fallback(&mut self, now: Instant, prompt: &str) {
        self.thinking_reply_fallback = Some((now + THINKING_REPLY_FALLBACK, prompt.to_string()));
    }

    /// The release's reply, when Gemini has produced nothing for it by its
    /// deadline and nothing now holds the floor: a turn Gemini chose not to
    /// answer, such as a filler, would otherwise leave the candidate who chose
    /// Continue waiting for the silence nudge. Taken once.
    pub(super) fn claim_thinking_reply_fallback(
        &mut self,
        state: &RuntimeState,
        now: Instant,
    ) -> Option<String> {
        let (due, _) = self.thinking_reply_fallback.as_ref()?;
        if now < *due
            || state.floor_held()
            || state.ended
            || self.floor != Floor::Listening
            || self.generating
        {
            return None;
        }
        self.thinking_reply_fallback
            .take()
            .map(|(_, prompt)| prompt)
    }

    pub(super) fn thinking_reply_prompt(&self) -> String {
        crate::agent::owed_reply(self.prompt_text.as_deref().filter(|_| self.owes_prompt()))
    }

    /// The dropped turn has ended. One the candidate cut off by speaking owes
    /// no reply of its own: that speech gets Gemini's native answer, and asking
    /// again would talk over them.
    pub(super) fn end_discard(&mut self, interrupted: bool) {
        self.discarding_output = false;
        if interrupted {
            self.reply_after_thinking_discard = false;
        }
    }

    /// The reply `defer_thinking_reply` held back is due: its discard has
    /// ended and nothing has put the floor back on hold. Taken once.
    pub(super) fn claim_thinking_reply(&mut self, state: &RuntimeState) -> bool {
        if self.discarding_output || state.floor_held() || state.ended {
            return false;
        }
        std::mem::take(&mut self.reply_after_thinking_discard)
    }

    /// Whether this pause is worth spending an idle-window review on.
    ///
    /// Every condition here is "nothing is happening": nobody holds the floor,
    /// no reply or tool response is owed, the interview is neither paused nor
    /// over, and the candidate has been quiet long enough that a call started
    /// now will probably finish before they speak again. The last one is not
    /// about the room at all -- it asks whether anything has been said since
    /// the last review, because a pause in a silent stretch is not new
    /// evidence, it is the same silence.
    ///
    /// Nothing here blocks the interview. A `false` costs a comparison, and a
    /// `true` starts a call that runs beside the room loop; the interview is
    /// never waiting on either.
    ///
    /// Stamps its own cooldown on the way out, the way `watch_prompt` below
    /// applies the stamps its decision asks for. The caller doing it instead
    /// made this the one cooldown in the file kept somewhere other than where
    /// it is read, and left half the bookkeeping for a pause two functions from
    /// the other half. `ended` is not among the conditions: the only caller is
    /// a select arm already guarded on it, and `watch_prompt` does not re-ask
    /// it either.
    pub(super) fn claim_interim_review(&mut self, state: &RuntimeState, now: Instant) -> bool {
        if self.interim_reviews >= self.max_interim_reviews {
            return false;
        }
        let due = self.interim_review_due(state, now);
        if due {
            self.last_interim = now;
            self.interim_reviews += 1;
        }
        due
    }

    /// Not once the end is due either: the interviewer has asked to close, or
    /// the planned time runs out before the call could return, and the end
    /// aborts a review still running.
    fn interim_review_due(&self, state: &RuntimeState, now: Instant) -> bool {
        let planned =
            Duration::from_secs(u64::from(state.coding_minutes + state.behavioral_minutes) * 60);
        !state.paused
            && !state.end_requested
            && now.saturating_duration_since(state.started_at)
                + crate::gemini::INTERIM_ATTEMPT_TIMEOUT
                < planned
            && self.floor == Floor::Listening
            && !self.reply_in_flight()
            && !self.tool_response_outstanding
            && now.duration_since(self.last_user_speech) >= INTERIM_IDLE
            && now.duration_since(self.last_interim) >= INTERIM_COOLDOWN
            && candidate_lines(&state.transcript[unreviewed_from(state)..]) >= INTERIM_MIN_NEW_TURNS
    }

    /// `now` is passed in rather than sampled here, like every other method on
    /// this struct. Sampling internally makes the boundaries untestable: a test
    /// can set `last_code_change` to exactly `CODE_SETTLE` ago, but the clock
    /// read inside would already have moved past it, so a half-open window and
    /// a closed one behave identically to every test that can be written.
    pub(super) fn watch_prompt(
        &mut self,
        state: &mut RuntimeState,
        now: Instant,
    ) -> Option<WatchPrompt> {
        let behavioral = state.behavioral_round_started;

        // The behavioral round has no reviews, so once its one nudge is spent
        // there is nothing left to watch for.
        if state.floor_held() || (behavioral && self.behavioral_nudged) {
            return None;
        }
        let decision = timing_decision(&TimingInput {
            agent_busy: self.floor != Floor::Listening,
            user_talking: now.duration_since(self.last_code_change) < CODE_SETTLE,
            idle_seconds: now
                .duration_since(
                    self.last_user_speech
                        .max(self.last_code_change)
                        .max(self.last_agent_speech),
                )
                .as_secs_f64(),
            since_last_nudge_seconds: now.duration_since(self.last_nudge).as_secs_f64(),
            since_last_review_seconds: now.duration_since(self.last_review).as_secs_f64(),
            since_last_interjection_seconds: now
                .duration_since(self.last_interjection)
                .as_secs_f64(),
            speech_gap_seconds: now
                .duration_since(self.last_user_speech.max(self.last_agent_speech))
                .as_secs_f64(),

            // A change worth a review, in code that parses now: a review of a
            // half-typed line is an "mm-hm" about something still being
            // written, and the next change that parses arms it again. Editor
            // changes cannot reopen coding during the behavioral round.
            significant_change: !behavioral
                && state.evidence_ledger.code.substantive_revision
                    > self.substantive_revision_at_last_review
                && state.evidence_ledger.code.parser_observation
                    == Some(crate::agent::CodeObservation::Parsed),
        });
        if decision.update_last_nudge {
            self.last_nudge = now;
        }
        if decision.update_last_review {
            self.last_review = now;
        }
        if decision.update_last_interjection {
            self.last_interjection = now;
        }
        if !(decision.silence_nudge || decision.proactive_review) {
            return None;
        }

        // Carries no evidence and no code, so it moves nothing the model has
        // been shown and leaves nothing for a failed send to put back.
        if decision.silence_nudge && behavioral {
            self.unsent_watch = None;
            return Some(WatchPrompt {
                text: with_timer(state, behavioral_silence_nudge()),
                behavioral_nudge: true,
                allows_silence: false,
            });
        }
        let excerpt = changed_excerpt(&state.language, &state.code_shown, &state.code);
        let lines = state.evidence_ledger.prompt_view(ViewFor::Watch);
        let evidence = evidence_delta(self.evidence_shown.as_deref(), &lines);

        // A nudge shows the code as a review does, so both move what the model
        // has seen; only a review consumes the change that armed it.
        let mut unsent = UnsentWatch {
            revision: None,
            code_shown: std::mem::replace(&mut state.code_shown, state.code.clone()),
            evidence_shown: self.evidence_shown.replace(lines),
        };
        if decision.sync_revision_at_last_review {
            unsent.revision = Some(std::mem::replace(
                &mut self.substantive_revision_at_last_review,
                state.evidence_ledger.code.substantive_revision,
            ));
        }

        // Returned uncounted. Anything that goes out over the live socket is
        // counted at the door, by `send_model_text`, and counting it here as
        // well would have been two answers to one question: a prompt this
        // returns is not always sent, because the caller gives up on a socket
        // Gemini has already closed.
        //
        // The interim review and the final report are the two that are counted
        // where they are built instead, because neither touches the socket --
        // both are one-shot HTTP calls, and the interim prompt is handed to a
        // spawned task that never sees the ledger.
        self.unsent_watch = Some(unsent);

        // Named by the branch that wrote the text, so the flag cannot describe
        // a different prompt from the one sent.
        let (text, allows_silence) = if decision.silence_nudge {
            // A board has no text to quote, so the two prompts differ in what
            // they can carry rather than only in wording; see
            // `board_silence_nudge`.
            let nudge = if state.interview_mode.is_whiteboard() {
                board_silence_nudge(&evidence, state.board_strokes)
            } else {
                silence_nudge(state, &evidence, excerpt.as_deref())
            };
            (nudge, false)
        } else {
            (proactive_review(state, &evidence, excerpt.as_deref()), true)
        };
        Some(WatchPrompt {
            text: with_timer(state, text),
            behavioral_nudge: false,
            allows_silence,
        })
    }

    /// Puts back what the last watch prompt moved, for one the socket refused.
    /// Those markers say what the model has seen, and a prompt that died on a
    /// closed socket showed it nothing: left moved, the next prompt on a
    /// resumed session that kept its context skipped the change and the
    /// evidence this one carried. The timing stamps stay moved, since the
    /// socket is dead until the close is reported and restoring them would
    /// build and fail a prompt on every tick until then.
    pub(super) fn unsend_watch_prompt(&mut self, state: &mut RuntimeState) {
        let Some(unsent) = self.unsent_watch.take() else {
            return;
        };
        if let Some(revision) = unsent.revision {
            self.substantive_revision_at_last_review = revision;
        }
        state.code_shown = unsent.code_shown;
        self.evidence_shown = unsent.evidence_shown;
    }
}

/// The watch evidence lines the model does not hold yet: every line that
/// differs from the last ones shown, and `key: none` for a line that was shown
/// and has since gone. A line vanishing said nothing, so a session flag that
/// cleared stayed in the model's context as though it still held.
fn evidence_delta(shown: Option<&[String]>, lines: &[String]) -> String {
    let key = |line: &str| {
        line.split_once(':')
            .map_or(line, |(key, _)| key)
            .to_string()
    };
    let mut delta = lines
        .iter()
        .filter(|line| shown.is_none_or(|shown| !shown.contains(line)))
        .cloned()
        .collect::<Vec<_>>();
    let current = lines.iter().map(|line| key(line)).collect::<Vec<_>>();
    for gone in shown.into_iter().flatten().map(|line| key(line)) {
        if !current.contains(&gone) {
            delta.push(format!("{gone}: none"));
        }
    }
    delta.join("\n")
}

/// What a watch prompt moved, as it was before: the code and the evidence the
/// model had been shown, and the review baseline when it was a review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct UnsentWatch {
    revision: Option<u64>,
    code_shown: String,
    evidence_shown: Option<Vec<String>>,
}

/// What one interview accumulates, minus the two things the select loop borrows
/// as futures. `gemini` and `media` have to stay outside: their arms hold a
/// mutable borrow for as long as the select is polled, so bundling them here
/// would make every other arm's condition fight the borrow checker.
///
/// The four that are left exist to make [`TurnState::context`] possible. Three
/// arms of the select need a `GeminiEventContext`, and it borrows seven things
/// mutably, so it lives exactly as long as the call it is handed to. That used
/// to be a macro, purely to stop the field list being written out three times.
///
/// Worth knowing before anyone tries to go further: bundling these four and
/// stopping there makes the function *longer*, because every use site grows a
/// prefix and the macro stays. It only pays once the method exists to replace
/// the macro outright.
pub(super) struct TurnState {
    pub(super) state: RuntimeState,
    pub(super) agent_state: String,
    pub(super) activity: RuntimeActivity,
    pub(super) turns: SpeakerTurns,
}

/// Whether a candidate talking over the interviewer cuts it short.
///
/// Carried as an argument rather than a flag on the context. It was a field
/// that `send_wrap_up_and_wait` set and restored, which is one early return
/// away from leaving barge-in off for good, and it read as state when it is
/// really a property of the turn being handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Interruptible {
    /// An ordinary exchange. The candidate speaking wins the floor.
    Yes,
    /// The closing message. Cutting it leaves the candidate without the ending,
    /// and the wrap-up wait reads the emptied queue as the turn being over, so
    /// a "thanks" mid sentence ended the interview there.
    No,
}

/// One open turn per speaker. Gemini interleaves input and output
/// transcription, so they cannot share a single accumulator.
#[derive(Debug, Default)]
pub(super) struct SpeakerTurns {
    pub(super) interviewer: SpeakerTurn,
    pub(super) candidate: SpeakerTurn,
}

/// The two speakers in the order their turns opened.
///
/// Both can be open at once, and whichever is written to the ledger first is a
/// claim about who spoke first. Closing them in a fixed speaker order made that
/// claim out of the source: a candidate answer that finished while a late
/// interviewer fragment was still arriving was recorded after the question it
/// had already answered, and a reader of the ledger saw an answer that preceded
/// nothing. A turn's transcript line is where it opened, so the lower line
/// spoke first. A speaker with no line has nothing to record and sorts last.
pub(super) fn closing_order(
    interviewer_line: Option<usize>,
    candidate_line: Option<usize>,
) -> [&'static str; 2] {
    match (interviewer_line, candidate_line) {
        (Some(interviewer), Some(candidate)) if candidate < interviewer => {
            ["candidate", "interviewer"]
        }
        (None, Some(_)) => ["candidate", "interviewer"],
        _ => ["interviewer", "candidate"],
    }
}

/// Why the room loop ended an interview it could no longer keep on a Gemini
/// socket. The report is written over HTTP from what this process holds, so
/// losing the interviewer costs the goodbye and not the assessment.
pub(super) const INTERVIEWER_UNAVAILABLE: &str = "interviewer_unavailable";

/// No goodbye for a candidate who left, and none from an interviewer that
/// stopped answering: asking it for one would only spend `WRAP_UP_WAIT` in
/// silence before the report the candidate is waiting for.
pub(super) fn should_send_wrap_up(reason: &str) -> bool {
    reason != "candidate_ended" && reason != INTERVIEWER_UNAVAILABLE
}

#[cfg(test)]
#[path = "../../tests/unit/livekit/turn.rs"]
mod tests;

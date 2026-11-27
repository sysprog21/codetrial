//! What a socket's death, and the one that replaces it, do to the activity.

use std::time::Instant;

use super::super::{RuntimeActivity, SocketWork};
use crate::agent::RuntimeState;

impl RuntimeActivity {
    /// What the dead socket still owed, for the replacement log line and the
    /// briefing: whether a reply is owed, why (the candidate's turn, the number
    /// of a prompt still unanswered or `none`, a tool continuation), and the
    /// prompt's own text so the briefing can name what to answer rather than
    /// leave the new socket to guess.
    ///
    /// Read before the cut clears the pending work: a reply the old socket owed
    /// is still owed, and the replacement is the only one left to give it.
    pub(in crate::livekit) fn take_handover_debt(&mut self) -> (bool, String, Option<String>) {
        let owes_prompt = self.owes_prompt();
        let owed_prompt = self.prompt_text.take().filter(|_| owes_prompt);
        let prompt = if owes_prompt {
            self.prompt_sequence.to_string()
        } else {
            "none".to_string()
        };
        let debt = format!(
            "candidate={} prompt={prompt} tool={} generating={}",
            self.reply_in_flight(),
            self.socket.tool_response_outstanding,
            self.generating,
        );
        (self.reply_unfinished(), debt, owed_prompt)
    }

    /// Drops work that could only have been completed by the replaced socket.
    ///
    /// A close request follows its tool acknowledgement: without that
    /// generation, the loop must not turn a request from the old socket into a
    /// closing from the new one. The other two flags are the same kind of
    /// debt. A closed socket cannot send the event that clears them, and
    /// carrying either into its replacement makes the new interviewer discard
    /// output or wait on work it did not create.
    pub(in crate::livekit) fn abandon_socket_work(&mut self, state: &mut RuntimeState) {
        self.buffered_startup_output_at = None;
        self.socket = SocketWork::default();
        state.recovery_reply_pending = false;

        // A provisional request waits on its utterance's end, which the closed
        // socket will not send. A declared hold is the candidate's and
        // survives.
        state.withdraw_thinking_request();
        state.end_requested = false;
    }

    /// Neither kind of replacement holds the evidence lines the next watch
    /// prompt would otherwise skip: a cold one has seen none, and a resumed one
    /// restarts from a checkpoint that may predate the latest of them. The same
    /// goes for the editor snapshot it was last handed.
    pub(in crate::livekit) fn forget_delivered_context(&mut self) {
        self.evidence_shown = None;
        self.silent_editor_delivery = None;
    }

    /// The startup probe saw output before the room loop was running, at
    /// `started_at`: the opening is already being answered, so the reply wait
    /// resumes from there, and the probe's output is held until the loop
    /// replays it. The wall-clock counterpart of `started_at` survives socket
    /// replacement, so a reconnect cannot grant a fresh clock.
    pub(in crate::livekit) fn note_startup_output_seen(&mut self, started_at: Instant) {
        self.resume_reply_wait(started_at);
        self.buffered_startup_output_at = Some(started_at);
        self.startup_ready_at = Some(
            crate::current_epoch_millis()
                .saturating_sub(crate::agent::saturating_ms(started_at.elapsed())),
        );
    }

    /// A speech handoff briefed the new socket to keep listening and the old
    /// one still owed a reply: the reply is owed again, and it waits on audio
    /// that may never come.
    pub(in crate::livekit) fn owe_handoff_reply(&mut self, now: Instant, text: Option<String>) {
        self.owe_prompt(now, text);
        self.arm_handoff_reply(now);
    }

    /// The editor snapshot `editor` (its revision and language) reached the
    /// model together with `code`, so neither is sent again until it changes.
    pub(in crate::livekit) fn record_editor_delivered(
        &mut self,
        state: &mut RuntimeState,
        editor: (u64, String),
        code: String,
    ) {
        state.code_shown = code;
        self.silent_editor_delivery = Some(editor);
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/livekit/turn/activity/handover.rs"]
mod tests;

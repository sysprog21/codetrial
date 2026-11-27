//! The agent's state as the page sees it: listening, thinking or speaking, and
//! the one task that writes it to the room, with the turn window the page
//! draws its countdown from.

// This module reads the hub's imports and types through `use
// crate::livekit::*`.
#[cfg_attr(
    not(test),
    expect(
        clippy::wildcard_imports,
        reason = "the module reads its hub through crate::livekit::*"
    )
)]
use crate::livekit::*;
use std::collections::HashMap;

/// The state the page shows, and the one task that writes it to the room.
///
/// The write is a LiveKit round trip, and awaited in the room loop it held up
/// the reply's next chunk and the microphone frames behind it just as a reply
/// began. One writer keeps the writes in order and skips straight to the
/// latest when they come faster than it can send. A failed write is logged and
/// left for the next state change to correct.
#[derive(Default)]
pub(crate) struct AgentState {
    pub(in crate::livekit) current: &'static str,
    pub(in crate::livekit) writer: Option<tokio::sync::watch::Sender<&'static str>>,
}

impl AgentState {
    pub(in crate::livekit) fn current(&self) -> &'static str {
        self.current
    }
}

pub(in crate::livekit) fn set_agent_state(
    room: &RoomLink,
    state: &mut AgentState,
    next: &'static str,
) {
    if state.current == next {
        return;
    }
    state.current = next;
    let writer = state.writer.get_or_insert_with(|| {
        let (writer, mut latest) = tokio::sync::watch::channel("");
        let participant = room.participant();
        tokio::spawn(async move {
            // Ends with the interview, when its state is dropped.
            while latest.changed().await.is_ok() {
                let next = *latest.borrow_and_update();
                if let Err(error) = participant
                    .set_attributes(agent_state_attributes(participant.attributes(), next))
                    .await
                {
                    eprintln!("agent state {next} not published: {error}");
                }
            }
        });
        writer
    });
    writer.send_replace(next);
}

/// How long Gemini waits through the candidate's silence before taking the
/// turn, so the page can draw the countdown to it. An attribute rather than a
/// message: it is fixed for the interview, and a page that joins or reloads
/// after the agent reads it without asking. `TURN_WINDOW_ATTRIBUTE` in
/// `web/lib.js` names the same key.
pub(in crate::livekit) const TURN_WINDOW_ATTRIBUTE: &str = "codetrial.silence_ms";

/// The agent's first attributes, listening and the turn window, in the one
/// round trip the room pays before it waits for the candidate.
pub(in crate::livekit) async fn publish_opening_attributes(
    room: &RoomLink,
    current: &mut AgentState,
    silence_ms: u32,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let participant = room.participant();
    let attributes = agent_state_attributes(participant.attributes(), AGENT_STATE_LISTENING);
    participant
        .set_attributes(turn_window_attributes(attributes, silence_ms))
        .await?;
    current.current = AGENT_STATE_LISTENING;
    Ok(())
}

pub(in crate::livekit) fn turn_window_attributes(
    mut attributes: HashMap<String, String>,
    silence_ms: u32,
) -> HashMap<String, String> {
    attributes.insert(TURN_WINDOW_ATTRIBUTE.to_string(), silence_ms.to_string());
    attributes
}

pub(in crate::livekit) fn agent_state_attributes(
    mut attributes: HashMap<String, String>,
    state: &str,
) -> HashMap<String, String> {
    attributes.insert(LIVEKIT_AGENT_STATE.to_string(), state.to_string());
    attributes
}

#[cfg(test)]
#[path = "../../../tests/unit/livekit/turn/agent_state.rs"]
mod tests;

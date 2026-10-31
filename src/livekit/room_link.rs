//! The room as the interview's handlers reach it.
//!
//! Every handler used to take the LiveKit `Room` itself, which only a
//! connected server can produce, so none of them could run in a test: the
//! decisions were tested by splitting room-free halves out of them, and the
//! order those halves run in, which is most of what goes wrong, was not
//! tested at all. What the handlers do with the room is small: name it in a
//! log line, send the page a data packet, set the agent's attributes, send a
//! transcript segment, and leave. `RoomLink` is those five, over the live
//! room in the server and over a recorder in tests, which compiles only into
//! the test build.
//!
//! What needs more than that, the media tracks, the room's event stream at
//! startup and the report's delivery receipts, still takes the connected room
//! through [`RoomLink::connected`].

use std::collections::HashMap;

use ::livekit::data_stream::api::StreamTextOptions;
use ::livekit::prelude::{DataPacket, LocalParticipant, Room};

type Error = Box<dyn std::error::Error + Send + Sync>;

pub(crate) struct RoomLink {
    /// Read once: a room keeps its name, and every log line asks for it.
    name: String,
    inner: Inner,
}

enum Inner {
    Live(Room),
    #[cfg(test)]
    Recorded(std::sync::Arc<Recorder>),
}

impl RoomLink {
    pub(super) fn live(room: Room) -> Self {
        Self {
            name: room.name(),
            inner: Inner::Live(room),
        }
    }

    #[cfg(test)]
    pub(super) fn observe_live_reader(&self, session: &crate::gemini::GeminiLiveSession) {
        if let Inner::Recorded(recorder) = &self.inner {
            recorder.readers.lock().unwrap().push(
                session
                    .inspect_reader_for_test()
                    .expect("completed setup owns its reader"),
            );
        }
    }

    #[cfg(test)]
    pub(super) fn take_eviction_seed(&self) -> Vec<String> {
        match &self.inner {
            Inner::Recorded(recorder) => {
                std::mem::take(&mut *recorder.eviction_seed.lock().unwrap())
            }
            Inner::Live(_) => Vec::new(),
        }
    }

    pub(super) fn name(&self) -> &str {
        &self.name
    }

    /// The connected room, for what only it can do: media tracks, the event
    /// stream at startup, and the report's delivery receipts. A test link has
    /// none, and reaching here from one means the test drove a path it cannot
    /// cover.
    pub(super) fn connected(&self) -> &Room {
        match &self.inner {
            Inner::Live(room) => room,
            #[cfg(test)]
            Inner::Recorded(_) => panic!("a recorded room has no connection to hand out"),
        }
    }

    pub(super) async fn publish_data(&self, packet: DataPacket) -> Result<(), Error> {
        self.participant().publish_data(packet).await
    }

    /// The agent participant, for a writer task that outlives one call.
    pub(super) fn participant(&self) -> ParticipantLink {
        match &self.inner {
            Inner::Live(room) => ParticipantLink::Live(room.local_participant()),
            #[cfg(test)]
            Inner::Recorded(recorder) => ParticipantLink::Recorded(std::sync::Arc::clone(recorder)),
        }
    }

    pub(super) async fn close(&self) -> Result<(), Error> {
        match &self.inner {
            Inner::Live(room) => Ok(room.close().await?),
            #[cfg(test)]
            Inner::Recorded(recorder) => {
                recorder.push(Sent::Left);
                Ok(())
            }
        }
    }
}

/// The agent participant as its writer tasks reach it.
#[derive(Clone)]
pub(super) enum ParticipantLink {
    Live(LocalParticipant),
    #[cfg(test)]
    Recorded(std::sync::Arc<Recorder>),
}

impl ParticipantLink {
    pub(super) fn attributes(&self) -> HashMap<String, String> {
        match self {
            Self::Live(participant) => participant.attributes(),
            #[cfg(test)]
            Self::Recorded(recorder) => recorder.attributes(),
        }
    }

    pub(super) async fn publish_data(&self, packet: DataPacket) -> Result<(), Error> {
        match self {
            Self::Live(participant) => Ok(participant.publish_data(packet).await?),
            #[cfg(test)]
            Self::Recorded(recorder) => {
                let gate = recorder.gate(&packet);
                if let Some((entered, release)) = gate {
                    entered.notify_one();
                    release.notified().await;
                }
                recorder.data(packet);
                Ok(())
            }
        }
    }

    pub(super) async fn set_attributes(
        &self,
        attributes: HashMap<String, String>,
    ) -> Result<(), Error> {
        match self {
            Self::Live(participant) => Ok(participant.set_attributes(attributes).await?),
            #[cfg(test)]
            Self::Recorded(recorder) => {
                recorder.set_attributes(attributes);
                Ok(())
            }
        }
    }

    pub(super) async fn send_text(
        &self,
        text: &str,
        options: StreamTextOptions,
    ) -> Result<(), Error> {
        match self {
            Self::Live(participant) => {
                participant.send_text(text, options).await?;
                Ok(())
            }
            #[cfg(test)]
            Self::Recorded(recorder) => {
                recorder.push(Sent::Text {
                    topic: options.topic,
                    text: text.to_string(),
                    attributes: options.attributes,
                });
                Ok(())
            }
        }
    }
}

/// What a test room was sent, in order.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Sent {
    Data {
        topic: Option<String>,
        message: serde_json::Value,
    },
    Attributes(HashMap<String, String>),
    Text {
        topic: String,
        text: String,
        attributes: HashMap<String, String>,
    },
    Left,
}

#[cfg(test)]
#[derive(Default)]
pub(super) struct Recorder {
    sent: std::sync::Mutex<Vec<Sent>>,
    attributes: std::sync::Mutex<HashMap<String, String>>,
    gates: std::sync::Mutex<Vec<DataGate>>,
    readers: std::sync::Mutex<Vec<crate::gemini::LiveReaderInspection>>,
    backoffs: std::sync::Mutex<Vec<tokio::time::Instant>>,
    eviction_seed: std::sync::Mutex<Vec<String>>,
}

#[cfg(test)]
struct DataGate {
    message_type: &'static str,
    reconnecting: Option<bool>,
    wait_seconds: Option<bool>,
    require_reason: bool,
    entered: std::sync::Arc<tokio::sync::Notify>,
    release: std::sync::Arc<tokio::sync::Notify>,
}

#[cfg(test)]
impl Recorder {
    // This seeds the private work queue, not SDK participant classification.
    pub(super) fn seed_evictions(&self, identities: &[&str]) {
        assert!(identities.len() <= 32);
        *self.eviction_seed.lock().unwrap() = identities.iter().map(|s| (*s).to_owned()).collect();
    }

    pub(super) fn hold_data(
        &self,
        message_type: &'static str,
        reconnecting: Option<bool>,
    ) -> (
        std::sync::Arc<tokio::sync::Notify>,
        std::sync::Arc<tokio::sync::Notify>,
    ) {
        self.hold_matching(message_type, reconnecting, None, false)
    }

    pub(super) fn hold_retry_notice(
        &self,
        wait_seconds: bool,
    ) -> (
        std::sync::Arc<tokio::sync::Notify>,
        std::sync::Arc<tokio::sync::Notify>,
    ) {
        self.hold_matching("interviewer_state", Some(true), Some(wait_seconds), true)
    }

    pub(super) fn take_live_readers(&self) -> Vec<crate::gemini::LiveReaderInspection> {
        std::mem::take(&mut *self.readers.lock().unwrap())
    }

    pub(super) fn take_retry_backoffs(&self) -> Vec<tokio::time::Instant> {
        std::mem::take(&mut *self.backoffs.lock().unwrap())
    }

    fn hold_matching(
        &self,
        message_type: &'static str,
        reconnecting: Option<bool>,
        wait_seconds: Option<bool>,
        require_reason: bool,
    ) -> (
        std::sync::Arc<tokio::sync::Notify>,
        std::sync::Arc<tokio::sync::Notify>,
    ) {
        let entered = std::sync::Arc::new(tokio::sync::Notify::new());
        let release = std::sync::Arc::new(tokio::sync::Notify::new());
        self.gates.lock().unwrap().push(DataGate {
            message_type,
            reconnecting,
            wait_seconds,
            require_reason,
            entered: entered.clone(),
            release: release.clone(),
        });
        (entered, release)
    }

    fn gate(
        &self,
        packet: &DataPacket,
    ) -> Option<(
        std::sync::Arc<tokio::sync::Notify>,
        std::sync::Arc<tokio::sync::Notify>,
    )> {
        let message: serde_json::Value = serde_json::from_slice(&packet.payload).ok()?;
        let mut gates = self.gates.lock().unwrap();
        let index = gates.iter().position(|gate| {
            message["type"] == gate.message_type
                && gate
                    .reconnecting
                    .is_none_or(|value| message["reconnecting"].as_bool() == Some(value))
                && gate
                    .wait_seconds
                    .is_none_or(|present| message.get("waitSeconds").is_some() == present)
                && (!gate.require_reason || message.get("reason").is_some())
        })?;
        let gate = gates.remove(index);
        Some((gate.entered, gate.release))
    }

    fn push(&self, sent: Sent) {
        self.sent.lock().unwrap().push(sent);
    }

    fn data(&self, packet: DataPacket) {
        let message = serde_json::from_slice(&packet.payload).unwrap_or_else(|_| {
            serde_json::Value::String(String::from_utf8_lossy(&packet.payload).into())
        });
        self.push(Sent::Data {
            topic: packet.topic,
            message,
        });
    }

    fn attributes(&self) -> HashMap<String, String> {
        self.attributes.lock().unwrap().clone()
    }

    fn set_attributes(&self, attributes: HashMap<String, String>) {
        self.attributes.lock().unwrap().clone_from(&attributes);
        self.push(Sent::Attributes(attributes));
    }

    /// Everything sent so far, cleared.
    pub(super) fn take(&self) -> Vec<Sent> {
        std::mem::take(&mut *self.sent.lock().unwrap())
    }
}

#[cfg(test)]
impl RoomLink {
    /// A room that records what the handlers send it.
    pub(super) fn recording(name: &str) -> (Self, std::sync::Arc<Recorder>) {
        let recorder = std::sync::Arc::new(Recorder::default());
        let link = Self {
            name: name.to_string(),
            inner: Inner::Recorded(std::sync::Arc::clone(&recorder)),
        };
        (link, recorder)
    }
}

#[cfg(test)]
pub(super) async fn observe_retry_backoff(
    room: Option<&RoomLink>,
    until: tokio::time::Instant,
    sleep: tokio::time::Sleep,
) {
    use std::future::Future;
    tokio::pin!(sleep);
    let mut announced = false;
    std::future::poll_fn(|cx| {
        let state = sleep.as_mut().poll(cx);
        if state.is_pending() && !announced {
            announced = true;
            if let Some(RoomLink {
                inner: Inner::Recorded(recorder),
                ..
            }) = room
            {
                recorder.backoffs.lock().unwrap().push(until);
            }
        }
        state
    })
    .await;
}

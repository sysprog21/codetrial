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

use ::livekit::data_stream::api::SendTextOptions;
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
    pub(in crate::livekit) fn live(room: Room) -> Self {
        Self {
            name: room.name(),
            inner: Inner::Live(room),
        }
    }

    #[cfg(test)]
    pub(in crate::livekit) fn observe_live_reader(
        &self,
        session: &crate::gemini::GeminiLiveSession,
    ) {
        if let Inner::Recorded(recorder) = &self.inner {
            recorder.note_reader(
                session
                    .inspect_reader_for_test()
                    .expect("completed setup owns its reader"),
            );
        }
    }

    #[cfg(test)]
    pub(in crate::livekit) fn take_eviction_seed(&self) -> Vec<String> {
        match &self.inner {
            Inner::Recorded(recorder) => recorder.take_eviction_seed(),
            Inner::Live(_) => Vec::new(),
        }
    }

    pub(in crate::livekit) fn name(&self) -> &str {
        &self.name
    }

    /// Leaves the room when `result` is an error, and hands the result back.
    /// Dropping a room does not disconnect it, so an exit that fails after the
    /// join would leave the candidate with a silent agent in the room.
    pub(in crate::livekit) async fn close_on_error<T, E>(
        &self,
        result: Result<T, E>,
    ) -> Result<T, E> {
        if result.is_err() {
            self.leave().await;
        }
        result
    }

    /// The connected room, for what only it can do: media tracks, the event
    /// stream at startup, and the report's delivery receipts. A test link has
    /// none, and reaching here from one means the test drove a path it cannot
    /// cover.
    pub(in crate::livekit) fn connected(&self) -> &Room {
        match &self.inner {
            Inner::Live(room) => room,
            #[cfg(test)]
            Inner::Recorded(_) => panic!("a recorded room has no connection to hand out"),
        }
    }

    pub(in crate::livekit) async fn publish_data(&self, packet: DataPacket) -> Result<(), Error> {
        self.participant().publish_data(packet).await
    }

    /// A publish the room loop awaits for a notice the interview does not
    /// depend on. A reconnecting connection holds a publish for up to a
    /// minute, and the loop drains audio and fires the hard deadline, so the
    /// wait is bounded and a timeout is an error like any other failure.
    pub(in crate::livekit) async fn publish_data_within_notice_limit(
        &self,
        packet: DataPacket,
    ) -> Result<(), Error> {
        tokio::time::timeout(
            crate::livekit::RECONNECT_NOTICE_LIMIT,
            self.publish_data(packet),
        )
        .await
        .unwrap_or_else(|_| Err("timed out".into()))
    }

    /// The agent participant, for a writer task that outlives one call.
    pub(in crate::livekit) fn participant(&self) -> ParticipantLink {
        match &self.inner {
            Inner::Live(room) => ParticipantLink::Live(room.local_participant()),
            #[cfg(test)]
            Inner::Recorded(recorder) => ParticipantLink::Recorded(std::sync::Arc::clone(recorder)),
        }
    }

    /// Disconnects this agent, which is all `Room::close` does: the room and
    /// the
    /// candidate in it outlive this by whatever LiveKit's own timeouts say.
    /// Named
    /// for what happens rather than for what the call is called, because every
    /// caller is deciding whether to abandon an interview, and "close the room"
    /// reads like the room goes with it.
    pub(in crate::livekit) async fn leave(&self) {
        if let Err(error) = self.close().await {
            eprintln!("leaving the room failed, ignoring: {error}");
        }
    }

    pub(in crate::livekit) async fn close(&self) -> Result<(), Error> {
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
pub(in crate::livekit) enum ParticipantLink {
    Live(LocalParticipant),
    #[cfg(test)]
    Recorded(std::sync::Arc<Recorder>),
}

impl ParticipantLink {
    pub(in crate::livekit) fn attributes(&self) -> HashMap<String, String> {
        match self {
            Self::Live(participant) => participant.attributes(),
            #[cfg(test)]
            Self::Recorded(recorder) => recorder.attributes(),
        }
    }

    pub(in crate::livekit) async fn publish_data(&self, packet: DataPacket) -> Result<(), Error> {
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

    pub(in crate::livekit) async fn set_attributes(
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

    pub(in crate::livekit) async fn send_text(
        &self,
        text: &str,
        options: SendTextOptions,
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
                    sender_identity: options.sender_identity.map(|identity| identity.to_string()),
                });
                Ok(())
            }
        }
    }
}

/// What a test room was sent, in order.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
pub(in crate::livekit) enum Sent {
    Data {
        topic: Option<String>,
        message: serde_json::Value,
    },
    Attributes(HashMap<String, String>),
    Text {
        topic: String,
        text: String,
        attributes: HashMap<String, String>,
        sender_identity: Option<String>,
    },
    Left,
}

#[cfg(test)]
#[derive(Default)]
pub(in crate::livekit) struct Recorder {
    sent: std::sync::Mutex<Vec<Sent>>,
    data_destinations: std::sync::Mutex<Vec<Vec<String>>>,
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
    fn note_reader(&self, reader: crate::gemini::LiveReaderInspection) {
        self.readers.lock().unwrap().push(reader);
    }

    fn note_backoff(&self, until: tokio::time::Instant) {
        self.backoffs.lock().unwrap().push(until);
    }

    fn take_eviction_seed(&self) -> Vec<String> {
        std::mem::take(&mut *self.eviction_seed.lock().unwrap())
    }

    // This seeds the private work queue, not SDK participant classification.
    pub(in crate::livekit) fn seed_evictions(&self, identities: &[&str]) {
        assert!(identities.len() <= 32);
        *self.eviction_seed.lock().unwrap() = identities.iter().map(|s| (*s).to_owned()).collect();
    }

    pub(in crate::livekit) fn hold_data(
        &self,
        message_type: &'static str,
        reconnecting: Option<bool>,
    ) -> (
        std::sync::Arc<tokio::sync::Notify>,
        std::sync::Arc<tokio::sync::Notify>,
    ) {
        self.hold_matching(message_type, reconnecting, None, false)
    }

    pub(in crate::livekit) fn hold_retry_notice(
        &self,
        wait_seconds: bool,
    ) -> (
        std::sync::Arc<tokio::sync::Notify>,
        std::sync::Arc<tokio::sync::Notify>,
    ) {
        self.hold_matching("interviewer_state", Some(true), Some(wait_seconds), true)
    }

    pub(in crate::livekit) fn take_live_readers(&self) -> Vec<crate::gemini::LiveReaderInspection> {
        std::mem::take(&mut *self.readers.lock().unwrap())
    }

    pub(in crate::livekit) fn take_retry_backoffs(&self) -> Vec<tokio::time::Instant> {
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
            entered: std::sync::Arc::clone(&entered),
            release: std::sync::Arc::clone(&release),
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
        self.data_destinations.lock().unwrap().push(
            packet
                .destination_identities
                .iter()
                .map(ToString::to_string)
                .collect(),
        );
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
    pub(in crate::livekit) fn take(&self) -> Vec<Sent> {
        std::mem::take(&mut *self.sent.lock().unwrap())
    }

    pub(in crate::livekit) fn take_data_destinations(&self) -> Vec<Vec<String>> {
        std::mem::take(&mut *self.data_destinations.lock().unwrap())
    }
}

#[cfg(test)]
impl RoomLink {
    /// A room that records what the handlers send it.
    pub(in crate::livekit) fn recording(name: &str) -> (Self, std::sync::Arc<Recorder>) {
        let recorder = std::sync::Arc::new(Recorder::default());
        let link = Self {
            name: name.to_string(),
            inner: Inner::Recorded(std::sync::Arc::clone(&recorder)),
        };
        (link, recorder)
    }
}

#[cfg(test)]
pub(in crate::livekit) async fn observe_retry_backoff(
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
                recorder.note_backoff(until);
            }
        }
        state
    })
    .await;
}

#[cfg(test)]
#[path = "../../../tests/unit/livekit/room/room_link.rs"]
mod tests;

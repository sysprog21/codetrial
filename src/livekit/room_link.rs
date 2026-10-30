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
    data_destinations: std::sync::Mutex<Vec<Vec<String>>>,
    attributes: std::sync::Mutex<HashMap<String, String>>,
}

#[cfg(test)]
impl Recorder {
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
    pub(super) fn take(&self) -> Vec<Sent> {
        std::mem::take(&mut *self.sent.lock().unwrap())
    }

    pub(super) fn take_data_destinations(&self) -> Vec<Vec<String>> {
        std::mem::take(&mut *self.data_destinations.lock().unwrap())
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

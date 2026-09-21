//! The candidate's whiteboard, from the data channel to Gemini's eyes.
//!
//! Its own module because a board does not arrive the way anything else the
//! browser sends does. Every other message is one small JSON packet, handled
//! where it is received; a board is tens of kilobytes of JPEG, several times
//! what a single LiveKit data packet carries, so it comes as a byte stream
//! that has to be drained before it is a message at all. Draining it inside
//! the room loop would park that loop on a read while the candidate's audio
//! queued behind it, so what the loop sees here is a channel of finished
//! boards and nothing else.
//!
//! Gemini sees a board the way it sees a camera frame: a realtime image on the
//! live socket. That is the only way an image can reach it mid-session, and it
//! is why `read_board` cannot answer with the picture itself -- a tool
//! response is JSON. The tool asks for the board to be sent again instead, and
//! `resend` is what answers.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ::livekit::data_stream::api::{ByteStreamReader, StreamReader};
use ::livekit::prelude::RoomEvent;
use futures_util::StreamExt;
use tokio::sync::mpsc::{Receiver, Sender, channel, error::TrySendError};

use crate::agent::{InterviewMode, RuntimeState};
use crate::gemini::GeminiLiveSession;
use crate::runtime::TOPIC_BOARD_IMAGE;

/// The most one board may weigh.
///
/// A board of ordinary handwriting at the size and quality the browser exports
/// encodes to well under a hundred kilobytes, so this is several times the
/// largest real one and exists to bound a client that is not the browser. The
/// room is opened with the same number, which refuses a stream whose header
/// declares more than this before any of it is read; this one catches a header
/// that declared nothing.
pub(crate) const MAX_BOARD_BYTES: usize = 512 * 1024;

/// Least time between two boards reaching Gemini.
///
/// The browser already waits for the drawing to settle before it exports one,
/// so this is not the debounce; it is the floor that holds when the browser is
/// not the one sending. A candidate drawing continuously would otherwise bill
/// a realtime image per stroke.
const BOARD_SEND_INTERVAL: Duration = Duration::from_secs(1);

/// Boards that may be waiting for the room loop at once.
///
/// Small, because a board is the whole state of the drawing rather than a
/// change to it: when two are queued the older one is worth nothing, and the
/// interviewer is better served by the newest board a moment later than by
/// both of them in order.
const BOARD_QUEUE: usize = 2;

/// One board as it left the browser.
pub(super) struct BoardSnapshot {
    bytes: Vec<u8>,
    /// How many strokes the browser counted on the board it sent. The
    /// candidate's own claim about their own work, exactly like the editor
    /// contents, and used the same way: to say how much is there, never to
    /// decide whether it is any good.
    strokes: u32,
}

/// The latest board, and when one last went out.
pub(super) struct Board {
    latest: Option<Vec<u8>>,
    last_sent: Option<Instant>,
    tx: Sender<BoardSnapshot>,
}

impl Board {
    pub(super) fn new() -> (Self, Receiver<BoardSnapshot>) {
        let (tx, rx) = channel(BOARD_QUEUE);
        (
            Self {
                latest: None,
                last_sent: None,
                tx,
            },
            rx,
        )
    }

    /// The end the reader tasks write finished boards to.
    pub(super) fn sender(&self) -> Sender<BoardSnapshot> {
        self.tx.clone()
    }

    /// The board as the candidate last left it, for the reviewer who has to
    /// grade it. `None` until one arrives, which is a candidate who drew
    /// nothing or a stream that never completed, and the report prompt says
    /// which of those it is looking at.
    pub(super) fn latest(&self) -> Option<&[u8]> {
        self.latest.as_deref()
    }
}

/// Whether a stream that just opened is a board this interview wants.
///
/// Written apart from the event it answers so all four refusals can be tested
/// without a room. Three of them are ordinary -- another topic, another
/// sender, a board in an interview that has no whiteboard -- and the fourth is
/// the one worth having: a header that declares more bytes than a board can
/// be, refused before a single chunk is read.
pub(super) fn board_stream_refusal(
    mode: InterviewMode,
    topic: &str,
    sender: &str,
    candidate_identity: &str,
    declared_length: Option<u64>,
) -> Option<&'static str> {
    if topic != TOPIC_BOARD_IMAGE {
        return Some("not the board topic");
    }
    if !mode.is_whiteboard() {
        return Some("this interview has no whiteboard");
    }
    if sender != candidate_identity {
        return Some("not the candidate");
    }
    if declared_length.is_some_and(|length| length > MAX_BOARD_BYTES as u64) {
        return Some("declared larger than a board may be");
    }
    None
}

/// How many strokes the browser says the board it sent carries.
///
/// The attributes are the only part of the header this reads, and they are
/// strings on the wire, so the parse is where a browser and an agent can come
/// to disagree; `tests/fixtures/board-stream.json` is the browser's own output
/// and this is tested against it. Anything missing or unparseable is zero
/// rather than an error: the picture is the message, and a board is still
/// worth showing when the count beside it did not survive.
pub(super) fn strokes_from_attributes(attributes: &HashMap<String, String>) -> u32 {
    attributes
        .get("strokes")
        .and_then(|strokes| strokes.parse::<u32>().ok())
        .unwrap_or(0)
}

/// Takes a `ByteStreamOpened` event off the room and starts draining it.
///
/// Returns whether the event was this module's, so the room loop can pass
/// anything else along. The reader is moved onto its own task: the loop learns
/// about the board when the whole of it has arrived, and stays free to carry
/// the interview's audio in the meantime.
pub(super) fn handle_board_event(
    mode: InterviewMode,
    candidate_identity: &str,
    board: &Board,
    event: &RoomEvent,
) -> bool {
    let RoomEvent::ByteStreamOpened {
        reader,
        topic,
        participant_identity,
    } = event
    else {
        return false;
    };
    if topic != TOPIC_BOARD_IMAGE {
        return false;
    }

    // Taken before the stream is judged, and exactly once: a reader taken
    // twice is `None` the second time, and the refusal path used to consume
    // the one the accepted path then went looking for. Dropping it is what
    // closes the stream, so a refusal below ends the sender's wait rather than
    // leaving it writing into a reader nobody owns.
    let Some(reader) = reader.take() else {
        return true;
    };
    if let Some(refusal) = board_stream_refusal(
        mode,
        topic,
        &participant_identity.0,
        candidate_identity,
        reader.info().total_length,
    ) {
        eprintln!("ignoring a board stream: {refusal}");
        return true;
    }
    tokio::spawn(drain(reader, board.sender()));
    true
}

/// Reads one board off the wire and hands it to the room loop.
///
/// The bound is applied while reading rather than after: a sender that
/// declares nothing and then writes forever would otherwise be a stream this
/// process buffers to the end of memory before deciding it was too big.
async fn drain(mut reader: ByteStreamReader, tx: Sender<BoardSnapshot>) {
    let strokes = strokes_from_attributes(&reader.info().attributes());
    let mut bytes = Vec::new();
    while let Some(chunk) = reader.next().await {
        match chunk {
            Ok(chunk) => {
                if bytes.len() + chunk.len() > MAX_BOARD_BYTES {
                    eprintln!("dropping a board over {MAX_BOARD_BYTES} bytes");
                    return;
                }
                bytes.extend_from_slice(&chunk);
            }

            // A board that arrived in pieces is not a board. Half a JPEG shown
            // to the interviewer is worse than no board at all: the candidate
            // is asked about a drawing they can see is complete.
            Err(error) => {
                eprintln!("dropping an incomplete board: {error}");
                return;
            }
        }
    }
    if bytes.is_empty() {
        return;
    }
    if let Err(TrySendError::Full(_)) = tx.try_send(BoardSnapshot { bytes, strokes }) {
        // The loop is behind and two boards are already waiting, so this one
        // would be shown after both of them were already stale. See
        // `BOARD_QUEUE`.
        eprintln!("dropping a board the room loop has not caught up with");
    }
}

/// Shows Gemini a board that just arrived, and records that it did.
///
/// The counts land in the interview state whether or not the socket takes the
/// image, because they are what the candidate drew; the image is what this
/// call can fail to deliver.
pub(super) async fn pump_board(
    board: &mut Board,
    gemini: &mut GeminiLiveSession,
    state: &mut RuntimeState,
    snapshot: BoardSnapshot,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let now = Instant::now();
    state.board_snapshots = state.board_snapshots.saturating_add(1);
    state.board_strokes = snapshot.strokes;
    state.last_board_at_ms = Some(crate::agent::elapsed_ms(state));

    // Held before the interval is weighed, so a board that is too soon to send
    // is still the one `read_board` answers with. Dropping it here would mean
    // asking for the board during a busy stretch of drawing returns the one
    // before it.
    board.latest = Some(snapshot.bytes);
    let too_soon = board
        .last_sent
        .is_some_and(|sent| now.duration_since(sent) < BOARD_SEND_INTERVAL);
    if too_soon {
        return Ok(());
    }
    send(board, gemini, now).await
}

/// Puts the board the interviewer already has in front of it again, for
/// `read_board` and for a session that lost its memory.
///
/// Not throttled: this is an explicit ask rather than the drawing arriving on
/// its own, and answering it with silence leaves the model looking at a board
/// several minutes old while the tool has told it the current one is there.
pub(super) async fn resend(
    board: &mut Board,
    gemini: &mut GeminiLiveSession,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if board.latest.is_none() {
        return Ok(());
    }
    send(board, gemini, Instant::now()).await
}

async fn send(
    board: &mut Board,
    gemini: &mut GeminiLiveSession,
    now: Instant,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(bytes) = board.latest.as_ref() else {
        return Ok(());
    };
    board.last_sent = Some(now);
    gemini
        .send_video_frame(bytes, crate::gemini::GEMINI_IMAGE_MIME_TYPE)
        .await
}

#[cfg(test)]
#[path = "../../tests/unit/livekit/board.rs"]
mod tests;

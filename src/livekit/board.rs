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
//! Drawings reach Gemini through its realtime image input. Whiteboard mode
//! disables camera forwarding from the start; coding disables it after the
//! first example drawing arrives, so camera frames cannot replace that drawing.
//! `read_board` asks for the image to be sent again because its tool response
//! carries only JSON.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ::livekit::data_stream::api::StreamReader;
use ::livekit::prelude::RoomEvent;
use futures_util::{Stream, StreamExt};
use tokio::sync::Semaphore;
use tokio::sync::mpsc::{Receiver, Sender, channel};

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
/// Small, because each one may weigh `MAX_BOARD_BYTES` and a board is the
/// whole state of the drawing rather than a change to it: the loop records
/// every one in the order it was read and only the newest is shown, so a
/// longer queue would only be more memory spent on boards about to be replaced.
const BOARD_QUEUE: usize = 2;

/// Board streams this process reads at once.
///
/// The browser sends one at a time, so two is one more than it ever uses. A
/// reader holds up to `MAX_BOARD_BYTES` while it drains and then waits for room
/// in the queue rather than dropping its board, so without a cap a client that
/// is not the browser could open streams until the agent ran out of memory.
/// With it, what boards can hold is this many in flight plus `BOARD_QUEUE`
/// waiting, and a stream past the cap is refused before any of it is read.
const MAX_BOARD_READERS: usize = 2;

/// The longest the report waits for boards still on their way when the
/// interview ends.
///
/// The browser puts the final board on the wire just before it sends
/// `end_interview`, and the two travel separately: the end can arrive while the
/// board is still being read, or read and queued behind it. A report frozen at
/// that moment would grade the board before the candidate's last work. Bounded,
/// because a stream that stalls must not hold the report hostage.
const BOARD_FINAL_WAIT: Duration = Duration::from_secs(2);

/// The longest one board stream may take to arrive whole.
///
/// The SDK ends an incoming stream only on its trailer or when the sender
/// leaves the room, so a writer that never closes would otherwise hold its
/// reader, and the permit with it, for the rest of the interview; two of them
/// and every later board is refused. A real board is tens of kilobytes and
/// arrives in well under a second, so this is generous for a slow link and
/// still short against an interview.
const BOARD_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// One board as it left the browser.
pub(super) struct BoardSnapshot {
    bytes: Vec<u8>,
    /// How many strokes the browser counted on the board it sent. The
    /// candidate's own claim about their own work, exactly like the editor
    /// contents, and used the same way: to say how much is there, never to
    /// decide whether it is any good.
    strokes: u32,
    /// The REACTO phase whose completion caused this snapshot, when it is a
    /// checkpoint rather than the ordinary settled board sent to Jim.
    checkpoint: Option<&'static str>,
}

/// One image handed to the report model, with the words that precede it in the
/// multimodal request.
pub(super) struct ReportBoard {
    pub(super) label: &'static str,
    pub(super) bytes: Vec<u8>,
}

/// The latest board, and when one last went out.
pub(super) struct Board {
    latest: Option<Vec<u8>>,
    example: bool,
    /// Whether `latest` is newer than the last board Gemini was shown. Set by
    /// every board that arrives and cleared by every send, so a board the
    /// interval held back is still owed and goes out once it lapses.
    unsent: bool,
    /// At most one image per coding phase. Kept beside the socket rather than
    /// in `RuntimeState` for the same reason as `latest`: these JPEGs are not
    /// cloned with the interview state and nothing else needs to mutate them.
    checkpoints: Vec<(&'static str, Vec<u8>)>,
    last_sent: Option<Instant>,
    tx: Sender<BoardSnapshot>,
    /// Finished boards, in the order their readers finished. Held here rather
    /// than by the room loop so the ending can take what is still queued
    /// before the report is frozen; see `settle_for_report`.
    pub(super) rx: Receiver<BoardSnapshot>,
    /// One permit per stream being read; see `MAX_BOARD_READERS`.
    readers: Arc<Semaphore>,
}

impl Board {
    pub(super) fn new() -> Self {
        let (tx, rx) = channel(BOARD_QUEUE);
        Self {
            latest: None,
            example: false,
            unsent: false,
            checkpoints: Vec::new(),
            last_sent: None,
            tx,
            rx,
            readers: Arc::new(Semaphore::new(MAX_BOARD_READERS)),
        }
    }

    /// Once a drawing owns the realtime image input, camera forwarding stays
    /// off until the interview ends, including reconnects and cleared drawings.
    pub(super) fn allows_camera(&self, configured: bool) -> bool {
        configured && self.latest.is_none()
    }

    /// The end the reader tasks write finished boards to.
    pub(super) fn sender(&self) -> Sender<BoardSnapshot> {
        self.tx.clone()
    }

    /// Records every board still on its way, for a report about to be frozen.
    ///
    /// Waits for streams still being read as well as for boards already
    /// queued, and at most `BOARD_FINAL_WAIT`. Readers are done when every
    /// permit is back; a reader waiting on a full queue holds its permit, so
    /// the queue is drained while waiting rather than after.
    pub(super) async fn settle_for_report(&mut self, state: &mut RuntimeState) {
        let deadline = tokio::time::Instant::now() + BOARD_FINAL_WAIT;
        let mut arrived = Vec::new();
        loop {
            tokio::select! {
                biased;
                Some(snapshot) = self.rx.recv() => arrived.push(snapshot),
                idle = self.readers.acquire_many(MAX_BOARD_READERS as u32) => {
                    drop(idle);
                    while let Ok(snapshot) = self.rx.try_recv() {
                        arrived.push(snapshot);
                    }
                    break;
                }
                () = tokio::time::sleep_until(deadline) => break,
            }
        }
        for snapshot in arrived {
            record(self, state, snapshot);
        }
    }

    /// Phase checkpoints in REACTO order, plus the final board when it differs
    /// from the last checkpoint.
    ///
    /// Copied only when the report begins, because the farewell keeps using the
    /// live board and the report runs beside it. Six ordinary JPEGs are still
    /// well below the memory bound the byte-stream ceiling establishes.
    pub(super) fn report_boards(&self) -> Vec<ReportBoard> {
        // Auxiliary drawings are displayed by the browser, never sent to the
        // assessment model. Whiteboard work remains assessable.
        if self.example {
            return Vec::new();
        }
        let mut images = self
            .checkpoints
            .iter()
            .map(|(label, bytes)| ReportBoard {
                label,
                bytes: bytes.clone(),
            })
            .collect::<Vec<_>>();
        if let Some(latest) = self.latest.as_ref()
            && self
                .checkpoints
                .last()
                .is_none_or(|(_, checkpoint)| checkpoint != latest)
        {
            images.push(ReportBoard {
                label: "Final board",
                bytes: latest.clone(),
            });
        }
        images
    }
}

/// Whether a stream that just opened is a board this interview wants.
///
/// Coding interviews use the same bounded stream for auxiliary example
/// drawings. Identity, topic and byte limits apply to both surfaces.
pub(super) fn board_stream_refusal(
    _mode: InterviewMode,
    topic: &str,
    sender: &str,
    candidate_identity: &str,
    declared_length: Option<u64>,
) -> Option<&'static str> {
    if topic != TOPIC_BOARD_IMAGE {
        return Some("not the board topic");
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

/// A browser-provided phase id, reduced to the six values a whiteboard report
/// understands and to the candidate-facing label the request will put beside
/// its image. Unknown values make an ordinary board, never an attachment with
/// attacker-chosen instructions for the reviewer.
pub(super) fn checkpoint_from_attributes(
    attributes: &HashMap<String, String>,
) -> Option<&'static str> {
    match attributes.get("checkpoint").map(String::as_str) {
        Some("repeat") => Some("Repeat checkpoint"),
        Some("example") => Some("Example checkpoint"),
        Some("algorithm") => Some("Approach checkpoint"),
        Some("coding") => Some("Trace checkpoint"),
        Some("test") => Some("Edge cases checkpoint"),
        Some("optimizations") => Some("Complexity checkpoint"),
        _ => None,
    }
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

    // Taken before the stream is judged, and exactly once: a reader taken twice
    // is `None` the second time, and the refusal path used to consume the one
    // the accepted path then went looking for. Dropping it is what closes the
    // stream, so a refusal below ends the sender's wait rather than leaving it
    // writing into a reader nobody owns.
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
    let Ok(permit) = Arc::clone(&board.readers).try_acquire_owned() else {
        eprintln!("ignoring a board stream: {MAX_BOARD_READERS} are already being read");
        return true;
    };
    let strokes = strokes_from_attributes(&reader.info().attributes());
    let checkpoint = checkpoint_from_attributes(&reader.info().attributes());
    let tx = board.sender();
    tokio::spawn(async move {
        drain(reader, strokes, checkpoint, tx).await;
        drop(permit);
    });
    true
}

/// Reads one board off the wire and hands it to the room loop.
///
/// Both bounds are applied while reading rather than after: a sender that
/// declares nothing and then writes forever would otherwise be a stream this
/// process buffers to the end of memory before deciding it was too big, and
/// one that never finishes would hold its permit for good. The deadline covers
/// the read and not the wait for the queue below, which is the room loop's to
/// end, not the sender's.
///
/// Any stream of chunks rather than the LiveKit reader alone, because the SDK
/// offers no way to build one outside a room and the bounds are the part of
/// this worth a test.
async fn drain<C, E>(
    chunks: impl Stream<Item = Result<C, E>> + Unpin,
    strokes: u32,
    checkpoint: Option<&'static str>,
    tx: Sender<BoardSnapshot>,
) where
    C: AsRef<[u8]>,
    E: std::fmt::Display,
{
    let bytes = match tokio::time::timeout(BOARD_READ_TIMEOUT, read_board(chunks)).await {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return,
        Err(_) => {
            eprintln!(
                "dropping a board stream unfinished after {}s",
                BOARD_READ_TIMEOUT.as_secs()
            );
            return;
        }
    };
    let snapshot = BoardSnapshot {
        bytes,
        strokes,
        checkpoint,
    };

    // Every board waits for room rather than being dropped. Dropping the one
    // that found the queue full threw away the newest and kept the older ones,
    // and a candidate who then stopped drawing left the interviewer on a stale
    // board for good. Waiting keeps them in the order they were read, so the
    // newest is the last the loop records. What it costs is bounded by
    // `MAX_BOARD_READERS`, and the room loop stays free because this is the
    // spawned drain task.
    let _ = tx.send(snapshot).await;
}

/// The whole of one board stream, or `None` for one that is no board.
async fn read_board<C, E>(mut chunks: impl Stream<Item = Result<C, E>> + Unpin) -> Option<Vec<u8>>
where
    C: AsRef<[u8]>,
    E: std::fmt::Display,
{
    let mut bytes = Vec::new();
    while let Some(chunk) = chunks.next().await {
        match chunk {
            Ok(chunk) => {
                let chunk = chunk.as_ref();
                if bytes.len() + chunk.len() > MAX_BOARD_BYTES {
                    eprintln!("dropping a board over {MAX_BOARD_BYTES} bytes");
                    return None;
                }
                bytes.extend_from_slice(chunk);
            }

            // A board that arrived in pieces is not a board. Half a JPEG shown
            // to the interviewer is worse than no board at all: the candidate
            // is asked about a drawing they can see is complete.
            Err(error) => {
                eprintln!("dropping an incomplete board: {error}");
                return None;
            }
        }
    }
    (!bytes.is_empty()).then_some(bytes)
}

/// Takes in a board that just arrived: its counts, its checkpoint, and the
/// image `read_board` and the next send will use. Sends nothing, so a paused
/// interview and an ending one can keep a board without showing it.
///
/// The counts land in the interview state whether or not the image ever
/// reaches the socket, because they are what the candidate drew.
pub(super) fn record(board: &mut Board, state: &mut RuntimeState, snapshot: BoardSnapshot) {
    board.example = !state.interview_mode.is_whiteboard();
    state.board_snapshots = state.board_snapshots.saturating_add(1);
    state.board_strokes = snapshot.strokes;
    state.board_drawn |= snapshot.strokes >= crate::agent::MIN_BOARD_STROKES;
    state.last_board_at_ms = Some(crate::agent::elapsed_ms(state));

    // Held whether or not it can go out yet, so a board that is too soon to
    // send is still the one `read_board` answers with. Dropping it here would
    // mean asking for the board during a busy stretch of drawing returns the
    // one before it.
    if let Some(label) = snapshot.checkpoint.filter(|_| !board.example) {
        if let Some((_, bytes)) = board
            .checkpoints
            .iter_mut()
            .find(|(stored, _)| *stored == label)
        {
            *bytes = snapshot.bytes.clone();
        } else {
            board.checkpoints.push((label, snapshot.bytes.clone()));
        }
    }
    board.latest = Some(snapshot.bytes);
    board.unsent = true;
}

/// Shows Gemini the newest board if it has not seen it and the interval allows.
///
/// Called for every board that arrives and on every watch tick. The tick is
/// what makes a board the interval held back go out once it lapses, rather
/// than waiting for the candidate's next stroke, which a candidate who has
/// stopped drawing to explain never makes.
pub(super) async fn send_if_due(
    board: &mut Board,
    gemini: &mut GeminiLiveSession,
    now: Instant,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !board.unsent || too_soon(board.last_sent, now) {
        return Ok(());
    }
    send(board, gemini, now).await
}

/// Whether a board sent at `last_sent` is too recent for another at `now`.
/// A whole interval since is not too soon.
fn too_soon(last_sent: Option<Instant>, now: Instant) -> bool {
    last_sent.is_some_and(|sent| now.duration_since(sent) < BOARD_SEND_INTERVAL)
}

/// Puts the board the interviewer already has in front of it again, for
/// `read_board` and for a session that lost its memory.
///
/// Not throttled: this is an explicit ask rather than the drawing arriving on
/// its own, and answering it with silence leaves the model looking at a board
/// several minutes old while the tool has told it the current one is there.
/// Deliver pending ink before a drawing handover, without letting a failed
/// image write abort the room loop or prevent its eventual report delivery.
pub(super) async fn resend_for_handover(
    board: &mut Board,
    state: &mut RuntimeState,
    gemini: &mut GeminiLiveSession,
) {
    board.settle_for_report(state).await;
    if let Err(error) = resend(board, gemini).await {
        eprintln!(
            "example-drawing handover image failed ({error}); waiting for the close to be reported"
        );
    }
}

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
        .await?;

    // Only once the socket took it. A failed write leaves the board owed, so
    // the next tick tries again rather than leaving the interviewer on the one
    // before it; `last_sent` is set either way, which spaces those tries out.
    board.unsent = false;
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/livekit/board.rs"]
mod tests;

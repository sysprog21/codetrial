//! Transcript segments on their way to the page, and the one task that sends
//! them.
//!
//! Each fragment either speaker says republishes its whole turn so far as a
//! LiveKit text stream: a deflate of the turn, then a header, a chunk and a
//! trailer, each waiting on the data channel, and before them a check that the
//! publisher is connected, which a signal reconnect can stretch to fifteen
//! seconds. Awaited in the room loop, that held the next Gemini event and the
//! candidate's microphone frames behind it, several times a second while
//! either side spoke. The loop now only queues a segment, the same way the
//! agent state attribute already had its own writer.
//!
//! Nothing is lost by sending less often than the loop queues. A turn's text is
//! cumulative under one segment id, so a newer view of a segment still waiting
//! replaces it where it stands, and the page patches one row per segment. The
//! order segments first appear in is kept, so a final follows its turn's
//! updates and the two speakers' turns go out in the order they were closed.

use std::collections::VecDeque;
use std::time::Duration;

use ::livekit::data_stream::api::SendTextOptions;
use tokio::sync::{mpsc, oneshot};

use crate::livekit::room_link::{ParticipantLink, RoomLink};
use crate::runtime::TOPIC_TRANSCRIPTION;

/// `lk.segment_id` is what lets the browser patch one row per turn instead of
/// reassembling sentences from timestamps, and `lk.transcription_final` says
/// whether the turn is still being spoken.
pub(in crate::livekit) fn transcript_stream_options(
    segment_id: String,
    final_segment: bool,
    sender_identity: Option<&str>,
) -> SendTextOptions {
    let options = SendTextOptions::new_with_topic(TOPIC_TRANSCRIPTION)
        .with_attribute("lk.segment_id", &segment_id)
        .with_attribute(
            "lk.transcription_final",
            if final_segment { "true" } else { "false" },
        );
    match sender_identity {
        Some(identity) => options.with_sender_identity(identity),
        None => options,
    }
}

/// One segment as the page shows it: the whole turn so far, under its id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::livekit) struct Segment {
    pub(in crate::livekit) id: String,
    pub(in crate::livekit) text: String,
    pub(in crate::livekit) final_segment: bool,
    /// The candidate's identity for their own speech; `None` publishes as the
    /// agent participant.
    pub(in crate::livekit) identity: Option<String>,
}

/// The segments waiting to be sent, newest view of each.
#[derive(Debug, Default)]
pub(in crate::livekit) struct Pending {
    queue: VecDeque<Segment>,
}

impl Pending {
    /// Queues `segment`, replacing an update of the same segment still waiting
    /// rather than sending both. A final is never replaced: it is the last word
    /// on its segment, and nothing is queued under that id after it.
    pub(in crate::livekit) fn push(&mut self, segment: Segment) {
        if let Some(waiting) = self
            .queue
            .iter_mut()
            .find(|waiting| waiting.id == segment.id && !waiting.final_segment)
        {
            *waiting = segment;
        } else {
            self.queue.push_back(segment);
        }
    }

    pub(in crate::livekit) fn pop(&mut self) -> Option<Segment> {
        self.queue.pop_front()
    }

    #[cfg(test)]
    pub(in crate::livekit) fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

/// What the loop hands the writer task.
enum Message {
    Segment(Segment),
    /// Answered once everything queued before it has been sent.
    Flush(oneshot::Sender<()>),
}

/// The interview's transcript writer. Started on the first segment, and its
/// task ends once this is dropped and the queue has been sent.
#[derive(Default)]
pub(crate) struct TranscriptWriter {
    task: Option<mpsc::UnboundedSender<Message>>,
}

impl TranscriptWriter {
    /// Queues a segment for the page without waiting on the room. A failed
    /// send is logged by the task: the transcript is already recorded, and the
    /// report is written from that record, so a segment the page misses is
    /// cosmetic.
    pub(in crate::livekit) fn publish(&mut self, room: &RoomLink, segment: Segment) {
        let task = self.task.get_or_insert_with(|| {
            let (task, messages) = mpsc::unbounded_channel();
            tokio::spawn(send_segments(room.participant(), messages));
            task
        });
        // The task only ends once this sender is dropped, so this cannot fail.
        let _ = task.send(Message::Segment(segment));
    }

    /// Waits, at most `limit`, for everything queued to have been sent. The
    /// ending calls this before the report goes out, so the page has the last
    /// turns before it is handed the report, as it did when each publish was
    /// awaited in place.
    pub(in crate::livekit) async fn flush(&self, limit: Duration) {
        let Some(task) = &self.task else { return };
        let (done, sent) = oneshot::channel();
        if task.send(Message::Flush(done)).is_err() {
            return;
        }
        if tokio::time::timeout(limit, sent).await.is_err() {
            eprintln!(
                "transcript segments still unsent after {:.1}s; going on without them",
                limit.as_secs_f64()
            );
        }
    }
}

async fn send_segments(
    participant: ParticipantLink,
    mut messages: mpsc::UnboundedReceiver<Message>,
) {
    let mut pending = Pending::default();
    let mut flushes: Vec<oneshot::Sender<()>> = Vec::new();
    let mut open = true;
    loop {
        let Some(segment) = pending.pop() else {
            // Answered before waiting again: a flush queued behind segments is
            // owed its answer once they are out, not on the next message.
            for flush in flushes.drain(..) {
                let _ = flush.send(());
            }
            if !open {
                return;
            }
            match messages.recv().await {
                Some(message) => take(message, &mut pending, &mut flushes),
                None => return,
            }
            continue;
        };
        let options = transcript_stream_options(
            segment.id,
            segment.final_segment,
            segment.identity.as_deref(),
        );
        let send = participant.send_text(&segment.text, options);
        tokio::pin!(send);

        // Taken in while the send is under way, not after it, so a send the
        // room holds for seconds leaves the newest view of each segment queued
        // rather than every fragment said meanwhile.
        loop {
            tokio::select! {
                sent = &mut send => {
                    if let Err(error) = sent {
                        eprintln!(
                            "publishing a transcript segment failed ({error}); continuing the interview"
                        );
                    }
                    break;
                }
                message = messages.recv(), if open => match message {
                    Some(message) => take(message, &mut pending, &mut flushes),
                    None => open = false,
                },
            }
        }
    }
}

fn take(message: Message, pending: &mut Pending, flushes: &mut Vec<oneshot::Sender<()>>) {
    match message {
        Message::Segment(segment) => pending.push(segment),
        Message::Flush(done) => flushes.push(done),
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/livekit/turn/transcripts.rs"]
mod tests;

//! The `tests` module of `src/livekit/transcripts.rs`, which declares this file
//! by path. The writer task needs a connected room; the queue it drains does
//! not, and the queue is what decides what the page is sent.

use super::*;

fn segment(id: &str, text: &str, final_segment: bool) -> Segment {
    Segment {
        id: id.into(),
        text: text.into(),
        final_segment,
        identity: None,
    }
}

fn drain(pending: &mut Pending) -> Vec<Segment> {
    std::iter::from_fn(|| pending.pop()).collect()
}

#[test]
fn a_newer_view_of_a_waiting_segment_replaces_it_where_it_stands() {
    let mut pending = Pending::default();
    pending.push(segment("candidate-1", "I will", false));
    pending.push(segment("interviewer-1", "Go", false));
    pending.push(segment("candidate-1", "I will use a map", false));
    pending.push(segment("interviewer-1", "Go ahead.", false));
    assert_eq!(
        drain(&mut pending),
        [
            segment("candidate-1", "I will use a map", false),
            segment("interviewer-1", "Go ahead.", false),
        ]
    );
    assert!(pending.is_empty());
}

#[test]
fn a_final_replaces_its_waiting_update_and_is_never_replaced_itself() {
    let mut pending = Pending::default();
    pending.push(segment("interviewer-1", "Could you", false));
    pending.push(segment("candidate-1", "Wait", false));
    pending.push(segment("interviewer-1", "Could you explain?", true));

    // A later view under a finished id is queued behind the final, not over it:
    // the final is the last word on the segment the page shows.
    pending.push(segment("interviewer-1", "Could you explain? Sure", false));
    pending.push(segment("candidate-1", "Wait, one thing.", true));
    assert_eq!(
        drain(&mut pending),
        [
            segment("interviewer-1", "Could you explain?", true),
            segment("candidate-1", "Wait, one thing.", true),
            segment("interviewer-1", "Could you explain? Sure", false),
        ]
    );
}

#[tokio::test]
async fn flushing_a_writer_that_never_sent_returns_at_once() {
    let writer = TranscriptWriter::default();
    tokio::time::timeout(
        Duration::from_millis(50),
        writer.flush(Duration::from_secs(5)),
    )
    .await
    .expect("nothing queued, nothing to wait for");
}

#[tokio::test]
async fn a_flush_behind_queued_segments_returns_once_they_are_sent() {
    let (room, recorder) = crate::livekit::room_link::RoomLink::recording("room");
    let mut writer = TranscriptWriter::default();
    writer.publish(&room, segment("candidate-1", "I will", false));
    writer.publish(&room, segment("candidate-1", "I will use a map", true));
    tokio::time::timeout(
        Duration::from_millis(500),
        writer.flush(Duration::from_secs(5)),
    )
    .await
    .expect("the flush is answered once the queue is out, not at its limit");
    let sent: Vec<_> = recorder
        .take()
        .into_iter()
        .filter_map(|sent| match sent {
            crate::livekit::room_link::Sent::Text { text, .. } => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(sent.last().map(String::as_str), Some("I will use a map"));
}

/// A candidate's own speech is published under their identity, an interviewer
/// turn under the agent's own.
#[tokio::test]
async fn a_segment_is_sent_under_the_identity_it_carries() {
    let (room, recorder) = crate::livekit::room_link::RoomLink::recording("room");
    let mut writer = TranscriptWriter::default();
    let mut candidate = segment("candidate-1", "I will use a map", true);
    candidate.identity = Some("candidate-fixed".into());
    writer.publish(&room, candidate);
    writer.publish(&room, segment("interviewer-1", "Go ahead.", true));
    writer.flush(Duration::from_secs(5)).await;
    let identities: Vec<_> = recorder
        .take()
        .into_iter()
        .filter_map(|sent| match sent {
            crate::livekit::room_link::Sent::Text {
                sender_identity, ..
            } => Some(sender_identity),
            _ => None,
        })
        .collect();
    assert_eq!(identities, [Some("candidate-fixed".to_string()), None]);
}

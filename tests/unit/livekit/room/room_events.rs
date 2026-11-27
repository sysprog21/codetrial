use super::*;

#[test]
fn replayed_events_come_out_ahead_of_newer_ones_and_replace_earlier_replays() {
    let (sender, events) = tokio::sync::mpsc::unbounded_channel();
    let mut reader = RoomEventReader::from(events);
    let at = Instant::now();
    sender.send(3u8).unwrap();
    reader.replay(vec![(at, 1)]);
    reader.replay(vec![(at, 2)]);
    assert_eq!(reader.try_recv().ok(), Some(2));
    assert_eq!(reader.try_recv().ok(), Some(3));
    assert!(reader.try_recv().is_err());
}

#[test]
fn the_page_asking_for_the_ready_notice_is_remembered_until_taken() {
    let (_sender, events) = tokio::sync::mpsc::unbounded_channel::<u8>();
    let mut reader = RoomEventReader::from(events);
    assert!(!reader.take_startup_sync_request());
    reader.ask_startup_sync();
    assert!(reader.take_startup_sync_request());
    assert!(!reader.take_startup_sync_request());
}

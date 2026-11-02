use super::*;

fn run(events: Vec<(u64, Event)>) -> Vec<Record> {
    let start = Instant::now();
    let mut tracker = FloorTracker::default();
    let mut records = Vec::new();
    for (ms, event) in events {
        tracker.observe(event, start + Duration::from_millis(ms), &mut |r| {
            records.push(r);
        });
    }
    records
}
fn start() -> Event {
    Event::Start {
        generating: true,
        ahead: Duration::from_millis(200),
    }
}

#[test]
fn pause_response_and_continue() {
    let records = run(vec![
        (0, Event::End),
        (123, Event::Output { audio: false }),
        (200, Event::Reset),
    ]);
    assert_eq!(records[0].outcome, "respond");
    assert_eq!(records[0].gap_ms, Some(123));
    assert_eq!(records[0].suspect, None);
    let records = run(vec![
        (0, Event::End),
        (
            123,
            Event::Start {
                generating: false,
                ahead: Duration::ZERO,
            },
        ),
    ]);
    assert_eq!(records[0].outcome, "continue");
    assert_eq!(records[0].gap_ms, Some(123));
}

#[test]
fn cut_off_boundaries_use_first_audio_not_tool_or_later_chunk() {
    for (gap, expected) in [(1499, true), (1500, true), (1501, false)] {
        let records = run(vec![
            (0, Event::End),
            (10, Event::Output { audio: false }),
            (100, Event::Output { audio: true }),
            (200, Event::Output { audio: true }),
            (100 + gap, start()),
        ]);
        assert_eq!(records[0].suspect == Some("cut_off"), expected);
        assert_eq!(records[0].gap_ms, Some(10));
    }
}

#[test]
fn false_stop_boundaries_and_both_interruption_orders() {
    for (ms, expected) in [(599, true), (600, false), (601, false)] {
        for early in [true, false] {
            let mut events = vec![(0, start())];
            if early {
                events.push((1, Event::Interrupted));
            }
            events.push((ms, Event::End));
            if !early {
                events.push((ms + 1, Event::Interrupted));
            }
            let records = run(events);
            assert_eq!(records[0].kind, "overlap");
            assert_eq!(records[0].outcome, "interrupted");
            assert_eq!(records[0].overlap_ms, Some(ms as u128));
            assert_eq!(records[0].playout_ahead_ms, Some(200));
            assert_eq!(records[0].suspect == Some("false_stop"), expected);
        }
    }
}

#[test]
fn missed_barge_in_waits_for_completion_and_drain() {
    for (ms, expected) in [(1499, false), (1500, true), (1501, true)] {
        let records = run(vec![
            (0, start()),
            (ms, Event::End),
            (ms + 1, Event::Complete { playing: true }),
            (ms + 2, Event::Drained),
        ]);
        let overlap = records.iter().find(|r| r.kind == "overlap").unwrap();
        assert_eq!(overlap.outcome, "continued");
        assert_eq!(overlap.suspect == Some("missed_barge_in"), expected);
    }
}

#[test]
fn missed_barge_in_stops_counting_at_drain() {
    for (ms, expected) in [(1499, false), (1500, true), (1501, true)] {
        let records = run(vec![
            (0, start()),
            (ms, Event::Complete { playing: false }),
            (3000, Event::End),
        ]);
        let overlap = records.iter().find(|r| r.kind == "overlap").unwrap();
        assert_eq!(overlap.outcome, "continued");
        assert_eq!(overlap.overlap_ms, Some(3000));
        assert_eq!(overlap.suspect == Some("missed_barge_in"), expected);
    }
}

#[test]
fn completion_before_activity_end_and_no_playout() {
    let records = run(vec![
        (0, start()),
        (1, Event::Complete { playing: false }),
        (1600, Event::End),
    ]);
    assert_eq!(records[0].outcome, "continued");
    assert_eq!(records[0].overlap_ms, Some(1600));
    assert_eq!(
        records[0].suspect, None,
        "drained 1 ms in, barely talked over"
    );
}

#[test]
fn reset_unknown_duration_and_fresh_socket() {
    let records = run(vec![
        (0, start()),
        (100, Event::Interrupted),
        (101, Event::Reset),
        (200, start()),
        (800, Event::End),
        (900, Event::Complete { playing: false }),
        (1000, Event::Reset),
    ]);
    assert_eq!(records[0].outcome, "unresolved");
    assert_eq!(records[0].overlap_ms, None);
    assert_eq!(records[0].suspect, None);
    let next_overlap = records
        .iter()
        .skip(1)
        .find(|r| r.kind == "overlap")
        .unwrap();
    assert_eq!(next_overlap.overlap_ms, Some(600));
    assert!(next_overlap.seq > records[0].seq);
    assert!(
        records[0]
            .log_line("synthetic", 1, 2, "0:00")
            .contains("overlap_ms=unknown")
    );
}

#[test]
fn duplicate_activity_does_not_restart_clocks_or_emit_twice() {
    let records = run(vec![
        (0, start()),
        (100, start()),
        (600, Event::End),
        (700, Event::End),
        (800, Event::Interrupted),
        (900, Event::Reset),
        (1000, Event::Reset),
    ]);
    assert_eq!(records.len(), 2);
    let overlap = records.iter().find(|r| r.kind == "overlap").unwrap();
    assert_eq!(overlap.overlap_ms, Some(600));
}

#[test]
fn existing_reply_chunks_do_not_answer_an_overlap_pause() {
    let records = run(vec![
        (0, Event::Output { audio: true }),
        (10, start()),
        (100, Event::End),
        (110, Event::Output { audio: true }),
        (200, Event::Complete { playing: false }),
        (300, Event::Reset),
    ]);
    let pause = records.iter().find(|r| r.kind == "pause").unwrap();
    assert_eq!(pause.outcome, "unresolved");
    assert_eq!(pause.gap_ms, None);
    assert_eq!(pause.suspect, None);
}

#[test]
fn tool_only_and_no_output_completion_do_not_invent_audio_or_continuation() {
    for output in [false, true] {
        let mut events = vec![(0, Event::End)];
        if output {
            events.push((10, Event::Output { audio: false }));
        }
        events.extend([(20, Event::Complete { playing: false }), (30, start())]);
        let records = run(events);
        assert_eq!(
            records[0].outcome,
            if output { "respond" } else { "unresolved" }
        );
        assert_eq!(records[0].suspect, None);
    }
}

#[test]
fn reversed_intervals_are_unknown_and_never_suspect() {
    let records = run(vec![
        (100, start()),
        (1, Event::End),
        (200, Event::Interrupted),
    ]);
    assert_eq!(records[0].overlap_ms, None);
    assert_eq!(records[0].suspect, None);
}

#[test]
fn lost_activity_flushes_unknown_then_next_utterance_has_its_own_clock() {
    let records = run(vec![
        (0, start()),
        (100, Event::Interrupted),
        (10000, Event::ActivityLost),
        (11000, start()),
        (11600, Event::End),
        (11700, Event::Interrupted),
    ]);
    assert_eq!(records[0].outcome, "unresolved");
    assert_eq!(records[0].overlap_ms, None);
    assert_eq!(records[1].overlap_ms, Some(600));
    assert_eq!(records[1].suspect, None);
}

#[test]
fn kept_interruption_padding_cannot_be_a_new_response() {
    let records = run(vec![
        (0, Event::Output { audio: true }),
        (10, start()),
        (100, Event::End),
        (110, Event::Interrupted),
        (120, Event::Output { audio: true }),
        (130, Event::Complete { playing: true }),
        (140, start()),
    ]);
    let pause = records.iter().find(|r| r.kind == "pause").unwrap();
    assert_eq!(pause.outcome, "unresolved");
    assert_eq!(pause.suspect, None);
}

#[test]
fn stray_interruption_without_provider_overlap_cannot_invent_one() {
    let records = run(vec![
        (0, Event::Interrupted),
        (10, Event::Drained),
        (20, Event::Reset),
    ]);
    assert!(records.is_empty());
}

#[test]
fn new_end_flushes_a_responded_pause_even_when_its_start_was_not_observed() {
    let records = run(vec![
        (0, Event::End),
        (100, Event::Output { audio: true }),
        (200, Event::Complete { playing: false }),
        (1000, Event::End),
        (1100, Event::Output { audio: true }),
        (1200, Event::Reset),
    ]);
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].outcome, "respond");
    assert_eq!(records[1].outcome, "respond");
    assert_eq!(records[0].gap_ms, Some(100));
    assert_eq!(records[1].gap_ms, Some(100));
}

#[test]
fn settled_continuation_is_not_relabelled_by_later_interruption() {
    let records = run(vec![
        (0, start()),
        (100, Event::Drained),
        (200, Event::Interrupted),
        (1600, Event::End),
    ]);
    assert_eq!(records[0].outcome, "continued");
    assert_eq!(records[0].overlap_ms, Some(1600));
    assert_eq!(records[0].suspect, None, "drained 100 ms in");
}

#[test]
fn next_start_displaces_ended_unsettled_overlap_without_a_flag() {
    let records = run(vec![
        (0, start()),
        (100, Event::End),
        (200, start()),
        (300, Event::Reset),
    ]);
    let overlaps: Vec<_> = records.iter().filter(|r| r.kind == "overlap").collect();
    assert_eq!(overlaps.len(), 2);
    assert_eq!(overlaps[0].outcome, "unresolved");
    assert_eq!(overlaps[0].overlap_ms, Some(100));
    assert_eq!(overlaps[0].suspect, None);
    assert_eq!(overlaps[1].overlap_ms, None);
}

#[test]
fn completed_tool_only_response_cannot_borrow_a_later_generations_audio() {
    let records = run(vec![
        (0, Event::End),
        (100, Event::Output { audio: false }),
        (200, Event::Complete { playing: false }),
        (1000, Event::Output { audio: true }),
        (1100, start()),
    ]);
    assert_eq!(records[0].outcome, "respond");
    assert_eq!(records[0].gap_ms, Some(100));
    assert_eq!(records[0].suspect, None);
}

#[test]
fn deferred_pause_retains_its_opening_and_first_resolution() {
    let records = run(vec![
        (10, Event::End),
        (30, Event::Output { audio: false }),
        (50, Event::Output { audio: true }),
        (200, Event::Output { audio: true }),
        (1000, Event::Reset),
    ]);
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.kind, "pause");
    assert_eq!(record.outcome, "respond");
    assert_eq!(
        record.resolved_at.unwrap().duration_since(record.opened_at),
        Duration::from_millis(20)
    );
    assert_eq!(record.activity_end_at, None);
}

#[test]
fn overlap_resolution_and_candidate_end_keep_distinct_clocks() {
    for (resolve, end) in [(100, 500), (700, 500)] {
        let mut events = vec![(0, start())];
        if resolve < end {
            events.push((resolve, Event::Interrupted));
        }
        events.push((end, Event::End));
        if resolve > end {
            events.push((resolve, Event::Interrupted));
        }
        events.push((1000, Event::Reset));
        let records = run(events);
        let record = records.iter().find(|r| r.kind == "overlap").unwrap();
        assert_eq!(
            record.resolved_at.unwrap().duration_since(record.opened_at),
            Duration::from_millis(resolve)
        );
        assert_eq!(
            record
                .activity_end_at
                .unwrap()
                .duration_since(record.opened_at),
            Duration::from_millis(end)
        );
    }
}

#[test]
fn continuation_resolves_at_drain_without_borrowing_the_emission_time() {
    let records = run(vec![
        (0, start()),
        (100, Event::Complete { playing: true }),
        (200, Event::Drained),
        (500, Event::End),
    ]);
    let record = records.iter().find(|r| r.kind == "overlap").unwrap();
    assert_eq!(record.outcome, "continued");
    assert_eq!(
        record.resolved_at.unwrap().duration_since(record.opened_at),
        Duration::from_millis(200)
    );
    assert_eq!(
        record
            .activity_end_at
            .unwrap()
            .duration_since(record.opened_at),
        Duration::from_millis(500)
    );
}

#[test]
fn loss_and_reset_clear_resolution_of_an_overlap_with_no_end() {
    for event in [Event::Reset, Event::ActivityLost] {
        let records = run(vec![(0, start()), (100, Event::Interrupted), (200, event)]);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].outcome, "unresolved");
        assert_eq!(records[0].resolved_at, None);
        assert_eq!(records[0].activity_end_at, None);
    }
}

#[test]
fn resumed_pause_keeps_the_end_to_start_resolution_clock() {
    let records = run(vec![
        (0, Event::End),
        (
            250,
            Event::Start {
                generating: false,
                ahead: Duration::ZERO,
            },
        ),
        (600, Event::Reset),
    ]);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].outcome, "continue");
    assert_eq!(
        records[0]
            .resolved_at
            .unwrap()
            .duration_since(records[0].opened_at),
        Duration::from_millis(250)
    );
}

#[test]
fn loss_after_end_does_not_clear_an_already_ended_overlap() {
    let records = run(vec![
        (0, start()),
        (100, Event::End),
        (200, Event::ActivityLost),
        (300, Event::Drained),
    ]);
    let record = records.iter().find(|r| r.kind == "overlap").unwrap();
    assert_eq!(record.outcome, "continued");
    assert_eq!(record.overlap_ms, Some(100));
    assert_eq!(
        record
            .activity_end_at
            .unwrap()
            .duration_since(record.opened_at),
        Duration::from_millis(100)
    );
}

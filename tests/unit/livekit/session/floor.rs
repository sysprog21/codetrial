use super::*;
use std::collections::HashMap;
use std::time::Duration;

fn fields(line: &str) -> HashMap<&str, &str> {
    line.split_whitespace()
        .filter_map(|s| s.split_once('='))
        .collect()
}

#[test]
fn emitted_floor_retains_precise_times_of_a_pause_resolved_before_teardown() {
    let start = Instant::now() - Duration::from_secs(1);
    let mut activity = RuntimeActivity::for_interview(start, 0, 123);
    activity.latency.bind("synthetic", 123, 1, 1500);
    activity.latency.anchor(start);
    activity
        .floor_tracker
        .observe(Event::End, start + Duration::from_millis(100), &mut |_| {
            panic!("pause must remain pending")
        });
    activity.floor_tracker.observe(
        Event::Output { audio: false },
        start + Duration::from_millis(250),
        &mut |_| panic!("responded pause emits at a later boundary"),
    );
    let mut output = Vec::new();
    write_floor(
        "synthetic",
        &mut activity,
        || "coarse-clock".into(),
        Event::Reset,
        &mut output,
    );
    let text = String::from_utf8(output).unwrap();
    let row = fields(&text);
    assert_eq!(row["opened_ms"], "100");
    assert_eq!(row["resolved_ms"], "250");
    assert_eq!(row["activity_end_ms"], "unknown");
    assert!(row["emitted_ms"].parse::<u64>().unwrap() >= 1000);
    assert!(row["start_unix_ms"].parse::<u64>().unwrap() > 0);
    assert!(row["clock_uncertainty_ms"].parse::<u64>().unwrap() >= 4);
    assert_eq!(row["pid"], std::process::id().to_string());
    assert_eq!(row["session"], "123");
    assert_eq!(row["socket"], "1");
}

#[test]
fn missing_anchor_does_not_fabricate_a_clock_from_runtime_state() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::for_interview(start, 0, 123);
    let mut output = Vec::new();
    write_floor(
        "synthetic",
        &mut activity,
        || "0:00".into(),
        Event::End,
        &mut output,
    );
    write_floor(
        "synthetic",
        &mut activity,
        || "0:00".into(),
        Event::Output { audio: false },
        &mut output,
    );
    write_floor(
        "synthetic",
        &mut activity,
        || "0:00".into(),
        Event::Reset,
        &mut output,
    );
    let text = String::from_utf8(output).unwrap();
    let row = fields(&text);
    for key in [
        "opened_ms",
        "resolved_ms",
        "emitted_ms",
        "start_unix_ms",
        "clock_uncertainty_ms",
    ] {
        assert_eq!(row[key], "unknown", "{key}");
    }
}

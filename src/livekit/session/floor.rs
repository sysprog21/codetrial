//! Floor log emission, separate from the event state machine and turn policy.
use std::time::Instant;

use crate::agent::RuntimeState;
use crate::livekit::floor::Event;
use crate::livekit::turn::RuntimeActivity;

use super::log_clock;

fn value(time: Option<i64>) -> String {
    time.map_or_else(|| "unknown".into(), |n| n.to_string())
}

/// Most events emit nothing, and some arrive per audio chunk or per loop step,
/// so the clock is only read when a record is written.
pub(in crate::livekit) fn observe_floor(
    room: &str,
    activity: &mut RuntimeActivity,
    state: &RuntimeState,
    event: Event,
) {
    write_floor(
        room,
        activity,
        || log_clock(state),
        event,
        &mut std::io::stderr().lock(),
    );
}

pub(super) fn write_floor(
    room: &str,
    activity: &mut RuntimeActivity,
    clock: impl Fn() -> String,
    event: Event,
    output: &mut impl std::io::Write,
) {
    let session = activity.live_session_id;
    let socket = activity.live_socket;
    let latency = &activity.latency;
    let now = Instant::now();
    // Sampling is lazy: most events emit nothing, including audio chunks.
    let mut at = None;
    let mut sample = None;
    activity.floor_tracker.observe(event, now, &mut |record| {
        let at = at.get_or_insert_with(&clock);
        let sample = sample.get_or_insert_with(|| latency.observation_clock());
        let number = |key: &str| value(sample[key].as_i64());
        let event_time = |at: Option<Instant>| value(at.and_then(|at| latency.interview_ms(at)));
        writeln!(output, "{} opened_ms={} resolved_ms={} activity_end_ms={} emitted_ms={} emitted_unix_ms={} start_unix_ms={} clock_uncertainty_ms={} pid={}",
            record.log_line(room, session, socket, at),
            event_time(Some(record.opened_at)), event_time(record.resolved_at), event_time(record.activity_end_at),
            number("emitted_ms"), number("emitted_unix_ms"), number("start_unix_ms"), number("clock_uncertainty_ms"), std::process::id())
            .expect("failed to write floor observation");
    });
}

pub(in crate::livekit) fn reconcile_floor(
    room: &str,
    activity: &mut RuntimeActivity,
    clock: impl Fn() -> String,
    playing: bool,
    output: &mut impl std::io::Write,
) {
    if !activity.candidate_voice_active {
        write_floor(room, activity, &clock, Event::ActivityLost, output);
    }
    if !activity.generating && !playing {
        write_floor(room, activity, &clock, Event::Drained, output);
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/livekit/session/floor.rs"]
mod tests;

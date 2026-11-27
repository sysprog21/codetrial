//! A sign of life while the one slow step runs. A cold index builds the
//! dependencies' build scripts and proc macros and takes minutes, and a
//! terminal that prints nothing for that long looks hung. On a terminal the
//! wait is a spinner with the elapsed time on one line that is erased when the
//! step ends; anywhere else (a log, a pipe, CI) it is one plain line a minute,
//! so a log file never fills with animation frames.

use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const TICK: Duration = Duration::from_millis(100);
const BEAT: Duration = Duration::from_secs(60);

/// Runs `work` on another thread and shows that it is running until it ends.
/// Returns what `work` returns; a panic in it is raised again here.
pub fn while_waiting<T: Send>(label: &str, work: impl FnOnce() -> T + Send) -> T {
    let animate =
        std::io::stderr().is_terminal() && std::env::var("TERM").ok().as_deref() != Some("dumb");
    let started = Instant::now();
    std::thread::scope(|scope| {
        let job = scope.spawn(work);
        let mut beat = started;
        let mut frame = 0usize;
        while !job.is_finished() {
            if animate {
                let spinner = FRAMES[frame % FRAMES.len()];
                eprint!("\r{spinner} {label} {}", clock(started.elapsed()));
                let _ = std::io::stderr().flush();
                frame += 1;
            } else if beat.elapsed() >= BEAT {
                eprintln!("{label} {}", clock(started.elapsed()));
                beat = Instant::now();
            }
            std::thread::sleep(TICK);
        }
        if animate {
            eprint!("\r\x1b[2K");
            let _ = std::io::stderr().flush();
        }
        match job.join() {
            Ok(value) => value,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    })
}

/// `m:ss`, or `h:mm:ss` from an hour on.
fn clock(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_reads_minutes_and_seconds_and_hours_only_when_there_are_some() {
        assert_eq!(clock(Duration::from_secs(0)), "0:00");
        assert_eq!(clock(Duration::from_secs(65)), "1:05");
        assert_eq!(clock(Duration::from_secs(3599)), "59:59");
        assert_eq!(clock(Duration::from_secs(3661)), "1:01:01");
    }

    #[test]
    fn what_the_work_returns_is_returned() {
        assert_eq!(while_waiting("test", || 7), 7);
    }
}

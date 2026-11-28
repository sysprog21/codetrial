//! Running the course's commands: their limits, their output and the
//! processes they leave behind.

use std::time::Duration;

use super::*;
#[cfg(unix)]
use crate::tasks::test_support::executable;

#[cfg(unix)]
#[test]
fn a_run_past_its_limit_loses_its_whole_process_group() {
    let dir = std::env::temp_dir();
    let marker = dir.join(format!("codetrial-defense-orphan-{}", std::process::id()));
    let script = format!("(sleep 2; touch {}) & sleep 30", marker.display());
    let ran = run(
        "sh",
        &["-c".to_owned(), script],
        &dir,
        &[],
        Duration::from_millis(300),
    );
    assert_eq!(ran.ended, Ended::Timeout);
    std::thread::sleep(Duration::from_millis(2500));
    assert!(!marker.exists(), "the background child was killed too");
}

#[cfg(unix)]
#[test]
fn a_run_keeps_only_the_tail_of_its_output() {
    let ran = run(
        "sh",
        &[
            "-c".to_owned(),
            "i=0; while [ $i -lt 20000 ]; do echo line-$i; i=$((i+1)); done; echo done >&2; exit 3"
                .to_owned(),
        ],
        &std::env::temp_dir(),
        &[("UNUSED".to_owned(), "1".to_owned())],
        Duration::from_secs(30),
    );
    assert_eq!(ran.ended, Ended::Exited(3));
    // The output is well over 64 KiB, so the tail is exactly that.
    assert_eq!(TAIL_BYTES, 64 * 1024);
    assert_eq!(ran.tail.len(), 64 * 1024);
    assert!(ran.tail.ends_with("done\n"));
    assert!(!ran.tail.contains("line-0\n"));
}

#[cfg(unix)]
#[test]
fn continuous_output_is_drained_until_timeout() {
    let ran = run(
        "sh",
        &[
            "-c".to_owned(),
            "while :; do printf '0123456789'; done".to_owned(),
        ],
        &std::env::temp_dir(),
        &[],
        Duration::from_millis(500),
    );
    assert_eq!(ran.ended, Ended::Timeout);
    assert_eq!(ran.tail.len(), TAIL_BYTES);
    assert!(ran.tail.bytes().all(|byte| byte.is_ascii_digit()));
}

/// A process that left the group, still holding the output pipe, keeps the
/// end from coming: what the command printed is kept all the same. The
/// escape is util-linux's `setsid`, which macOS does not ship.
#[cfg(target_os = "linux")]
#[test]
fn output_is_kept_when_an_escaped_process_holds_the_pipe() {
    let ran = run(
        "sh",
        &[
            "-c".to_owned(),
            "setsid sleep 5 & echo printed-before-exit; exit 0".to_owned(),
        ],
        &std::env::temp_dir(),
        &[],
        Duration::from_secs(10),
    );
    assert_eq!(ran.ended, Ended::Exited(0));
    assert_eq!(ran.tail, "printed-before-exit\n");
}

#[test]
fn a_command_that_does_not_exist_is_a_telemetry_gap() {
    let ran = run(
        "codetrial-no-such-tool",
        &[],
        &std::env::temp_dir(),
        &[],
        Duration::from_secs(1),
    );
    assert!(matches!(ran.ended, Ended::NotStarted(_)));
}

/// A relative path is the one in the directory the command runs in.
#[cfg(unix)]
#[test]
fn a_relative_command_is_the_one_in_its_directory() {
    let dir = std::env::temp_dir().join(format!("codetrial-resolve-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    let tool = dir.join("bin/say");
    std::fs::write(&tool, "#!/bin/sh\necho from-dir\n").unwrap();
    executable(&tool);
    let ran = run("bin/say", &[], &dir, &[], Duration::from_secs(5));
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(ran.ended, Ended::Exited(0));
    assert_eq!(ran.tail.trim(), "from-dir");
}

/// A command's leftover process is killed with its group when the command
/// ends by itself, not only when it outlives its limit.
#[cfg(unix)]
#[test]
fn a_command_that_ends_takes_its_leftovers_with_it() {
    let dir = std::env::temp_dir();
    let marker = dir.join(format!("codetrial-defense-leftover-{}", std::process::id()));
    let _gone = std::fs::remove_file(&marker);
    let script = format!("(sleep 1; touch {}) > /dev/null 2>&1 &", marker.display());
    let ran = run(
        "sh",
        &["-c".to_owned(), script],
        &dir,
        &[],
        Duration::from_secs(20),
    );
    assert_eq!(ran.ended, Ended::Exited(0));
    std::thread::sleep(Duration::from_millis(2500));
    assert!(!marker.exists(), "the leftover was killed with its group");
}

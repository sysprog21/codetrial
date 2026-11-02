use super::*;
use serde_json::Value;

#[test]
fn closing_transition_is_logged_once_for_received_and_server_controls() {
    const NAME: &str = "livekit::packets::tests::closing_transition_is_logged_once_for_received_and_server_controls";
    if let Ok(origin) = std::env::var("CODETRIAL_REPLAY_END_TEST") {
        let received = origin == "received";
        let start = Instant::now();
        let mut state = RuntimeState::default();
        let mut activity = RuntimeActivity::for_interview(start, 0, 7);
        activity.latency.bind("closing-test", 7, 1, 1500);
        activity.latency.anchor(start);
        let payload = serde_json::json!({"type":"end_interview", "reason":"synthetic-end"});
        let first = reduce_data_packet(
            &mut state,
            &mut activity,
            TOPIC_CONTROL,
            &payload,
            received,
            1000,
        );
        assert!(first.finish_interview.is_some());
        assert!(state.ended);

        // The record must precede this marker: logging only on a repeat would
        // still leave one record in total.
        eprintln!("step:first");
        let again = reduce_data_packet(
            &mut state,
            &mut activity,
            TOPIC_CONTROL,
            &payload,
            received,
            1001,
        );
        assert!(again.finish_interview.is_none());
        eprintln!("step:again");
        return;
    }
    for origin in ["received", "server"] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME, "--nocapture"])
            .env("CODETRIAL_REPLAY_END_TEST", origin)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        let order = stderr
            .lines()
            .filter_map(|line| {
                if line.starts_with("codetrial replay_gate ") {
                    Some("record")
                } else if line.starts_with("step:") {
                    Some(line)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(
            order,
            ["record", "step:first", "step:again"],
            "origin={origin}"
        );
        let records = stderr
            .lines()
            .filter_map(|line| line.strip_prefix("codetrial replay_gate "))
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records[0]["stage"], "closing");
        assert_eq!(records[0]["session"], 7);
        assert_eq!(records[0]["socket"], 1);
        assert_eq!(records[0]["room"], "closing-test");
        assert!(records[0]["turn"].is_null());
    }
}

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

#[test]
fn a_test_result_reads_its_reaction_clock_for_received_and_server_packets() {
    for received in [false, true] {
        for expired in [false, true] {
            let now = Instant::now();

            // The pure decision pins the threshold. A synthetic future instant
            // gives zero elapsed without waiting at the cooldown edge; opposite
            // ages expose a wrong clock source.
            let old = now.checked_sub(Duration::from_secs(100)).unwrap();
            let future = now.checked_add(Duration::from_secs(3600)).unwrap();
            let mut state = RuntimeState::default();
            assert!(!state.ended && !state.thinking_hold.is_active());
            let mut activity = RuntimeActivity::for_interview(now, 0, 7);
            activity.last_test_reaction = if expired { old } else { future };
            activity.last_code_change = if expired { future } else { old };
            activity.last_interjection = activity.last_code_change;
            let result = reduce_data_packet(
                &mut state,
                &mut activity,
                crate::runtime::TOPIC_TEST_RESULTS,
                &serde_json::json!({"passed":1,"total":2}),
                received,
                1000,
            );
            assert_eq!(state.test_runs, 1);
            assert_eq!(state.last_test_run.as_ref().unwrap()["passed"], 1);
            assert_eq!(state.last_test_run.as_ref().unwrap()["total"], 2);
            assert!(state.tested_code.is_none());
            assert!(state.framework_evidence.is_empty());
            assert!(state.runner_unavailable.is_none());
            assert!(!state.ended && !state.thinking_hold.is_active());
            assert_eq!(
                state.evidence_ledger.metrics.raw_events_received,
                u64::from(received)
            );
            assert!(result.test_run.is_some());
            assert_eq!(
                result.generate_reply.is_some(),
                expired,
                "{received}/{expired}"
            );
            assert_eq!(
                result.update_last_test_reaction, expired,
                "{received}/{expired}"
            );
            assert_eq!(
                result.update_last_interjection, expired,
                "{received}/{expired}"
            );
            assert!(result.held_context.is_none());
        }
    }
}

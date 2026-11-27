// Included in the turn test module so existing test paths stay unchanged.

#[test]
fn latency_recovery_prompt_keeps_the_wait_and_runtime_prompt_debt() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    let mut probes = TurnProbes::default();
    probes.latency.voice(start);
    activity.live_socket = 2;
    session::reset_context_observations(&mut activity, &mut probes, true);
    activity.mark_recovery_prompted(start + Duration::from_secs(10), Some("owed"));
    let record = probes
        .latency
        .audio(start + Duration::from_secs(11))
        .unwrap();
    assert_eq!(record["origin"], "last_voiced_pcm");
    assert_eq!(record["elapsed_ms"], 11_000);
    assert!(activity.owes_prompt());
    assert_eq!(activity.prompt_text.as_deref(), Some("owed"));
}

#[test]
fn a_late_transcript_does_not_seed_the_next_latency_trace() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    let mut probes = TurnProbes::default();
    probes.latency.voice(start);
    probes.latency.audio(start + Duration::from_millis(500));
    let ended = start + Duration::from_secs(1);
    probes
        .latency
        .boundary(super::super::latency::Boundary::TurnComplete, ended);
    activity.turn_completed_at = Some(ended);
    session::note_latency_transcript(&activity, &mut probes, ended + Duration::from_millis(500));
    let record = probes
        .latency
        .audio(ended + Duration::from_secs(30))
        .unwrap();
    assert_eq!(record["origin"], "provider_event");
    assert_eq!(record["elapsed_ms"], 0);
}

#[test]
fn either_confirmation_path_closes_a_provisional_holds_latency_trace() {
    for timed_out in [false, true] {
        let start = Instant::now();
        let mut activity = RuntimeActivity::new(start);
        let mut probes = TurnProbes::default();
        probes.latency.voice(start);
        let mut state = RuntimeState {
            thinking_hold: ThinkingHold::Requested { since: start },
            ..RuntimeState::default()
        };
        let declared = start + crate::agent::THINKING_REQUEST_SETTLE;
        if timed_out {
            session::settle_stale_hold(&mut state, &mut activity, &mut probes, declared, 1);
        } else {
            session::confirm_hold_request(&mut state, &mut activity, &mut probes, declared, 1);
        }
        assert!(state.thinking_hold.is_active());
        assert!(state.end_thinking(2));
        let released = declared + Duration::from_secs(45);
        session::note_latency_transcript(&activity, &mut probes, released);
        let record = probes
            .latency
            .audio(released + Duration::from_millis(500))
            .unwrap();
        assert_eq!(record["origin"], "transcript");
        assert_eq!(record["elapsed_ms"], 500);
    }
}

/// A prompt the room asks for itself, or the reply a handoff socket owed, is
/// owed to the activity and opens the latency trace its reply is timed on.
#[test]
fn a_prompt_asked_for_directly_is_owed_and_traced() {
    let start = Instant::now();
    let later = start + Duration::from_secs(1);

    let mut activity = RuntimeActivity::new(start);
    let mut probes = TurnProbes::default();
    session::mark_asked(&mut activity, &mut probes, start, "Check in.");
    assert!(activity.owes_prompt());
    assert_eq!(activity.prompt_text.as_deref(), Some("Check in."));
    assert!(probes.latency.audio(later).is_some());

    let mut activity = RuntimeActivity::new(start);
    let mut probes = TurnProbes::default();
    session::renew_handoff_prompt(&mut activity, &mut probes, start);
    assert!(activity.owes_prompt());
    assert!(probes.latency.audio(later).is_some());
}

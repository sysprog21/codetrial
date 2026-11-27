// Included in the session test module so existing test paths stay unchanged.

#[tokio::test]
async fn an_unqueued_model_audio_chunk_freezes_the_speech_proxy() {
    let start = Instant::now();
    for mime_type in ["audio/pcm;rate=24000", "audio/unsupported"] {
        let mut activity = RuntimeActivity::new(start);
        let mut probes = TurnProbes::default();
        probes.turn_timing.note_voice(start);
        note_candidate_finished(&mut activity, &mut probes, start, false);
        let event = GeminiEvent::Audio {
            bytes: vec![0, 0],
            mime_type: mime_type.into(),
        };
        assert!(accept_timed(
            &event,
            &mut activity,
            &mut probes,
            false,
            start
        ));
        let (mut output, _queued) = test_output_audio();
        assert!(!output.capture(&[0, 0], mime_type).await.unwrap());
        assert!(probes.turn_timing.note_audio(start, false).is_none());
        probes
            .turn_timing
            .note_voice(start + Duration::from_millis(900));
        let summary = probes
            .turn_timing
            .note_audio(start + Duration::from_secs(1), true)
            .unwrap();
        assert_eq!(summary.wait, Some(Duration::from_secs(1)));
    }
}

#[test]
fn late_output_transcripts_do_not_reopen_a_completed_measurement() {
    let start = Instant::now();
    for audio in [false, true] {
        let mut activity = RuntimeActivity::new(start);
        let mut probes = TurnProbes::default();
        note_candidate_finished(&mut activity, &mut probes, start, false);
        let text = GeminiEvent::OutputTranscript("reply".into());
        assert!(accept_timed(
            &text,
            &mut activity,
            &mut probes,
            false,
            start
        ));
        if audio {
            assert!(probes.turn_timing.note_audio(start, true).is_some());
        }
        assert!(accept_timed(
            &GeminiEvent::TurnComplete,
            &mut activity,
            &mut probes,
            false,
            start
        ));
        probes.turn_timing.finish(start, TimingOutcome::NoAudio);
        assert!(accept_timed(
            &text,
            &mut activity,
            &mut probes,
            false,
            start
        ));
        assert!(
            probes
                .turn_timing
                .finish(start, TimingOutcome::Ended)
                .is_none()
        );

        // A real new candidate observation still allows transcript-only output.
        probes
            .turn_timing
            .note_transcript(start + Duration::from_secs(2));
        assert!(accept_timed(
            &text,
            &mut activity,
            &mut probes,
            false,
            start
        ));
        probes
            .turn_timing
            .note_voice(start + Duration::from_millis(2_900));
        let next = probes
            .turn_timing
            .note_audio(start + Duration::from_secs(3), true)
            .unwrap();
        assert_eq!(next.id, 2);
        assert_eq!(next.wait, Some(Duration::from_secs(1)));
    }
}

#[test]
fn activity_before_interruption_preserves_reply_debt_and_next_turn_timing() {
    let start = Instant::now();
    let state = RuntimeState::default();
    let mut activity = RuntimeActivity::new(start);
    let mut probes = TurnProbes::default();
    note_candidate_finished(&mut activity, &mut probes, start, false);
    assert!(accept_timed(
        &GeminiEvent::Text("old reply".into()),
        &mut activity,
        &mut probes,
        false,
        start
    ));
    let voice = start + Duration::from_millis(300);
    activity.note_candidate_voice(voice);
    probes.turn_timing.note_voice(voice);
    assert!(observe_voice_activity(
        &state,
        &mut activity,
        &mut probes,
        true,
        None,
        voice,
        true
    ));
    assert!(accept_timed(
        &GeminiEvent::Interrupted,
        &mut activity,
        &mut probes,
        false,
        voice
    ));
    let (mut output, _queued) = test_output_audio();
    cut_off_turn(&mut RuntimeState::default(), &mut activity, &mut output);
    probes.turn_timing.finish(voice, TimingOutcome::Interrupted);
    assert!(!accept_timed(
        &GeminiEvent::TurnComplete,
        &mut activity,
        &mut probes,
        false,
        voice
    ));
    // The guard rebases existing debt; it does not create debt by itself.
    activity.owe_prompt(voice, Some("new prompt".into()));
    let end = start + Duration::from_millis(400);
    assert!(observe_voice_activity(
        &state,
        &mut activity,
        &mut probes,
        false,
        Some(Duration::from_secs(50)),
        end,
        true
    ));
    assert_eq!(
        activity.prompted_at,
        Some(end),
        "activity end must rebase the owed prompt"
    );
    let next = probes
        .turn_timing
        .note_audio(start + Duration::from_secs(1), true)
        .unwrap();
    assert_eq!(next.id, 2);
    assert_eq!(next.wait, Some(Duration::from_millis(700)));
    assert_eq!(next.turn_close, Some(Duration::from_millis(100)));
}

#[test]
fn timing_emission_writes_one_line_per_measurement_with_session_identity() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::for_interview(start, 0, 42);
    let mut probes = TurnProbes::default();
    activity.live_socket = 3;
    let mut output = Vec::new();
    finish_turn_timing(
        "room-a",
        &mut activity,
        &mut probes,
        "0:01.000",
        TimingOutcome::Audio,
        &mut output,
    );
    finish_turn_timing(
        "room-a",
        &mut activity,
        &mut probes,
        "0:01.100",
        TimingOutcome::Audio,
        &mut output,
    );
    finish_turn_timing(
        "room-a",
        &mut activity,
        &mut probes,
        "0:01.200",
        TimingOutcome::NoAudio,
        &mut output,
    );
    probes.turn_timing.note_transcript(start);
    finish_turn_timing(
        "room-a",
        &mut activity,
        &mut probes,
        "0:02.000",
        TimingOutcome::Held,
        &mut output,
    );
    finish_turn_timing(
        "room-a",
        &mut activity,
        &mut probes,
        "0:02.100",
        TimingOutcome::Ended,
        &mut output,
    );
    let log = String::from_utf8(output).unwrap();
    let lines = log.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2);
    assert!(
        lines[0].starts_with(
            "codetrial turn_timing room=room-a session=42 socket=3 turn=1 at=0:01.000 "
        )
    );
    assert!(lines[0].ends_with("outcome=audio"));
    assert!(lines[1].contains("turn=2 at=0:02.000 "));
    assert!(lines[1].contains("wait_ms=unknown "));
    assert!(lines[1].ends_with("outcome=held"));
    assert!(log.ends_with('\n'));
}

#[test]
fn voice_observations_preserve_runtime_and_timing_hold_filters() {
    let start = Instant::now();
    for (held, discarding, has_audio) in [
        (false, false, true),
        (true, false, true),
        (false, true, true),
        (false, false, false),
    ] {
        for active in [true, false] {
            let mut state = RuntimeState::default();
            if held {
                state.thinking_hold = ThinkingHold::Held { since: start };
            }
            let mut activity = RuntimeActivity::new(start);
            let mut probes = TurnProbes::default();
            activity.socket.discarding_output = discarding;
            activity.note_candidate_voice(start);
            probes.turn_timing.note_voice(start);
            let observed = observe_voice_activity(
                &state,
                &mut activity,
                &mut probes,
                active,
                Some(Duration::from_secs(5)),
                start,
                has_audio,
            );
            assert_eq!(observed, has_audio);
            assert_eq!(activity.incoming_speech_active(start), observed && active);
            let summary = probes.turn_timing.finish(start, TimingOutcome::NoAudio);
            assert_eq!(summary.is_some(), !held && !discarding);
            if let Some(summary) = summary {
                assert_eq!(
                    summary.turn_close_reason,
                    if active { "not_observed" } else { "observed" }
                );
                assert_eq!(
                    summary.audio_offset,
                    if active {
                        None
                    } else {
                        Some(Duration::from_secs(5))
                    }
                );
            }
        }
    }
}

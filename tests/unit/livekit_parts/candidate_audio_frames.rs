// Included in the livekit test module so existing test paths stay unchanged.

/// After the socket has transcribed the candidate, without compression or a
/// provider speech event, a voiced frame no longer feeds the idle timers but
/// is still reported, which is what keeps the camera gate open during speech.
#[test]
fn voiced_frames_are_reported_after_transcription_without_provider_events() {
    let start = Instant::now();
    let state = RuntimeState::default();
    let mut activity = RuntimeActivity::new(start);
    activity.transcribed_on_socket = true;
    let loud = ::livekit::webrtc::audio_frame::AudioFrame {
        data: vec![1000i16; 1600].into(),
        sample_rate: 16_000,
        num_channels: 1,
        samples_per_channel: 1600,
    };
    let quiet = ::livekit::webrtc::audio_frame::AudioFrame {
        data: vec![0i16; 1600].into(),
        sample_rate: 16_000,
        num_channels: 1,
        samples_per_channel: 1600,
    };
    assert!(note_candidate_audio_frame(
        &state,
        &mut activity,
        &mut TurnProbes::default(),
        Some(&loud),
        start
    ));
    assert!(!activity.incoming_speech_active(start));
    assert!(!note_candidate_audio_frame(
        &state,
        &mut activity,
        &mut TurnProbes::default(),
        Some(&quiet),
        start
    ));
}

#[test]
fn timing_pcm_ignores_silent_missing_and_thinking_frames() {
    let start = Instant::now();
    let loud = ::livekit::webrtc::audio_frame::AudioFrame {
        data: vec![1000i16; 1600].into(),
        sample_rate: 16_000,
        num_channels: 1,
        samples_per_channel: 1600,
    };
    let quiet = ::livekit::webrtc::audio_frame::AudioFrame {
        data: vec![0i16; 1600].into(),
        sample_rate: 16_000,
        num_channels: 1,
        samples_per_channel: 1600,
    };
    for mode in 0..4 {
        let mut state = RuntimeState::default();
        if mode == 1 {
            state.thinking_hold = crate::agent::ThinkingHold::Held { since: start };
        }
        let mut activity = RuntimeActivity::new(start);
        let mut probes = TurnProbes::default();
        let frame = match mode {
            2 => Some(&quiet),
            3 => None,
            _ => Some(&loud),
        };
        note_candidate_audio_frame(&state, &mut activity, &mut probes, frame, start);
        probes
            .turn_timing
            .note_transcript(start + Duration::from_millis(100));
        let summary = probes
            .turn_timing
            .note_audio(start + Duration::from_secs(1), true)
            .unwrap();
        assert_eq!(
            summary.start_source,
            if mode == 0 {
                "pcm_level"
            } else {
                "last_transcript"
            },
            "mode {mode}"
        );
        assert_eq!(
            summary.wait,
            Some(Duration::from_millis(if mode == 0 { 1000 } else { 900 })),
            "mode {mode}"
        );
    }
}

#[test]
fn actual_pcm_path_protects_long_speech_but_not_silent_input() {
    let start = Instant::now();
    let state = RuntimeState::default();
    let mut activity = RuntimeActivity::new(start);
    activity.mark_prompted(start, Some("resume"), false);
    activity.note_candidate_voice_activity(true, start);
    let loud = ::livekit::webrtc::audio_frame::AudioFrame {
        data: vec![1000i16; 1600].into(),
        sample_rate: 16_000,
        num_channels: 1,
        samples_per_channel: 1600,
    };
    let quiet = ::livekit::webrtc::audio_frame::AudioFrame {
        data: vec![0i16; 1600].into(),
        sample_rate: 16_000,
        num_channels: 1,
        samples_per_channel: 1600,
    };
    let mut restart = DeferredRestart::default();
    assert!(!restart.request(Floor::Listening, false, true, false));
    for seconds in 0..=180 {
        let now = start + Duration::from_secs(seconds);
        note_candidate_audio_frame(
            &state,
            &mut activity,
            &mut TurnProbes::default(),
            Some(&loud),
            now,
        );
        let stalls = activity.settle_stalls(now, false);
        assert!(!stalls.prompt_released && !stalls.spend_restart);
        assert_eq!(
            reply_watch(&state, &activity, now, false),
            ReplyWatch::Owed { shown: false }
        );

        // An ordinary restart waits the speech out; the handoff that finalizes
        // the utterance first is what the watch spends instead.
        assert!(!restart.can_spend(&activity, now));
        assert!(restart.handoff_due(&activity, now));
    }
    let end = start + Duration::from_secs(181);
    activity.note_candidate_voice_activity(false, end);
    let after_end = end + PROMPT_STALL;
    let stalls = activity.settle_stalls(after_end, false);
    assert!(restart.watch_due(&activity, stalls.spend_restart, after_end));
    restart.cancel();
    assert!(!restart.watch_due(&activity, true, after_end));
    assert_eq!(
        reply_watch(&state, &activity, end, false),
        ReplyWatch::Owed { shown: false }
    );
    assert_eq!(
        reply_watch(&state, &activity, end + Duration::from_secs(45), false),
        ReplyWatch::Recover
    );

    // Past the quiet window of the last loud frame, a VAD edge carried only by
    // silent or missing frames is not speech.
    let quiet_at = end + CHECKPOINT_VOICE_QUIET;
    activity.mark_prompted(quiet_at, Some("next"), false);
    activity.note_candidate_voice_activity(true, quiet_at);
    note_candidate_audio_frame(
        &state,
        &mut activity,
        &mut TurnProbes::default(),
        Some(&quiet),
        quiet_at,
    );
    note_candidate_audio_frame(
        &state,
        &mut activity,
        &mut TurnProbes::default(),
        None,
        quiet_at,
    );
    assert!(!activity.incoming_speech_active(quiet_at));
    assert_eq!(
        reply_watch(&state, &activity, quiet_at + Duration::from_secs(45), false),
        ReplyWatch::Recover
    );
    for resumed in [false, true] {
        note_candidate_audio_frame(
            &state,
            &mut activity,
            &mut TurnProbes::default(),
            Some(&loud),
            end,
        );
        activity.reset_context_observations(resumed);
        activity.note_candidate_voice_activity(true, end);
        assert!(!activity.incoming_speech_active(end));
    }
}

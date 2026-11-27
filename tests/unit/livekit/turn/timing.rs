use super::*;

fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}

#[test]
fn old_microphone_noise_is_not_the_start_of_a_quiet_answer() {
    let start = Instant::now();
    let mut timing = TurnTiming::default();
    timing.note_voice(start);
    timing.note_transcript(start + ms(60_000));
    let summary = timing.note_audio(start + ms(61_000), true).unwrap();
    assert_eq!(summary.start_source, "last_transcript");
    assert_eq!(summary.wait, Some(ms(1_000)));
}

#[test]
fn activity_end_does_not_borrow_old_microphone_noise() {
    let start = Instant::now();
    let mut timing = TurnTiming::default();
    timing.note_voice(start);
    timing.note_activity_end(start + ms(60_000), None);
    let summary = timing.note_audio(start + ms(61_000), true).unwrap();
    assert_eq!(summary.start_source, "unknown");
    assert_eq!(summary.wait, None);
    assert_eq!(summary.turn_close_reason, "no_start");
}

#[test]
fn pcm_freshness_rejects_the_quiet_boundary_and_future_frames() {
    let start = Instant::now();
    for (voice, source) in [
        (start + ms(1), "pcm_level"),
        (start, "last_transcript"),
        (start + ms(2_001), "last_transcript"),
    ] {
        let mut timing = TurnTiming::default();
        timing.note_voice(voice);
        timing.note_transcript(start + ms(2_000));
        let summary = timing.note_audio(start + ms(3_000), true).unwrap();
        assert_eq!(summary.start_source, source);
    }
    let mut timing = TurnTiming::default();
    timing.note_voice(start);
    timing.note_activity_start(start + ms(60_000));
    timing.note_transcript(start + ms(60_100));
    assert_eq!(
        timing.note_audio(start + ms(61_000), true).unwrap().wait,
        Some(ms(900))
    );
}

#[test]
fn only_a_new_thinking_hold_finalizes_a_measurement() {
    let start = Instant::now();
    let mut timing = TurnTiming::default();
    timing.note_transcript(start);
    assert!(
        timing
            .finish_thinking_change(start + ms(100), None)
            .is_none()
    );
    assert!(
        timing
            .finish_thinking_change(start + ms(200), Some(false))
            .is_none()
    );
    let summary = timing
        .finish_thinking_change(start + ms(300), Some(true))
        .unwrap();
    assert_eq!(summary.elapsed, Some(ms(300)));
    assert!(matches!(summary.outcome, TimingOutcome::Held));
    assert!(
        timing
            .finish_thinking_change(start + ms(400), Some(true))
            .is_none()
    );
}

#[test]
fn response_errors_count_failures_rather_than_successes() {
    let start = Instant::now();
    for (sent, errors) in [(true, 0), (false, 1)] {
        let mut timing = TurnTiming::default();
        timing.note_transcript(start);
        timing.note_tool_response(start + ms(100), start + ms(200), sent);
        let summary = timing.note_audio(start + ms(300), true).unwrap();
        assert_eq!(summary.tool_response_errors, errors);
    }
}

#[test]
fn coincident_boundaries_are_valid_zero_durations() {
    let start = Instant::now();
    let mut timing = TurnTiming::default();
    timing.note_transcript(start);
    timing.note_activity_end(start, None);
    timing.note_tool_execution(start, start);
    let summary = timing.note_audio(start, true).unwrap();
    assert_eq!(summary.wait, Some(Duration::ZERO));
    assert_eq!(summary.turn_close, Some(Duration::ZERO));
    assert_eq!(summary.turn_close_reason, "observed");
    assert_eq!(summary.tool_wait, Some(Duration::ZERO));
    assert_eq!(summary.tool_after_close, Some(Duration::ZERO));
    assert_eq!(summary.other, Some(Duration::ZERO));
    assert_eq!(
        span_time(&[(start + ms(20), start + ms(30))], start + ms(10), start),
        None
    );
}

#[test]
fn transcript_wait_without_tools_has_an_unknown_close() {
    let start = Instant::now();
    let mut timing = TurnTiming::default();
    timing.note_transcript(start);
    timing.note_transcript(start + ms(500));
    let summary = timing
        .finish(start + ms(2_500), TimingOutcome::Audio)
        .unwrap();
    assert_eq!(summary.wait, Some(ms(2_000)));
    assert_eq!(summary.start_source, "last_transcript");
    assert_eq!(summary.turn_close, None);
    assert_eq!(summary.tool_calls, 0);
    assert_eq!(summary.tool_exec, Duration::ZERO);
    assert!(
        summary
            .log_line("room-a", 10, 1, "0:02.500")
            .contains("turn_close_ms=unknown")
    );
}

#[test]
fn observed_endpoint_uses_receipt_clock_not_provider_audio_offset() {
    let start = Instant::now();
    let mut timing = TurnTiming::default();
    timing.note_voice(start);
    timing.note_activity_end(start + ms(1_000), Some(ms(123_456)));
    // A later transcript must not move the PCM proxy past the endpoint.
    timing.note_transcript(start + ms(1_200));
    let summary = timing
        .finish(start + ms(2_000), TimingOutcome::Audio)
        .unwrap();
    assert_eq!(summary.start_source, "pcm_level");
    assert_eq!(summary.wait, Some(ms(2_000)));
    assert_eq!(summary.turn_close, Some(ms(1_000)));
    assert_eq!(summary.audio_offset, Some(ms(123_456)));
    let line = summary.log_line("room-a", 10, 1, "0:02.000");
    assert!(line.contains("turn_close_source=voice_activity_received"));
    assert!(line.contains("vad_audio_offset_ms=123456"));
}

#[test]
fn multiple_tools_and_batches_sum_execution_and_send_separately() {
    let start = Instant::now();
    let mut timing = TurnTiming::default();
    timing.note_transcript(start);
    timing.note_tool_execution(start + ms(100), start + ms(300));
    timing.note_tool_execution(start + ms(400), start + ms(700));
    timing.note_tool_response(start + ms(700), start + ms(710), true);
    timing.note_tool_execution(start + ms(750), start + ms(875));
    timing.note_tool_response(start + ms(900), start + ms(930), false);
    let summary = timing
        .finish(start + ms(2_000), TimingOutcome::Audio)
        .unwrap();
    assert_eq!(summary.wait, Some(ms(2_000)));
    assert_eq!(summary.tool_calls, 3);
    assert_eq!(summary.tool_exec, ms(625));
    assert_eq!(summary.tool_response, ms(40));
    assert_eq!(summary.tool_response_errors, 1);
    assert_eq!(summary.tool_wait, Some(ms(665)));
    assert_eq!(summary.other, None);
    assert_eq!(
        summary.turn_close, None,
        "the residual must not be called VAD time"
    );
}

#[test]
fn prompt_reply_with_no_candidate_timestamp_does_not_invent_a_wait() {
    let start = Instant::now();
    let mut timing = TurnTiming::default();
    timing.note_tool_execution(start, start + ms(10));
    let summary = timing.finish(start + ms(20), TimingOutcome::Audio).unwrap();
    assert_eq!(summary.wait, None);
    assert_eq!(summary.start_source, "unknown");
    assert_eq!(summary.tool_calls, 1);
    assert!(
        summary
            .log_line("room-a", 10, 1, "0:00.000")
            .contains("wait_ms=unknown")
    );
}

#[test]
fn audio_chunks_and_tools_after_first_audio_do_not_duplicate_the_summary() {
    let start = Instant::now();
    let mut timing = TurnTiming::default();
    timing.note_transcript(start);
    assert!(
        timing
            .finish(start + ms(100), TimingOutcome::Audio)
            .is_some()
    );
    timing.note_tool_execution(start + ms(200), start + ms(210));
    timing.note_tool_response(start + ms(220), start + ms(240), true);
    timing.note_activity_end(start + ms(200), None);
    assert!(
        timing
            .finish(start + ms(300), TimingOutcome::Audio)
            .is_none()
    );
    assert!(
        timing
            .finish(start + ms(400), TimingOutcome::NoAudio)
            .is_none()
    );
    timing.note_transcript(start + ms(1_000));
    let summary = timing
        .finish(start + ms(1_500), TimingOutcome::Audio)
        .unwrap();
    assert_eq!(summary.id, 2);
    assert_eq!(summary.wait, Some(ms(500)));
    assert_eq!(summary.tool_calls, 0);
    assert_eq!(summary.turn_close, None);
}

#[test]
fn non_audio_endings_flush_once_and_never_reuse_old_times_or_tools() {
    let start = Instant::now();
    for outcome in [
        TimingOutcome::NoAudio,
        TimingOutcome::Interrupted,
        TimingOutcome::Replaced,
        TimingOutcome::Held,
        TimingOutcome::Ended,
    ] {
        let mut timing = TurnTiming::default();
        timing.note_voice(start);
        timing.note_transcript(start + ms(100));
        timing.note_activity_end(start + ms(200), Some(ms(40)));
        timing.note_tool_execution(start + ms(300), start + ms(600));
        let summary = timing.finish(start + ms(1_000), outcome).unwrap();
        assert_eq!(summary.wait, None, "there was no first reply audio");
        assert_eq!(summary.elapsed, Some(ms(1_000)));
        assert_eq!(summary.tool_after_close, None);
        assert!(
            summary
                .log_line("room-a", 10, 1, "0:01.000")
                .ends_with(outcome.label())
        );
        assert!(timing.finish(start + ms(1_100), outcome).is_none());
        timing.note_transcript(start + ms(2_000));
        let next = timing
            .finish(start + ms(2_100), TimingOutcome::Audio)
            .unwrap();
        assert_eq!(next.id, 2);
        assert_eq!(next.wait, Some(ms(100)));
        assert_eq!(next.start_source, "last_transcript");
        assert_eq!(next.tool_calls, 0);
        assert_eq!(next.audio_offset, None);
    }
}

#[test]
fn reordered_endpoint_before_transcript_is_unknown_not_zero() {
    let start = Instant::now();
    let mut timing = TurnTiming::default();
    timing.note_activity_end(start, Some(ms(40)));
    timing.note_transcript(start + ms(100));
    let summary = timing
        .finish(start + ms(200), TimingOutcome::Audio)
        .unwrap();
    assert_eq!(summary.wait, Some(ms(100)));
    assert_eq!(summary.turn_close, None);
}

#[test]
fn confirmed_activity_start_reopens_speech_before_model_output() {
    let start = Instant::now();
    let mut timing = TurnTiming::default();
    timing.note_voice(start);
    timing.note_transcript(start + ms(100));
    timing.note_activity_end(start + ms(200), Some(ms(40)));
    timing.note_activity_start(start + ms(250));
    timing.note_voice(start + ms(300));
    timing.note_activity_end(start + ms(600), Some(ms(55_000)));
    let summary = timing
        .finish(start + ms(800), TimingOutcome::Audio)
        .unwrap();
    assert_eq!(summary.wait, Some(ms(500)));
    assert_eq!(summary.turn_close, Some(ms(300)));
    assert_eq!(summary.audio_offset, Some(ms(55_000)));
}

#[test]
fn reset_clears_socket_observations_but_keeps_turn_ids_unique() {
    let start = Instant::now();
    let mut timing = TurnTiming::default();
    timing.note_voice(start);
    timing.note_transcript(start + ms(100));
    timing.note_tool_execution(start + ms(110), start + ms(120));
    timing.reset();
    let summary = timing
        .finish(start + ms(1_000), TimingOutcome::Audio)
        .unwrap();
    assert_eq!(summary.id, 2);
    assert_eq!(summary.wait, None);
    assert_eq!(summary.tool_calls, 0);
}

#[test]
fn summaries_identify_the_room_session_socket_and_turn() {
    let mut timing = TurnTiming::default();
    let summary = timing.finish(Instant::now(), TimingOutcome::Audio).unwrap();
    assert_eq!(
        summary.log_line("room-a", 42, 3, "1:02.003"),
        "codetrial turn_timing room=room-a session=42 socket=3 turn=1 at=1:02.003 start_source=unknown wait_ms=unknown elapsed_ms=unknown transcript_wait_ms=unknown turn_close_ms=unknown turn_close_source=unknown turn_close_reason=no_start vad_audio_offset_ms=unknown tool_calls=0 pre_tool_ms=unknown tool_phase_ms=unknown tool_exec_ms=0 tool_response_ms=0 tool_response_errors=0 tool_wait_ms=unknown tool_after_close_ms=unknown other_ms=unknown outcome=audio"
    );
}

#[test]
fn rejected_audio_leaves_the_wait_and_tools_for_the_first_accepted_chunk() {
    let start = Instant::now();
    let mut timing = TurnTiming::default();
    timing.note_transcript(start);
    timing.note_tool_execution(start + ms(100), start + ms(500));
    assert!(timing.note_audio(start + ms(1_000), false).is_none());
    let summary = timing.note_audio(start + ms(2_000), true).unwrap();
    assert_eq!(summary.wait, Some(ms(2_000)));
    assert_eq!(summary.tool_calls, 1);
    assert_eq!(summary.tool_exec, ms(400));
    assert!(timing.note_audio(start + ms(2_100), true).is_none());
}

#[test]
fn text_only_response_has_one_no_audio_summary_with_no_invented_wait() {
    let now = Instant::now();
    let mut timing = TurnTiming::default();
    timing.note_model_output();
    let summary = timing.finish(now, TimingOutcome::NoAudio).unwrap();
    assert_eq!(summary.wait, None);
    assert_eq!(summary.start_source, "unknown");
    assert!(timing.finish(now, TimingOutcome::NoAudio).is_none());
}

#[test]
fn tool_phase_includes_the_model_continuation_without_calling_it_execution() {
    let start = Instant::now();
    let mut timing = TurnTiming::default();
    timing.note_transcript(start);
    timing.note_tool_request(start + ms(1_000));
    timing.note_tool_execution(start + ms(1_000), start + ms(1_400));
    timing.note_tool_response(start + ms(1_400), start + ms(1_420), true);
    timing.note_tool_request(start + ms(1_500));
    timing.note_tool_execution(start + ms(1_500), start + ms(1_600));
    let summary = timing.note_audio(start + ms(2_000), true).unwrap();
    assert_eq!(summary.pre_tool, Some(ms(1_000)));
    assert_eq!(summary.tool_phase, Some(ms(1_000)));
    assert_eq!(summary.tool_exec, ms(500));
    assert_eq!(summary.turn_close, None);
}

#[test]
fn frozen_wait_survives_pcm_after_the_first_tool() {
    let start = Instant::now();
    let mut timing = TurnTiming::default();
    timing.note_voice(start);
    timing.note_transcript(start + ms(10));
    timing.note_tool_request(start + ms(100));
    timing.note_voice(start + ms(1_097));
    let summary = timing.note_audio(start + ms(1_100), true).unwrap();
    assert_eq!(summary.wait, Some(ms(1_100)));
    assert_eq!(summary.pre_tool, Some(ms(100)));
    assert_eq!(summary.tool_phase, Some(ms(1_000)));
}

#[test]
fn frozen_wait_keeps_the_endpoint_when_later_pcm_arrives() {
    let start = Instant::now();
    let mut timing = TurnTiming::default();
    timing.note_voice(start);
    timing.note_activity_end(start + ms(400), Some(ms(99_000)));
    timing.note_voice(start + ms(997));
    timing.note_transcript(start + ms(998));
    let summary = timing.note_audio(start + ms(1_000), true).unwrap();
    assert_eq!(summary.wait, Some(ms(1_000)));
    assert_eq!(summary.turn_close, Some(ms(400)));
    assert_eq!(summary.audio_offset, Some(ms(99_000)));
}

#[test]
fn frozen_wait_survives_pcm_after_model_text() {
    let start = Instant::now();
    let mut timing = TurnTiming::default();
    timing.note_voice(start);
    timing.note_transcript(start + ms(10));
    timing.note_model_output();
    timing.note_voice(start + ms(997));
    let summary = timing.note_audio(start + ms(1_000), true).unwrap();
    assert_eq!(summary.wait, Some(ms(1_000)));
}

#[test]
fn wait_split_counts_only_tool_work_after_close_without_double_counting() {
    let start = Instant::now();
    let mut timing = TurnTiming::default();
    timing.note_voice(start);
    timing.note_transcript(start + ms(100));
    timing.note_tool_request(start + ms(1_400));
    timing.note_tool_execution(start + ms(1_400), start + ms(1_700));
    timing.note_activity_end(start + ms(1_500), None);
    timing.note_tool_response(start + ms(1_700), start + ms(1_800), true);
    timing.note_tool_request(start + ms(2_250));
    timing.note_tool_execution(start + ms(2_250), start + ms(2_450));
    let summary = timing.note_audio(start + ms(3_000), true).unwrap();
    assert_eq!(summary.wait, Some(ms(3_000)));
    assert_eq!(summary.transcript_wait, Some(ms(2_900)));
    assert_eq!(summary.turn_close, Some(ms(1_500)));
    assert_eq!(summary.tool_exec, ms(500));
    assert_eq!(summary.tool_response, ms(100));
    assert_eq!(summary.tool_wait, Some(ms(600)));
    assert_eq!(summary.tool_after_close, Some(ms(500)));
    assert_eq!(summary.other, Some(ms(1_000)));
    assert_eq!(
        summary.wait.unwrap(),
        summary.turn_close.unwrap() + summary.tool_after_close.unwrap() + summary.other.unwrap()
    );
}

#[test]
fn tool_intervals_are_clipped_to_the_wait_even_with_a_late_transcript() {
    let start = Instant::now();
    let mut timing = TurnTiming::default();
    timing.note_tool_request(start + ms(100));
    timing.note_tool_execution(start + ms(100), start + ms(900));
    timing.note_transcript(start + ms(500));
    timing.note_activity_end(start + ms(800), None);
    let summary = timing.note_audio(start + ms(1_000), true).unwrap();
    assert_eq!(summary.start_source, "last_transcript");
    assert_eq!(summary.wait, Some(ms(500)));
    assert_eq!(summary.pre_tool, None);
    assert_eq!(summary.tool_exec, ms(800));
    assert_eq!(summary.tool_wait, Some(ms(400)));
    assert_eq!(summary.tool_after_close, Some(ms(100)));
    assert_eq!(summary.other, Some(ms(100)));
}

#[test]
fn overlapping_tool_intervals_do_not_make_the_split_exceed_the_wait() {
    let start = Instant::now();
    let mut timing = TurnTiming::default();
    timing.note_transcript(start);
    timing.note_tool_request(start + ms(100));
    timing.note_tool_execution(start + ms(100), start + ms(500));
    timing.note_tool_execution(start + ms(400), start + ms(700));
    timing.note_activity_end(start + ms(250), None);
    timing.note_tool_response(start + ms(650), start + ms(800), true);
    let summary = timing.note_audio(start + ms(1_000), true).unwrap();
    assert_eq!(summary.tool_exec, ms(700));
    assert_eq!(summary.tool_response, ms(150));
    assert_eq!(summary.tool_wait, Some(ms(700)));
    assert_eq!(summary.tool_after_close, Some(ms(550)));
    assert_eq!(summary.other, Some(ms(200)));
}

#[test]
fn invalid_tool_interval_leaves_attribution_unknown() {
    let start = Instant::now();
    let mut timing = TurnTiming::default();
    timing.note_transcript(start);
    timing.note_activity_end(start + ms(100), None);
    timing.note_tool_execution(start + ms(300), start + ms(200));
    let summary = timing.note_audio(start + ms(1_000), true).unwrap();
    assert_eq!(summary.tool_wait, None);
    assert_eq!(summary.tool_after_close, None);
    assert_eq!(summary.other, None);
}

#[test]
fn microphone_levels_alone_cannot_assign_a_candidate_wait_to_a_greeting() {
    let start = Instant::now();
    let mut timing = TurnTiming::default();
    timing.note_voice(start);
    timing.note_model_output();
    timing.note_voice(start + ms(997));
    let summary = timing.note_audio(start + ms(1_000), true).unwrap();
    assert_eq!(summary.wait, None);
    assert_eq!(summary.start_source, "unknown");
}

#[test]
fn native_interruption_starts_a_fresh_wait_after_frozen_tool_work() {
    let start = Instant::now();
    let mut timing = TurnTiming::default();
    timing.note_voice(start);
    timing.note_transcript(start + ms(10));
    timing.note_activity_end(start + ms(100), Some(ms(40)));
    timing.note_tool_request(start + ms(200));
    timing.note_tool_execution(start + ms(200), start + ms(300));
    timing.note_activity_start(start + ms(350));
    timing.note_voice(start + ms(400));
    let cut = timing
        .finish(start + ms(500), TimingOutcome::Interrupted)
        .unwrap();
    assert_eq!(cut.elapsed, Some(ms(500)));
    assert_eq!(cut.turn_close, Some(ms(100)));
    assert_eq!(cut.tool_calls, 1);
    timing.note_activity_start(start + ms(600));
    timing.note_voice(start + ms(1_000));
    timing.note_activity_end(start + ms(1_200), Some(ms(99_000)));
    let next = timing.note_audio(start + ms(1_500), true).unwrap();
    assert_eq!(next.id, 2);
    assert_eq!(next.wait, Some(ms(500)));
    assert_eq!(next.turn_close, Some(ms(200)));
    assert_eq!(next.tool_calls, 0);
    assert_eq!(next.tool_after_close, Some(Duration::ZERO));
    assert_eq!(next.other, Some(ms(300)));
}

#[test]
fn close_reasons_distinguish_missing_and_reordered_signals() {
    let start = Instant::now();
    let mut timing = TurnTiming::default();
    timing.note_transcript(start);
    let missing = timing.note_audio(start + ms(100), true).unwrap();
    assert_eq!(missing.turn_close_reason, "not_observed");
    assert_eq!(missing.tool_wait, Some(Duration::ZERO));
    assert_eq!(missing.other, None);
    timing.finish(start + ms(200), TimingOutcome::NoAudio);
    timing.note_activity_end(start + ms(300), None);
    timing.note_transcript(start + ms(400));
    let reordered = timing.note_audio(start + ms(500), true).unwrap();
    assert_eq!(reordered.turn_close_reason, "before_start");
    assert_eq!(reordered.other, None);
    timing.finish(start + ms(600), TimingOutcome::NoAudio);
    timing.note_transcript(start + ms(700));
    timing.note_activity_end(start + ms(900), None);
    let future = timing.note_audio(start + ms(800), true).unwrap();
    assert_eq!(future.turn_close_reason, "after_audio");
    assert_eq!(future.other, None);
}

#[test]
fn delayed_activity_start_keeps_pcm_from_a_short_new_utterance() {
    let start = Instant::now();
    let mut timing = TurnTiming::default();
    timing.note_voice(start);
    timing.note_activity_end(start + ms(100), None);
    timing.note_voice(start + ms(500));
    timing.note_activity_start(start + ms(550));
    timing.note_activity_end(start + ms(800), None);
    let summary = timing.note_audio(start + ms(1_000), true).unwrap();
    assert_eq!(summary.wait, Some(ms(500)));
    assert_eq!(summary.turn_close, Some(ms(300)));
}

#[test]
fn activity_start_does_not_borrow_pcm_from_the_previous_closed_utterance() {
    let start = Instant::now();
    let mut timing = TurnTiming::default();
    timing.note_voice(start);
    timing.note_activity_end(start + ms(100), None);
    timing.note_activity_start(start + ms(200));
    timing.note_transcript(start + ms(500));
    timing.note_activity_end(start + ms(800), None);
    let summary = timing.note_audio(start + ms(1_000), true).unwrap();
    assert_eq!(summary.start_source, "last_transcript");
    assert_eq!(summary.wait, Some(ms(500)));
}

#[test]
fn early_activity_start_and_end_survive_the_old_turn_boundary() {
    let start = Instant::now();
    for old_audio in [false, true] {
        let mut timing = TurnTiming::default();
        timing.note_transcript(start);
        timing.note_activity_end(start + ms(100), None);
        timing.note_model_output();
        if old_audio {
            timing.note_audio(start + ms(200), true).unwrap();
        }
        timing.note_voice(start + ms(300));
        timing.note_activity_start(start + ms(310));
        timing.note_voice(start + ms(400));
        timing.note_activity_end(start + ms(500), Some(ms(99_000)));
        timing.note_activity_end(start + ms(600), None);
        timing.note_voice(start + ms(700));
        timing.finish(start + ms(800), TimingOutcome::NoAudio);
        let next = timing.note_audio(start + ms(1_000), true).unwrap();
        assert_eq!(next.id, 2);
        assert_eq!(next.wait, Some(ms(600)));
        assert_eq!(next.turn_close, Some(ms(100)));
        assert_eq!(next.audio_offset, Some(ms(99_000)));
        assert_eq!(next.tool_calls, 0);
    }
}

#[test]
fn output_transcripts_freeze_open_turns_but_cannot_reopen_closed_ones() {
    let start = Instant::now();
    let mut timing = TurnTiming::default();
    timing.note_output_transcript();
    let greeting = timing.finish(start, TimingOutcome::NoAudio).unwrap();
    assert_eq!(greeting.id, 1);
    assert_eq!(greeting.wait, None);
    timing.note_output_transcript();
    assert!(
        timing
            .finish(start + ms(100), TimingOutcome::Ended)
            .is_none()
    );
    timing.note_voice(start + ms(200));
    timing.note_transcript(start + ms(210));
    timing.note_output_transcript();
    timing.note_voice(start + ms(997));
    let next = timing.note_audio(start + ms(1_000), true).unwrap();
    assert_eq!(next.id, 2);
    assert_eq!(next.wait, Some(ms(800)));
}

#[test]
fn deferred_activity_borrows_only_pcm_after_the_old_speech_boundary() {
    let start = Instant::now();
    for (voice, source, wait) in [
        (50, "last_transcript", 200),
        (100, "last_transcript", 200),
        (150, "pcm_level", 350),
    ] {
        let mut timing = TurnTiming::default();
        timing.note_voice(start);
        timing.note_activity_end(start + ms(100), None);
        timing.note_model_output();
        timing.note_voice(start + ms(voice));
        timing.note_activity_start(start + ms(200));
        timing.finish(start + ms(250), TimingOutcome::Interrupted);
        timing.note_transcript(start + ms(300));
        let next = timing.note_audio(start + ms(500), true).unwrap();
        assert_eq!(next.start_source, source);
        assert_eq!(next.wait, Some(ms(wait)));
    }
}

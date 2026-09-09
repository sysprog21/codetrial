//! The `tests` module of `src/livekit/media.rs`, which declares this file by
//! path.
//! Everything here reaches into `src/livekit/media.rs` through `super`, so it
//! is a unit
//! test and not an integration test: private items are in scope.

use super::*;
use crate::config::load_from_pairs;
use crate::livekit::GEMINI_OUTPUT_AUDIO_SAMPLE_RATE;
use crate::livekit::tests::test_output_audio;
use ::livekit::webrtc::video_frame::{I420Buffer, VideoBuffer, VideoFrame, VideoRotation};

/// A sink that records instead of dialling Gemini, and can be told to fail.
#[derive(Default)]
struct RecordingSink {
    sent: Vec<Vec<u8>>,
    fail: bool,
}

impl AudioSink for RecordingSink {
    async fn send_pcm_16khz(
        &mut self,
        bytes: &[u8],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.fail {
            return Err("the socket is gone".into());
        }
        self.sent.push(bytes.to_vec());
        Ok(())
    }
}

fn frame_of(samples: usize) -> AudioFrame<'static> {
    AudioFrame {
        data: vec![0i16; samples].into(),
        sample_rate: 16_000,
        num_channels: 1,
        samples_per_channel: samples as u32,
    }
}

/// The agent publishes its voice as a microphone track.
///
/// Left to the default the track has no source, and a client looking for
/// who is speaking does not find it: the browser subscribes by source.
#[test]
fn the_agent_voice_is_published_as_a_microphone() {
    assert_eq!(agent_voice_track_options().source, TrackSource::Microphone);
}

/// Playout is over on the deadline, not after it.
///
/// The room loop arms a timer on this and asks again when it fires, so a
/// deadline that still counts as playing at the instant it is reached
/// re-arms the timer for zero and spins.
#[test]
fn playout_ends_on_its_deadline() {
    let (mut output_audio, _frames) = test_output_audio();
    output_audio.playout_deadline = Instant::now() + Duration::from_secs(1);
    let deadline = output_audio.playout_deadline;
    assert!(output_audio.playing_at(deadline - Duration::from_nanos(1)));
    assert!(
        !output_audio.playing_at(deadline),
        "the deadline is the end of it"
    );
    assert!(!output_audio.playing_at(deadline + Duration::from_millis(1)));
}

/// Speech is buffered until there is enough of it to be worth sending, and
/// the tail still goes when the track ends.
///
/// Sending every arriving frame would be a websocket message every ten
/// milliseconds, and dropping the tail would lose whatever was said last.
/// Both were unreachable from a test while this took the concrete session
/// type, which needs a live socket to exist at all.
#[tokio::test]
async fn audio_is_buffered_to_a_threshold_and_the_tail_still_goes() {
    let mut media = CandidateMedia::new();
    let mut sink = RecordingSink::default();

    pump_audio(&mut media, &mut sink, Some(frame_of(160)))
        .await
        .expect("buffering cannot fail");
    assert!(sink.sent.is_empty(), "a tenth of a second is not a message");
    assert_eq!(media.audio_bytes.len(), 320);

    // Bounded, because a flush empties the buffer: waiting on the buffer to
    // reach the threshold is waiting for something that is cleared the instant
    // it gets there.
    for _ in 0..GEMINI_AUDIO_BUFFER_BYTES {
        if !sink.sent.is_empty() {
            break;
        }
        pump_audio(&mut media, &mut sink, Some(frame_of(160)))
            .await
            .expect("buffering cannot fail");
    }
    assert_eq!(sink.sent.len(), 1, "one flush, not a message per frame");
    assert!(sink.sent[0].len() >= GEMINI_AUDIO_BUFFER_BYTES);
    assert!(
        media.audio_bytes.is_empty(),
        "a sent buffer is not sent twice"
    );

    pump_audio(&mut media, &mut sink, Some(frame_of(160)))
        .await
        .expect("buffering cannot fail");
    pump_audio(&mut media, &mut sink, None)
        .await
        .expect("the tail flush succeeds here");
    assert_eq!(sink.sent.len(), 2);
    assert_eq!(sink.sent[1].len(), 320, "the tail is what was left");

    pump_audio(&mut media, &mut sink, None)
        .await
        .expect("an empty tail is not an error");
    assert_eq!(sink.sent.len(), 2, "nothing held is nothing to send");
}

/// Playout is cut into ten-millisecond frames and the remainder waits.
///
/// A partial frame handed to the audio source is a click, and a dropped
/// one is a gap, so what is left over has to survive until the next
/// packet arrives rather than being padded or discarded.
#[test]
fn pcm_is_framed_at_ten_milliseconds_and_the_remainder_waits() {
    // Stereo on purpose: at one channel a frame size computed by dividing by
    // the channel count instead of multiplying comes out the same.
    const RATE: u32 = 24_000;
    const CHANNELS: u32 = 2;
    let frame_bytes = (RATE / 100 * CHANNELS) as usize * 2;
    assert_eq!(frame_bytes, 960);

    let mut pending = Vec::new();
    let frames = take_pcm16_frames(&vec![7u8; frame_bytes + 480], RATE, CHANNELS, &mut pending);
    assert_eq!(frames.len(), 1, "one whole frame, not the half behind it");
    assert_eq!(frames[0].len(), frame_bytes / 2, "samples, not bytes");
    assert_eq!(pending.len(), 480, "the half frame is held, not padded out");

    // The held half joins what comes next instead of being dropped.
    let frames = take_pcm16_frames(&vec![7u8; 480], RATE, CHANNELS, &mut pending);
    assert_eq!(frames.len(), 1, "the two halves make one frame");
    assert!(pending.is_empty());

    // Nothing whole yet is nothing to play, and it is all still held.
    let frames = take_pcm16_frames(&[7u8; 16], RATE, CHANNELS, &mut pending);
    assert!(frames.is_empty());
    assert_eq!(pending.len(), 16);
}

/// A dead socket is reported rather than swallowed. The room loop logs it
/// and keeps going, which is only right because this says it went wrong.
#[tokio::test]
async fn a_failed_flush_is_an_error_the_caller_can_see() {
    let mut media = CandidateMedia::new();
    let mut sink = RecordingSink {
        fail: true,
        ..RecordingSink::default()
    };
    media.audio_bytes.extend_from_slice(&[1, 2, 3, 4]);
    assert!(
        pump_audio(&mut media, &mut sink, None).await.is_err(),
        "a flush that never reached Gemini is not a success"
    );
}

/// An ended audio track is let go of even when the final flush fails.
///
/// The flush writes to Gemini, which is exactly what is broken when a
/// session dies mid-interview, and the room loop logs that failure and
/// keeps going. Releasing only on the success path left the ended stream
/// in place, and both `select!` arms are guarded on the stream being
/// present, so the arm re-fired forever. Read here as source because
/// failing a real flush needs a live websocket, while the ordering is the
/// whole of the bug.
#[test]
fn an_ended_audio_track_is_released_even_when_the_flush_fails() {
    let loop_source = include_str!("../../../src/livekit.rs");
    let arm = loop_source
        .split("frame = next_audio_frame")
        .nth(1)
        .expect("the room loop still has an audio arm");
    let pump = arm.find("pump_audio").expect("the arm still pumps audio");
    let release = arm
        .find("release_if_ended(&mut media.audio")
        .expect("the arm must let go of an ended audio stream itself");
    assert!(
        release > pump,
        "releasing inside pump_audio skips the error path, which is the one that repeats"
    );

    // Ordering alone would still hold if the release were made conditional on
    // the write succeeding, which is the original bug written a second way.
    // Nothing may stand between the pump and the release.
    let between = &arm[pump..release];
    for guard in ["is_ok", "if let Ok", "match ", "?;"] {
        assert!(
            !between.contains(guard),
            "the release must not be reachable only when the write succeeded: found {guard}"
        );
    }

    // And it has to sit at the arm's own level rather than inside either
    // branch, because the paused path has no release of its own any more: every
    // brace opened since the pause test is closed again before the release is
    // reached.
    let branch = arm
        .find("if turn.state.paused")
        .expect("the arm still decides on the pause first");
    let depth = arm[branch..release].chars().fold(0i32, |depth, c| match c {
        '{' => depth + 1,
        '}' => depth - 1,
        _ => depth,
    });
    assert_eq!(
        depth, 0,
        "the release is nested in a branch, so some path through the arm keeps an ended stream"
    );
    assert!(
        !include_str!("../../../src/livekit/media.rs")
            .split("pub(super) fn discard_paused_audio")
            .nth(1)
            .and_then(|rest| rest.split("\n}").next())
            .expect("discard_paused_audio is still defined here")
            .contains("release_if_ended"),
        "two answers to one question: the arm releases for every path, including this one"
    );
    let pump_audio_source = include_str!("../../../src/livekit/media.rs")
        .split("pub(super) async fn pump_audio")
        .nth(1)
        .and_then(|rest| rest.split("\npub(super)").next())
        .expect("pump_audio is still defined here");
    assert!(
        !pump_audio_source.contains("media.audio = None"),
        "releasing here too would put the rule back in the place that skips the error path"
    );
}

/// A pause drops frames but must not drop the fact that a track ended.
///
/// Both `select!` arms in the room loop are guarded on the stream still
/// being present. An ended stream yields `None` the instant it is polled,
/// so leaving it in place while paused re-arms the arm forever: the loop
/// spins for the whole pause instead of waiting. A camera taken by another
/// application is exactly how a track ends mid-interview.
#[test]
fn a_track_that_ends_while_paused_is_let_go_of() {
    // A stand-in for the stream, because a real one needs a real track. What is
    // under test is the rule, and the rule is what was missing.
    let mut stream = Some("camera");
    release_if_ended(&mut stream, false);
    assert_eq!(
        stream,
        Some("camera"),
        "a paused track is still wanted on resume"
    );
    release_if_ended(&mut stream, true);
    assert!(
        stream.is_none(),
        "an ended track re-arms the select arm forever"
    );

    // The buffer half: a paused interview resumes into silence rather than into
    // the tail of whatever was said as it was pausing.
    let mut media = CandidateMedia::new();
    media.audio_bytes.extend_from_slice(&[1, 2, 3, 4]);
    discard_paused_audio(&mut media);
    assert!(media.audio_bytes.is_empty());
}

#[test]
fn jpeg_frame_geometry_rejects_overflowing_and_empty_frames() {
    assert_eq!(
        jpeg_frame_geometry(640, 480),
        Some((640, 480, 640 * 480 * 4))
    );
    // 4K fits, which is past any camera a candidate is interviewing on.
    assert_eq!(
        jpeg_frame_geometry(3840, 2160),
        Some((3840, 2160, 3840 * 2160 * 4))
    );

    // A quarter of a gigabyte for one frame, off a header this process does not
    // control, and the buffer is allocated before anything is encoded.
    assert_eq!(jpeg_frame_geometry(8192, 8192), None);

    // Would wrap a u32 multiply and under-allocate the buffer `to_argb` writes
    // into.
    assert_eq!(jpeg_frame_geometry(u32::MAX, 4), None);
    assert_eq!(jpeg_frame_geometry(0, 480), None);

    // Clears the byte budget and still has no representable JPEG side.
    assert_eq!(jpeg_frame_geometry(100_000, 1), None);

    // The budget itself, in both directions.
    assert_eq!(
        jpeg_frame_geometry(4096, 4096),
        Some((4096, 4096, 64 << 20))
    );
    assert_eq!(jpeg_frame_geometry(4097, 4096), None);

    // The exact boundary, in both directions, because 65535 is the largest side
    // a JPEG SOF can carry and off-by-one here is a silent truncation.
    assert_eq!(
        jpeg_frame_geometry(65_535, 1),
        Some((65_535, 1, 65_535 * 4))
    );
    assert_eq!(jpeg_frame_geometry(65_536, 1), None);
    assert_eq!(jpeg_frame_geometry(1, 65_536), None);
}

#[test]
fn candidate_video_frames_are_opt_in_for_gemini() {
    let default_config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "google-key"),
    ])
    .unwrap();
    let enabled_config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "google-key"),
        ("CODETRIAL_GEMINI_CANDIDATE_VIDEO_ENABLED", "true"),
    ])
    .unwrap();

    assert!(
        !candidate_video_frames_go_to_gemini(
            Some("candidate-fixed"),
            "candidate-fixed",
            default_config.gemini_candidate_video_enabled,
        ),
        "default integrity mode must not forward candidate video frames server-side"
    );
    assert!(candidate_video_frames_go_to_gemini(
        Some("candidate-fixed"),
        "candidate-fixed",
        enabled_config.gemini_candidate_video_enabled,
    ));
    assert!(!candidate_video_frames_go_to_gemini(
        Some("candidate-other"),
        "candidate-fixed",
        enabled_config.gemini_candidate_video_enabled,
    ));
}

#[test]
fn audio_sample_rate_reads_pcm_mime_rate() {
    assert_eq!(
        audio_sample_rate("audio/pcm;rate=24000", 16_000),
        Some(24_000)
    );
    assert_eq!(audio_sample_rate("audio/pcm", 24_000), Some(24_000));
    assert_eq!(audio_sample_rate("audio/webm", 24_000), None);
}

#[test]
fn video_frame_throttle_matches_gemini_live_limit() {
    assert!(!should_send_video_frame(Duration::from_millis(999)));
    assert!(should_send_video_frame(Duration::from_secs(1)));
}

// Moved here from `livekit.rs`, where they tested these functions from the room
// loop's test module. `pcm16_bytes`, `OutputAudio`, `take_pcm16_frames` and the
// JPEG encoder all live in this file, so the tests that pin them were reaching
// across a module boundary to do it, and a reader of this file saw untested
// code that was in fact covered next door.

#[test]
fn pcm16_bytes_serializes_little_endian_samples() {
    let frame = AudioFrame {
        data: [1_i16, -2_i16, 0x1234_i16].as_slice().into(),
        sample_rate: GEMINI_AUDIO_SAMPLE_RATE as u32,
        num_channels: GEMINI_AUDIO_CHANNELS as u32,
        samples_per_channel: 3,
    };

    let mut bytes = Vec::new();
    append_pcm16_bytes(&frame, &mut bytes);

    assert_eq!(bytes, vec![1, 0, 254, 255, 0x34, 0x12]);
}

#[test]
fn output_audio_interrupt_clears_partial_pcm_frame_and_cancels_old_queue() {
    let (mut output_audio, _) = test_output_audio();
    output_audio.pending_bytes = vec![1, 2, 3];
    let old_cancellation = output_audio.output_cancellation.clone();
    output_audio.playout_deadline = Instant::now() + Duration::from_secs(10);

    output_audio.interrupt();

    assert!(output_audio.pending_bytes.is_empty());
    assert!(old_cancellation.is_cancelled());
    assert!(!output_audio.output_cancellation.is_cancelled());
    assert!(!output_audio.is_playing());
}

#[tokio::test]
async fn output_audio_capture_queues_frames_with_current_cancellation_token() {
    let (mut output_audio, mut queued_frames) = test_output_audio();
    let start_deadline = output_audio.playout_deadline;
    let bytes = vec![0_u8; 240 * 2];

    assert!(
        output_audio
            .capture(
                &bytes,
                &format!("audio/pcm;rate={GEMINI_OUTPUT_AUDIO_SAMPLE_RATE}"),
            )
            .await
            .unwrap()
    );

    let frame = queued_frames.try_recv().unwrap();
    assert!(!frame.output_cancellation.is_cancelled());
    assert_eq!(frame.samples.len(), 240);

    // By how much, not merely that it moved. The deadline is what paces
    // playout, and ten milliseconds of audio buys ten milliseconds of it:
    // arithmetic that divides where it should multiply still moves this
    // forward, only by minutes or by nothing.
    let advance = output_audio.playout_deadline - start_deadline;
    assert!(
        advance >= Duration::from_millis(10),
        "240 samples at 24 kHz is ten milliseconds of playout, got {advance:?}"
    );
    assert!(
        advance < Duration::from_millis(500),
        "ten milliseconds of audio must not reserve more, got {advance:?}"
    );
    output_audio.interrupt();
    assert!(frame.output_cancellation.is_cancelled());
    assert!(queued_frames.try_recv().is_err());
}

#[test]
fn take_pcm16_frames_decodes_full_ten_ms_frames_and_keeps_remainder() {
    let frame_bytes = 240 * 2;
    let mut bytes = vec![0_u8; frame_bytes + 2];
    bytes[0] = 1;
    bytes[1] = 0;
    bytes[frame_bytes] = 9;
    let mut pending = Vec::new();

    let frames = take_pcm16_frames(&bytes, 24_000, 1, &mut pending);

    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].len(), 240);
    assert_eq!(frames[0][0], 1);
    assert_eq!(pending, vec![9, 0]);
}

#[test]
fn encode_video_frame_jpeg_outputs_jpeg_bytes() {
    let buffer: Box<dyn VideoBuffer> = Box::new(I420Buffer::new_black(16, 16));
    let frame = VideoFrame {
        rotation: VideoRotation::VideoRotation0,
        timestamp_us: 0,
        frame_metadata: None,
        buffer,
    };

    let (rgba, width, height) = frame_to_rgba(&frame).unwrap();
    let bytes = encode_rgba_jpeg(&rgba, width, height, GEMINI_VIDEO_JPEG_QUALITY).unwrap();

    assert!(bytes.starts_with(&[0xff, 0xd8]));
    assert!(bytes.ends_with(&[0xff, 0xd9]));
}

/// A black frame cannot catch a permuted channel, because every channel
/// holds the same value. This one is solid red in BT.601 limited range, so
/// the four libyuv layouts land on four different byte patterns.
#[test]
fn channel_order_survives_the_libyuv_naming_trap() {
    let mut buffer = I420Buffer::new(16, 16);
    let (luma, blue, red) = buffer.data_mut();
    luma.fill(81);
    blue.fill(90);
    red.fill(240);
    let frame = VideoFrame {
        rotation: VideoRotation::VideoRotation0,
        timestamp_us: 0,
        frame_metadata: None,
        buffer: Box::new(buffer) as Box<dyn VideoBuffer>,
    };

    let (rgba, width, height) = frame_to_rgba(&frame).unwrap();

    assert_eq!((width, height), (16, 16));

    // Checked on the first pixel and on the last. The first is right whatever
    // the row stride is, because row zero starts at offset zero, so a stride
    // computed any other way still paints it correctly and leaves the bottom of
    // the image as the zeroes it was allocated with.
    for (where_, pixel) in [("first", &rgba[..4]), ("last", &rgba[rgba.len() - 4..])] {
        assert!(
            pixel[0] > 200,
            "red belongs in byte 0 of the {where_}, got {pixel:?}"
        );
        assert!(
            pixel[1] < 60,
            "green belongs in byte 1 of the {where_}, got {pixel:?}"
        );
        assert!(
            pixel[2] < 60,
            "blue belongs in byte 2 of the {where_}, got {pixel:?}"
        );
        assert_eq!(
            pixel[3], 255,
            "alpha belongs in byte 3 of the {where_}, got {pixel:?}"
        );
    }
}

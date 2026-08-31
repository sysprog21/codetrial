//! Audio and video, in both directions.
//!
//! Split from the room loop because none of it is about the interview: these
//! are the buffers, codecs and pumps that carry the candidate's microphone and
//! camera to Gemini and Gemini's voice back to the room. The room decides when
//! a turn starts; this decides what a frame becomes on the way.

use std::time::{Duration, Instant};

use ::livekit::options::TrackPublishOptions;
use ::livekit::prelude::{
    LocalAudioTrack, LocalTrack, RemoteAudioTrack, RemoteParticipant, RemoteTrack, Room, RoomEvent,
    RtcAudioSource, TrackSource,
};
use ::livekit::webrtc::audio_frame::AudioFrame;
use ::livekit::webrtc::audio_source::AudioSourceOptions;
use ::livekit::webrtc::audio_source::native::NativeAudioSource;
use ::livekit::webrtc::audio_stream::native::NativeAudioStream;
use ::livekit::webrtc::video_frame::{BoxVideoFrame, VideoFormatType};
use ::livekit::webrtc::video_stream::native::NativeVideoStream;
use futures_util::StreamExt;
use jpeg_encoder::{ColorType, Encoder};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::gemini::GeminiLiveSession;

use super::is_interview_participant;

pub(super) const GEMINI_AUDIO_SAMPLE_RATE: i32 = 16_000;

pub(super) const GEMINI_AUDIO_CHANNELS: i32 = 1;

pub(super) const GEMINI_AUDIO_BUFFER_BYTES: usize = 3_200;

pub(super) const GEMINI_VIDEO_MIME_TYPE: &str = "image/jpeg";

pub(super) const GEMINI_VIDEO_FRAME_INTERVAL: Duration = Duration::from_secs(1);

pub(super) const GEMINI_VIDEO_JPEG_QUALITY: u8 = 75;

pub(super) const LIVEKIT_OUTPUT_CHANNELS: u32 = 1;

pub(super) const LIVEKIT_OUTPUT_QUEUE_MS: u32 = 100;

pub(super) const LIVEKIT_OUTPUT_FRAME_QUEUE: usize = 6_000;

/// Buffers one arriving audio frame, and flushes when there is enough to send.
///
/// `None` is the track ending: what is buffered goes now, because nothing else
/// is going to arrive to push it over the threshold.
pub(super) async fn pump_audio(
    media: &mut CandidateMedia,
    gemini: &mut GeminiLiveSession,
    frame: Option<AudioFrame<'static>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(frame) = frame else {
        flush_audio(gemini, &mut media.audio_bytes).await?;
        media.audio = None;
        return Ok(());
    };

    // Not `last_user_speech`. LiveKit delivers frames for as long as the track
    // is live, silence included, so stamping here made the interview
    // permanently "just spoke": `idle_seconds` never reached
    // SILENCE_THRESHOLD_S and the nudge never fired. Real speech is stamped on
    // `InputTranscript`, where Gemini has already decided that words were said.
    append_pcm16_bytes(&frame, &mut media.audio_bytes);
    if media.audio_bytes.len() >= GEMINI_AUDIO_BUFFER_BYTES {
        flush_audio(gemini, &mut media.audio_bytes).await?;
    }
    Ok(())
}

/// Sends one arriving video frame, at most one per interval.
pub(super) async fn pump_video(
    media: &mut CandidateMedia,
    gemini: &mut GeminiLiveSession,
    frame: Option<BoxVideoFrame>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(frame) = frame else {
        media.video = None;
        return Ok(());
    };
    if !should_send_video_frame(media.last_video_frame.elapsed()) {
        return Ok(());
    }

    // Stamped on the attempt, not the success: the dimensions come off the
    // wire, and a source stuck emitting frames that will not encode must cost
    // one try per interval rather than one per arriving frame.
    media.last_video_frame = Instant::now();
    match encode_video_frame_jpeg_off_thread(&frame, GEMINI_VIDEO_JPEG_QUALITY).await {
        Ok(bytes) => {
            gemini
                .send_video_frame(&bytes, GEMINI_VIDEO_MIME_TYPE)
                .await?
        }
        Err(error) => eprintln!("skipping unencodable video frame: {error}"),
    }
    Ok(())
}

/// The candidate's inbound media, plus the identity their tracks arrived on.
/// Bundled because every track event touches two or three of these together.
pub(super) struct CandidateMedia {
    pub(super) audio: Option<NativeAudioStream>,
    pub(super) video: Option<NativeVideoStream>,
    pub(super) audio_bytes: Vec<u8>,
    pub(super) identity: Option<String>,
    last_video_frame: Instant,
}

impl CandidateMedia {
    pub(super) fn new() -> Self {
        Self {
            audio: None,
            video: None,
            audio_bytes: Vec::new(),
            identity: None,

            // Seeded in the past so the first frame sends immediately. The
            // monotonic clock starts at boot, so on a machine that just came up
            // there may be nothing to subtract from.
            last_video_frame: Instant::now()
                .checked_sub(GEMINI_VIDEO_FRAME_INTERVAL)
                .unwrap_or_else(Instant::now),
        }
    }

    fn attach_audio(&mut self, track: &RemoteAudioTrack, participant: &RemoteParticipant) {
        // Drop any half-buffered frame from a previous publication so the new
        // stream does not start mid-sample.
        self.audio_bytes.clear();
        self.identity = Some(participant.identity().to_string());
        self.audio = Some(NativeAudioStream::new(
            track.rtc_track(),
            GEMINI_AUDIO_SAMPLE_RATE,
            GEMINI_AUDIO_CHANNELS,
        ));
    }
}

/// The candidate's tracks arriving and going away. Reports whether the event
/// was media, so the caller's match only has to carry what is left.
pub(super) async fn handle_media_event(
    media: &mut CandidateMedia,
    gemini: &mut GeminiLiveSession,
    candidate_identity: &str,
    forward_video_to_gemini: bool,
    event: &RoomEvent,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    match event {
        RoomEvent::TrackSubscribed {
            track: RemoteTrack::Audio(track),
            participant,
            ..
        } if is_interview_participant(Some(&participant.identity().0), candidate_identity) => {
            media.attach_audio(track, participant);
        }
        RoomEvent::TrackSubscribed {
            track: RemoteTrack::Video(track),
            participant,
            ..
        } if candidate_video_frames_go_to_gemini(
            Some(&participant.identity().0),
            candidate_identity,
            forward_video_to_gemini,
        ) =>
        {
            media.video = Some(NativeVideoStream::new(track.rtc_track()));
        }

        // Guarded like the subscribe arms: an unrelated participant leaving
        // must not tear down the stream the candidate is still speaking into.
        RoomEvent::TrackUnsubscribed {
            track: RemoteTrack::Audio(_),
            participant,
            ..
        } if is_interview_participant(Some(&participant.identity().0), candidate_identity) => {
            flush_audio(gemini, &mut media.audio_bytes).await?;
            media.audio = None;
        }
        RoomEvent::TrackUnsubscribed {
            track: RemoteTrack::Video(_),
            participant,
            ..
        } if is_interview_participant(Some(&participant.identity().0), candidate_identity) => {
            media.video = None;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn candidate_video_frames_go_to_gemini(
    sender: Option<&str>,
    candidate_identity: &str,
    opt_in: bool,
) -> bool {
    opt_in && is_interview_participant(sender, candidate_identity)
}

async fn flush_audio(
    gemini: &mut GeminiLiveSession,
    bytes: &mut Vec<u8>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !bytes.is_empty() {
        gemini.send_audio_pcm_16khz(bytes).await?;
        bytes.clear();
    }
    Ok(())
}

pub(super) async fn next_audio_frame(
    stream: &mut Option<NativeAudioStream>,
) -> Option<AudioFrame<'static>> {
    stream.as_mut()?.next().await
}

pub(super) async fn next_video_frame(
    stream: &mut Option<NativeVideoStream>,
) -> Option<BoxVideoFrame> {
    stream.as_mut()?.next().await
}

pub(super) struct OutputAudio {
    pub(super) source: NativeAudioSource,
    pub(super) sample_rate: u32,
    pub(super) pending_bytes: Vec<u8>,
    pub(super) playout_deadline: Instant,
    pub(super) frames: mpsc::Sender<QueuedOutputFrame>,
    pub(super) output_cancellation: CancellationToken,
}

pub(super) struct QueuedOutputFrame {
    pub(super) output_cancellation: CancellationToken,
    pub(super) samples: Vec<i16>,
}

impl OutputAudio {
    pub(super) fn interrupt(&mut self) {
        self.output_cancellation.cancel();
        self.output_cancellation = CancellationToken::new();
        self.source.clear_buffer();
        self.pending_bytes.clear();
        self.playout_deadline = Instant::now();
    }

    pub(super) fn is_playing(&self) -> bool {
        Instant::now() < self.playout_deadline
    }

    /// Whether `capture` would queue anything for this chunk.
    ///
    /// Split out so the caller can ask before dropping a turn that is still
    /// playing. Dropping first and then finding the new chunk unusable cuts the
    /// interviewer off mid-sentence with nothing behind it.
    pub(super) fn accepts(&self, bytes: &[u8], mime_type: &str) -> bool {
        !bytes.is_empty()
            && audio_sample_rate(mime_type, self.sample_rate) == Some(self.sample_rate)
    }

    pub(super) async fn capture(
        &mut self,
        bytes: &[u8],
        mime_type: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        if !self.accepts(bytes, mime_type) {
            return Ok(false);
        }
        let mut queued = false;
        for samples in take_pcm16_frames(
            bytes,
            self.sample_rate,
            LIVEKIT_OUTPUT_CHANNELS,
            &mut self.pending_bytes,
        ) {
            queued = true;
            let samples_per_channel = samples.len() as u32 / LIVEKIT_OUTPUT_CHANNELS;
            let frame_duration =
                Duration::from_secs_f64(samples_per_channel as f64 / self.sample_rate as f64);
            self.playout_deadline = self.playout_deadline.max(Instant::now()) + frame_duration;
            self.frames
                .try_send(QueuedOutputFrame {
                    output_cancellation: self.output_cancellation.clone(),
                    samples,
                })
                .map_err(|error| {
                    std::io::Error::other(match error {
                        mpsc::error::TrySendError::Full(_) => "output audio worker queue full",
                        mpsc::error::TrySendError::Closed(_) => "output audio worker stopped",
                    })
                })?;
        }
        Ok(queued)
    }
}

async fn output_audio_worker(
    source: NativeAudioSource,
    sample_rate: u32,
    mut frames: mpsc::Receiver<QueuedOutputFrame>,
) {
    while let Some(frame) = frames.recv().await {
        if frame.output_cancellation.is_cancelled() {
            continue;
        }
        let cancelled = frame.output_cancellation.cancelled();
        tokio::pin!(cancelled);
        let samples_per_channel = frame.samples.len() as u32 / LIVEKIT_OUTPUT_CHANNELS;
        let audio_frame = AudioFrame {
            data: frame.samples.into(),
            sample_rate,
            num_channels: LIVEKIT_OUTPUT_CHANNELS,
            samples_per_channel,
        };
        tokio::select! {
            result = source.capture_frame(&audio_frame) => {
                if let Err(error) = result {
                    eprintln!("failed to publish output audio frame: {error}");
                }
            }
            _ = &mut cancelled => {
                source.clear_buffer();
            }
        }
    }
}

pub(super) async fn publish_output_audio(
    room: &Room,
    sample_rate: u32,
) -> Result<OutputAudio, Box<dyn std::error::Error + Send + Sync>> {
    let source = NativeAudioSource::new(
        AudioSourceOptions::default(),
        sample_rate,
        LIVEKIT_OUTPUT_CHANNELS,
        LIVEKIT_OUTPUT_QUEUE_MS,
    );
    let track = LocalAudioTrack::create_audio_track(
        "interviewer-audio",
        RtcAudioSource::Native(source.clone()),
    );
    room.local_participant()
        .publish_track(
            LocalTrack::Audio(track),
            TrackPublishOptions {
                source: TrackSource::Microphone,
                ..Default::default()
            },
        )
        .await?;
    let (frames, queued_frames) = mpsc::channel(LIVEKIT_OUTPUT_FRAME_QUEUE);
    tokio::spawn(output_audio_worker(
        source.clone(),
        sample_rate,
        queued_frames,
    ));
    Ok(OutputAudio {
        source,
        sample_rate,
        pending_bytes: Vec::new(),
        playout_deadline: Instant::now(),
        frames,
        output_cancellation: CancellationToken::new(),
    })
}

pub(super) fn append_pcm16_bytes(frame: &AudioFrame<'_>, bytes: &mut Vec<u8>) {
    frame
        .data
        .iter()
        .for_each(|sample| bytes.extend(sample.to_le_bytes()));
}

fn audio_sample_rate(mime_type: &str, default_pcm_rate: u32) -> Option<u32> {
    mime_type
        .split(';')
        .find_map(|part| {
            part.trim()
                .strip_prefix("rate=")
                .and_then(|rate| rate.parse().ok())
        })
        .or_else(|| (mime_type.trim() == "audio/pcm").then_some(default_pcm_rate))
}

pub(super) fn take_pcm16_frames(
    bytes: &[u8],
    sample_rate: u32,
    channels: u32,
    pending: &mut Vec<u8>,
) -> Vec<Vec<i16>> {
    let frame_bytes = (sample_rate / 100 * channels) as usize * 2;
    if frame_bytes == 0 {
        return Vec::new();
    }

    pending.extend_from_slice(bytes);
    let complete_len = pending.len() - pending.len() % frame_bytes;
    let frames = pending[..complete_len]
        .chunks_exact(frame_bytes)
        .map(decode_pcm16)
        .collect();
    pending.drain(..complete_len);
    frames
}

fn decode_pcm16(bytes: &[u8]) -> Vec<i16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect()
}

fn should_send_video_frame(elapsed: Duration) -> bool {
    elapsed >= GEMINI_VIDEO_FRAME_INTERVAL
}

/// JPEG sides and RGBA byte count for a frame, or `None` when the dimensions
/// are absurd. Both bounds live here because they answer one question and a
/// frame that clears the pixel cap can still overflow a 16-bit JPEG side: the
/// pixel cap is well past 8K and only exists so a bogus header cannot ask for a
/// gigabyte, while a 100000x1 frame sits under it and still has no valid SOF.
fn jpeg_frame_geometry(width: u32, height: u32) -> Option<(u16, u16, usize)> {
    const MAX_PIXELS: u32 = 8192 * 8192;
    let pixels = width.checked_mul(height)?;
    (1..=MAX_PIXELS).contains(&pixels).then_some(())?;
    Some((
        u16::try_from(width).ok()?,
        u16::try_from(height).ok()?,
        pixels as usize * 4,
    ))
}

/// Converts a frame to packed R, G, B, A bytes.
///
/// The format name is a trap. libyuv names its 32-bit formats after the
/// little-endian word rather than the byte order, so `ABGR` is the variant that
/// writes R, G, B, A into memory and `RGBA` writes A, B, G, R. Asking for
/// `RGBA` pins the red channel to the alpha constant and transposes green and
/// blue, which `channel_order_survives_the_libyuv_naming_trap` pins down.
pub(super) fn frame_to_rgba(frame: &BoxVideoFrame) -> Result<(Vec<u8>, u16, u16), String> {
    let width = frame.buffer.as_ref().width();
    let height = frame.buffer.as_ref().height();

    // Remote-controlled dimensions: overflow here would panic in debug and
    // under-allocate the buffer `to_argb` writes into in release.
    let Some((jpeg_width, jpeg_height, rgba_len)) = jpeg_frame_geometry(width, height) else {
        return Err(format!(
            "frame {width}x{height} is outside JPEG limits (each side at most 65535, at most 8192x8192 pixels)"
        ));
    };
    let mut rgba = vec![0; rgba_len];
    frame.buffer.as_ref().to_argb(
        VideoFormatType::ABGR,
        &mut rgba,
        width * 4,
        width as i32,
        height as i32,
    );
    Ok((rgba, jpeg_width, jpeg_height))
}

/// The compression on its own, so the caller can put it somewhere other than
/// the executor. Owned pixels rather than the frame, because the frame does not
/// cross a thread and this does.
pub(super) fn encode_rgba_jpeg(
    rgba: &[u8],
    jpeg_width: u16,
    jpeg_height: u16,
    quality: u8,
) -> Result<Vec<u8>, String> {
    // Chroma is subsampled 2x2 here, because that is what `Encoder::new` picks
    // below quality 90 and nothing below overrides it. Deliberate: the source
    // is I420, whose chroma is already 4:2:0, so encoding it at full resolution
    // would spend about a fifth of the payload on interpolated values. The
    // override, if a future frame source is not I420, is
    // `set_sampling_factor(SamplingFactor::F_1_1)` before the call.
    let mut jpeg = Vec::new();
    Encoder::new(&mut jpeg, quality)
        .encode(rgba, jpeg_width, jpeg_height, ColorType::Rgba)
        .map_err(|error| error.to_string())?;
    Ok(jpeg)
}

/// A frame to JPEG bytes, with the compression moved off the executor.
///
/// The colour conversion stays inline: it is libyuv, which is compiled
/// optimized whatever profile this crate is built at, and the frame it borrows
/// does not cross a thread. The JPEG pass is the one that costs, and it is pure
/// Rust: measured at 6.8ms per 720p frame in release and 138ms in a dev build.
/// It shares an executor with Gemini's audio events, so at a frame a second
/// that stall lands in the middle of a conversation and the candidate hears it
/// as the reply arriving late.
pub(super) async fn encode_video_frame_jpeg_off_thread(
    frame: &BoxVideoFrame,
    quality: u8,
) -> Result<Vec<u8>, String> {
    let (rgba, jpeg_width, jpeg_height) = frame_to_rgba(frame)?;
    tokio::task::spawn_blocking(move || encode_rgba_jpeg(&rgba, jpeg_width, jpeg_height, quality))
        .await
        .map_err(|error| format!("jpeg encode task did not finish: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::load_from_pairs;

    #[test]
    fn jpeg_frame_geometry_rejects_overflowing_and_empty_frames() {
        assert_eq!(
            jpeg_frame_geometry(640, 480),
            Some((640, 480, 640 * 480 * 4))
        );
        assert_eq!(
            jpeg_frame_geometry(8192, 8192),
            Some((8192, 8192, 8192 * 8192 * 4))
        );

        // Would wrap a u32 multiply and under-allocate the buffer `to_argb`
        // writes into.
        assert_eq!(jpeg_frame_geometry(u32::MAX, 4), None);
        assert_eq!(jpeg_frame_geometry(8193, 8192), None);
        assert_eq!(jpeg_frame_geometry(0, 480), None);

        // Clears the pixel cap and still has no representable JPEG side.
        assert_eq!(jpeg_frame_geometry(100_000, 1), None);

        // The exact boundary, in both directions, because 65535 is the largest
        // side a JPEG SOF can carry and off-by-one here is a silent truncation.
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
}

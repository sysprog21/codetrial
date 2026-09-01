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
pub(super) async fn pump_audio<S: AudioSink>(
    media: &mut CandidateMedia,
    gemini: &mut S,
    frame: Option<AudioFrame<'static>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(frame) = frame else {
        // Only the flush. Letting go of the stream is the caller's, because it
        // has to happen whether or not this fails: the caller logs a write
        // error and keeps looping, and a stream still in place is polled again
        // at once, ends again and fails again, which is the busy loop
        // `discard_paused_audio` already describes for the paused path.
        flush_audio(gemini, &mut media.audio_bytes).await?;
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

/// What a media frame is worth while the interview is paused.
///
/// The frame itself is dropped: a paused interview is not collecting evidence,
/// and a tenth of a second of speech the resumed session never needed is not
/// worth propagating a write error over.
///
/// Letting go of an ended track is the caller's, once, for every path through
/// the arm. Doing it here as well would be a second answer to a question that
/// already has one, and the bug worth guarding against is a path that answers
/// it nowhere: both `select!` arms are guarded on the stream still being
/// present, and an ended stream yields `None` the moment it is polled, so a
/// stream left in place re-arms the arm and fires again immediately, for as
/// long as the room lasts. That is a busy loop, not a lost frame.
pub(super) fn discard_paused_audio(media: &mut CandidateMedia) {
    media.audio_bytes.clear();
}

/// Generic so it can be tested, and called directly for video, which has no
/// buffer to clear and so needs nothing wrapped around it. A real stream needs
/// a real track, so a rule
/// written directly against `NativeAudioStream` is one no test can reach, and
/// this rule is the whole of the bug.
pub(super) fn release_if_ended<T>(stream: &mut Option<T>, ended: bool) {
    if ended {
        *stream = None;
    }
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

/// Where buffered candidate audio goes.
///
/// Named as a trait with one method so the buffering above can be tested. The
/// real implementation is a websocket to Gemini, so a test that took the
/// concrete type could not call `pump_audio` at all, and the flush threshold,
/// the tail flush on a track ending, and the error on a dead socket all went
/// unchecked because the argument was unconstructible rather than because the
/// behaviour was uninteresting.
pub(super) trait AudioSink {
    fn send_pcm_16khz(
        &mut self,
        bytes: &[u8],
    ) -> impl Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>>;
}

impl AudioSink for GeminiLiveSession {
    fn send_pcm_16khz(
        &mut self,
        bytes: &[u8],
    ) -> impl Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> {
        self.send_audio_pcm_16khz(bytes)
    }
}

async fn flush_audio<S: AudioSink>(
    gemini: &mut S,
    bytes: &mut Vec<u8>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !bytes.is_empty() {
        gemini.send_pcm_16khz(bytes).await?;
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
        self.playing_at(Instant::now())
    }

    /// `now` is a parameter rather than an `Instant::now()` inside, so the
    /// moment playout ends can be asked about: a clock read in here can be
    /// compared with `<` or `<=` and no test could ever tell, because it
    /// cannot land on the deadline exactly.
    fn playing_at(&self, now: Instant) -> bool {
        now < self.playout_deadline
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

            // Ten milliseconds, which is what `take_pcm16_frames` cut this
            // into. Dividing the sample count by the channel count says the
            // same thing, but the channel count is one, so it says it with an
            // operation that reads identically whichever way round it is
            // written and cannot be wrong in a way anything could observe.
            let samples_per_channel = self.sample_rate / 100;
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
        // Ten milliseconds, as cut by `take_pcm16_frames`. See `capture`.
        let samples_per_channel = sample_rate / 100;
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

/// How the agent's own voice is published.
///
/// Named as the microphone rather than left to default, because a track with
/// no source is not the one a client picks when it looks for who is speaking:
/// the browser subscribes by source, and Meet presentation mode routes on it.
fn agent_voice_track_options() -> TrackPublishOptions {
    TrackPublishOptions {
        source: TrackSource::Microphone,
        ..Default::default()
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
        .publish_track(LocalTrack::Audio(track), agent_voice_track_options())
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
/// frame that clears the size cap can still overflow a 16-bit JPEG side, while
/// a 100000x1 frame sits under the cap and still has no valid SOF.
///
/// Bounded in bytes rather than in pixels, because bytes are what is actually
/// at stake: this is the buffer the frame is unpacked into before it is
/// encoded. A pixel cap set past 8K allowed a quarter of a gigabyte for one
/// frame off a header this process does not control. The budget below fits 4K
/// with room to spare, which is past any camera a candidate is interviewing on.
fn jpeg_frame_geometry(width: u32, height: u32) -> Option<(u16, u16, usize)> {
    const MAX_RGBA_BYTES: usize = 64 * 1024 * 1024;
    let pixels = width.checked_mul(height)?;
    if pixels == 0 {
        return None;
    }
    let bytes = (pixels as usize).checked_mul(4)?;
    (bytes <= MAX_RGBA_BYTES).then_some(())?;
    Some((
        u16::try_from(width).ok()?,
        u16::try_from(height).ok()?,
        bytes,
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
        let (frames, _queued) = tokio::sync::mpsc::channel(1);
        let output_audio = OutputAudio {
            source: NativeAudioSource::new(
                AudioSourceOptions::default(),
                24_000,
                LIVEKIT_OUTPUT_CHANNELS,
                LIVEKIT_OUTPUT_QUEUE_MS,
            ),
            sample_rate: 24_000,
            pending_bytes: Vec::new(),
            playout_deadline: Instant::now() + Duration::from_secs(1),
            frames,
            output_cancellation: CancellationToken::new(),
        };
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
        // reach the threshold is waiting for something that is cleared the
        // instant it gets there.
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
        // Stereo on purpose: at one channel a frame size computed by dividing
        // by the channel count instead of multiplying comes out the same.
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
        let loop_source = include_str!("../livekit.rs");
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

        // Ordering alone would still hold if the release were made conditional
        // on the write succeeding, which is the original bug written a second
        // way. Nothing may stand between the pump and the release.
        let between = &arm[pump..release];
        for guard in ["is_ok", "if let Ok", "match ", "?;"] {
            assert!(
                !between.contains(guard),
                "the release must not be reachable only when the write succeeded: found {guard}"
            );
        }

        // And it has to sit at the arm's own level rather than inside either
        // branch, because the paused path has no release of its own any more:
        // every brace opened since the pause test is closed again before the
        // release is reached.
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
            !include_str!("media.rs")
                .split("pub(super) fn discard_paused_audio")
                .nth(1)
                .and_then(|rest| rest.split("\n}").next())
                .expect("discard_paused_audio is still defined here")
                .contains("release_if_ended"),
            "two answers to one question: the arm releases for every path, including this one"
        );
        let pump_audio_source = include_str!("media.rs")
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
        // A stand-in for the stream, because a real one needs a real track.
        // What is under test is the rule, and the rule is what was missing.
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

        // The buffer half: a paused interview resumes into silence rather than
        // into the tail of whatever was said as it was pausing.
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

        // A quarter of a gigabyte for one frame, off a header this process does
        // not control, and the buffer is allocated before anything is encoded.
        assert_eq!(jpeg_frame_geometry(8192, 8192), None);

        // Would wrap a u32 multiply and under-allocate the buffer `to_argb`
        // writes into.
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

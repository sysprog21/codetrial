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
use ::livekit::webrtc::audio_stream::native::{NativeAudioStream, NativeAudioStreamOptions};
use ::livekit::webrtc::audio_track::RtcAudioTrack;
use ::livekit::webrtc::video_frame::{BoxVideoFrame, VideoFormatType};
use ::livekit::webrtc::video_stream::native::NativeVideoStream;
use futures_util::StreamExt;
use jpeg_encoder::{ColorType, Encoder};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::is_interview_participant;
pub(super) use super::streams::{
    CANDIDATE_AUDIO_QUEUE_FRAMES, CandidateInput, CandidateStreams, GEMINI_AUDIO_SAMPLE_RATE,
    Ingress,
};
use crate::gemini::GeminiLiveSession;

pub(super) const GEMINI_AUDIO_CHANNELS: i32 = 1;

pub(super) fn candidate_audio_stream(track: RtcAudioTrack) -> NativeAudioStream {
    NativeAudioStream::with_options(
        track,
        GEMINI_AUDIO_SAMPLE_RATE,
        GEMINI_AUDIO_CHANNELS,
        NativeAudioStreamOptions {
            // Forty seconds covers partial transcription and a bounded
            // connection/setup attempt without the default 100 ms overflow.
            queue_size_frames: Some(CANDIDATE_AUDIO_QUEUE_FRAMES),
        },
    )
}

/// Forward each complete ten-millisecond LiveKit frame as 16 kHz mono PCM16.
/// Combining two frames held the first for another ten milliseconds, including
/// an utterance ending on an odd frame. This doubles websocket message count
/// from 50 to 100 per second; the audio is unchanged, and only the framing
/// each message carries is added. A shorter frame still waits in the buffer
/// for the rest of one.
pub(super) const GEMINI_AUDIO_BUFFER_BYTES: usize = 320;

/// One frame in five seconds. Each frame stays in the Live context and is
/// billed again on every later turn, and a presence check needs no more.
pub(super) const GEMINI_VIDEO_FRAME_INTERVAL: Duration = Duration::from_secs(5);

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
        // at once, ends again and fails again. Both `select!` arms are guarded
        // on the stream still being present, so that is a busy loop for as long
        // as the room lasts, not a lost frame.
        flush_candidate_audio(gemini, media).await?;
        return Ok(());
    };

    // Not `last_user_speech`. LiveKit delivers frames for as long as the track
    // is live, silence included, so stamping here made the interview
    // permanently "just spoke": `idle_seconds` never reached
    // SILENCE_THRESHOLD_S and the nudge never fired. Real speech is stamped on
    // `InputTranscript`, where Gemini has already decided that words were said.
    if media.push_mic_frame(&frame) {
        flush_candidate_audio(gemini, media).await?;
    }
    Ok(())
}

/// Generic so it can be tested: a real stream needs a real track, so a rule
/// written directly against `NativeAudioStream` is one no test can reach, and
/// this rule is the whole of the bug.
pub(super) fn release_if_ended<T>(stream: &mut Option<T>, ended: bool) {
    if ended {
        *stream = None;
    }
}

/// Sends one arriving video frame, at most one per interval, and outside the
/// candidate's speech only the socket's first. On 3.1 Live every frame counts
/// toward the turn and stays in the context billed each turn after, while a
/// presence check needs only the frames that accompany what was said. Images
/// of the board take their own path and are not held back by this.
pub(super) async fn pump_video(
    media: &mut CandidateMedia,
    streams: &mut CandidateStreams,
    gemini: &mut GeminiLiveSession,
    frame: Option<BoxVideoFrame>,
    candidate_speaking: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(frame) = frame else {
        streams.video = None;
        return Ok(());
    };

    // Stamped on the attempt, not the success: the dimensions come off the
    // wire, and a source stuck emitting frames that will not encode must cost
    // one try per interval rather than one per arriving frame.
    if !media.begin_video_attempt(Instant::now(), candidate_speaking) {
        return Ok(());
    }
    match encode_video_frame_jpeg_off_thread(&frame, GEMINI_VIDEO_JPEG_QUALITY).await {
        Ok(bytes) => {
            gemini
                .send_video_frame(&bytes, crate::gemini::GEMINI_IMAGE_MIME_TYPE)
                .await?;
            media.mark_presence_sent();
        }
        Err(error) => eprintln!("skipping unencodable video frame: {error}"),
    }
    Ok(())
}

/// Voice within the quiet window a pause between sentences fits in.
pub(super) fn voiced_recently(voiced_at: Option<Instant>, now: Instant) -> bool {
    voiced_at
        .is_some_and(|at| now.saturating_duration_since(at) < super::timing::CHECKPOINT_VOICE_QUIET)
}

/// The candidate's inbound media, plus the identity their tracks arrived on.
/// Bundled because every track event touches two or three of these together.
pub(super) struct CandidateMedia {
    pub(super) uplink: Uplink,
    pub(super) identity: Option<String>,
    last_video_frame: Instant,
    /// Whether the current socket has a camera frame yet. A replacement clears
    /// it, since a cold socket has seen none and a resumed one may predate it.
    pub(super) presence_frame_sent: bool,
    /// Set when a new audio stream is attached, so the first frame taken from
    /// it is not measured against the last frame of an earlier stream.
    mic_stream_fresh: bool,
}

/// The microphone samples buffered for Gemini, and what is known about whether
/// a write of them reached it.
#[derive(Default)]
pub(super) struct Uplink {
    /// Samples not yet confirmed written.
    pub(super) bytes: Vec<u8>,
    /// Whether a write of `bytes` was attempted and may have reached the
    /// provider, so replaying them could say the same words twice.
    pub(super) unconfirmed: bool,
    /// Bytes dropped rather than replayed for that reason, until logged.
    pub(super) quarantined: usize,
}

impl Uplink {
    /// Adds one frame; whether a write is due.
    fn push(&mut self, frame: &AudioFrame<'_>) -> bool {
        append_pcm16_bytes(frame, &mut self.bytes);
        self.bytes.len() >= GEMINI_AUDIO_BUFFER_BYTES
    }

    /// Keeps provenance before the first suspension: a failed or cancelled
    /// write cannot be replayed as if no bytes could have reached the provider.
    async fn flush<S: AudioSink>(
        &mut self,
        gemini: &mut S,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.unconfirmed |= !self.bytes.is_empty();
        flush_audio(gemini, &mut self.bytes).await?;
        self.unconfirmed = false;
        Ok(())
    }

    /// Moves an ambiguous attempted write's bytes to the quarantine count.
    fn quarantine_unconfirmed(&mut self) {
        if self.unconfirmed {
            self.quarantined = self.quarantined.saturating_add(self.bytes.len());
            self.bytes.clear();
            self.unconfirmed = false;
        }
    }

    /// What a socket replacement carries over: the unsent samples, and how
    /// many bytes were quarantined, that count including an unconfirmed write.
    fn hand_over(&mut self) -> (Vec<u8>, usize) {
        self.quarantine_unconfirmed();
        (
            std::mem::take(&mut self.bytes),
            std::mem::take(&mut self.quarantined),
        )
    }
}

impl CandidateMedia {
    /// Whether a camera frame is due at `now`, given when one was last tried
    /// and whether the candidate is speaking. A due frame counts as tried from
    /// here, whatever becomes of it.
    fn begin_video_attempt(&mut self, now: Instant, candidate_speaking: bool) -> bool {
        let due = should_send_video_frame(
            now.saturating_duration_since(self.last_video_frame),
            candidate_speaking,
            self.presence_frame_sent,
        );
        if due {
            self.last_video_frame = now;
        }
        due
    }

    fn mark_presence_sent(&mut self) {
        self.presence_frame_sent = true;
    }

    /// A replacement socket has seen no camera frame yet.
    pub(super) fn forget_presence_frame(&mut self) {
        self.presence_frame_sent = false;
    }

    /// Adds a microphone frame to the uplink; whether a write is due.
    pub(super) fn push_mic_frame(&mut self, frame: &AudioFrame<'_>) -> bool {
        self.uplink.push(frame)
    }

    /// Writes the uplink to `gemini`; see [`Uplink::flush`].
    pub(super) async fn flush_audio_to<S: AudioSink>(
        &mut self,
        gemini: &mut S,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.uplink.flush(gemini).await
    }

    /// Forgets the samples not yet written, for a silenced or ended input.
    pub(super) fn drop_unsent_audio(&mut self) {
        self.uplink.bytes.clear();
    }

    /// The unsent samples and the quarantined byte count, for a socket
    /// replacement to carry over or to log.
    pub(super) fn hand_over_audio(&mut self) -> (Vec<u8>, usize) {
        self.uplink.hand_over()
    }

    /// Puts back the samples a replacement carried over, all of them
    /// unattempted on the new socket.
    pub(super) fn restore_audio(&mut self, bytes: Vec<u8>) {
        self.uplink.bytes = bytes;
        self.uplink.unconfirmed = false;
    }

    /// Bytes quarantined and not yet logged.
    pub(super) fn quarantined_bytes(&self) -> usize {
        self.uplink.quarantined
    }

    /// Takes the quarantine count, to log it once.
    pub(super) fn take_quarantined(&mut self) -> usize {
        std::mem::take(&mut self.uplink.quarantined)
    }

    pub(super) fn new() -> Self {
        Self {
            uplink: Uplink::default(),
            identity: None,

            // Seeded in the past so the first frame sends immediately. The
            // monotonic clock starts at boot, so on a machine that just came up
            // there may be nothing to subtract from.
            last_video_frame: Instant::now()
                .checked_sub(GEMINI_VIDEO_FRAME_INTERVAL)
                .unwrap_or_else(Instant::now),
            presence_frame_sent: false,
            mic_stream_fresh: false,
        }
    }

    /// A new audio stream is about to be attached: its first frame is not
    /// measured against the last frame of an earlier one.
    pub(super) fn begin_mic_stream(&mut self) {
        self.mic_stream_fresh = true;
    }

    /// Accounts for one microphone dequeue, `frame` being whether it yielded a
    /// frame (`false` is the stream's end). Called from every place that takes
    /// a frame from the SDK queue and from no place that replays a buffered
    /// one.
    pub(super) fn note_mic_dequeue(
        &mut self,
        latency: &mut super::latency::ReplyLatency,
        frame: bool,
        now: Instant,
    ) {
        if std::mem::take(&mut self.mic_stream_fresh) || !frame {
            latency.mic_stream_broken();
        }
        if frame {
            latency.mic_dequeued(now);
        }
    }

    /// Never-attempted samples and whole retained frames still precede a new
    /// publication if republish wins the select just after recovery commits.
    /// Only an ambiguous attempted write is quarantined instead of replayed.
    pub(super) fn begin_audio_publication(&mut self, streams: &mut CandidateStreams) {
        self.uplink.quarantine_unconfirmed();
        if !streams.recovery_audio.is_empty() {
            streams.recovery_audio.yielded(Instant::now());
        }
    }

    pub(super) fn attach_audio(
        &mut self,
        streams: &mut CandidateStreams,
        track: &RemoteAudioTrack,
        participant: &RemoteParticipant,
    ) {
        self.begin_audio_publication(streams);
        self.identity = Some(participant.identity().to_string());
        self.begin_mic_stream();
        streams.audio = Some(candidate_audio_stream(track.rtc_track()));
    }
}

/// The candidate's tracks arriving and going away. Reports whether the event
/// was media, so the caller's match only has to carry what is left.
pub(super) async fn handle_media_event(
    media: &mut CandidateMedia,
    streams: &mut CandidateStreams,
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
            media.attach_audio(streams, track, participant);
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
            streams.video = Some(NativeVideoStream::new(track.rtc_track()));
        }

        // Guarded like the subscribe arms: an unrelated participant leaving
        // must not tear down the stream the candidate is still speaking into.
        RoomEvent::TrackUnsubscribed {
            track: RemoteTrack::Audio(_),
            participant,
            ..
        } if is_interview_participant(Some(&participant.identity().0), candidate_identity) => {
            flush_candidate_audio(gemini, media).await?;
            streams.audio = None;
        }
        RoomEvent::TrackUnsubscribed {
            track: RemoteTrack::Video(_),
            participant,
            ..
        } if is_interview_participant(Some(&participant.identity().0), candidate_identity) => {
            streams.video = None;
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

/// Keep provenance before the first suspension: a failed or cancelled write
/// cannot be replayed as if no bytes could have reached the provider.
pub(super) async fn flush_candidate_audio<S: AudioSink>(
    gemini: &mut S,
    media: &mut CandidateMedia,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    media.flush_audio_to(gemini).await
}

pub(super) async fn flush_audio<S: AudioSink>(
    gemini: &mut S,
    bytes: &mut Vec<u8>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !bytes.is_empty() {
        gemini.send_pcm_16khz(bytes).await?;
        bytes.clear();
    }
    Ok(())
}

pub(super) async fn next_candidate_input(
    audio: &mut Option<NativeAudioStream>,
    pending: &mut Ingress,
) -> CandidateInput {
    if let Some(input) = pending.pop(Instant::now()) {
        return input;
    }
    match next_audio_frame(audio).await {
        Some(frame) => CandidateInput::Frame(frame),
        None => CandidateInput::End,
    }
}

pub(super) async fn next_audio_frame(
    stream: &mut Option<NativeAudioStream>,
) -> Option<AudioFrame<'static>> {
    let stream = stream.as_mut()?;

    // A test hosting the room loop may hold this one dequeue; the loop's other
    // arms keep running, and a cancelled wait has taken no frame.
    #[cfg(test)]
    {
        super::probe::next_through_probe(stream).await
    }
    #[cfg(not(test))]
    {
        stream.next().await
    }
}

pub(super) async fn next_video_frame(
    stream: &mut Option<NativeVideoStream>,
) -> Option<BoxVideoFrame> {
    stream.as_mut()?.next().await
}

pub(super) struct OutputAudio {
    pub(super) latency_marker: Option<(serde_json::Value, Instant)>,
    /// The acknowledgement a tool round may play, once it has been generated.
    pub(super) tool_ack: std::sync::Arc<std::sync::OnceLock<Vec<u8>>>,
    /// The request generating it, cancelled with the session so an interview
    /// that ends or fails to open first does not pay for a clip nobody hears.
    pub(super) tool_ack_task: Option<AbortOnDrop>,
    tool_ack_active: bool,
    pub(super) source: NativeAudioSource,
    pub(super) sample_rate: u32,
    pub(super) pending_bytes: Vec<u8>,
    pub(super) playout_deadline: Instant,
    pub(super) frames: mpsc::Sender<QueuedOutputFrame>,
    /// Names the room on the one line this writes about a full queue.
    room_name: String,
    /// Full-queue lines written, one per backlog, and whether this backlog has
    /// had its line. A backlog ends when the worker has taken every frame.
    queue_full_lines: u32,
    backlog_reported: bool,
    pub(super) output_cancellation: CancellationToken,
}

/// Aborts a spawned task when dropped, so the task lives no longer than its
/// owner.
pub(super) struct AbortOnDrop(pub(super) tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) struct QueuedOutputFrame {
    pub(super) latency_marker: Option<(serde_json::Value, Instant)>,
    pub(super) output_cancellation: CancellationToken,
    pub(super) samples: Vec<i16>,
}

/// Speech generators pad a clip with silence; played in front of a reply, the
/// padding is only delay. Keeps the voiced stretch of 16-bit mono PCM plus 20
/// ms either side, or nothing when no sample is voiced, and ends it on a whole
/// 10 ms frame, padded with silence: a tail shorter than a frame would wait in
/// `pending_bytes` and be spliced onto the front of the reply.
pub(super) fn trim_silence(pcm: &[u8], sample_rate: u32) -> Vec<u8> {
    const VOICED: u16 = 500;
    let samples = decode_pcm16(pcm);
    let voiced = |sample: &i16| sample.unsigned_abs() >= VOICED;
    let (Some(first), Some(last)) = (
        samples.iter().position(voiced),
        samples.iter().rposition(voiced),
    ) else {
        return Vec::new();
    };
    let pad = sample_rate as usize / 50;
    let start = first.saturating_sub(pad);
    let end = (last + pad + 1).min(samples.len());
    let mut clip = pcm[start * 2..end * 2].to_vec();
    let frame_bytes = sample_rate as usize / 100 * 2;
    clip.resize(clip.len().next_multiple_of(frame_bytes), 0);
    clip
}

impl OutputAudio {
    /// How far past `now` what has been queued keeps playing.
    pub(super) fn playout_ahead(&self, now: Instant) -> Duration {
        self.playout_deadline.saturating_duration_since(now)
    }

    /// When what has been queued stops playing.
    pub(super) fn playout_ends_at(&self) -> Instant {
        self.playout_deadline
    }

    /// Keeps the latency record of the reply's first audio, with when it was
    /// accepted.
    pub(super) fn mark_first_audio(&mut self, record: serde_json::Value, accepted_at: Instant) {
        self.latency_marker = Some((record, accepted_at));
    }

    /// Whether the tool acknowledgement has been generated and has sound in it.
    pub(super) fn tool_ack_ready(&self) -> bool {
        self.tool_ack.get().is_some_and(|clip| !clip.is_empty())
    }

    /// Queues the generated acknowledgement, if there is one, as filler the
    /// reply's first chunk cuts. Whether it was queued.
    pub(super) async fn queue_tool_ack(
        &mut self,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let slot = std::sync::Arc::clone(&self.tool_ack);
        let Some(clip) = slot.get().filter(|clip| !clip.is_empty()) else {
            return Ok(false);
        };
        let mime = format!("audio/pcm;rate={}", self.sample_rate);
        self.capture_tool_ack(clip, &mime).await
    }

    /// Takes the acknowledgement being generated and the request generating it.
    pub(super) fn adopt_tool_ack(
        &mut self,
        slot: std::sync::Arc<std::sync::OnceLock<Vec<u8>>>,
        task: Option<AbortOnDrop>,
    ) {
        self.tool_ack = slot;
        self.tool_ack_task = task;
    }

    /// Frames sent to the worker and not yet taken by it.
    pub(super) fn queued_frames(&self) -> usize {
        self.frames.max_capacity() - self.frames.capacity()
    }

    pub(super) fn new(
        source: NativeAudioSource,
        sample_rate: u32,
        frames: mpsc::Sender<QueuedOutputFrame>,
        room_name: String,
    ) -> Self {
        Self {
            source,
            sample_rate,
            latency_marker: None,
            tool_ack: Default::default(),
            tool_ack_task: None,
            tool_ack_active: false,
            pending_bytes: Vec::new(),
            playout_deadline: Instant::now(),
            frames,
            room_name,
            queue_full_lines: 0,
            backlog_reported: false,
            output_cancellation: CancellationToken::new(),
        }
    }

    /// The line for a full worker queue, which names the room since one
    /// process hosts several.
    fn queue_full_line(&self) -> String {
        format!(
            "output audio worker queue full room={}; the rest of this chunk is dropped",
            self.room_name
        )
    }

    /// The line to write when the worker's queue is full, or `None` while this
    /// backlog already had one: a line per dropped chunk would repeat for every
    /// audio event of a reply.
    fn note_queue_full(&mut self) -> Option<String> {
        if std::mem::replace(&mut self.backlog_reported, true) {
            return None;
        }
        self.queue_full_lines += 1;
        Some(self.queue_full_line())
    }

    pub(super) fn interrupt(&mut self) {
        if let Some(record) = self.cancel_latency_marker() {
            super::latency::log(&record);
        }
        self.tool_ack_active = false;
        self.output_cancellation.cancel();
        self.output_cancellation = CancellationToken::new();
        self.source.clear_buffer();
        self.pending_bytes.clear();
        self.playout_deadline = Instant::now();
    }

    fn cancel_latency_marker(&mut self) -> Option<serde_json::Value> {
        let marker = self.latency_marker.take()?;
        let now = Instant::now();
        Some(first_frame_record(marker, now, None, now))
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

    /// Nothing playing and nothing of a reply waiting to be: no frame still
    /// ahead of the playout deadline, no chunk too short to make a frame, and
    /// no first-audio marker waiting for one. Audio queued now would be heard
    /// alone.
    pub(super) fn idle_at(&self, now: Instant) -> bool {
        !self.playing_at(now) && self.pending_bytes.is_empty() && self.latency_marker.is_none()
    }

    /// Whether `capture` would queue anything for this chunk.
    ///
    /// Split out, through `prepare_reply`, so the caller can ask before
    /// dropping a turn that is still playing. Dropping first and then finding
    /// the new chunk unusable cuts the
    /// interviewer off mid-sentence with nothing behind it.
    pub(super) fn accepts(&self, bytes: &[u8], mime_type: &str) -> bool {
        !bytes.is_empty()
            && audio_sample_rate(mime_type, self.sample_rate) == Some(self.sample_rate)
    }

    /// Whether this chunk, with what an earlier one left short of a frame,
    /// completes a frame for the worker. A reply's latency marker waits for
    /// that chunk, so the wait for it is counted as the provider's rather than
    /// as output queueing.
    pub(super) fn fills_frame(&self, bytes: &[u8]) -> bool {
        self.pending_bytes.len() + bytes.len()
            >= pcm16_frame_bytes(self.sample_rate, LIVEKIT_OUTPUT_CHANNELS)
    }

    /// The silence that completes a frame from what a finished reply left
    /// short of one, if it left anything. Left alone, that tail would never
    /// play, would hold the output from idle so the next acknowledgement
    /// could not start, and would be spliced onto the front of the next reply.
    pub(super) fn tail_padding(&self) -> Option<Vec<u8>> {
        let frame = pcm16_frame_bytes(self.sample_rate, LIVEKIT_OUTPUT_CHANNELS);
        let short = self.pending_bytes.len() % frame;
        (short > 0).then(|| vec![0; frame - short])
    }

    /// Whether a reply chunk will be queued, cutting any acknowledgement still
    /// queued or playing first, before the reply's queue is measured or its
    /// marker installed: the reply never waits for the filler to finish. The
    /// filler went out into idle output, so everything the cut cancels is the
    /// filler's.
    pub(super) fn prepare_reply(&mut self, bytes: &[u8], mime_type: &str) -> bool {
        if !self.accepts(bytes, mime_type) {
            return false;
        }
        if self.tool_ack_active {
            self.interrupt();
        }
        true
    }

    /// Queues the acknowledgement as filler that the reply's first chunk cuts.
    pub(super) async fn capture_tool_ack(
        &mut self,
        bytes: &[u8],
        mime_type: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        self.tool_ack_active |= self.accepts(bytes, mime_type);
        self.capture(bytes, mime_type).await
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

        // The worker has taken everything, so a full queue from here on is a
        // new backlog. Under sustained overload it frees one slot at a time and
        // never gets here, which is one backlog and one line.
        if self.queued_frames() == 0 {
            self.backlog_reported = false;
        }
        for samples in take_pcm16_frames(
            bytes,
            self.sample_rate,
            LIVEKIT_OUTPUT_CHANNELS,
            &mut self.pending_bytes,
        ) {
            // Ten milliseconds, which is what `take_pcm16_frames` cut this
            // into. Dividing the sample count by the channel count says the
            // same thing, but the channel count is one, so it says it with an
            // operation that reads identically whichever way round it is
            // written and cannot be wrong in a way anything could observe.
            let samples_per_channel = self.sample_rate / 100;
            let frame_duration =
                Duration::from_secs_f64(samples_per_channel as f64 / self.sample_rate as f64);
            let frame = QueuedOutputFrame {
                output_cancellation: self.output_cancellation.clone(),
                samples,
                latency_marker: self.latency_marker.take(),
            };
            match self.frames.try_send(frame) {
                Ok(()) => {
                    queued = true;
                    self.playout_deadline =
                        self.playout_deadline.max(Instant::now()) + frame_duration;
                }
                Err(error) => {
                    let stopped = matches!(error, mpsc::error::TrySendError::Closed(_));
                    if let Some(marker) = error.into_inner().latency_marker {
                        let record = enqueue_failure_record(marker, Instant::now());
                        super::latency::log(&record);
                    }
                    if stopped {
                        return Err(std::io::Error::other("output audio worker stopped").into());
                    }

                    // The worker plays at real time and Gemini can send faster,
                    // so a long reply can fill the queue. Drop the rest of this
                    // chunk: an error here leaves the room loop and ends the
                    // interview with no report. Frames that were not queued add
                    // nothing to the playout deadline. A later chunk of the
                    // reply is queued again once the worker frees space.
                    if let Some(line) = self.note_queue_full() {
                        eprintln!("{line}");
                    }
                    break;
                }
            }
        }
        Ok(queued)
    }
}

/// The native source as the output worker uses it, so a test can stand in a
/// sink whose capture blocks or completes as cancellation lands.
trait OutputAudioSink {
    fn capture_frame(
        &self,
        frame: &AudioFrame<'_>,
    ) -> impl Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send;
    fn clear_buffer(&self);
}

impl OutputAudioSink for NativeAudioSource {
    async fn capture_frame(
        &self,
        frame: &AudioFrame<'_>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        NativeAudioSource::capture_frame(self, frame).await?;
        Ok(())
    }

    fn clear_buffer(&self) {
        NativeAudioSource::clear_buffer(self);
    }
}

async fn output_audio_worker(
    source: impl OutputAudioSink,
    sample_rate: u32,
    mut frames: mpsc::Receiver<QueuedOutputFrame>,
) {
    while let Some(frame) = frames.recv().await {
        if frame.output_cancellation.is_cancelled() {
            if let Some(marker) = frame.latency_marker {
                log_first_frame(marker, Instant::now(), None);
            }
            continue;
        }
        let marker = frame.latency_marker;
        let dequeued_at = marker.as_ref().map(|_| Instant::now());
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
            biased;
            _ = &mut cancelled => {
                if let (Some(marker), Some(dequeued_at)) = (marker, dequeued_at) {
                    log_first_frame(marker, dequeued_at, None);
                }
                source.clear_buffer();
            }
            result = source.capture_frame(&audio_frame) => {
                if let (Some(marker), Some(dequeued_at)) = (marker, dequeued_at) {
                    log_first_frame(marker, dequeued_at, Some(result.is_ok()));
                }
                if let Err(error) = result {
                    eprintln!("failed to publish output audio frame: {error}");
                }

                // Capture can enqueue after the producer's clear and complete
                // as cancellation arrives. Clear before capturing a new token.
                if frame.output_cancellation.is_cancelled() {
                    source.clear_buffer();
                }
            }
        }
    }
}

/// The reply's first frame left the worker: captured, failed (`Some(false)`),
/// or cancelled before capture (`None`), so an accepted reply never vanishes
/// from the record without a terminal outcome.
fn enqueue_failure_record(marker: (serde_json::Value, Instant), now: Instant) -> serde_json::Value {
    let mut record = first_frame_record(marker, now, None, now);
    record["stage"] = "audio_enqueue_failed".into();
    record["enqueued"] = false.into();
    record
}

fn log_first_frame(
    (record, accepted_at): (serde_json::Value, Instant),
    dequeued_at: Instant,
    captured: Option<bool>,
) {
    let record = first_frame_record((record, accepted_at), dequeued_at, captured, Instant::now());
    super::latency::log(&record);
}

fn first_frame_record(
    (mut record, accepted_at): (serde_json::Value, Instant),
    dequeued_at: Instant,
    captured: Option<bool>,
    now: Instant,
) -> serde_json::Value {
    let since_accepted =
        i64::try_from(crate::agent::saturating_ms(now - accepted_at)).unwrap_or(i64::MAX);
    let total = record["elapsed_ms"]
        .as_i64()
        .map(|ms| ms.saturating_add(since_accepted));
    record["stage"] = match captured {
        Some(_) => "native_audio_capture",
        None => "native_audio_cancelled",
    }
    .into();
    record["output_queue_ms"] =
        crate::agent::saturating_ms(dequeued_at.saturating_duration_since(accepted_at)).into();
    record["native_capture_ms"] =
        crate::agent::saturating_ms(now.saturating_duration_since(dequeued_at)).into();
    record["server_total_ms"] = total.into();
    if let Some(observed) = record["observed_ms"].as_i64() {
        record["observed_ms"] = observed.saturating_add(since_accepted).into();
    }
    record["captured"] = captured.unwrap_or(false).into();
    record["native_queue_bound_ms"] = LIVEKIT_OUTPUT_QUEUE_MS.into();
    record
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
    Ok(OutputAudio::new(source, sample_rate, frames, room.name()))
}

/// Root-mean-square level, in PCM16 units, above which a frame counts as the
/// candidate speaking: about -40 dBFS, the top of a quiet room and below soft
/// speech. Used to hold a checkpoint back, and a silence nudge while the socket
/// has not transcribed the candidate, so it errs toward calling sound speech: a
/// noisy room waits longer, which the watch tick retries, where a soft speaker
/// taken for silence would be interrupted.
const VOICE_RMS: f64 = 316.0;

pub(super) fn frame_has_voice(frame: &AudioFrame<'_>) -> bool {
    pcm16_has_voice(&frame.data)
}

fn pcm16_has_voice(samples: &[i16]) -> bool {
    if samples.is_empty() {
        return false;
    }
    let energy = samples
        .iter()
        .map(|sample| f64::from(*sample).powi(2))
        .sum::<f64>();
    (energy / samples.len() as f64).sqrt() > VOICE_RMS
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
    let frame_bytes = pcm16_frame_bytes(sample_rate, channels);
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

/// The bytes in one ten-millisecond PCM16 frame.
fn pcm16_frame_bytes(sample_rate: u32, channels: u32) -> usize {
    (sample_rate / 100 * channels) as usize * 2
}

fn decode_pcm16(bytes: &[u8]) -> Vec<i16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect()
}

fn should_send_video_frame(
    elapsed: Duration,
    candidate_speaking: bool,
    presence_sent: bool,
) -> bool {
    elapsed >= GEMINI_VIDEO_FRAME_INTERVAL && (candidate_speaking || !presence_sent)
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
            "frame {width}x{height} is outside JPEG limits (each side at most 65535, and at most 64 MiB unpacked: 16777216 pixels in all)"
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
#[path = "../../tests/unit/livekit/media.rs"]
mod tests;

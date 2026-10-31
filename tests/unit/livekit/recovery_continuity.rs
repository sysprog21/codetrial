//! Speech continuity across a held microphone consumer.
//!
//! STATUS: THE LIVE ARMS IN THIS FILE HAVE NOT BEEN RUN. Nothing here was
//! observed against LiveKit or Gemini. `live_sdk_clip_continuity_arm` is
//! `#[ignore]`d and wired; what runs in the ordinary lane is local: the arm
//! table and its budgets, the clip loader, the publisher's schedule, the
//! dequeue gate on fakes and on a real SDK stream, the wire seen by a recording
//! Gemini peer, the answer-key comparison and the report.
//!
//! The question is whether bounded buffering keeps what a candidate said while
//! the room loop is not reading the microphone. One arm publishes authored,
//! numbered speech through a real SDK track from a real candidate participant
//! with the consumer free (the control); four hold the consumer's dequeue for
//! 20, 39, 41 and 60 seconds, starting when the clip starts. The SDK's store is
//! 4000 ten-millisecond frames (40 s), so the first two fit and the last two
//! overflow if the SDK keeps its documented drop-oldest rule. The room's
//! controls, the Gemini reader and the keepalive polling run throughout: only
//! the one future that takes the next frame waits, and it waits before it takes
//! one, so nothing is lost by the wait itself. A consumer already parked on the
//! SDK stream when the gate closes may take the one frame that arrives next, so
//! the hold begins within about one frame (10 ms) of the clip's first frame.
//!
//! One clip per arm is descriptive evidence. It does not estimate a loss rate,
//! and exact SDK drops are not inferred from the speech-to-text transcript:
//! the SDK counts them nowhere a caller can read, and a transcript can lack a
//! sentence that was transmitted.

use super::*;
use crate::livekit::probe::{self, ContinuityProbe, DequeueGate};
use ::livekit::options::TrackPublishOptions;
use ::livekit::prelude::{LocalAudioTrack, LocalTrack, RtcAudioSource, TrackSource};
use ::livekit::webrtc::audio_frame::AudioFrame;
use ::livekit::webrtc::audio_source::AudioSourceOptions;
use ::livekit::webrtc::audio_source::native::NativeAudioSource;
use std::time::Instant;

// The arms and their budget.

/// Seconds the join may take: the pinned binary's re-check, the room, Gemini's
/// setup and the interviewer's subscription.
const CLIP_SETUP_S: u64 = 60;
/// Seconds for the clip, the hold and the drain after the release.
const CLIP_RUN_S: u64 = 180;
const CLIP_CLEANUP_S: u64 = 60;
const CLIP_CAP_S: u64 = CLIP_SETUP_S + CLIP_RUN_S + CLIP_CLEANUP_S;
/// Seconds observed after the later of the release and the end of the clip, so
/// the transcript of what the burst carried has time to arrive.
const DRAIN_S: u64 = 60;
const FRAME_MS: u64 = 10;
const FRAME_SAMPLES: usize = 160;
const SAMPLE_RATE: u32 = 16_000;
/// Silence published before the clip, until the interviewer has read this
/// many frames. The clip starts only after its first frames could be taken, so
/// a track that was not yet subscribed cannot cost the start of the speech.
const SUBSCRIBED_FRAMES: u64 = 100;
const SUBSCRIBE_WAIT: Duration = Duration::from_secs(20);
/// A turn is late if its words complete this long after the later of the
/// release and the end of its clip.
const DELAY_BUDGET_MS: u64 = 20_000;
/// The clips every arm publishes, in order, once.
const CLIP_IDS: [&str; 2] = ["c01_say_plan", "c01_say_loop"];

const DESCRIPTIVE_NOTICE: &str = "One clip per arm is descriptive evidence only: it is not an estimate of a loss rate, and exact SDK drops are not inferred from the transcript, because the SDK counts them nowhere a caller can read and a transcript can lack words that were sent.";

/// One arm. `stall_s` is how long the consumer's dequeue is held, from the
/// start of the clip; zero is the matched unstalled control.
pub(super) struct Arm {
    id: &'static str,
    stall_s: u64,
}

const ARMS: [Arm; 5] = [
    Arm {
        id: "s0",
        stall_s: 0,
    },
    Arm {
        id: "s20",
        stall_s: 20,
    },
    Arm {
        id: "s39",
        stall_s: 39,
    },
    Arm {
        id: "s41",
        stall_s: 41,
    },
    Arm {
        id: "s60",
        stall_s: 60,
    },
];

/// The scenario the shared room fixture hosts for every arm: two planned
/// actions and the reserved close permit. It never reports, and its End is the
/// fixture closing what it owns.
static CLIP_SCENARIO: Scenario = Scenario {
    name: "s_clip_continuity",
    actions: &["candidate_join", "clip_start"],
    scripted_end: true,
    ceiling: 3,
    end_mode: EndMode::Manual,
    outer_cap_s: CLIP_CAP_S,
    report: false,
};

pub(super) fn arm_for(id: &str) -> Option<&'static Arm> {
    ARMS.iter().find(|arm| arm.id == id)
}

/// The digest the manifest's `fixture_sha256` carries: the clip set's own
/// manifest when its directory is named, a placeholder otherwise.
pub(super) fn clips_fixture_sha(dir: Option<String>) -> String {
    dir.and_then(|dir| std::fs::read(Path::new(&dir).join("manifest.json")).ok())
        .map_or_else(|| "b".repeat(64), |raw| crate::sha256_hex(&[&raw]))
}

impl Arm {
    fn run_id(&self) -> String {
        format!("clip-{}", self.id)
    }

    fn is_control(&self) -> bool {
        self.stall_s == 0
    }

    fn stall_frames(&self) -> u64 {
        self.stall_s * 1000 / FRAME_MS
    }

    pub(super) fn manifest(
        &self,
        candidate: &Path,
        candidate_sha256: &str,
        fixture: &str,
    ) -> Value {
        json!({
            "schema_version": 1,
            "run_id": self.run_id(),
            "binaries": {"candidate": {
                "path": candidate.to_str().unwrap(),
                "sha256": candidate_sha256,
            }},
            "settings_sha256": "a".repeat(64),
            "fixture_sha256": fixture,
            "selected_duration_s": CLIP_SETUP_S + CLIP_RUN_S,
            "outer_wall_cap_s": CLIP_CAP_S,
            "cleanup_reserve_s": CLIP_CLEANUP_S,
            "end_mode": "native_close",
            "run_kind": "clip",
            "duration_min": SCENARIO_DURATION_MIN,
            "observation_target_s": CLIP_RUN_S,
            "max_archive_bytes": (CLIP_CAP_S / 30 + 2) * 4096 + 256 * 1024,
            "actions": CLIP_SCENARIO.actions,
            "action_ceiling": CLIP_SCENARIO.actions.len() + 1,
            "limits": {},
            "scopes": {"scripted_actions": "hard", "wall_time": "hard"},
        })
    }
}

/// The interview length the token asks for. A clip run closes itself long
/// before the interview's own end, so the product minimum is all it needs.
fn clip_duration_min(manifest: &Value, recording_max_min: Option<u32>) -> Result<u32, String> {
    use crate::config::{MAX_DURATION_MIN, MIN_DURATION_MIN};
    if manifest["run_kind"] != "clip" || manifest["end_mode"] != "native_close" {
        return Err("a clip run is run_kind clip with end_mode native_close".into());
    }
    let duration = manifest["duration_min"]
        .as_u64()
        .and_then(|minutes| u32::try_from(minutes).ok())
        .ok_or("duration_min must be a whole number of minutes")?;
    if !(MIN_DURATION_MIN..=MAX_DURATION_MIN).contains(&duration) {
        return Err(format!(
            "duration_min {duration} is outside the product range"
        ));
    }
    let ceiling = crate::web::duration_ceiling(recording_max_min);
    if duration > ceiling {
        return Err(format!(
            "the recording cap would clamp the token to {ceiling} minutes, below {duration}"
        ));
    }
    match manifest["observation_target_s"].as_u64() {
        Some(target) if target <= u64::from(duration) * 60 => Ok(duration),
        other => Err(format!(
            "observation_target_s {other:?} is not within the {duration} minute interview"
        )),
    }
}

// The authored clips and the answer key.

/// What a spoken turn must show in a transcript, as groups of alternatives all
/// of which must appear. Words are as `normalize` leaves them. Each group is
/// said once in the authored text, so a group heard twice is a repeat.
struct Turn {
    n: usize,
    clip_id: &'static str,
    part: usize,
    text: &'static str,
    required: &'static [&'static [&'static str]],
}

impl Turn {
    fn id(&self) -> String {
        format!("t{}:{}/{}", self.n, self.clip_id, self.part + 1)
    }
}

/// The answer key. `text` is the authored part and is checked against the clip
/// manifest on load, so the key cannot drift from the audio it describes.
const ANSWER_KEY: [Turn; 6] = [
    Turn {
        n: 1,
        clip_id: "c01_say_plan",
        part: 0,
        text: "Stage 1. We are now in the algorithm stage.",
        required: &[&["algorithm"], &["we are now", "we re now"]],
    },
    Turn {
        n: 2,
        clip_id: "c01_say_plan",
        part: 1,
        text: "My constraints are at most ten thousand numbers, exactly one valid pair, and never reuse the same element.",
        required: &[&["thousand", "10 000", "10000"], &["valid pair"]],
    },
    Turn {
        n: 3,
        clip_id: "c01_say_plan",
        part: 2,
        text: "For the record, my codename for this attempt is saffron heron.",
        required: &[&["saffron"], &["heron"]],
    },
    Turn {
        n: 4,
        clip_id: "c01_say_loop",
        part: 0,
        text: "For each value I check the complement",
        required: &[&["complement"], &["each value", "for each"]],
    },
    Turn {
        n: 5,
        clip_id: "c01_say_loop",
        part: 1,
        text: "before adding the current index to the map.",
        required: &[&["current index"], &["map"]],
    },
    Turn {
        n: 6,
        clip_id: "c01_say_loop",
        part: 2,
        text: "One question is still open for you: should I return the two positions in ascending order?",
        required: &[&["ascending"], &["positions", "two positions"]],
    },
];

struct Clip {
    id: String,
    samples: Vec<i16>,
}

impl Clip {
    fn frames(&self) -> u64 {
        (self.samples.len() as u64).div_ceil(FRAME_SAMPLES as u64)
    }
}

/// The clips of one validated set, in `CLIP_IDS` order.
struct ClipSet {
    clips: Vec<Clip>,
}

/// 16 kHz mono signed 16-bit PCM from a RIFF/WAVE file.
fn parse_wav(bytes: &[u8]) -> Result<Vec<i16>, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }
    let mut at = 12;
    let mut format_ok = false;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = at + 8;
        let end = body
            .checked_add(size)
            .filter(|end| *end <= bytes.len())
            .ok_or("a chunk runs past the end of the file")?;
        if id == b"fmt " {
            let fmt = &bytes[body..end];
            if fmt.len() < 16 {
                return Err("the fmt chunk is too short".into());
            }
            let field = |from: usize| u16::from_le_bytes(fmt[from..from + 2].try_into().unwrap());
            let rate = u32::from_le_bytes(fmt[4..8].try_into().unwrap());
            if field(0) != 1 || field(2) != 1 || rate != SAMPLE_RATE || field(14) != 16 {
                return Err("the clip is not 16 kHz mono PCM16".into());
            }
            format_ok = true;
        } else if id == b"data" {
            if !format_ok {
                return Err("the data chunk comes before the fmt chunk".into());
            }
            return Ok(bytes[body..end]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| i16::from_le_bytes(*pair))
                .collect());
        }
        at = end + (size & 1);
    }
    Err("no data chunk".into())
}

impl ClipSet {
    /// Loads the authored clips from a directory holding the spoken clips'
    /// `manifest.json` and their WAV files. It refuses a set that does not
    /// declare speech, a clip whose bytes do not match its recorded digest, a
    /// clip that is not 16 kHz mono PCM16, and a clip whose authored parts are
    /// not the answer key's.
    fn load(dir: &Path) -> Result<Self, String> {
        let raw = std::fs::read(dir.join("manifest.json"))
            .map_err(|error| format!("cannot read the clip manifest: {:?}", error.kind()))?;
        let manifest: Value =
            serde_json::from_slice(&raw).map_err(|_| "the clip manifest is not JSON")?;
        if manifest["contains_speech"] != true {
            return Err("the clip manifest does not declare speech".into());
        }
        let format = &manifest["format"];
        if format["codec"] != "pcm_s16le"
            || format["sample_rate"] != SAMPLE_RATE
            || format["channels"] != 1
        {
            return Err("the clip manifest is not 16 kHz mono pcm_s16le".into());
        }
        let mut clips = Vec::new();
        for id in CLIP_IDS {
            let entry = manifest["clips"]
                .as_array()
                .and_then(|clips| clips.iter().find(|clip| clip["clip_id"] == id))
                .ok_or_else(|| format!("the clip manifest has no {id}"))?;
            let file = entry["file"].as_str().ok_or("a clip names no file")?;
            if file.contains('/') || file.contains("..") {
                return Err(format!("{id} names a file outside the clip directory"));
            }
            let bytes = std::fs::read(dir.join(file))
                .map_err(|error| format!("cannot read {file}: {:?}", error.kind()))?;
            if entry["sha256"].as_str() != Some(&crate::sha256_hex(&[&bytes])) {
                return Err(format!("{id} does not match its recorded digest"));
            }
            let parts: Vec<&str> = entry["parts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let wanted: Vec<&str> = ANSWER_KEY
                .iter()
                .filter(|turn| turn.clip_id == id)
                .map(|turn| turn.text)
                .collect();
            if parts != wanted {
                return Err(format!(
                    "{id} is not the text the answer key was authored on"
                ));
            }
            clips.push(Clip {
                id: id.to_owned(),
                samples: parse_wav(&bytes).map_err(|why| format!("{id}: {why}"))?,
            });
        }
        Ok(Self { clips })
    }

    fn frames(&self) -> u64 {
        self.clips.iter().map(Clip::frames).sum()
    }
}

/// The clips laid end to end, once. Past the last one it is silence, never the
/// clip again.
struct Timeline {
    samples: Vec<i16>,
    /// Each clip's id and its half-open frame range.
    spans: Vec<(String, u64, u64)>,
}

impl Timeline {
    fn new(clips: &ClipSet) -> Self {
        let mut samples = Vec::new();
        let mut spans = Vec::new();
        for clip in &clips.clips {
            let from = samples.len() as u64 / FRAME_SAMPLES as u64;
            samples.extend_from_slice(&clip.samples);
            samples.resize(samples.len().next_multiple_of(FRAME_SAMPLES), 0);
            spans.push((
                clip.id.clone(),
                from,
                samples.len() as u64 / FRAME_SAMPLES as u64,
            ));
        }
        Self { samples, spans }
    }

    fn clip_frames(&self) -> u64 {
        self.samples.len() as u64 / FRAME_SAMPLES as u64
    }

    fn frame(&self, index: u64) -> [i16; FRAME_SAMPLES] {
        let mut frame = [0i16; FRAME_SAMPLES];
        let from = index as usize * FRAME_SAMPLES;
        if let Some(chunk) = self.samples.get(from..from + FRAME_SAMPLES) {
            frame.copy_from_slice(chunk);
        }
        frame
    }
}

/// When the gate is held and when the arm is over, in frames since the clip
/// began.
struct Schedule {
    stall_frames: u64,
    total_frames: u64,
    /// One frame per this much of the controller's clock.
    pace: Duration,
}

impl Schedule {
    fn new(arm: &Arm, clip_frames: u64) -> Self {
        Self {
            stall_frames: arm.stall_frames(),
            total_frames: arm.stall_frames().max(clip_frames) + DRAIN_S * 1000 / FRAME_MS,
            pace: Duration::from_millis(FRAME_MS),
        }
    }

    fn holds_at(&self, frame: u64) -> bool {
        frame < self.stall_frames
    }

    fn total_s(&self) -> u64 {
        self.total_frames * FRAME_MS / 1000
    }

    /// Seconds the clip, the hold and the drain take, which must fit the
    /// manifest's run window.
    fn check_budget(&self) -> Result<(), String> {
        if self.total_s() > CLIP_RUN_S {
            return Err(format!(
                "the arm takes {}s, over the {CLIP_RUN_S}s window",
                self.total_s()
            ));
        }
        Ok(())
    }
}

// The publisher: controller-clocked, one pass, then silence.

trait FrameSink {
    async fn capture(&self, samples: &[i16]) -> Result<(), String>;
}

impl FrameSink for NativeAudioSource {
    async fn capture(&self, samples: &[i16]) -> Result<(), String> {
        let frame = AudioFrame {
            data: samples.into(),
            sample_rate: SAMPLE_RATE,
            num_channels: 1,
            samples_per_channel: u32::try_from(samples.len()).unwrap(),
        };
        self.capture_frame(&frame)
            .await
            .map_err(|error| error.to_string())
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ClipPublication {
    clip_id: String,
    first_ms: u64,
    last_ms: u64,
}

struct Published {
    /// Frames handed to the track from the clip's first frame on.
    clip_frames: u64,
    /// The instant the clip began, on the clock the stats use.
    began: Option<Instant>,
    clips: Vec<ClipPublication>,
    error: Option<String>,
}

/// Publishes silence until the interviewer has read some, then the clip and
/// then silence, one frame per `pace` of the controller's clock: a frame is
/// never captured before its time, and the clip is never played again. The
/// gate is held from the clip's first frame and released at the schedule's
/// frame, whatever else the arm is doing.
async fn run_publisher<S: FrameSink>(
    sink: &S,
    timeline: &Timeline,
    schedule: &Schedule,
    gate: &DequeueGate,
    ready: impl Fn() -> bool,
    abort: impl Fn() -> Option<String>,
    mut began: impl FnMut(Instant),
) -> Published {
    let pace = schedule.pace;
    let start = tokio::time::Instant::now();
    let mut published = Published {
        clip_frames: 0,
        began: None,
        clips: timeline
            .spans
            .iter()
            .map(|(id, _, _)| ClipPublication {
                clip_id: id.clone(),
                first_ms: 0,
                last_ms: 0,
            })
            .collect(),
        error: None,
    };
    let mut clip_start: Option<(u64, tokio::time::Instant)> = None;
    let mut index = 0u64;
    loop {
        tokio::time::sleep_until(start + pace * u32::try_from(index).unwrap()).await;
        if let Some(why) = abort() {
            published.error = Some(why);
            break;
        }
        if clip_start.is_none() {
            if index >= SUBSCRIBED_FRAMES && ready() {
                clip_start = Some((index, tokio::time::Instant::now()));
                let now = Instant::now();
                published.began = Some(now);
                began(now);
            } else if start.elapsed()
                > SUBSCRIBE_WAIT + pace * u32::try_from(SUBSCRIBED_FRAMES).unwrap()
            {
                published.error = Some("the interviewer never read the track".into());
                break;
            }
        }
        let offset = clip_start.map(|(first, _)| index - first);
        if let Some(offset) = offset {
            let held = schedule.holds_at(offset);
            if held && !gate.is_held() {
                gate.hold();
            } else if !held && gate.is_held() {
                gate.release();
            }
            if offset >= schedule.total_frames {
                break;
            }
        }
        let samples = offset.map_or([0i16; FRAME_SAMPLES], |offset| timeline.frame(offset));
        if let Err(error) = sink.capture(&samples).await {
            published.error = Some(redact(&error));
            break;
        }
        if offset.is_some() {
            published.clip_frames += 1;
        }
        if let (Some(offset), Some((_, at))) = (offset, clip_start) {
            let ms = u64::try_from(at.elapsed().as_millis()).unwrap_or(u64::MAX);
            for ((_, from, to), clip) in timeline.spans.iter().zip(&mut published.clips) {
                if (*from..*to).contains(&offset) {
                    if offset == *from {
                        clip.first_ms = ms;
                    }
                    clip.last_ms = ms;
                }
            }
        }
        index += 1;
    }
    gate.release();
    published
}

// The SDK's own overflow log line.

/// The pinned libwebrtc logs `native audio stream queue overflow; dropped N
/// queued frames` through the `log` crate on a queue's first drop and on every
/// 100th, and nothing in the product installs a log backend. This fixture does,
/// for the one process an arm runs in, and keeps only those lines.
mod overflow_log {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Mutex, Once};
    use std::time::Instant;

    /// Most records kept, and most characters of a target and of a message.
    pub(super) const MAX_RECORDS: usize = 64;
    pub(super) const MAX_TARGET: usize = 64;
    pub(super) const MAX_MESSAGE: usize = 160;
    /// Only the SDK crates that own the audio stream are listened to.
    const TARGET_PREFIXES: [&str; 3] = ["libwebrtc", "livekit", "webrtc"];

    #[derive(Clone, Debug, PartialEq)]
    pub(super) struct Captured {
        pub(super) at: Instant,
        pub(super) target: String,
        pub(super) message: String,
    }

    #[derive(Clone, Debug, Default)]
    pub(super) struct Snapshot {
        pub(super) installed: bool,
        pub(super) records: Vec<Captured>,
        pub(super) unstored: u64,
    }

    /// A bounded store behind a `log::Log`. The process-wide one is `CAPTURE`;
    /// tests make their own.
    pub(super) struct Capture {
        records: Mutex<Vec<Captured>>,
        unstored: AtomicU64,
    }

    pub(super) static CAPTURE: Capture = Capture::new();
    static INSTALL: Once = Once::new();
    static INSTALLED: AtomicBool = AtomicBool::new(false);

    /// An audio-stream overflow or dropped-frames line from an SDK target.
    pub(super) fn wanted(target: &str, message: &str) -> bool {
        let message = message.to_ascii_lowercase();
        TARGET_PREFIXES
            .iter()
            .any(|prefix| target.starts_with(prefix))
            && message.contains("audio")
            && (message.contains("overflow")
                || (message.contains("dropped") && message.contains("frames")))
    }

    fn bounded(text: &str, limit: usize) -> String {
        text.chars().take(limit).collect()
    }

    impl Capture {
        pub(super) const fn new() -> Self {
            Self {
                records: Mutex::new(Vec::new()),
                unstored: AtomicU64::new(0),
            }
        }

        pub(super) fn snapshot(&self, installed: bool) -> Snapshot {
            Snapshot {
                installed,
                records: self.records.lock().unwrap().clone(),
                unstored: self.unstored.load(Ordering::Relaxed),
            }
        }

        pub(super) fn clear(&self) {
            self.records.lock().unwrap().clear();
            self.unstored.store(0, Ordering::Relaxed);
        }
    }

    impl log::Log for Capture {
        fn enabled(&self, metadata: &log::Metadata) -> bool {
            metadata.level() <= log::Level::Warn
                && TARGET_PREFIXES
                    .iter()
                    .any(|prefix| metadata.target().starts_with(prefix))
        }

        fn log(&self, record: &log::Record) {
            if !self.enabled(record.metadata()) {
                return;
            }
            let message = record.args().to_string();
            if !wanted(record.target(), &message) {
                return;
            }
            let mut records = self.records.lock().unwrap();
            if records.len() >= MAX_RECORDS {
                self.unstored.fetch_add(1, Ordering::Relaxed);
                return;
            }
            records.push(Captured {
                at: Instant::now(),
                target: bounded(record.target(), MAX_TARGET),
                message: bounded(&super::redact(&message), MAX_MESSAGE),
            });
        }

        fn flush(&self) {}
    }

    /// Makes `CAPTURE` the process's logger, once. True if it is the logger:
    /// false if another one was installed first, which `log` allows only once.
    /// Calling it again changes nothing and returns the same answer.
    pub(super) fn install() -> bool {
        INSTALL.call_once(|| {
            if log::set_logger(&CAPTURE).is_ok() {
                log::set_max_level(log::LevelFilter::Warn);
                INSTALLED.store(true, Ordering::SeqCst);
            }
        });
        INSTALLED.load(Ordering::SeqCst)
    }

    pub(super) fn snapshot() -> Snapshot {
        CAPTURE.snapshot(INSTALLED.load(Ordering::SeqCst))
    }
}

/// One captured line placed on the arm's clock.
#[derive(Clone, Debug, PartialEq)]
struct OverflowLog {
    /// Milliseconds from the clip's first frame; negative if logged before it.
    at_ms: i64,
    /// The N the line states: the stream's drop count when it was logged.
    dropped: Option<u64>,
    target: String,
    message: String,
}

impl OverflowLog {
    fn at(record: &overflow_log::Captured, began: Instant) -> Self {
        Self {
            at_ms: match record.at.checked_duration_since(began) {
                Some(after) => i64::try_from(after.as_millis()).unwrap_or(i64::MAX),
                None => {
                    -i64::try_from(began.duration_since(record.at).as_millis()).unwrap_or(i64::MAX)
                }
            },
            dropped: dropped_count(&record.message),
            target: record.target.clone(),
            message: record.message.clone(),
        }
    }

    fn to_json(&self) -> Value {
        json!({"at_ms": self.at_ms, "dropped": self.dropped, "target": self.target, "message": self.message})
    }

    fn from_json(value: &Value) -> Option<Self> {
        Some(Self {
            at_ms: value["at_ms"].as_i64()?,
            dropped: value["dropped"].as_u64(),
            target: value["target"].as_str()?.to_owned(),
            message: value["message"].as_str()?.to_owned(),
        })
    }
}

/// The number after "dropped" in the SDK's line.
fn dropped_count(message: &str) -> Option<u64> {
    let (_, rest) = message.split_once("dropped ")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

// What one arm observed, as the log carries it.

#[derive(Clone, Debug, PartialEq)]
struct Observation {
    arm: String,
    stall_s: u64,
    clip_ids: Vec<String>,
    turn_ids: Vec<String>,
    frames_planned: u64,
    frames_published: u64,
    /// The whole planned schedule was published and nothing stopped it.
    complete: bool,
    clips: Vec<ClipPublication>,
    held_ms: Option<u64>,
    released_ms: Option<u64>,
    held_arrivals: u64,
    dequeued_frames: u64,
    first_dequeue_ms: Option<u64>,
    last_dequeue_ms: Option<u64>,
    first_after_release_ms: Option<u64>,
    frames_after_release: u64,
    per_second: Vec<(i64, u64)>,
    transcripts: Vec<(u64, String)>,
    dropped_bytes: u64,
    quarantined_bytes: u64,
    /// Whether the overflow logger was the process's logger. Without it an
    /// empty list says nothing.
    sdk_overflow_logger: bool,
    sdk_overflow_logs: Vec<OverflowLog>,
    /// Matching records the logger saw after its bounded store was full.
    sdk_overflow_unstored: u64,
}

impl Observation {
    fn build(
        arm: &Arm,
        schedule: &Schedule,
        published: &Published,
        probe: &ContinuityProbe,
        overflow: &overflow_log::Snapshot,
    ) -> Self {
        let began = published.began.unwrap_or(probe.epoch);
        let ms = |at: Instant| {
            u64::try_from(at.saturating_duration_since(began).as_millis()).unwrap_or(0)
        };
        let stats = probe.gate.stats();
        let retention = probe.retention();
        Self {
            arm: arm.id.to_owned(),
            stall_s: arm.stall_s,
            clip_ids: CLIP_IDS.iter().map(|id| (*id).to_owned()).collect(),
            turn_ids: ANSWER_KEY.iter().map(Turn::id).collect(),
            frames_planned: schedule.total_frames,
            frames_published: published.clip_frames,
            complete: published.error.is_none() && published.clip_frames == schedule.total_frames,
            clips: published.clips.clone(),
            held_ms: stats.held_at.map(ms),
            released_ms: stats.released_at.map(ms),
            held_arrivals: stats.held_arrivals,
            dequeued_frames: stats.frames,
            first_dequeue_ms: stats.first_frame_at.map(ms),
            last_dequeue_ms: stats.last_frame_at.map(ms),
            first_after_release_ms: stats.first_after_release_at.map(ms),
            frames_after_release: stats.frames_after_release,
            per_second: stats.per_second.into_iter().collect(),
            transcripts: probe
                .transcripts()
                .into_iter()
                .map(|line| (ms(line.at), redact(&line.text)))
                .collect(),
            dropped_bytes: retention.dropped_bytes as u64,
            quarantined_bytes: retention.quarantined_bytes as u64,
            sdk_overflow_logger: overflow.installed,
            sdk_overflow_logs: overflow
                .records
                .iter()
                .map(|record| OverflowLog::at(record, began))
                .collect(),
            sdk_overflow_unstored: overflow.unstored,
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "arm": self.arm,
            "stall_s": self.stall_s,
            "clip_ids": self.clip_ids,
            "turn_ids": self.turn_ids,
            "frames_planned": self.frames_planned,
            "frames_published": self.frames_published,
            "complete": self.complete,
            "clips": self.clips.iter().map(|clip| json!({
                "clip_id": clip.clip_id, "first_ms": clip.first_ms, "last_ms": clip.last_ms,
            })).collect::<Vec<_>>(),
            "held_ms": self.held_ms,
            "released_ms": self.released_ms,
            "held_arrivals": self.held_arrivals,
            "dequeued_frames": self.dequeued_frames,
            "first_dequeue_ms": self.first_dequeue_ms,
            "last_dequeue_ms": self.last_dequeue_ms,
            "first_after_release_ms": self.first_after_release_ms,
            "frames_after_release": self.frames_after_release,
            "per_second": self.per_second.iter().map(|(s, n)| json!([s, n])).collect::<Vec<_>>(),
            "transcripts": self.transcripts.iter().map(|(at, text)| json!({"at_ms": at, "text": text})).collect::<Vec<_>>(),
            "dropped_bytes": self.dropped_bytes,
            "quarantined_bytes": self.quarantined_bytes,
            "sdk_overflow_logger": self.sdk_overflow_logger,
            "sdk_overflow_logs": self.sdk_overflow_logs.iter().map(OverflowLog::to_json).collect::<Vec<_>>(),
            "sdk_overflow_unstored": self.sdk_overflow_unstored,
            "notice": DESCRIPTIVE_NOTICE,
        })
    }

    fn from_json(value: &Value) -> Result<Self, String> {
        let number = |name: &str| {
            value[name]
                .as_u64()
                .ok_or_else(|| format!("{name} is not a whole number"))
        };
        let optional = |name: &str| value[name].as_u64();
        let strings = |name: &str| -> Vec<String> {
            value[name]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        };
        Ok(Self {
            arm: value["arm"].as_str().ok_or("arm is missing")?.to_owned(),
            stall_s: number("stall_s")?,
            clip_ids: strings("clip_ids"),
            turn_ids: strings("turn_ids"),
            frames_planned: number("frames_planned")?,
            frames_published: number("frames_published")?,
            complete: value["complete"].as_bool().unwrap_or(false),
            clips: value["clips"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|clip| {
                    Ok(ClipPublication {
                        clip_id: clip["clip_id"].as_str().ok_or("clip_id")?.to_owned(),
                        first_ms: clip["first_ms"].as_u64().ok_or("first_ms")?,
                        last_ms: clip["last_ms"].as_u64().ok_or("last_ms")?,
                    })
                })
                .collect::<Result<_, String>>()?,
            held_ms: optional("held_ms"),
            released_ms: optional("released_ms"),
            held_arrivals: number("held_arrivals")?,
            dequeued_frames: number("dequeued_frames")?,
            first_dequeue_ms: optional("first_dequeue_ms"),
            last_dequeue_ms: optional("last_dequeue_ms"),
            first_after_release_ms: optional("first_after_release_ms"),
            frames_after_release: number("frames_after_release")?,
            per_second: value["per_second"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|pair| Some((pair[0].as_i64()?, pair[1].as_u64()?)))
                .collect(),
            transcripts: value["transcripts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|line| {
                    Some((line["at_ms"].as_u64()?, line["text"].as_str()?.to_owned()))
                })
                .collect(),
            dropped_bytes: number("dropped_bytes")?,
            quarantined_bytes: number("quarantined_bytes")?,
            sdk_overflow_logger: value["sdk_overflow_logger"].as_bool().unwrap_or(false),
            sdk_overflow_logs: value["sdk_overflow_logs"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(OverflowLog::from_json)
                .collect(),
            sdk_overflow_unstored: optional("sdk_overflow_unstored").unwrap_or(0),
        })
    }
}

/// The `arm_result` observations in a worker log, in order. Lines that are not
/// observation lines, and observation lines of other kinds, are ignored.
fn parse_arm_results(log: &str) -> Vec<Observation> {
    log.lines()
        .filter_map(|line| line.split_once("LIVE_OBS ").map(|(_, rest)| rest))
        .filter_map(|rest| serde_json::from_str::<Value>(rest).ok())
        .filter(|line| line["kind"] == "arm_result")
        .filter_map(|line| Observation::from_json(&line["fields"]).ok())
        .collect()
}

// Comparing a transcript with the answer key.

/// Lowercase words separated by single spaces; everything else is a space.
fn normalize(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// What an arm's transcript shows for one turn, before comparison with the
/// control.
#[derive(Clone, Debug, PartialEq)]
struct Raw {
    missing: bool,
    repeated: bool,
    delayed: bool,
    /// When the first and the last of the turn's words were heard.
    first_ms: Option<u64>,
    completed_ms: Option<u64>,
}

fn raw_findings(observation: &Observation) -> Vec<Raw> {
    // The text after each fragment, so the time a word was complete is the time
    // of the fragment that completed it, whichever way the fragments split.
    let mut so_far = String::new();
    let prefixes: Vec<(u64, String)> = observation
        .transcripts
        .iter()
        .map(|(at, text)| {
            so_far.push_str(text);
            (*at, normalize(&so_far))
        })
        .collect();
    let whole = prefixes.last().map(|(_, text)| text.as_str()).unwrap_or("");
    ANSWER_KEY
        .iter()
        .map(|turn| {
            let mut heard_at = Vec::new();
            let mut repeats = true;
            for group in turn.required {
                match prefixes
                    .iter()
                    .find(|(_, text)| group.iter().any(|alt| text.contains(alt)))
                {
                    Some((at, _)) => heard_at.push(*at),
                    None => repeats = false,
                }
                let count = group
                    .iter()
                    .map(|alt| whole.matches(alt).count())
                    .max()
                    .unwrap_or(0);
                repeats &= count >= 2;
            }
            let missing = heard_at.len() != turn.required.len();
            let completed = (!missing).then(|| heard_at.iter().copied().max().unwrap_or(0));
            let clip_end = observation
                .clips
                .iter()
                .find(|clip| clip.clip_id == turn.clip_id)
                .map_or(0, |clip| clip.last_ms);
            let reference = observation.released_ms.unwrap_or(0).max(clip_end);
            Raw {
                missing,
                repeated: !missing && repeats,
                delayed: completed.is_some_and(|at| at > reference + DELAY_BUDGET_MS),
                first_ms: heard_at.iter().copied().min(),
                completed_ms: completed,
            }
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Missing,
    Repeated,
    Delayed,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Repeated => "repeated",
            Self::Delayed => "delayed",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Status {
    Intact,
    /// Seen in a stalled arm and not in the control.
    Finding(Kind),
    /// Seen in a stalled arm and also in the control, or with no control to
    /// compare against: the stall cannot be blamed.
    Inconclusive(Kind, &'static str),
    /// Seen in the control itself: a baseline, never a loss.
    Baseline(Kind),
}

struct TurnReport {
    turn_id: String,
    statuses: Vec<Status>,
    raw: Raw,
}

struct ArmReport {
    arm: &'static Arm,
    observation: Option<Observation>,
    turns: Vec<TurnReport>,
}

impl ArmReport {
    fn count(&self, wanted: impl Fn(&Status) -> bool) -> usize {
        self.turns
            .iter()
            .flat_map(|turn| &turn.statuses)
            .filter(|status| wanted(status))
            .count()
    }

    fn findings(&self, kind: Kind) -> usize {
        self.count(|status| *status == Status::Finding(kind))
    }

    fn inconclusive(&self) -> usize {
        self.count(|status| matches!(status, Status::Inconclusive(..)))
    }
}

fn kinds(raw: &Raw) -> Vec<Kind> {
    [
        (raw.missing, Kind::Missing),
        (raw.repeated, Kind::Repeated),
        (raw.delayed, Kind::Delayed),
    ]
    .into_iter()
    .filter_map(|(present, kind)| present.then_some(kind))
    .collect()
}

/// One report per declared arm, in the arm table's order. A miss, repeat or
/// delay that the unstalled control shows too is inconclusive, not a loss; so
/// is every finding when the control was not observed.
fn compare(observations: &[Observation]) -> Vec<ArmReport> {
    let seen = |arm: &Arm| observations.iter().find(|item| item.arm == arm.id);
    let control = ARMS.iter().find(|arm| arm.is_control()).and_then(seen);
    // A control that stopped early, or whose gate was held, is no baseline.
    let control_raw = control
        .filter(|control| control.complete && control.held_ms.is_none())
        .map(raw_findings);
    ARMS.iter()
        .map(|arm| {
            let observation = seen(arm).cloned();
            let turns = observation
                .as_ref()
                .map(|observation| {
                    raw_findings(observation)
                        .into_iter()
                        .enumerate()
                        .map(|(at, raw)| {
                            let statuses = if kinds(&raw).is_empty() {
                                vec![Status::Intact]
                            } else {
                                kinds(&raw)
                                    .into_iter()
                                    .map(|kind| {
                                        if !observation.complete {
                                            Status::Inconclusive(kind, "the arm did not complete")
                                        } else if arm.is_control() {
                                            Status::Baseline(kind)
                                        } else if let Some(base) = &control_raw {
                                            if kinds(&base[at]).contains(&kind) {
                                                Status::Inconclusive(
                                                    kind,
                                                    "also present in the control",
                                                )
                                            } else {
                                                Status::Finding(kind)
                                            }
                                        } else {
                                            Status::Inconclusive(kind, "no control observation")
                                        }
                                    })
                                    .collect()
                            };
                            TurnReport {
                                turn_id: ANSWER_KEY[at].id(),
                                statuses,
                                raw,
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            ArmReport {
                arm,
                observation,
                turns,
            }
        })
        .collect()
}

fn render_report(observations: &[Observation]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "SPEECH CONTINUITY REPORT");
    let _ = writeln!(out, "{DESCRIPTIVE_NOTICE}");
    let _ = writeln!(
        out,
        "Times are milliseconds from the clip's first frame. A turn is late if complete more than {DELAY_BUDGET_MS} ms after the later of the release and its clip's last frame."
    );
    for arm in ARMS.iter().map(|arm| arm.id) {
        let copies = observations.iter().filter(|item| item.arm == arm).count();
        if copies > 1 {
            let _ = writeln!(
                out,
                "WARNING: {copies} results for {arm}; the first is reported."
            );
        }
    }
    for report in compare(observations) {
        let arm = report.arm;
        let _ = writeln!(
            out,
            "\nARM {} ({})",
            arm.id,
            if arm.is_control() {
                "unstalled control".to_owned()
            } else {
                format!("consumer held {} s", arm.stall_s)
            }
        );
        let Some(observation) = &report.observation else {
            let _ = writeln!(
                out,
                "  NOT RUN: no arm_result line for this arm; nothing is inferred and it is not rerun here."
            );
            continue;
        };
        let _ = writeln!(
            out,
            "  published {} of {} planned frames; clips {}",
            observation.frames_published,
            observation.frames_planned,
            observation
                .clips
                .iter()
                .map(|clip| format!("{} {}..{} ms", clip.clip_id, clip.first_ms, clip.last_ms))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let _ = writeln!(
            out,
            "  gate held at {:?} ms, released at {:?} ms; the consumer began {} wait(s) at the gate while held",
            observation.held_ms, observation.released_ms, observation.held_arrivals
        );
        let _ = writeln!(
            out,
            "  transmission: {} frames taken from the SDK queue, silence before the clip included (first {:?} ms, last {:?} ms); first after release {:?} ms; {} after release",
            observation.dequeued_frames,
            observation.first_dequeue_ms,
            observation.last_dequeue_ms,
            observation.first_after_release_ms,
            observation.frames_after_release
        );
        let _ = writeln!(
            out,
            "  retention markers (the room loop's own counters; the SDK's drops are not counted anywhere readable): dropped_bytes={} quarantined_bytes={}",
            observation.dropped_bytes, observation.quarantined_bytes
        );
        render_overflow(&mut out, observation);
        for turn in &report.turns {
            let words: Vec<String> = turn
                .statuses
                .iter()
                .map(|status| match status {
                    Status::Intact => "intact".to_owned(),
                    Status::Finding(kind) => kind.name().to_owned(),
                    Status::Inconclusive(kind, why) => {
                        format!("inconclusive ({} - {why})", kind.name())
                    }
                    Status::Baseline(kind) => format!("baseline {}", kind.name()),
                })
                .collect();
            let _ = writeln!(
                out,
                "  {}: {}  (first heard {:?} ms, complete {:?} ms)",
                turn.turn_id,
                words.join("; "),
                turn.raw.first_ms,
                turn.raw.completed_ms
            );
        }
        let _ = writeln!(
            out,
            "  totals: missing {} repeated {} delayed {} inconclusive {}",
            report.findings(Kind::Missing),
            report.findings(Kind::Repeated),
            report.findings(Kind::Delayed),
            report.inconclusive()
        );
        let _ = writeln!(
            out,
            "  transcript lines ({}):",
            observation.transcripts.len()
        );
        for (at, text) in &observation.transcripts {
            let _ = writeln!(out, "    {at:>7} ms  {text:?}");
        }
    }
    out
}

/// What the SDK's own overflow log says for one arm, and what it cannot say.
fn render_overflow(out: &mut String, observation: &Observation) {
    use std::fmt::Write as _;
    let _ = writeln!(
        out,
        "  SDK overflow log (libwebrtc logs a queue's first drop and then every 100th; a line gives that stream's drop count N and a time, not which frames were lost, whether they were speech, or whether any turn lost words):"
    );
    if !observation.sdk_overflow_logger {
        let _ = writeln!(
            out,
            "    the overflow logger was not the process's logger: no line could be captured, so nothing is concluded."
        );
        return;
    }
    if observation.sdk_overflow_logs.is_empty() {
        let _ = writeln!(
            out,
            "    no matching line was captured (warn level or above, from libwebrtc/livekit/webrtc targets): the pinned SDK logs a queue's first drop at warn, so this means no audio-stream drop if the SDK logs as pinned; only the offline wire test proves the logger receives that line."
        );
        return;
    }
    for line in &observation.sdk_overflow_logs {
        let _ = writeln!(
            out,
            "    {:>7} ms  dropped={:?}  [{}] {:?}",
            line.at_ms, line.dropped, line.target, line.message
        );
    }
    let last = observation
        .sdk_overflow_logs
        .iter()
        .filter_map(|line| line.dropped)
        .max();
    let _ = writeln!(
        out,
        "    {} line(s); the highest count logged by any audio stream is {:?}, so that stream had dropped that many and at most 99 more by the end (a count within 99); the first line is at {} ms and each time is when a line was logged, which for a later line is its 100th drop, not the first; a negative time is before the clip began; frame identity is not known.",
        observation.sdk_overflow_logs.len(),
        last,
        observation.sdk_overflow_logs[0].at_ms
    );
    if observation.sdk_overflow_unstored > 0 {
        let _ = writeln!(
            out,
            "    {} further matching line(s) were not stored (the store holds {} lines).",
            observation.sdk_overflow_unstored,
            overflow_log::MAX_RECORDS
        );
    }
}

include!("recovery_continuity_parts/report_and_clip_arms.rs");
include!("recovery_continuity_parts/manifest_writer_and_mic_gap.rs");

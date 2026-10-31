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

/// Reads every `worker-s*.log` in a directory and writes the report beside
/// them. Does nothing unless the launcher wrapper asks, like the manifest
/// writer; it touches no provider.
#[test]
fn live_clip_report() {
    let Ok(dir) = std::env::var("CODETRIAL_LIVE_REPORT_DIR") else {
        return;
    };
    let mut observations = Vec::new();
    for arm in &ARMS {
        let log = std::fs::read_to_string(Path::new(&dir).join(format!("worker-{}.log", arm.id)))
            .unwrap_or_default();
        observations.extend(
            parse_arm_results(&log)
                .into_iter()
                .filter(|item| item.arm == arm.id),
        );
    }
    let report = render_report(&observations);
    std::fs::write(Path::new(&dir).join("continuity-report.txt"), &report).unwrap();
    println!("{report}");
}

// The live arm. NOT RUN.

/// The shared room fixture with an arm's own pieces.
struct ClipEffects<'a> {
    live: LiveEffects<'a>,
    arm: &'static Arm,
    timeline: Timeline,
    schedule: Schedule,
    probe: Arc<ContinuityProbe>,
    observation: Option<Observation>,
    published_error: Option<String>,
}

impl<'a> ClipEffects<'a> {
    fn new(
        arm: &'static Arm,
        clips: &ClipSet,
        probe: Arc<ContinuityProbe>,
        mut live: LiveEffects<'a>,
    ) -> Self {
        live.setup_s = CLIP_SETUP_S;
        let timeline = Timeline::new(clips);
        let schedule = Schedule::new(arm, timeline.clip_frames());
        Self {
            live,
            arm,
            timeline,
            schedule,
            probe,
            observation: None,
            published_error: None,
        }
    }

    /// Publishes the track, then runs the arm's clock to its end. Reads and
    /// waits here are not actions; the permit for `clip_start` covers the
    /// audio that leaves the candidate.
    async fn run_clip(&mut self) -> Result<(), String> {
        let link = self
            .live
            .link
            .as_ref()
            .ok_or("the candidate is not connected")?;
        let source = NativeAudioSource::new(AudioSourceOptions::default(), SAMPLE_RATE, 1, 100);
        let track = LocalAudioTrack::create_audio_track(
            "candidate-clip",
            RtcAudioSource::Native(source.clone()),
        );
        link.room
            .local_participant()
            .publish_track(
                LocalTrack::Audio(track),
                TrackPublishOptions {
                    source: TrackSource::Microphone,
                    ..Default::default()
                },
            )
            .await
            .map_err(|error| redact(&error.to_string()))?;
        emit(
            "clip_track_published",
            json!({"arm": self.arm.id, "clips": CLIP_IDS, "turns": ANSWER_KEY.iter().map(Turn::id).collect::<Vec<_>>()}),
        );
        let stop_file = self.live.stop_file.clone();
        let script_stop = self.live.script_stop;
        let live = &self.live;
        let probe = &self.probe;
        let emitted = std::cell::Cell::new(0usize);
        let overflow_emitted = std::cell::Cell::new(0usize);
        let began_at = std::cell::Cell::new(None::<Instant>);
        let published = run_publisher(
            &source,
            &self.timeline,
            &self.schedule,
            &probe.gate,
            || probe.gate.stats().frames >= SUBSCRIBED_FRAMES,
            || {
                if stop_file.as_deref().is_some_and(Path::exists) {
                    Some("stopped by the launcher".to_owned())
                } else if Instant::now() >= script_stop {
                    Some("the outer wall cap came during the arm".to_owned())
                } else if !live.agent_running() {
                    Some("the interviewer ended during the arm".to_owned())
                } else {
                    emit_new_transcripts(probe, began_at.get(), &emitted);
                    emit_new_overflows(began_at.get(), &overflow_emitted);
                    None
                }
            },
            |began| {
                probe.gate.rebase(began);
                began_at.set(Some(began));
                emit("clip_began", json!({"arm": self.arm.id}));
            },
        )
        .await;
        emit_new_transcripts(probe, published.began, &emitted);
        emit_new_overflows(published.began, &overflow_emitted);
        self.observation = Some(Observation::build(
            self.arm,
            &self.schedule,
            &published,
            probe,
            &overflow_log::snapshot(),
        ));
        match published.error {
            None => Ok(()),
            Some(error) => {
                self.published_error = Some(error.clone());
                Err(error)
            }
        }
    }

    /// Closes what the fixture owns, directly: the candidate's connection, then
    /// the interviewer's task. No End is sent, so no assessment report is
    /// generated or paid for.
    async fn close_owned(&mut self) -> Result<(), String> {
        self.probe.gate.release();
        let mut errors = Vec::new();
        if let Some(link) = self.live.link.take() {
            if let Err(error) = link.room.close().await {
                errors.push(redact(&error.to_string()));
            }
            link.observer.abort();
        }
        if let Some(agent) = self.live.agent.take() {
            agent.abort();
            self.live.agent_end = Some(AgentEnd::Aborted);
        }
        emit("owned_closed", json!({"errors": errors}));
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

/// Emits the overflow lines not yet emitted, on the arm's clock.
fn emit_new_overflows(began: Option<Instant>, emitted: &std::cell::Cell<usize>) {
    let snapshot = overflow_log::snapshot();
    let lines = &snapshot.records;
    for record in &lines[emitted.get().min(lines.len())..] {
        let at = began.map(|began| OverflowLog::at(record, began));
        emit(
            "sdk_overflow_log",
            json!({
                "at_ms": at.as_ref().map(|line| line.at_ms),
                "dropped": at.as_ref().and_then(|line| line.dropped),
                "target": record.target,
                "message": record.message,
            }),
        );
    }
    emitted.set(lines.len());
}

fn emit_new_transcripts(
    probe: &ContinuityProbe,
    began: Option<Instant>,
    emitted: &std::cell::Cell<usize>,
) {
    let lines = probe.transcripts();
    for line in &lines[emitted.get().min(lines.len())..] {
        let at = began.map(|began| line.at.saturating_duration_since(began).as_millis() as u64);
        emit(
            "transcript_line",
            json!({"at_ms": at, "text": redact(&line.text)}),
        );
    }
    emitted.set(lines.len());
}

impl Effects for ClipEffects<'_> {
    async fn perform(&mut self, action: &str) -> Result<(), String> {
        emit("action", json!({"action": action}));
        match action {
            "candidate_join" => self.live.join().await,
            "clip_start" => self.run_clip().await,
            FINAL_ACTION_ID => self.close_owned().await,
            other => Err(format!("{other} has no clip effect")),
        }
    }

    async fn await_gate_entered(&mut self) -> Result<(), String> {
        Ok(())
    }

    async fn await_terminal(&mut self, _: &Scenario) -> Result<(), String> {
        Ok(())
    }

    async fn release_gate(&mut self) {
        self.probe.gate.release();
    }
}

/// The problems an arm's own run shows. The transcript is not judged here: it
/// is the evidence, compared in the report.
fn judge_arm(
    arm: &Arm,
    observation: Option<&Observation>,
    teardown: &Teardown,
    script: &Result<(), ScriptError>,
) -> Vec<String> {
    let mut problems = Vec::new();
    if let Err(error) = script {
        problems.push(format!("the script did not complete: {error:?}"));
    }
    match observation {
        None => problems.push("the arm produced no observation".to_owned()),
        Some(observation) => {
            if !observation.complete {
                problems.push(format!(
                    "the arm did not complete: published {} of {} planned frames",
                    observation.frames_published, observation.frames_planned
                ));
            }
            if !arm.is_control() {
                if observation.held_arrivals == 0 {
                    problems.push("the consumer never reached the held gate".to_owned());
                }
                if observation.released_ms.is_none() || observation.frames_after_release == 0 {
                    problems.push("no frame was taken after the release".to_owned());
                }
            } else if observation.held_ms.is_some() {
                problems.push("the control's gate was held".to_owned());
            }
        }
    }

    // The interviewer runs in this process and is aborted rather than ended (an
    // End would produce a report, which this arm must not). LiveKit keeps
    // listing an aborted participant until the room is deleted, so it is
    // reported but not a leak; any other participant, or a room still listed
    // after deletion, is.
    if let Some(left) = &teardown.participants {
        let (aborted, others): (Vec<_>, Vec<_>) = left
            .iter()
            .partition(|identity| identity.starts_with("interviewer-"));
        if !aborted.is_empty() {
            eprintln!("aborted interviewer listed until the room was deleted: {aborted:?}");
        }
        if !others.is_empty() {
            problems.push(format!("participants left in the room: {others:?}"));
        }
    }
    if !teardown.room_gone_after_delete {
        problems.push("the room was still listed after it was deleted".into());
    }
    if teardown.readers_unfinished != 0 {
        problems.push(format!(
            "{} Live reader tasks were still running",
            teardown.readers_unfinished
        ));
    }
    problems
}

/// NOT RUN against LiveKit or Gemini. One arm per invocation, chosen by
/// `CODETRIAL_LIVE_SCENARIO` (s0, s20, s39, s41 or s60), under the B0.1
/// launcher with the clip manifest `live_write_manifest` writes: planned
/// actions `candidate_join` and `clip_start`, plus the reserved close permit.
/// It needs `CODETRIAL_LIVE_CONFIG` and `CODETRIAL_LIVE_CLIPS`, the directory
/// of
/// the spoken clips outside the repository.
///
/// Real here: the LiveKit room and SDK transport on both ends, an SDK audio
/// track published by the candidate participant, the interviewer's room loop,
/// its microphone dequeue and Gemini Live. Not real: the interview's end (the
/// fixture closes its own participants and sends no End, so no assessment
/// report is generated and no end or report behavior is certified) and the
/// transcript's fidelity, which is Gemini's speech-to-text.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "NOT RUN: spends LiveKit and Gemini quota; needs the B0.1 launcher variables, CODETRIAL_LIVE_CONFIG, CODETRIAL_LIVE_CLIPS, CODETRIAL_LIVE_SCENARIO and budget approval"]
async fn live_sdk_clip_continuity_arm() {
    let mut permits = PermitClient::from_env()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let id = std::env::var("CODETRIAL_LIVE_SCENARIO")
        .expect("set CODETRIAL_LIVE_SCENARIO to s0, s20, s39, s41 or s60");
    let arm = arm_for(&id).expect("CODETRIAL_LIVE_SCENARIO is one of s0, s20, s39, s41, s60");
    assert_eq!(permits.manifest["run_id"], json!(arm.run_id()));
    assert_eq!(permits.manifest["actions"], json!(CLIP_SCENARIO.actions));
    assert_eq!(permits.manifest["action_ceiling"], json!(3));
    assert_eq!(permits.manifest["outer_wall_cap_s"], json!(CLIP_CAP_S));
    assert_eq!(permits.manifest["end_mode"], json!("native_close"));
    assert_eq!(permits.manifest["run_kind"], json!("clip"));
    let clips_dir = std::env::var("CODETRIAL_LIVE_CLIPS").expect("set CODETRIAL_LIVE_CLIPS");
    assert_eq!(
        permits.manifest["fixture_sha256"],
        json!(clips_fixture_sha(Some(clips_dir.clone()))),
        "the clip set is not the one the manifest was written for"
    );
    let clips = ClipSet::load(Path::new(&clips_dir)).unwrap_or_else(|error| panic!("{error}"));
    let timeline = Timeline::new(&clips);
    Schedule::new(arm, timeline.clip_frames())
        .check_budget()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(clips.frames(), timeline.clip_frames());

    let recording_cap = std::env::var("CODETRIAL_RECORDING_MAX_MINUTES")
        .ok()
        .map(|raw| raw.parse::<u32>().expect("a whole number of minutes"));
    let duration = clip_duration_min(&permits.manifest, recording_cap)
        .unwrap_or_else(|error| panic!("{error}"));
    let config = crate::config::load_from_pairs(
        crate::config::read_config_file(Path::new(
            &std::env::var("CODETRIAL_LIVE_CONFIG").expect("set CODETRIAL_LIVE_CONFIG"),
        ))
        .expect("read the isolated live config"),
    )
    .unwrap_or_else(|_| panic!("invalid isolated live config"));
    assert!(
        !config.google_api_keys.is_empty(),
        "the live config names no Gemini key"
    );
    register_secrets(config.google_api_keys.iter().cloned().chain([
        config.livekit_api_key.clone(),
        config.livekit_api_secret.clone(),
    ]));

    let unarmed = RecoveryGate {
        entered: Arc::new(tokio::sync::Notify::new()),
        release: Arc::new(tokio::sync::Notify::new()),
    };
    let probe = ContinuityProbe::new();
    let _installed = probe::install(&probe);
    overflow_log::install();
    overflow_log::CAPTURE.clear();
    let live = LiveEffects::new(
        &CLIP_SCENARIO,
        config,
        recording_cap,
        duration,
        &unarmed,
        permits.stop_file.clone(),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let mut effects = ClipEffects::new(arm, &clips, probe, live);
    emit(
        "start",
        json!({"scenario": arm.run_id(), "arm": arm.id, "stall_s": arm.stall_s, "room": effects.live.room_name}),
    );
    emit("notice", json!({"text": DESCRIPTIVE_NOTICE}));

    let result = run_script(&mut permits, &CLIP_SCENARIO, &mut effects).await;
    effects.probe.gate.release();
    let cleanup = cleanup_end_if_needed(&mut permits, &mut effects, &result).await;
    let teardown = effects.live.teardown().await;
    let problems = judge_arm(arm, effects.observation.as_ref(), &teardown, &result);
    if let Some(observation) = &effects.observation {
        emit("arm_result", observation.to_json());
    }
    emit(
        "result",
        json!({
            "scenario": arm.run_id(),
            "script": format!("{result:?}"),
            "cleanup_end": format!("{cleanup:?}"),
            "permits": permits.granted,
            "publisher_error": effects.published_error,
            "participants_left": teardown.participants,
            "room_gone_after_delete": teardown.room_gone_after_delete,
            "readers_unfinished": teardown.readers_unfinished,
            "teardown_notes": teardown.notes,
            "problems": problems,
        }),
    );
    if let Err(error) = cleanup {
        eprintln!("cleanup close failed: {error:?}");
    }
    result.expect("the scripted clip arm completed");
    assert!(problems.is_empty(), "{problems:?}");
}

// Local tests. Everything below runs in the ordinary lane.

fn tone(len: usize, level: i16) -> Vec<i16> {
    (0..len)
        .map(|n| if n % 2 == 0 { level } else { -level })
        .collect()
}

fn wav_bytes(samples: &[i16]) -> Vec<u8> {
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut bytes = b"RIFF".to_vec();
    bytes.extend((36 + data.len() as u32).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(SAMPLE_RATE.to_le_bytes());
    bytes.extend((SAMPLE_RATE * 2).to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend((data.len() as u32).to_le_bytes());
    bytes.extend(data);
    bytes
}

/// A clip directory in the shape of the spoken clips: the manifest, one WAV per
/// clip, parts equal to the answer key's. The audio is a tone, not speech.
fn write_clip_dir(scratch: &Scratch, mutate: impl FnOnce(&mut Value)) -> PathBuf {
    let dir = scratch.path("clips");
    std::fs::create_dir_all(&dir).unwrap();
    let mut entries = Vec::new();
    for (index, id) in CLIP_IDS.iter().enumerate() {
        let bytes = wav_bytes(&tone(1600 * (index + 2), 1000 * (index as i16 + 1)));
        std::fs::write(dir.join(format!("{id}.wav")), &bytes).unwrap();
        entries.push(json!({
            "clip_id": id,
            "file": format!("{id}.wav"),
            "sha256": crate::sha256_hex(&[&bytes]),
            "parts": ANSWER_KEY.iter().filter(|t| t.clip_id == *id).map(|t| t.text).collect::<Vec<_>>(),
        }));
    }
    let mut manifest = json!({
        "contains_speech": true,
        "format": {"container": "wav", "codec": "pcm_s16le", "sample_rate": 16000, "channels": 1},
        "clips": entries,
    });
    mutate(&mut manifest);
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    dir
}

/// Declared before running: five arms in a fixed order, one control and the
/// four stalls the leaf names; each arm fits the 60 s setup, 180 s run and 60 s
/// cleanup it is allotted for the longest stall plus the drain, and the
/// manifest declares two planned actions, the close permit as the third, the
/// native-close clip kind and a 300 s cap the controller accepts.
#[test]
fn the_five_arms_declare_the_stalls_and_fit_their_budgets() {
    assert_eq!(
        ARMS.iter()
            .map(|arm| (arm.id, arm.stall_s))
            .collect::<Vec<_>>(),
        [
            ("s0", 0),
            ("s20", 20),
            ("s39", 39),
            ("s41", 41),
            ("s60", 60)
        ]
    );
    assert_eq!(ARMS.iter().filter(|arm| arm.is_control()).count(), 1);
    assert_eq!(CLIP_SETUP_S + CLIP_RUN_S + CLIP_CLEANUP_S, 300);

    // The authored clips are about 37 s; any clip up to the window's slack
    // fits.
    for clip_frames in [3_740u64, 6_000] {
        for arm in &ARMS {
            let schedule = Schedule::new(arm, clip_frames);
            schedule.check_budget().unwrap();
            assert_eq!(
                schedule.total_s(),
                arm.stall_s.max(clip_frames / 100) + DRAIN_S
            );
        }
    }
    assert!(Schedule::new(&ARMS[4], 12_100).check_budget().is_err());
    let scratch = Scratch::new("arm-manifests");
    let candidate = scratch.path("candidate");
    std::fs::write(&candidate, "x").unwrap();
    for arm in &ARMS {
        let manifest = arm.manifest(&candidate, &"c".repeat(64), &"d".repeat(64));
        assert_eq!(manifest["actions"], json!(["candidate_join", "clip_start"]));
        assert_eq!(manifest["action_ceiling"], 3);
        assert_eq!(manifest["run_kind"], "clip");
        assert_eq!(manifest["end_mode"], "native_close");
        assert_eq!(manifest["outer_wall_cap_s"], 300);
        assert_eq!(manifest["cleanup_reserve_s"], 60);
        assert_eq!(manifest["selected_duration_s"], 240);
        let path = scratch.path("manifest.json");
        std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let output = std::process::Command::new("python3")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/interview-run-budget.py"))
            .args(["validate", "--manifest"])
            .arg(&path)
            .output()
            .expect("python3 runs the controller");
        assert!(
            output.status.success(),
            "{}: {}{}",
            arm.id,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            clip_duration_min(&manifest, None),
            Ok(SCENARIO_DURATION_MIN)
        );
    }
    assert!(arm_for("s40").is_none() && arm_for("a").is_none() && arm_for("").is_none());
    let mut manifest = ARMS[1].manifest(&candidate, &"c".repeat(64), &"d".repeat(64));
    manifest["end_mode"] = json!("manual");
    assert!(clip_duration_min(&manifest, None).is_err());
}

/// Declared before running: the loader accepts a set in the spoken clips'
/// shape and refuses one that declares no speech, has a digest that does not
/// match its bytes, is not 16 kHz mono PCM16, or whose authored parts differ
/// from the answer key; the numbered turns cover the six parts exactly.
#[test]
fn the_clip_loader_checks_speech_digests_format_and_the_answer_keys_text() {
    let scratch = Scratch::new("clips");
    let good = write_clip_dir(&scratch, |_| {});
    let set = ClipSet::load(&good).unwrap();
    assert_eq!(
        set.clips.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        CLIP_IDS
    );
    assert_eq!(set.clips[0].samples.len(), 3200);
    assert_eq!(
        ANSWER_KEY.iter().map(Turn::id).collect::<Vec<_>>(),
        [
            "t1:c01_say_plan/1",
            "t2:c01_say_plan/2",
            "t3:c01_say_plan/3",
            "t4:c01_say_loop/1",
            "t5:c01_say_loop/2",
            "t6:c01_say_loop/3"
        ]
    );
    for (label, mutate) in [
        (
            "speech",
            (|m: &mut Value| m["contains_speech"] = json!(false)) as fn(&mut Value),
        ),
        ("digest", |m| {
            m["clips"][0]["sha256"] = json!("0".repeat(64));
        }),
        ("rate", |m| m["format"]["sample_rate"] = json!(48000)),
        ("text", |m| {
            m["clips"][1]["parts"][2] = json!("something else");
        }),
        ("path", |m| m["clips"][0]["file"] = json!("../x.wav")),
        ("missing", |m| m["clips"] = json!([])),
    ] {
        let scratch = Scratch::new("clips-bad");
        let dir = write_clip_dir(&scratch, mutate);
        assert!(ClipSet::load(&dir).is_err(), "{label} must be refused");
    }
    // A WAV that is stereo, or truncated, is not a clip.
    let mut stereo = wav_bytes(&[1, 2, 3, 4]);
    stereo[22] = 2;
    assert!(parse_wav(&stereo).is_err());
    assert!(parse_wav(&wav_bytes(&[1, 2, 3, 4])[..30]).is_err());
    assert!(parse_wav(b"not a wav").is_err());
}

/// Declared before running: the timeline lays the clips end to end once, pads
/// each to a whole frame, and is silence after the last; a frame past the end
/// is never a repeat of the start.
#[test]
fn the_timeline_plays_each_clip_once_and_never_loops() {
    let set = ClipSet {
        clips: vec![
            Clip {
                id: "one".into(),
                samples: vec![7; 250],
            },
            Clip {
                id: "two".into(),
                samples: vec![9; 160],
            },
        ],
    };
    let timeline = Timeline::new(&set);
    assert_eq!(
        timeline.spans,
        [("one".to_owned(), 0, 2), ("two".to_owned(), 2, 3)]
    );
    assert_eq!(timeline.clip_frames(), 3);
    assert!(timeline.frame(0).iter().all(|s| *s == 7));
    assert_eq!(timeline.frame(1).iter().filter(|s| **s == 7).count(), 90);
    assert!(timeline.frame(2).iter().all(|s| *s == 9));
    for after in [3, 4, 100, 10_000] {
        assert_eq!(
            timeline.frame(after),
            [0i16; FRAME_SAMPLES],
            "frame {after}"
        );
    }
}

/// Records what the publisher captured and when, on the virtual clock.
struct Recorder {
    log: std::sync::Mutex<Vec<(tokio::time::Instant, i16)>>,
}

impl FrameSink for Recorder {
    async fn capture(&self, samples: &[i16]) -> Result<(), String> {
        self.log
            .lock()
            .unwrap()
            .push((tokio::time::Instant::now(), samples[0]));
        Ok(())
    }
}

/// Declared before running, on a paused clock so a 60 s arm takes no wall time:
/// for each arm the publisher sends silence until the reader is ready, holds
/// the gate from the clip's first frame for exactly the stall, releases it at
/// the stall's frame and never earlier, publishes each clip frame exactly once
/// in order at the controller's pace (never ahead of it), and stops after the
/// drain. The control never holds the gate.
#[tokio::test(start_paused = true)]
async fn the_publisher_holds_from_the_first_clip_frame_for_exactly_the_stall() {
    let set = ClipSet {
        clips: vec![
            Clip {
                id: "one".into(),
                samples: vec![5; 16000],
            },
            Clip {
                id: "two".into(),
                samples: vec![6; 16000],
            },
        ],
    };
    let timeline = Timeline::new(&set);
    for arm in &ARMS {
        let schedule = Schedule::new(arm, timeline.clip_frames());
        let gate = DequeueGate::new(Instant::now());
        let sink = Recorder {
            log: Default::default(),
        };
        let ready_after = std::cell::Cell::new(0u32);
        let seen = std::cell::RefCell::new(Vec::new());
        let published = run_publisher(
            &sink,
            &timeline,
            &schedule,
            &gate,
            || {
                ready_after.set(ready_after.get() + 1);
                ready_after.get() > 150
            },
            || {
                seen.borrow_mut()
                    .push((gate.is_held(), tokio::time::Instant::now()));
                None
            },
            |_| {},
        )
        .await;
        assert_eq!(published.error, None, "{}", arm.id);
        assert!(!gate.is_held(), "{}: released at the end", arm.id);
        let log = sink.log.lock().unwrap();
        let start = log[0].0;
        // The clip began at the first frame after 150 ready checks.
        let lead = log.iter().take_while(|(_, marker)| *marker == 0).count();
        assert!(lead >= 150, "{}: silence first, {lead}", arm.id);
        let clip: Vec<i16> = log.iter().map(|(_, m)| *m).skip(lead).take(200).collect();
        assert_eq!(&clip[..100], &[5; 100][..], "{}: clip one once", arm.id);
        assert_eq!(&clip[100..], &[6; 100][..], "{}: clip two once", arm.id);
        assert_eq!(
            log.iter().filter(|(_, m)| *m != 0).count(),
            200,
            "{}: the clips are published once and never again",
            arm.id
        );
        for (position, (at, _)) in log.iter().enumerate() {
            assert!(
                *at >= start + Duration::from_millis(FRAME_MS * position as u64),
                "{}: frame {position} was captured early",
                arm.id
            );
        }
        assert_eq!(
            log.len() as u64,
            lead as u64 + schedule.total_frames,
            "{}",
            arm.id
        );

        // The gate was held for the stall's frames after the lead and not for
        // one more: the abort probe runs once per frame, before the gate moves.
        let held: Vec<bool> = seen.borrow().iter().map(|(held, _)| *held).collect();
        let held_frames = held.iter().filter(|held| **held).count() as u64;
        assert_eq!(held_frames, arm.stall_frames(), "{}: held frames", arm.id);
        if arm.is_control() {
            assert!(held.iter().all(|held| !*held));
        }
        let stats = gate.stats();
        assert_eq!(stats.held_at.is_some(), !arm.is_control(), "{}", arm.id);
        assert_eq!(stats.released_at.is_some(), !arm.is_control(), "{}", arm.id);
    }
}

/// Declared before running: a publisher whose reader never becomes ready
/// gives up with an error, and a sink failure or an abort stops it, with the
/// gate released in every case.
#[tokio::test(start_paused = true)]
async fn the_publisher_gives_up_and_always_releases_the_gate() {
    let set = ClipSet {
        clips: vec![Clip {
            id: "one".into(),
            samples: vec![5; 1600],
        }],
    };
    let timeline = Timeline::new(&set);
    let schedule = Schedule::new(&ARMS[1], timeline.clip_frames());

    let gate = DequeueGate::new(Instant::now());
    let sink = Recorder {
        log: Default::default(),
    };
    let never = run_publisher(
        &sink,
        &timeline,
        &schedule,
        &gate,
        || false,
        || None,
        |_| {},
    )
    .await;
    assert!(never.error.as_deref().unwrap().contains("never read"));
    assert!(never.began.is_none() && !gate.is_held());

    let gate = DequeueGate::new(Instant::now());
    let count = std::cell::Cell::new(0u32);
    let aborted = run_publisher(
        &sink,
        &timeline,
        &schedule,
        &gate,
        || true,
        || {
            count.set(count.get() + 1);
            (count.get() > 400).then(|| "stop".to_owned())
        },
        |_| {},
    )
    .await;
    assert_eq!(aborted.error.as_deref(), Some("stop"));
    assert!(
        !gate.is_held(),
        "an abort in the middle of the hold releases"
    );
}

// The answer key, the comparison and the report.

fn observation(arm: &Arm, lines: &[(u64, &str)]) -> Observation {
    Observation {
        arm: arm.id.to_owned(),
        stall_s: arm.stall_s,
        clip_ids: CLIP_IDS.iter().map(|id| (*id).to_owned()).collect(),
        turn_ids: ANSWER_KEY.iter().map(Turn::id).collect(),
        frames_planned: 100,
        frames_published: 100,
        complete: true,
        clips: vec![
            ClipPublication {
                clip_id: "c01_say_plan".into(),
                first_ms: 0,
                last_ms: 20_790,
            },
            ClipPublication {
                clip_id: "c01_say_loop".into(),
                first_ms: 20_800,
                last_ms: 37_400,
            },
        ],
        held_ms: (!arm.is_control()).then_some(0),
        released_ms: (!arm.is_control()).then_some(arm.stall_s * 1000),
        held_arrivals: u64::from(!arm.is_control()),
        dequeued_frames: 10,
        first_dequeue_ms: Some(1),
        last_dequeue_ms: Some(2),
        first_after_release_ms: None,
        frames_after_release: 0,
        per_second: vec![(0, 100), (-1, 3)],
        transcripts: lines
            .iter()
            .map(|(at, text)| (*at, (*text).to_owned()))
            .collect(),
        dropped_bytes: 0,
        quarantined_bytes: 0,
        sdk_overflow_logger: true,
        sdk_overflow_logs: Vec::new(),
        sdk_overflow_unstored: 0,
    }
}

const FULL: [(u64, &str); 6] = [
    (2_000, "Stage one. We are now in the algorithm stage. "),
    (
        6_000,
        "My constraints are at most 10,000 numbers, exactly one valid pair. ",
    ),
    (14_000, "Codename saffron heron. "),
    (24_000, "For each value I check the complement "),
    (28_000, "before adding the current index to the map. "),
    (
        34_000,
        "Should I return the two positions in ascending order? ",
    ),
];

fn statuses(report: &ArmReport) -> Vec<Vec<Status>> {
    report
        .turns
        .iter()
        .map(|turn| turn.statuses.clone())
        .collect()
}

/// Declared before running, from the authored text: a transcript carrying all
/// six turns is intact in every arm; an arm lacking turn 1 whose control has it
/// has a missing finding; a repeated codename is a repeat; a turn heard after
/// the budget is delayed; and normalization copes with punctuation, case and
/// "10,000".
#[test]
fn the_answer_key_counts_missing_repeated_and_delayed_turns() {
    let control = observation(&ARMS[0], &FULL);
    let mut lines = FULL.to_vec();
    lines.remove(0);
    lines.insert(2, (15_000, "saffron heron again. "));
    lines.push((90_000, "ignored text. "));
    let late = [(70_000, "For each value I check the complement ")];
    let mut stalled = observation(&ARMS[1], &lines);
    stalled
        .transcripts
        .retain(|(_, text)| !text.contains("For each value"));
    stalled
        .transcripts
        .extend(late.iter().map(|(at, t)| (*at, (*t).to_owned())));
    let reports = compare(&[control, stalled]);
    assert_eq!(
        reports.len(),
        5,
        "a report row exists for every declared arm"
    );
    assert!(
        reports[0]
            .turns
            .iter()
            .all(|t| t.statuses == [Status::Intact])
    );
    let s20 = statuses(&reports[1]);
    assert_eq!(s20[0], [Status::Finding(Kind::Missing)]);
    assert_eq!(s20[1], [Status::Intact]);
    assert_eq!(s20[2], [Status::Finding(Kind::Repeated)]);
    assert_eq!(s20[3], [Status::Finding(Kind::Delayed)], "heard at 70 s");
    assert_eq!(reports[1].findings(Kind::Missing), 1);
    assert_eq!(reports[1].findings(Kind::Repeated), 1);
    assert_eq!(reports[1].findings(Kind::Delayed), 1);
    assert_eq!(reports[1].inconclusive(), 0);
    for not_run in &reports[2..] {
        assert!(not_run.observation.is_none() && not_run.turns.is_empty());
    }
    assert_eq!(normalize("It's 10,000-- A, b!"), "it s 10 000 a b");
}

/// Declared before running: a miss that the unstalled control also shows is
/// inconclusive and never counted as a loss, so is a repeat or a delay the
/// control shares; the control's own findings are baselines; with no control
/// every finding is inconclusive. Fragments split mid-word are still heard
/// when the pieces join.
#[test]
fn a_miss_also_present_in_the_control_is_inconclusive_not_a_loss() {
    let mut without_two: Vec<(u64, &str)> = FULL.to_vec();
    without_two.remove(1);
    let control = observation(&ARMS[0], &without_two);
    let stalled = observation(&ARMS[3], &without_two);
    let mut lost_three = without_two.clone();
    lost_three.remove(1);
    let lost = observation(&ARMS[4], &lost_three);
    let reports = compare(&[control, stalled.clone(), lost]);
    assert_eq!(statuses(&reports[0])[1], [Status::Baseline(Kind::Missing)]);
    assert_eq!(
        statuses(&reports[3])[1],
        [Status::Inconclusive(
            Kind::Missing,
            "also present in the control"
        )]
    );
    assert_eq!(reports[3].findings(Kind::Missing), 0);
    assert_eq!(reports[3].inconclusive(), 1);
    assert_eq!(statuses(&reports[4])[2], [Status::Finding(Kind::Missing)]);
    assert_eq!(
        statuses(&reports[4])[1],
        [Status::Inconclusive(
            Kind::Missing,
            "also present in the control"
        )]
    );
    assert_eq!(reports[4].findings(Kind::Missing), 1);

    let no_control = compare(&[stalled]);
    assert_eq!(
        statuses(&no_control[3])[1],
        [Status::Inconclusive(
            Kind::Missing,
            "no control observation"
        )]
    );
    assert_eq!(no_control[3].findings(Kind::Missing), 0);

    // Late in the control too: the stall is not to blame.
    let mut slow_control = observation(&ARMS[0], &FULL);
    slow_control.transcripts[0].0 = 60_000;
    let mut slow = observation(&ARMS[1], &FULL);
    slow.transcripts[0].0 = 70_000;
    let reports = compare(&[slow_control, slow]);
    assert_eq!(statuses(&reports[0])[0], [Status::Baseline(Kind::Delayed)]);
    assert_eq!(
        statuses(&reports[1])[0],
        [Status::Inconclusive(
            Kind::Delayed,
            "also present in the control"
        )]
    );

    let split = observation(
        &ARMS[0],
        &[
            (1_000, "the algo"),
            (2_000, "rithm. we a"),
            (3_000, "re now"),
            (4_000, " saff"),
            (5_000, "ron he"),
            (6_000, "ron"),
        ],
    );
    let raw = raw_findings(&split);
    assert!(!raw[0].missing && !raw[2].missing);
    assert_eq!(raw[0].completed_ms, Some(3_000));
    assert_eq!(raw[2].first_ms, Some(5_000));
    assert_eq!(raw[2].completed_ms, Some(6_000));
}

/// Declared before running: the arm_result line survives the round trip through
/// the log's own JSON, noise lines are skipped, and the report names every
/// declared arm, says plainly that one clip per arm is descriptive and that
/// exact SDK drops are not inferred from the transcript, and marks an arm with
/// no result as not run.
#[test]
fn the_report_reads_the_log_and_states_its_limits_for_every_arm() {
    let control = observation(&ARMS[0], &FULL);
    let mut lost = observation(&ARMS[4], &FULL[3..]);
    lost.dropped_bytes = 320;
    lost.quarantined_bytes = 4;
    let mut log = String::from("running 1 test\nLIVE_OBS not json\n");
    for item in [&control, &lost] {
        log.push_str(&format!(
            "LIVE_OBS {}\n",
            json!({"kind": "arm_result", "fields": item.to_json()})
        ));
    }
    log.push_str("LIVE_OBS {\"kind\":\"transcript_line\",\"fields\":{}}\nother\n");
    let parsed = parse_arm_results(&log);
    assert_eq!(parsed, [control.clone(), lost]);

    let report = render_report(&parsed);
    assert!(report.contains("One clip per arm is descriptive evidence only"));
    assert!(report.contains("exact SDK drops are not inferred from the transcript"));
    for arm in &ARMS {
        assert!(report.contains(&format!("ARM {} ", arm.id)), "{}", arm.id);
    }
    assert_eq!(report.matches("NOT RUN").count(), 3);
    assert!(report.contains("dropped_bytes=320 quarantined_bytes=4"));
    assert!(report.contains("t1:c01_say_plan/1: missing"));
    assert!(report.contains("totals: missing 3"));
    assert!(report.contains("Should I return the two positions"));
    let twice = render_report(&[control.clone(), control]);
    assert!(twice.contains("2 results for s0"));
}

// The dequeue gate.

/// A stream a test feeds by hand.
struct Fed(tokio::sync::mpsc::UnboundedReceiver<u32>);

impl futures_util::Stream for Fed {
    type Item = u32;
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<u32>> {
        self.0.poll_recv(context)
    }
}

/// Declared before running: with the gate held and frames waiting, the consumer
/// takes nothing for as long as it is held, while another task and a timer on
/// the same runtime keep running; on release it takes the frames in order, the
/// first transmission is stamped after the release, and the held arrival is
/// counted once. A consumer that was cancelled while held has taken nothing.
#[tokio::test]
async fn a_held_dequeue_takes_nothing_while_the_runtime_keeps_running() {
    let (feed, frames) = tokio::sync::mpsc::unbounded_channel();
    for n in 1..=5 {
        feed.send(n).unwrap();
    }
    let gate = Arc::new(DequeueGate::new(Instant::now()));
    gate.hold();
    let ticks = Arc::new(AtomicUsize::new(0));
    let ticker = tokio::spawn({
        let ticks = std::sync::Arc::clone(&ticks);
        async move {
            loop {
                tokio::time::sleep(Duration::from_millis(5)).await;
                ticks.fetch_add(1, Ordering::SeqCst);
            }
        }
    });
    let mut stream = Fed(frames);

    // A consumer cancelled while held takes nothing.
    {
        let cancelled = probe::gated_next(Some(gate.as_ref()), &mut stream);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), cancelled)
                .await
                .is_err()
        );
    }
    assert_eq!(gate.stats().frames, 0);

    let consumer = tokio::spawn({
        let gate = std::sync::Arc::clone(&gate);
        async move {
            let mut taken = Vec::new();
            while taken.len() < 5 {
                taken.push(
                    probe::gated_next(Some(gate.as_ref()), &mut stream)
                        .await
                        .unwrap(),
                );
            }
            taken
        }
    });
    let ticks_before = ticks.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        ticks.load(Ordering::SeqCst) >= ticks_before + 10,
        "the rest of the runtime ran while the consumer was held"
    );
    assert!(!consumer.is_finished());
    assert_eq!(gate.stats().frames, 0, "nothing was taken while held");
    assert_eq!(
        gate.stats().held_arrivals,
        2,
        "one per waiting consumer, not per wake"
    );
    assert!(gate.is_held());

    gate.release();
    let released = Instant::now();
    assert_eq!(consumer.await.unwrap(), [1, 2, 3, 4, 5]);
    ticker.abort();
    let stats = gate.stats();
    assert_eq!(stats.frames, 5);
    assert_eq!(stats.frames_after_release, 5);
    assert!(
        stats.first_after_release_at.unwrap() >= stats.released_at.unwrap(),
        "a frame is stamped after the release it waited for"
    );
    assert!(stats.released_at.unwrap() <= released);
    assert!(stats.held_at.unwrap() <= stats.released_at.unwrap());
    drop(feed);
}

/// Declared before running: with no gate, or an open one, the dequeue takes at
/// once and records the frames by second; hold and release are idempotent and
/// the per-second tally rebases to a new epoch.
#[tokio::test]
async fn an_open_gate_takes_at_once_and_tallies_by_second_from_its_epoch() {
    let (feed, frames) = tokio::sync::mpsc::unbounded_channel();
    let mut stream = Fed(frames);
    feed.send(1).unwrap();
    assert_eq!(probe::gated_next(None, &mut stream).await, Some(1));
    let gate = DequeueGate::new(Instant::now() - Duration::from_secs(3));
    feed.send(2).unwrap();
    assert_eq!(probe::gated_next(Some(&gate), &mut stream).await, Some(2));
    assert_eq!(gate.stats().held_arrivals, 0);
    assert_eq!(
        gate.stats().per_second.keys().copied().collect::<Vec<_>>(),
        [3]
    );
    gate.rebase(Instant::now() + Duration::from_secs(2));
    feed.send(3).unwrap();
    assert_eq!(probe::gated_next(Some(&gate), &mut stream).await, Some(3));
    assert_eq!(
        gate.stats().per_second.keys().copied().collect::<Vec<_>>(),
        [-2]
    );
    assert_eq!(gate.stats().first_after_release_at, None, "never released");
    gate.release();
    assert_eq!(
        gate.stats().released_at,
        None,
        "release of an open gate is nothing"
    );
    gate.hold();
    let held = gate.stats().held_at;
    gate.hold();
    assert_eq!(gate.stats().held_at, held);
    drop(feed);
    assert_eq!(probe::gated_next(None, &mut stream).await, None);
}

// The recording peer: the real SDK stream, the real pump and a fake Gemini.

/// What the fake Gemini socket received: when, and the audio's first sample.
struct WireAudio {
    at: Instant,
    marker: i16,
    bytes: usize,
}

fn recording_peer(
    listener: tokio::net::TcpListener,
    wire: Arc<std::sync::Mutex<Vec<WireAudio>>>,
) -> tokio::task::JoinHandle<()> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        while let Some(Ok(frame)) = socket.next().await {
            if frame.is_close() {
                break;
            }
            let Ok(text) = frame.to_text() else { continue };
            let value: Value = serde_json::from_str(text).unwrap();
            if let Some(data) = value["realtimeInput"]["audio"]["data"].as_str() {
                let bytes = STANDARD.decode(data).unwrap();
                wire.lock().unwrap().push(WireAudio {
                    at: Instant::now(),
                    marker: i16::from_le_bytes([bytes[0], bytes[1]]),
                    bytes: bytes.len(),
                });
            }
        }
    })
}

use super::super::{Rig, isolated, listener, numbered_frame};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

/// Declared before running, per row of the arm table in frames (captured in
/// one burst: the SDK's rule does not depend on pacing). The consumer is held
/// while the loop's other arms run; during the hold the fake Gemini receives no
/// audio, the room control and timer arms still run and the gate counted the
/// consumer arriving. After the release the wire carries one 320-byte message
/// per frame, in order, with the numbered samples intact: all of them when the
/// stall fits the 4000-frame store, and only the newest 4000 when it does not
/// (so frame N-3999 first), with the oldest dropped silently. The first
/// transmission is stamped after the release, and the control is never held.
#[tokio::test]
async fn the_recording_peer_sees_nothing_while_held_and_the_retained_frames_after() {
    if isolated(
        "live::continuity::the_recording_peer_sees_nothing_while_held_and_the_retained_frames_after",
    ) {
        return;
    }
    for (label, frames, hold) in [
        ("control", 2000i16, false),
        ("s20", 2000, true),
        ("s39", 3900, true),
        ("s41", 4100, true),
        ("s60", 6000, true),
    ] {
        assert!(overflow_log::install(), "{label}: another logger was first");
        overflow_log::CAPTURE.clear();
        let probe = ContinuityProbe::new();
        let _installed = probe::install(&probe);
        let (peer, endpoint) = listener().await;
        let wire = Arc::new(std::sync::Mutex::new(Vec::new()));
        let server = recording_peer(peer, std::sync::Arc::clone(&wire));
        let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
        let boot = crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45);
        let session = crate::gemini::live_session_with_keys_at(&endpoint, &rig.keys, &boot, None)
            .await
            .unwrap();
        let mut first = std::mem::replace(&mut rig.gemini, session);
        first.shutdown().await.unwrap();
        rig.old.take().unwrap().await.unwrap();

        let source = NativeAudioSource::new(AudioSourceOptions::default(), SAMPLE_RATE, 1, 0);
        let track = LocalAudioTrack::create_audio_track(
            "continuity-wire",
            RtcAudioSource::Native(source.clone()),
        );
        rig.media.audio = Some(crate::livekit::media::candidate_audio_stream(
            track.rtc_track(),
        ));
        if hold {
            probe.gate.hold();
        }
        let captured_from = Instant::now();
        for marker in 1..=frames {
            source
                .capture(&numbered_frame(marker, FRAME_SAMPLES).data)
                .await
                .unwrap();
        }
        let retained = frames.min(4000);
        let first_marker = frames - retained + 1;

        let mut controls = 0usize;
        let mut ticks = 0usize;
        let mut released_at = None;
        let mut ticker = tokio::time::interval(Duration::from_millis(10));
        let ended = tokio::time::Instant::now() + Duration::from_secs(20);
        let mut quiet_since: Option<tokio::time::Instant> = None;
        loop {
            tokio::select! {
                input = crate::livekit::media::next_candidate_input(
                    &mut rig.media.audio,
                    &mut rig.media.recovery_audio,
                ), if rig.media.audio.is_some() => {
                    let mut context = rig.turn.context(
                        &mut rig.output, &mut rig.gemini, &mut rig.board, &mut rig.media,
                    );
                    crate::livekit::on_candidate_input(&mut context, input, 1500).await;
                }
                _ = ticker.tick() => {
                    ticks += 1;

                    // Room controls and keepalive polling stand in as the arms
                    // that must keep running: a control is answered each tick.
                    controls += 1;
                    if hold && released_at.is_none() && ticks == 30 {
                        assert!(wire.lock().unwrap().is_empty(), "{label}: audio reached Gemini while held");
                        assert_eq!(probe.gate.stats().frames, 0, "{label}");
                        assert!(probe.gate.stats().held_arrivals >= 1, "{label}");
                        released_at = Some(Instant::now());
                        probe.gate.release();
                    }
                    if probe.gate.stats().frames == u64::from(retained.unsigned_abs()) {
                        let since = *quiet_since.get_or_insert_with(tokio::time::Instant::now);
                        if since.elapsed() > Duration::from_millis(300)
                            && wire.lock().unwrap().len() == usize::try_from(retained).unwrap()
                        {
                            break;
                        }
                    }
                    assert!(tokio::time::Instant::now() < ended, "{label}: never drained");
                }
            }
        }
        assert!(controls >= 30, "{label}: other arms kept running");
        rig.gemini.shutdown().await.unwrap();
        server.await.unwrap();

        let wire = wire.lock().unwrap();
        assert_eq!(
            wire.len(),
            usize::try_from(retained).unwrap(),
            "{label}: one message per retained frame"
        );
        assert!(wire.iter().all(|item| item.bytes == 320), "{label}");
        let markers: Vec<i16> = wire.iter().map(|item| item.marker).collect();
        assert_eq!(
            markers,
            (first_marker..=frames).collect::<Vec<i16>>(),
            "{label}: the newest frames, in order, none repeated"
        );
        let stats = probe.gate.stats();
        assert_eq!(stats.frames, u64::from(retained.unsigned_abs()), "{label}");

        // Declared: the SDK logs the queue's first drop and every 100th, so N =
        // 1, 100, 200, ... up to the drops; none when nothing dropped.
        let dropped = u64::from((frames - retained).unsigned_abs());
        let mut want: Vec<u64> = Vec::new();
        if dropped > 0 {
            want.push(1);
            want.extend((1..=dropped / 100).map(|step| step * 100));
        }
        let snapshot = overflow_log::snapshot();
        let seen: Vec<OverflowLog> = snapshot
            .records
            .iter()
            .map(|record| OverflowLog::at(record, captured_from))
            .collect();
        assert_eq!(
            seen.iter()
                .filter_map(|line| line.dropped)
                .collect::<Vec<_>>(),
            want,
            "{label}: the SDK's overflow lines"
        );
        assert_eq!(snapshot.unstored, 0, "{label}");
        for line in &seen {
            assert!(line.target.starts_with("libwebrtc"), "{label}: {line:?}");
            assert!(line.message.contains("native audio stream queue overflow"));
        }
        if hold {
            // The drops happen while the capture burst runs, before the hold is
            // released at tick 30.
            let released_ms = u64::try_from(
                released_at
                    .unwrap()
                    .saturating_duration_since(captured_from)
                    .as_millis(),
            )
            .unwrap();
            assert!(
                seen.iter().all(|line| line.at_ms <= released_ms as i64),
                "{label}: {seen:?} vs release at {released_ms} ms"
            );
        }
        if hold {
            let released_at = released_at.unwrap();
            assert!(
                wire[0].at >= released_at,
                "{label}: first transmission precedes the release"
            );
            assert!(stats.first_after_release_at.unwrap() >= released_at);
            assert_eq!(stats.frames_after_release, stats.frames, "{label}");
            assert!(stats.held_at.is_some());
        } else {
            assert_eq!(stats.held_arrivals, 0, "{label}: the control is never held");
            assert!(stats.held_at.is_none() && stats.released_at.is_none());
            assert!(wire[0].at >= captured_from);
        }
    }
}

/// Declared before running: the manifest writer, started as the launcher
/// wrapper starts it, writes for each arm a clip manifest the controller
/// accepts with its binary pin verified, naming the two planned actions and
/// the clip set's own digest.
#[test]
fn the_manifest_writer_produces_each_arms_clip_manifest() {
    let scratch = Scratch::new("arm-writer");
    let clips = write_clip_dir(&scratch, |_| {});
    let name = format!(
        "{}::live_write_manifest",
        module_path!()
            .split_once("::")
            .unwrap()
            .1
            .rsplit_once("::continuity")
            .unwrap()
            .0
    );
    for arm in &ARMS {
        let out = scratch.path(&format!("manifest-{}.json", arm.id));
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--nocapture"])
            .env("CODETRIAL_LIVE_MANIFEST_OUT", &out)
            .env("CODETRIAL_LIVE_SCENARIO", arm.id)
            .env("CODETRIAL_LIVE_CANDIDATE", std::env::current_exe().unwrap())
            .env("CODETRIAL_LIVE_CLIPS", &clips)
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{}",
            String::from_utf8_lossy(&child.stderr)
        );
        let manifest: Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(manifest["run_id"], json!(arm.run_id()));
        assert_eq!(manifest["actions"], json!(CLIP_SCENARIO.actions));
        assert_eq!(
            manifest["fixture_sha256"],
            json!(clips_fixture_sha(Some(clips.to_str().unwrap().to_owned())))
        );
        let checked = std::process::Command::new("python3")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/interview-run-budget.py"))
            .args(["validate", "--verify-binaries", "--manifest"])
            .arg(&out)
            .output()
            .unwrap();
        assert!(
            checked.status.success(),
            "{}: {}{}",
            arm.id,
            String::from_utf8_lossy(&checked.stdout),
            String::from_utf8_lossy(&checked.stderr)
        );
    }
}

fn clean_teardown() -> Teardown {
    Teardown {
        participants: Some(Vec::new()),
        room_gone_after_delete: true,
        readers_unfinished: 0,
        notes: Vec::new(),
    }
}

/// Declared before running: an arm that held its gate, saw the consumer reach
/// it, took frames after the release, published every planned frame and left a
/// clean room passes; each departure is named, the control's gate is never
/// held, and a transcript is never part of the verdict.
#[test]
fn the_arm_verdict_names_each_departure_and_ignores_the_transcript() {
    let arm = &ARMS[2];
    let mut good = observation(arm, &[]);
    good.frames_after_release = 5;
    assert!(judge_arm(arm, Some(&good), &clean_teardown(), &Ok(())).is_empty());
    let mut bad = good;
    bad.frames_published = 10;
    bad.complete = false;
    bad.held_arrivals = 0;
    bad.frames_after_release = 0;
    let problems = judge_arm(arm, Some(&bad), &clean_teardown(), &Ok(()));
    assert_eq!(problems.len(), 3, "{problems:?}");
    let dirty = Teardown {
        participants: Some(vec!["candidate-lives".into()]),
        room_gone_after_delete: false,
        readers_unfinished: 2,
        notes: Vec::new(),
    };
    let problems = judge_arm(
        arm,
        None,
        &dirty,
        &Err(ScriptError::Permit("denied".into())),
    );
    assert_eq!(problems.len(), 5, "{problems:?}");
    let mut control = observation(&ARMS[0], &FULL);
    assert!(judge_arm(&ARMS[0], Some(&control), &clean_teardown(), &Ok(())).is_empty());
    control.held_ms = Some(3);
    assert_eq!(
        judge_arm(&ARMS[0], Some(&control), &clean_teardown(), &Ok(())).len(),
        1
    );
}

/// Loads the spoken clips when `CODETRIAL_LIVE_CLIPS` names them. It reads
/// local files and nothing else: the check that the authored answer key is the
/// text of the real clips.
#[test]
fn the_spoken_clips_load_when_their_directory_is_named() {
    let Ok(dir) = std::env::var("CODETRIAL_LIVE_CLIPS") else {
        return;
    };
    let set = ClipSet::load(Path::new(&dir)).unwrap();
    let timeline = Timeline::new(&set);
    println!(
        "clips {:?} frames {} spans {:?}",
        CLIP_IDS,
        timeline.clip_frames(),
        timeline.spans
    );
    for arm in &ARMS {
        Schedule::new(arm, timeline.clip_frames())
            .check_budget()
            .unwrap();
    }
}

/// Declared before running: an arm that did not complete is inconclusive in
/// every finding, never a loss; a control that did not complete, or whose gate
/// was held, is no baseline, so a stalled arm's findings are inconclusive for
/// want of one.
#[test]
fn an_incomplete_arm_or_control_never_turns_a_miss_into_a_loss() {
    let mut cut = observation(&ARMS[1], &FULL[3..]);
    cut.complete = false;
    let reports = compare(&[observation(&ARMS[0], &FULL), cut]);
    assert_eq!(reports[1].findings(Kind::Missing), 0);
    assert_eq!(reports[1].inconclusive(), 3);
    assert_eq!(
        statuses(&reports[1])[0],
        [Status::Inconclusive(
            Kind::Missing,
            "the arm did not complete"
        )]
    );

    let whole = observation(&ARMS[1], &FULL[3..]);
    let mut bad_control = observation(&ARMS[0], &FULL);
    bad_control.complete = false;
    let reports = compare(&[bad_control.clone(), whole.clone()]);
    assert_eq!(reports[1].findings(Kind::Missing), 0);
    assert_eq!(reports[1].inconclusive(), 3);
    bad_control.complete = true;
    bad_control.held_ms = Some(0);
    assert_eq!(compare(&[bad_control, whole])[1].findings(Kind::Missing), 0);
}

// The SDK's overflow log, offline.

fn captured(at: Instant, target: &str, message: &str) -> overflow_log::Captured {
    overflow_log::Captured {
        at,
        target: target.to_owned(),
        message: message.to_owned(),
    }
}

fn feed(capture: &overflow_log::Capture, level: log::Level, target: &str, message: &str) {
    use log::Log as _;
    capture.log(
        &log::Record::builder()
            .level(level)
            .target(target)
            .args(format_args!("{message}"))
            .build(),
    );
}

const SDK_TARGET: &str = "libwebrtc::native::audio_stream";
const SDK_LINE: &str = "native audio stream queue overflow; dropped 100 queued frames";

/// Declared before running: records from the SDK's audio-stream targets that
/// mention overflow or dropped frames are kept in order with their time and
/// target; the video stream's line, an unrelated SDK line, an application
/// target and an info-level line are not; the store holds at most 64 lines,
/// each
/// target at most 64 characters and each message at most 160, and counts the
/// lines it could not store; a registered secret in a message is redacted.
#[test]
fn the_overflow_logger_keeps_only_bounded_audio_stream_overflow_lines() {
    let capture = overflow_log::Capture::new();
    feed(&capture, log::Level::Warn, SDK_TARGET, SDK_LINE);
    feed(
        &capture,
        log::Level::Warn,
        "livekit::rtc_engine",
        "audio frames dropped: 3",
    );
    for (level, target, message) in [
        (
            log::Level::Warn,
            "libwebrtc::native::video_stream",
            "native video stream queue overflow; dropped 100 queued frames",
        ),
        (log::Level::Warn, SDK_TARGET, "native audio stream started"),
        (log::Level::Warn, "codetrial::livekit", SDK_LINE),
        (log::Level::Info, SDK_TARGET, SDK_LINE),
    ] {
        feed(&capture, level, target, message);
    }
    let snapshot = capture.snapshot(true);
    assert_eq!(snapshot.records.len(), 2);
    assert_eq!(snapshot.records[0].target, SDK_TARGET);
    assert_eq!(snapshot.records[0].message, SDK_LINE);
    assert_eq!(snapshot.records[1].target, "livekit::rtc_engine");
    assert!(snapshot.records[0].at <= snapshot.records[1].at);
    assert_eq!(dropped_count(SDK_LINE), Some(100));
    assert_eq!(dropped_count("no count here"), None);

    // Bounded in number, target length and message length.
    let capture = overflow_log::Capture::new();
    let long = format!("{SDK_LINE} {}", "x".repeat(500));
    let long_target = format!("libwebrtc::{}", "t".repeat(200));
    for _ in 0..(overflow_log::MAX_RECORDS + 5) {
        feed(&capture, log::Level::Warn, &long_target, &long);
    }
    let snapshot = capture.snapshot(true);
    assert_eq!(snapshot.records.len(), overflow_log::MAX_RECORDS);
    assert_eq!(snapshot.unstored, 5);
    assert!(snapshot.records.iter().all(|r| {
        r.message.chars().count() <= overflow_log::MAX_MESSAGE
            && r.target.chars().count() <= overflow_log::MAX_TARGET
    }));
    capture.clear();
    assert!(capture.snapshot(true).records.is_empty());

    // A secret the process registered never reaches the record.
    register_secrets(["overflow-test-secret-value".to_owned()]);
    let capture = overflow_log::Capture::new();
    feed(
        &capture,
        log::Level::Warn,
        SDK_TARGET,
        "native audio stream queue overflow; dropped 1 queued frames overflow-test-secret-value",
    );
    let snapshot = capture.snapshot(true);
    assert!(
        !snapshot.records[0]
            .message
            .contains("overflow-test-secret-value")
    );
}

/// Declared before running: installing is idempotent and answers the same every
/// time; when this logger is the process's logger a `log::warn!` from an
/// SDK-shaped target reaches it through the `log` facade while a macro call
/// with another target does not. Other tests in the process never installed a
/// logger and are unaffected.
#[test]
fn installing_the_overflow_logger_twice_is_harmless_and_reaches_the_facade() {
    let first = overflow_log::install();
    assert_eq!(overflow_log::install(), first);
    assert_eq!(overflow_log::snapshot().installed, first);
    assert!(first, "no other test in this process installs a logger");
    {
        log::warn!(
            target: "libwebrtc::native::audio_stream",
            "native audio stream queue overflow; dropped 7777001 queued frames"
        );
        log::warn!(
            target: "codetrial::other",
            "native audio stream queue overflow; dropped 7777002 queued frames"
        );
        let seen: Vec<Option<u64>> = overflow_log::snapshot()
            .records
            .iter()
            .map(|r| dropped_count(&r.message))
            .collect();
        assert!(seen.contains(&Some(7_777_001)));
        assert!(!seen.contains(&Some(7_777_002)));
        overflow_log::CAPTURE.clear();
    }
}

/// Declared before running: a record is placed on the clip's clock, not the
/// probe's: with the clip beginning 300 ms after the probe and lines logged 500
/// and 900 ms after the probe, the observation says 200 and 600 ms and says the
/// logger was installed; the lines survive the arm_result round trip.
#[test]
fn overflow_lines_are_timestamped_from_the_clips_first_frame() {
    let probe = ContinuityProbe::new();
    let began = probe.epoch + Duration::from_millis(300);
    let arm = &ARMS[3];
    let schedule = Schedule::new(arm, 100);
    let published = Published {
        clip_frames: schedule.total_frames,
        began: Some(began),
        clips: Vec::new(),
        error: None,
    };
    let snapshot = overflow_log::Snapshot {
        installed: true,
        records: vec![
            captured(
                probe.epoch + Duration::from_millis(100),
                SDK_TARGET,
                SDK_LINE,
            ),
            captured(
                probe.epoch + Duration::from_millis(500),
                SDK_TARGET,
                SDK_LINE,
            ),
            captured(
                probe.epoch + Duration::from_millis(900),
                SDK_TARGET,
                "native audio stream queue overflow; dropped 200 queued frames",
            ),
        ],
        unstored: 0,
    };
    let built = Observation::build(arm, &schedule, &published, &probe, &snapshot);
    assert_eq!(
        built
            .sdk_overflow_logs
            .iter()
            .map(|line| (line.at_ms, line.dropped))
            .collect::<Vec<_>>(),
        [(-200, Some(100)), (200, Some(100)), (600, Some(200))]
    );
    assert!(built.sdk_overflow_logger);
    let line = format!(
        "LIVE_OBS {}\n",
        json!({"kind": "arm_result", "fields": built.to_json()})
    );
    assert_eq!(parse_arm_results(&line), [built]);
}

/// Declared before running: the report prints each overflow line with its time
/// and count and says a count within 99 and a time is all it is, not frame
/// identity; an arm with no line says the first drop always logs; an arm whose
/// logger was not installed concludes nothing; stored-line overflow is said.
#[test]
fn the_report_prints_overflow_lines_and_says_they_are_not_frame_identity() {
    let mut hit = observation(&ARMS[3], &FULL);
    hit.sdk_overflow_logs = vec![
        OverflowLog {
            at_ms: 4_100,
            dropped: Some(1),
            target: SDK_TARGET.into(),
            message: "native audio stream queue overflow; dropped 1 queued frames".into(),
        },
        OverflowLog {
            at_ms: 4_200,
            dropped: Some(100),
            target: SDK_TARGET.into(),
            message: SDK_LINE.into(),
        },
    ];
    hit.sdk_overflow_unstored = 3;
    let clear = observation(&ARMS[0], &FULL);
    let mut blind = observation(&ARMS[1], &FULL);
    blind.sdk_overflow_logger = false;
    let report = render_report(&[clear, blind, hit]);
    assert!(report.contains("4100 ms  dropped=Some(1)"));
    assert!(report.contains("4200 ms  dropped=Some(100)"));
    assert!(report.contains("highest count logged by any audio stream is Some(100)"));
    assert!(report.contains("a count within 99"));
    assert!(report.contains("frame identity is not known"));
    assert!(report.contains("not which frames were lost"));
    assert!(report.contains("no matching line was captured"));
    assert!(report.contains("was not the process's logger"));
    assert!(report.contains("3 further matching line(s) were not stored"));
    assert_eq!(report.matches("SDK overflow log (").count(), 3);
}

/// Declared before running: the real room-loop dequeue behind the
/// speech-continuity gate.
/// 100 frames are taken at once (so no gap above a few hundred ms), the gate
/// is then held for 1300 ms while 100 more are captured and the loop's timer
/// arm keeps running, and then released. The interview's measured maximum gap
/// is at least the hold and well under it plus a second of scheduling slack,
/// (a consumer parked on the stream may take the one frame that arrives right
/// after the hold starts, so the lower bound allows 100 ms) exactly one gap is
/// over 1 s, none over 10 s or 30 s, and the same figures
/// are on the next `interview clock:` line. A control with no hold measures
/// no gap over 1 s.
#[tokio::test]
async fn the_measured_microphone_gap_follows_the_held_dequeue_gate() {
    if isolated("live::continuity::the_measured_microphone_gap_follows_the_held_dequeue_gate") {
        return;
    }
    const HOLD: Duration = Duration::from_millis(1300);
    for (label, hold) in [("control", false), ("held", true)] {
        let probe = ContinuityProbe::new();
        let _installed = probe::install(&probe);
        let (peer, endpoint) = listener().await;
        let wire = Arc::new(std::sync::Mutex::new(Vec::new()));
        let server = recording_peer(peer, std::sync::Arc::clone(&wire));
        let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
        let boot = crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45);
        let session = crate::gemini::live_session_with_keys_at(&endpoint, &rig.keys, &boot, None)
            .await
            .unwrap();
        let mut first = std::mem::replace(&mut rig.gemini, session);
        first.shutdown().await.unwrap();
        rig.old.take().unwrap().await.unwrap();
        let began = Instant::now();
        rig.turn.activity.latency.anchor(began);

        let source = NativeAudioSource::new(AudioSourceOptions::default(), SAMPLE_RATE, 1, 0);
        let track = LocalAudioTrack::create_audio_track(
            "continuity-gap",
            RtcAudioSource::Native(source.clone()),
        );
        rig.media.audio = Some(crate::livekit::media::candidate_audio_stream(
            track.rtc_track(),
        ));
        for marker in 1..=100i16 {
            source
                .capture(&numbered_frame(marker, FRAME_SAMPLES).data)
                .await
                .unwrap();
        }

        let mut ticker = tokio::time::interval(Duration::from_millis(10));
        let ended = tokio::time::Instant::now() + Duration::from_secs(20);
        let mut held_since = None;
        let mut captured_more = false;
        let mut controls = 0usize;
        loop {
            tokio::select! {
                input = crate::livekit::media::next_candidate_input(
                    &mut rig.media.audio,
                    &mut rig.media.recovery_audio,
                ), if rig.media.audio.is_some() => {
                    let mut context = rig.turn.context(
                        &mut rig.output, &mut rig.gemini, &mut rig.board, &mut rig.media,
                    );
                    crate::livekit::on_candidate_input(&mut context, input, 1500).await;
                }
                _ = ticker.tick() => {
                    controls += 1;
                    if held_since.is_none() && probe.gate.stats().frames >= 100 {
                        if hold {
                            probe.gate.hold();
                        }
                        held_since = Some(Instant::now());
                    }
                    if let Some(since) = held_since {
                        if !captured_more {
                            captured_more = true;
                            for marker in 101..=200i16 {
                                source
                                    .capture(&numbered_frame(marker, FRAME_SAMPLES).data)
                                    .await
                                    .unwrap();
                            }
                        }
                        if hold && since.elapsed() >= HOLD && probe.gate.is_held() {
                            probe.gate.release();
                        }
                        if probe.gate.stats().frames >= 200 {
                            break;
                        }
                    }
                    assert!(tokio::time::Instant::now() < ended, "{label}: never drained");
                }
            }
        }
        assert!(!hold || controls >= 100, "{label}: other arms kept running");
        let gaps = rig.turn.activity.latency.mic_gaps().fields();
        let field = |line: &str, name: &str| -> String {
            line.split_whitespace()
                .find_map(|token| token.strip_prefix(&format!("{name}=")))
                .unwrap()
                .to_string()
        };
        let max: u64 = field(&gaps, "mic_gap_max_ms").parse().unwrap();
        if hold {
            assert!(
                (1200..=2300).contains(&max),
                "{label}: max gap {max} ms vs a 1300 ms hold"
            );
            assert_eq!(field(&gaps, "mic_gap_over_1s"), "1", "{label}");
        } else {
            assert!(max < 1000, "{label}: max gap {max} ms");
            assert_eq!(field(&gaps, "mic_gap_over_1s"), "0", "{label}");
        }
        assert_eq!(field(&gaps, "mic_gap_over_10s"), "0", "{label}");
        assert_eq!(field(&gaps, "mic_gap_over_30s"), "0", "{label}");

        // The same figures reach the next clock line, with no new sample
        // needed: the line a minute later carries them.
        let line = rig
            .turn
            .activity
            .latency
            .clock_sample(began + Duration::from_secs(61))
            .unwrap();
        assert_eq!(field(&line, "mic_gap_max_ms"), max.to_string(), "{label}");
        rig.gemini.shutdown().await.unwrap();
        server.await.unwrap();
    }
}

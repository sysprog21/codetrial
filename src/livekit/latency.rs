//! Per-reply server latency: one trace per reply, from the input that asked for
//! it to the first native audio capture, logged as `codetrial latency {JSON}`.
//!
//! Offsets are monotonic milestones from one origin rather than a sum of
//! provider stages, because Gemini Live's stages overlap and are not exposed.
//! Input that arrives while a reply is already under way belongs to the next
//! reply, so it waits in `pending` until the current one reaches a boundary
//! instead of moving the current origin or being lost with it.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Episodes remembered on one obligation. A reply that outlives more
/// replacements than this still reports the true count in
/// `obligation_episode_count`.
const EPISODES_KEPT: usize = 8;

/// Spacing of `interview clock:` samples on the assessment clock.
const CLOCK_SAMPLE_MS: u64 = 60_000;

/// Spacing of `runtime tasks:` samples on the assessment clock, the same
/// cadence as the harness's process resource samples.
const TASK_SAMPLE_MS: u64 = 30_000;

/// Version 1 never shipped; 2 keys turns by process and interview session, not
/// by socket, so a reply that survives a socket replacement stays one turn.
const SCHEMA_VERSION: u64 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Origin {
    /// The latest voiced input frame the room loop consumed.
    Voice,
    /// A platform prompt sent to the model.
    Prompt,
    /// A candidate transcript with no voiced frame before it.
    Transcript,
    /// Gemini's end of speech with no voiced frame before it.
    VoiceEnd,
    /// Output nothing local asked for; its offsets read near zero by
    /// construction and must not be pooled with the others.
    ProviderEvent,
}

impl Origin {
    fn label(self) -> &'static str {
        match self {
            Self::Voice => "last_voiced_pcm",
            Self::Prompt => "prompt",
            Self::Transcript => "transcript",
            Self::VoiceEnd => "provider_voice_end",
            Self::ProviderEvent => "provider_event",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Boundary {
    TurnComplete,
    Interrupted,
    /// A thinking hold cut the turn locally.
    Held,
    /// The socket was replaced; a reply not yet started keeps waiting.
    SocketReplaced,
}

impl Boundary {
    fn label(self) -> &'static str {
        match self {
            Self::TurnComplete => "turn_complete",
            Self::Interrupted => "interrupted",
            Self::Held => "held",
            Self::SocketReplaced => "socket_replaced",
        }
    }
}

#[derive(Clone, Copy)]
struct Milestones {
    voice_end: Option<Instant>,
    first_transcript: Option<Instant>,
    transcript: Option<Instant>,
    yielded: Option<Instant>,
    transcripts_allowed: bool,
}

struct Trace {
    turn: u64,
    origin: Origin,
    start: Instant,
    origin_socket: u64,

    // Until output starts, receipt timestamps can precede the last consumed
    // microphone frame. Keep earlier inputs to recover the matching origin.
    inputs: Vec<(Origin, Instant, u64)>,
    saved: Vec<(Instant, Milestones)>,
    voice_end: Option<Instant>,
    /// The first input transcript fragment, which shows whether the words
    /// arrive early enough to judge a finished sentence before END.
    first_transcript: Option<Instant>,
    transcript: Option<Instant>,
    /// The candidate's explicit "my turn is done", which ends the wait for
    /// the silence window.
    yielded: Option<Instant>,
    first_output: Option<Instant>,
    first_tool_call: Option<Instant>,
    /// A local acknowledgement queued while a tool round held the reply.
    tool_ack: Option<Instant>,
    last_tool_response: Option<Instant>,
    tool_batches: u64,
    tool_handler_ms: u64,
    audio_accepted: bool,
    /// The first audio the handler queued for native capture, and the socket
    /// that produced it.
    first_accept: Option<(Instant, u64)>,
    /// Replacement episodes that began while this reply was owed, oldest first
    /// and bounded; `episode_count` keeps the true number.
    episodes: Vec<u64>,
    episode_count: u64,
}

/// How an owed reply ended. `Open` is every record before its terminal one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Open,
    /// Native capture accepted audio for it and its turn then ended.
    Answered,
    /// Something local cut it off, or a barge-in ended its generation, before
    /// any audio was captured.
    Superseded,
    /// Its turn completed with no captured audio.
    Ended,
}

impl Outcome {
    fn label(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Answered => "answered",
            Self::Superseded => "superseded",
            Self::Ended => "ended",
        }
    }
}

/// Input for the next reply, held while the current one is still under way.
#[derive(Clone)]
struct Pending {
    first_transcript: Option<Instant>,
    transcript: Option<Instant>,
    transcripts_allowed: bool,
    inputs: Vec<(Origin, Instant, u64)>,
    saved: Vec<(Instant, Milestones)>,
    socket: u64,
    origin: Origin,
    at: Instant,
    voice_end: Option<Instant>,
    yielded: Option<Instant>,
}

#[derive(Default)]
pub(super) struct ReplyLatency {
    room: String,
    session: u64,
    socket: u64,
    /// The provider's configured end-of-speech silence, which the analyzer
    /// separates from the rest of the wait before END.
    silence_window_ms: u32,
    sequence: u64,
    /// Replacement episodes begun in this interview session, one per attempt to
    /// bring up a new socket whatever its result.
    episodes: u64,
    /// Sockets installed by a replacement.
    replacements: u64,
    /// `GoAway` advisories received since a replacement last took the count.
    advisories: u32,
    /// The interview's start, the one clock every numeric `interview_ms` reads.
    anchor: Option<Instant>,
    /// Wall-clock milliseconds at the anchor, `None` if the system clock reads
    /// before the epoch.
    start_unix_ms: Option<u64>,
    /// The last whole minute a clock sample was logged for.
    sampled: u64,
    /// The last whole 30 second period a task sample was logged for.
    task_sampled: u64,
    clock: Option<Instant>,
    voice_ended: bool,
    voice_started: bool,
    trace: Option<Trace>,
    pending: Option<Pending>,
    /// The prompt size of the last single-generation turn: the context a reply
    /// starts from, to see whether replies slow as a session grows.
    context_tokens: Option<u64>,
}

/// Signed, so a milestone observed before a later-moved origin reads as
/// negative rather than as missing.
fn offset_ms(at: Instant, start: Instant) -> i64 {
    match at.checked_duration_since(start) {
        Some(after) => i64::try_from(crate::agent::saturating_ms(after)).unwrap_or(i64::MAX),
        None => -i64::try_from(start.duration_since(at).as_millis()).unwrap_or(i64::MAX),
    }
}

/// Whether an input or milestone at `at` was received by `now`. Output splits
/// the trace at its receipt with this one rule for its inputs and on both sides
/// of its milestones, so one exactly at the split stays with the reply that
/// output began and the next reply gets only what came after: none is kept
/// twice and none is dropped.
fn received_by(at: Instant, now: Instant) -> bool {
    at <= now
}

/// How long a voiced frame stays a possible origin. A provider event is
/// received at most this far behind the microphone frames already consumed,
/// and past it a long utterance would hold one entry per frame.
const INPUT_HORIZON: Duration = Duration::from_secs(10);

/// Entries held before older voiced frames are pruned, so pruning runs once
/// per this many frames rather than on every one.
const INPUT_PRUNE_AT: usize = 2048;

/// Records an input, dropping voiced frames older than `INPUT_HORIZON` but the
/// newest of them, which still answers an event older than every kept frame.
fn push_input(inputs: &mut Vec<(Origin, Instant, u64)>, input: (Origin, Instant, u64)) {
    if inputs.len() >= INPUT_PRUNE_AT
        && let Some(cut) = input.1.checked_sub(INPUT_HORIZON)
    {
        let stale =
            |(origin, at, _): &(Origin, Instant, u64)| *origin == Origin::Voice && *at < cut;
        if let Some(keep) = inputs.iter().rposition(stale) {
            let mut index = 0;
            inputs.retain(|entry| {
                index += 1;
                index - 1 == keep || !stale(entry)
            });
        }
    }
    inputs.push(input);
}

/// The snapshot saved for an input at `at`: the earliest taken at or after
/// it, the latest pushed among equals.
fn nearest_saved(saved: &[(Instant, Milestones)], at: Instant) -> Option<usize> {
    saved
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, (saved_at, _))| *saved_at >= at)
        .min_by_key(|(_, (saved_at, _))| *saved_at)
        .map(|(index, _)| index)
}

fn historical_milestones<'a>(
    inputs: &[(Origin, Instant, u64)],
    saved: &'a mut [(Instant, Milestones)],
    now: Instant,
) -> Option<&'a mut Milestones> {
    let (_, input_at, _) = inputs.iter().rev().find(|(_, at, _)| *at <= now)?;
    let index = nearest_saved(saved, *input_at)?;
    Some(&mut saved[index].1)
}

impl Trace {
    /// Takes back the milestones saved for the input it now starts from.
    fn restore_saved(&mut self) {
        if let Some(index) = nearest_saved(&self.saved, self.start) {
            let saved = self.saved[index].1;
            self.voice_end = saved.voice_end;
            self.first_transcript = saved.first_transcript;
            self.transcript = saved.transcript;
            self.yielded = saved.yielded;
        }
    }
}

pub(super) fn log(record: &Value) {
    eprintln!("codetrial latency {record}");
}

impl ReplyLatency {
    pub(super) fn bind(&mut self, room: &str, session: u64, socket: u64, silence_window_ms: u32) {
        self.room = room.to_string();
        self.session = session;
        self.socket = socket;
        self.silence_window_ms = silence_window_ms;
    }

    pub(super) fn set_socket(&mut self, socket: u64) {
        self.socket = socket;
    }

    /// Milliseconds on the assessment clock, `None` before it is anchored.
    pub(super) fn interview_ms(&self, now: Instant) -> Option<i64> {
        self.anchor.map(|anchor| offset_ms(now, anchor))
    }

    /// Starts a replacement episode and ties it to the reply still owed, which
    /// keeps its origin, start and number across the swap. Returns the episode
    /// number and that reply's obligation, if any.
    pub(super) fn begin_episode(&mut self) -> (u64, Option<u64>) {
        self.episodes += 1;
        let episode = self.episodes;
        let obligation = self.trace.as_mut().and_then(|trace| {
            // Audio already accepted means the debt is paid: the swap that
            // follows ends an answered reply and owes it nothing.
            if trace.origin == Origin::ProviderEvent || trace.audio_accepted {
                return None;
            }
            trace.episode_count += 1;
            if trace.episodes.len() == EPISODES_KEPT {
                trace.episodes.remove(0);
            }
            trace.episodes.push(episode);
            Some(trace.turn)
        });
        (episode, obligation)
    }

    /// Anchors the clock and returns the line that says where it started.
    ///
    /// `start_unix_ms` is the wall clock at the interview's start, repeated on
    /// every sample so a log cut short or joined from two files still knows it.
    pub(super) fn anchor(&mut self, started: Instant) -> String {
        self.anchor = Some(started);
        self.start_unix_ms = std::time::SystemTime::now()
            .checked_sub(started.elapsed())
            .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
            .map(crate::agent::saturating_ms);
        self.sampled = 0;
        self.task_sampled = 0;
        self.clock_line(0, 0)
    }

    /// One bounded elapsed sample per whole minute of the assessment clock, at
    /// most. A tick late by several minutes yields one sample, not a burst.
    pub(super) fn clock_sample(&mut self, now: Instant) -> Option<String> {
        let elapsed = u64::try_from(self.interview_ms(now)?).ok()?;
        let minute = elapsed / CLOCK_SAMPLE_MS;
        if minute <= self.sampled {
            return None;
        }
        self.sampled = minute;
        Some(self.clock_line(elapsed, minute))
    }

    /// One `runtime tasks:` sample per whole 30 seconds of the assessment
    /// clock, at most. `alive` is the runtime's own count of tasks spawned and
    /// not yet finished, or `None` where no runtime answers; that is logged as
    /// `none`, never as zero. On a multithreaded runtime the count is
    /// approximate, and it does not replace joining a reader task.
    pub(super) fn task_sample(&mut self, now: Instant, alive: Option<usize>) -> Option<String> {
        let elapsed = u64::try_from(self.interview_ms(now)?).ok()?;
        let period = elapsed / TASK_SAMPLE_MS;
        if period <= self.task_sampled {
            return None;
        }
        self.task_sampled = period;
        Some(format!(
            "runtime tasks: pid={} session={} elapsed_ms={elapsed} sample={period} alive_tasks={} room={}",
            std::process::id(),
            self.session,
            alive.map_or_else(|| "none".to_string(), |count| count.to_string()),
            self.room
        ))
    }

    fn clock_line(&self, elapsed_ms: u64, sample: u64) -> String {
        format!(
            "interview clock: pid={} session={} start_unix_ms={} elapsed_ms={elapsed_ms} sample={sample} room={}",
            std::process::id(),
            self.session,
            self.start_unix_ms
                .map_or_else(|| "none".to_string(), |ms| ms.to_string()),
            self.room
        )
    }

    /// Counts a `GoAway` advisory and says whether to print it. `held` is
    /// whether an earlier advisory is still waiting on this socket: only then
    /// does this one repeat it. The first of a run prints and then each
    /// doubling, so a server repeating itself cannot flood the log; the raw
    /// count is kept for [`Self::take_advisories`].
    pub(super) fn note_advisory(&mut self, held: bool) -> Option<u32> {
        if !held {
            self.advisories = 0;
        }
        self.advisories = self.advisories.saturating_add(1);
        self.advisories.is_power_of_two().then_some(self.advisories)
    }

    /// The raw advisory count the replacement that starts now is spending.
    pub(super) fn take_advisories(&mut self) -> u32 {
        std::mem::take(&mut self.advisories)
    }

    /// Numbers a socket a replacement installed.
    pub(super) fn installed_replacement(&mut self) -> u64 {
        self.replacements += 1;
        self.replacements
    }

    /// The origin of the reply owed now, for the line that starts an episode.
    pub(super) fn owed_origin(&self) -> Option<&'static str> {
        self.trace
            .as_ref()
            .filter(|trace| trace.origin != Origin::ProviderEvent && !trace.audio_accepted)
            .map(|trace| trace.origin.label())
    }

    /// A usage report. One that includes tool rounds sums several
    /// invocations, so it says what was spent, not how large the context is.
    pub(super) fn observe_usage(&mut self, prompt: u64, tool_use_prompt: u64) {
        if prompt > 0 && tool_use_prompt == 0 {
            self.context_tokens = Some(prompt);
        }
    }

    /// A cold socket starts from the system instruction alone, so the last
    /// socket's size says nothing about it until its own first report.
    pub(super) fn forget_context(&mut self) {
        self.context_tokens = None;
    }

    fn open(&mut self, origin: Origin, start: Instant) -> &mut Trace {
        self.clock.get_or_insert(start);
        self.sequence += 1;
        self.trace.insert(Trace {
            turn: self.sequence,
            origin,
            start,
            origin_socket: self.socket,
            inputs: vec![(origin, start, self.socket)],
            saved: Vec::new(),
            voice_end: None,
            first_transcript: None,
            transcript: None,
            yielded: None,
            first_output: None,
            first_tool_call: None,
            tool_ack: None,
            last_tool_response: None,
            tool_batches: 0,
            tool_handler_ms: 0,
            audio_accepted: false,
            first_accept: None,
            episodes: Vec::new(),
            episode_count: 0,
        })
    }

    fn trace_or_open(&mut self, origin: Origin, now: Instant) -> &mut Trace {
        if self.trace.is_none() {
            return self.open(origin, now);
        }
        self.trace.as_mut().expect("checked above")
    }

    /// Input that asks for a reply. Until the reply starts, the latest input is
    /// what it answers; after, the input belongs to the next reply.
    fn input(&mut self, origin: Origin, now: Instant) {
        match self.trace.as_mut() {
            None => {
                self.open(origin, now);
            }
            Some(trace) if trace.first_output.is_some() => {
                let mut previous = self.pending.take();
                let same_voice = origin == Origin::Voice
                    && previous.as_ref().is_some_and(|p| p.origin == Origin::Voice);
                let transcripts_allowed = if same_voice {
                    previous.as_ref().is_some_and(|p| p.transcripts_allowed)
                } else {
                    self.voice_started && origin == Origin::Voice
                };
                let mut saved = previous
                    .as_mut()
                    .map(|p| std::mem::take(&mut p.saved))
                    .unwrap_or_default();
                if let Some(p) = previous.as_ref()
                    && !same_voice
                {
                    saved.push((
                        p.at,
                        Milestones {
                            voice_end: p.voice_end,
                            first_transcript: p.first_transcript,
                            transcript: p.transcript,
                            yielded: p.yielded,
                            transcripts_allowed: p.transcripts_allowed,
                        },
                    ));
                }
                let yielded = previous
                    .as_ref()
                    .filter(|_| same_voice)
                    .and_then(|p| p.yielded);
                let first_transcript = previous
                    .as_ref()
                    .filter(|_| same_voice)
                    .and_then(|p| p.first_transcript);
                let transcript = previous
                    .as_ref()
                    .filter(|_| same_voice)
                    .and_then(|p| p.transcript);
                let mut inputs = previous.map(|p| p.inputs).unwrap_or_default();
                push_input(&mut inputs, (origin, now, self.socket));
                self.pending = Some(Pending {
                    first_transcript,
                    transcript,
                    transcripts_allowed,
                    saved,
                    inputs,
                    socket: self.socket,
                    origin,
                    at: now,
                    voice_end: None,
                    yielded,
                });
            }

            Some(trace) => {
                if origin == Origin::Prompt || origin != trace.origin {
                    trace.saved.push((
                        trace.start,
                        Milestones {
                            voice_end: trace.voice_end,
                            first_transcript: trace.first_transcript,
                            transcript: trace.transcript,
                            yielded: trace.yielded,
                            transcripts_allowed: self.voice_started,
                        },
                    ));
                }
                push_input(&mut trace.inputs, (origin, now, self.socket));
                trace.origin = origin;
                trace.start = now;
                trace.origin_socket = self.socket;
                if origin == Origin::Prompt {
                    trace.voice_end = None;
                    trace.first_transcript = None;
                    trace.transcript = None;
                    trace.yielded = None;
                }
            }
        }
    }

    pub(super) fn voice(&mut self, now: Instant) {
        // A queued platform prompt changes attribution, not the provider's
        // judgement that trailing microphone frames are no longer speech.
        if !self.voice_ended {
            self.input(Origin::Voice, now);
        }
    }

    pub(super) fn prompt(&mut self, now: Instant) {
        self.input(Origin::Prompt, now);
    }

    /// A replacement briefing carries an existing reply debt, rather than
    /// asking a new question and restarting the candidate's wait.
    pub(super) fn recovery_prompt(&mut self, now: Instant) {
        if self.trace.is_none() {
            self.prompt(now);
        }
    }

    pub(super) fn voice_start(&mut self) {
        self.voice_ended = false;
        self.voice_started = true;
        match self.trace.as_mut() {
            // Words from before this utterance say nothing about how early its
            // own words arrive. A yield from before this utterance did not end
            // it.
            Some(trace) if trace.first_output.is_none() => {
                trace.saved.push((
                    trace.start,
                    Milestones {
                        voice_end: trace.voice_end,
                        first_transcript: trace.first_transcript,
                        transcript: trace.transcript,
                        yielded: trace.yielded,
                        transcripts_allowed: true,
                    },
                ));
                trace.voice_end = None;
                trace.first_transcript = None;
                trace.transcript = None;
                trace.yielded = None;
            }
            _ => {
                if let Some(pending) = self.pending.as_mut() {
                    pending.saved.push((
                        pending.at,
                        Milestones {
                            voice_end: pending.voice_end,
                            first_transcript: pending.first_transcript,
                            transcript: pending.transcript,
                            yielded: pending.yielded,
                            transcripts_allowed: pending.transcripts_allowed,
                        },
                    ));
                    pending.voice_end = None;
                    pending.yielded = None;
                    pending.first_transcript = None;
                    pending.transcript = None;
                    pending.transcripts_allowed = pending.origin == Origin::Voice;
                }
            }
        }
    }

    fn align_input(&mut self, now: Instant, output: bool) {
        let Some(trace) = self.trace.as_mut() else {
            return;
        };
        if trace.first_output.is_some() || trace.start <= now {
            return;
        }
        let index = trace
            .inputs
            .iter()
            .rposition(|(_, at, _)| received_by(*at, now));
        if !output {
            trace
                .inputs
                .retain(|(origin, at, _)| *at <= now || *origin == Origin::Prompt);
            if let Some(&(origin, at, socket)) = trace.inputs.last() {
                trace.origin = origin;
                trace.start = at;
                trace.origin_socket = socket;
                trace.restore_saved();
            } else {
                self.open(Origin::VoiceEnd, now);
            }
            return;
        }
        let later = trace.inputs.split_off(index.map_or(0, |i| i + 1));
        if let Some(&(origin, at, socket)) = later.last() {
            self.pending = Some(Pending {
                saved: trace.saved.clone(),
                inputs: later,
                origin,
                at,
                socket,
                voice_end: trace.voice_end.filter(|at| !received_by(*at, now)),
                yielded: trace.yielded.filter(|at| !received_by(*at, now)),
                first_transcript: trace.first_transcript.filter(|at| !received_by(*at, now)),
                transcript: trace.transcript.filter(|at| !received_by(*at, now)),
                transcripts_allowed: self.voice_started && origin == Origin::Voice,
            });
        }
        if let Some(index) = index {
            let (origin, at, socket) = trace.inputs[index];
            trace.origin = origin;
            trace.start = at;
            trace.origin_socket = socket;
            trace.restore_saved();
            trace.voice_end = trace.voice_end.filter(|at| received_by(*at, now));
            trace.yielded = trace.yielded.filter(|at| received_by(*at, now));
            trace.first_transcript = trace.first_transcript.filter(|at| received_by(*at, now));
            trace.transcript = trace.transcript.filter(|at| received_by(*at, now));
        } else {
            self.open(Origin::ProviderEvent, now);
        }
    }

    pub(super) fn voice_end(&mut self, now: Instant) {
        if let Some(trace) = self.trace.as_mut()
            && trace.first_output.is_none()
            && now < trace.start
            && let Some(saved) = historical_milestones(&trace.inputs, &mut trace.saved, now)
        {
            saved.voice_end = Some(now);
        }
        if let Some(pending) = self.pending.as_mut()
            && now < pending.at
            && let Some(saved) = historical_milestones(&pending.inputs, &mut pending.saved, now)
        {
            saved.voice_end = Some(now);
        }
        self.align_input(now, false);
        self.voice_ended = true;
        if let Some(pending) = self.pending.as_mut()
            && pending.at > now
        {
            pending
                .inputs
                .retain(|(origin, at, _)| *at <= now || *origin == Origin::Prompt);
            if let Some(&(origin, at, socket)) = pending.inputs.last() {
                pending.origin = origin;
                pending.at = at;
                pending.socket = socket;
                if let Some(index) = nearest_saved(&pending.saved, at) {
                    let saved = pending.saved[index].1;
                    pending.voice_end = saved.voice_end;
                    pending.first_transcript = saved.first_transcript;
                    pending.transcript = saved.transcript;
                    pending.yielded = saved.yielded;
                    pending.transcripts_allowed = saved.transcripts_allowed;
                }
                if origin == Origin::Prompt {
                    pending.yielded = None;
                    pending.first_transcript = None;
                    pending.transcript = None;
                }
            }
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.inputs.is_empty())
        {
            self.pending = None;
        }
        match self.trace.as_mut() {
            Some(trace) if trace.first_output.is_some() => {
                if let Some(pending) = self.pending.as_mut()
                    && pending.origin == Origin::Voice
                    && pending.at <= now
                {
                    pending.voice_end = Some(now);
                } else {
                    trace.voice_end.get_or_insert(now);
                }
            }
            Some(trace) if trace.start <= now => trace.voice_end = Some(now),
            Some(_) => {}
            None => self.open(Origin::VoiceEnd, now).voice_end = Some(now),
        }
    }

    pub(super) fn transcript(&mut self, now: Instant) {
        if let Some(trace) = self.trace.as_mut()
            && trace.first_output.is_none()
            && now < trace.start
            && let Some(saved) = historical_milestones(&trace.inputs, &mut trace.saved, now)
        {
            saved.first_transcript.get_or_insert(now);
            saved.transcript = Some(now);
            return;
        }
        if let Some(pending) = self.pending.as_mut()
            && now < pending.at
            && let Some(saved) = historical_milestones(&pending.inputs, &mut pending.saved, now)
            && saved.transcripts_allowed
        {
            saved.first_transcript.get_or_insert(now);
            saved.transcript = Some(now);
            return;
        }
        match self.trace.as_mut() {
            // Transcription can trail output for the same utterance. It is not
            // evidence of another reply without new input already queued.
            Some(trace) if trace.first_output.is_some() => {
                if let Some(pending) = self.pending.as_mut()
                    && pending.origin == Origin::Voice
                    && pending.transcripts_allowed
                {
                    pending.first_transcript.get_or_insert(now);
                    pending.transcript = Some(now);
                }
            }
            Some(trace) => {
                trace.first_transcript.get_or_insert(now);
                trace.transcript = Some(now);
            }
            None => {
                let trace = self.open(Origin::Transcript, now);
                trace.first_transcript = Some(now);
                trace.transcript = Some(now);
            }
        }
    }

    /// A yield ends the wait for the candidate input, including input queued
    /// behind a reply already under way.
    pub(super) fn yielded(&mut self, now: Instant) {
        self.voice_ended = true;
        if let Some(trace) = self.trace.as_mut()
            && trace.first_output.is_none()
        {
            trace.yielded = Some(now);
        } else if let Some(pending) = self.pending.as_mut()
            && pending.origin == Origin::Voice
        {
            pending.yielded = Some(now);
        }
    }

    /// The first substantive output of a generation: a tool call, an output
    /// transcript or audio.
    pub(super) fn output(&mut self, now: Instant, tool_call: bool) {
        self.align_input(now, true);
        if self
            .trace
            .as_ref()
            .is_none_or(|trace| trace.first_output.is_none())
        {
            self.voice_started = false;
        }
        let trace = self.trace_or_open(Origin::ProviderEvent, now);
        trace.inputs.clear();
        trace.first_output.get_or_insert(now);
        if tool_call {
            trace.first_tool_call.get_or_insert(now);
        }
    }

    /// The candidate hears something before the reply itself: the first
    /// sound, which the reply's own audio record still measures past.
    pub(super) fn tool_ack(&mut self, now: Instant) {
        if let Some(trace) = self.trace.as_mut()
            && !trace.audio_accepted
        {
            trace.tool_ack.get_or_insert(now);
        }
    }

    /// A batch that failed to send is recorded but does not move the
    /// continuation's start: nothing reached the model for it to resume on.
    pub(super) fn tool_response(&mut self, now: Instant, handler_ms: u64, sent: bool) -> Value {
        let trace = self.trace_or_open(Origin::ProviderEvent, now);
        if sent {
            trace.last_tool_response = Some(now);
            trace.tool_batches += 1;
            trace.tool_handler_ms = trace.tool_handler_ms.saturating_add(handler_ms);
        }
        self.record("tool_response", now, Outcome::Open)
    }

    /// The first accepted audio chunk of this reply, or `None` after it.
    pub(super) fn audio(&mut self, now: Instant) -> Option<Value> {
        if self
            .trace
            .as_ref()
            .is_some_and(|trace| trace.audio_accepted)
        {
            return None;
        }
        self.output(now, false);
        let trace = self.trace.as_mut()?;
        if std::mem::replace(&mut trace.audio_accepted, true) {
            return None;
        }
        trace.first_accept = Some((now, self.socket));
        Some(self.record("audio_accepted", now, Outcome::Open))
    }

    pub(super) fn boundary(&mut self, boundary: Boundary, now: Instant) -> Option<Value> {
        if matches!(boundary, Boundary::Held) {
            self.voice_ended = false;
            self.voice_started = false;
        }
        let trace = self.trace.as_ref()?;
        let started = trace.first_output.is_some();
        let accepted = trace.audio_accepted;
        if !started
            && now < trace.start
            && matches!(boundary, Boundary::TurnComplete | Boundary::Interrupted)
        {
            return None;
        }
        match boundary {
            // A barge-in can reach the microphone before the old generation's
            // `Interrupted`; the input it started keeps its clock.
            Boundary::Interrupted if !started => return None,

            // The reply this input is owed now comes from the new socket, and
            // the stall belongs in its total.
            Boundary::SocketReplaced if !accepted => {
                let mut record = self.record(boundary.label(), now, Outcome::Open);
                record["continuing"] = true.into();
                return Some(record);
            }
            _ => {}
        }
        let outcome = match boundary {
            _ if accepted => Outcome::Answered,
            Boundary::TurnComplete => Outcome::Ended,
            _ => Outcome::Superseded,
        };
        let mut record = self.record(boundary.label(), now, outcome);
        if boundary == Boundary::SocketReplaced {
            record["continuing"] = false.into();
        }
        self.trace = None;
        let pending = self.pending.take();
        if matches!(boundary, Boundary::Held) {
            return Some(record);
        }
        if pending.is_none() {
            self.voice_ended = false;
        }
        if let Some(pending) = pending {
            let trace = self.open(pending.origin, pending.at);
            trace.voice_end = pending.voice_end;
            trace.yielded = pending.yielded;
            trace.first_transcript = pending.first_transcript;
            trace.transcript = pending.transcript;
            trace.origin_socket = pending.socket;
            trace.inputs = pending.inputs;
            trace.saved = pending.saved;
        }
        Some(record)
    }

    /// Whether a reply is being traced, so padding that only finishes an
    /// earlier reply's audio can stay out of a trace it would otherwise open.
    pub(super) fn is_open(&self) -> bool {
        self.trace.is_some()
    }

    /// Ends the trace at a boundary nothing else needs the record of, logging
    /// it.
    pub(super) fn close(&mut self, boundary: Boundary, now: Instant) {
        if let Some(record) = self.boundary(boundary, now) {
            log(&record);
        }
    }

    /// Every caller has just made sure a trace exists.
    fn record(&self, stage: &str, now: Instant, outcome: Outcome) -> Value {
        let trace = self.trace.as_ref().expect("a trace to record");
        let at = |milestone: Option<Instant>| milestone.map(|at| offset_ms(at, trace.start));
        let owed = trace.origin != Origin::ProviderEvent;
        json!({
            "event": "reply_latency", "version": SCHEMA_VERSION,
            "pid": std::process::id(), "room": self.room,
            "session": self.session, "socket": self.socket,
            "origin_socket": trace.origin_socket, "turn": trace.turn,
            "stage": stage, "origin": trace.origin.label(),
            "silence_window_ms": self.silence_window_ms,
            "elapsed_ms": offset_ms(now, trace.start),
            "observed_ms": offset_ms(now, self.clock.expect("clock opened with trace")),
            "provider_voice_end_ms": at(trace.voice_end),
            "first_transcript_ms": at(trace.first_transcript),
            "latest_transcript_ms": at(trace.transcript),
            "candidate_yield_ms": at(trace.yielded),
            "first_provider_output_ms": at(trace.first_output),
            "first_tool_call_ms": at(trace.first_tool_call),
            "tool_ack_ms": at(trace.tool_ack),
            "last_tool_response_ms": at(trace.last_tool_response),
            "tool_batches": trace.tool_batches,
            "tool_handler_ms": trace.tool_handler_ms,
            "context_tokens": self.context_tokens,
            "interview_ms": self.interview_ms(now),
            "obligation": owed.then_some(trace.turn),
            "obligation_outcome": owed.then(|| outcome.label()),
            "owed_start_interview_ms": self.interview_ms(trace.start).filter(|_| owed),
            "obligation_episodes": if owed { trace.episodes.clone() } else { Vec::new() },
            "obligation_episode_count": if owed { trace.episode_count } else { 0 },
            "first_accept_ms": trace.first_accept.map(|(at, _)| offset_ms(at, trace.start)),
            "first_accept_socket": trace.first_accept.map(|(_, socket)| socket),
        })
    }
}

/// What identifies one replacement episode on its lines. Episodes are numbered
/// from 1 in order within one `(pid, room, session)`; two lines of one episode
/// carry the same `episode`, and the same number from another scope is another
/// episode.
pub(super) struct EpisodeStart<'a> {
    pub(super) room: &'a str,
    pub(super) session: u64,
    pub(super) episode: u64,
    pub(super) trigger: &'a str,
    pub(super) old_socket: u64,
    /// The reply owed when the episode began, kept across the swap.
    pub(super) obligation: Option<u64>,
    pub(super) obligation_origin: Option<&'a str>,
    /// Advisories received on the replaced socket, before coalescing.
    pub(super) advisories: u32,
    pub(super) interview_ms: Option<i64>,
}

fn opt<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "none".to_string(), |value| value.to_string())
}

/// `replacement start:` -- logged once as an episode begins, so a log cut
/// short before its `replacement end:` shows an unresolved episode and not
/// nothing.
pub(super) fn episode_start_line(start: &EpisodeStart<'_>) -> String {
    format!(
        "replacement start: pid={} session={} episode={} trigger={} old_socket={} obligation={} obligation_origin={} advisories={} interview_ms={} room={}",
        std::process::id(),
        start.session,
        start.episode,
        start.trigger,
        start.old_socket,
        opt(start.obligation),
        start.obligation_origin.unwrap_or("none"),
        start.advisories,
        opt(start.interview_ms),
        start.room
    )
}

/// How an episode ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EpisodeResult {
    /// The new socket was installed.
    Installed,
    /// Setup was tried and no socket was installed.
    Failed,
    /// No setup was tried: the restart budget or billing refused first.
    NoSetup,
    /// A terminal intent (end, deadline, absence, room closed) took over.
    Cancelled,
}

impl EpisodeResult {
    fn label(self) -> &'static str {
        match self {
            Self::Installed => "installed",
            Self::Failed => "failed",
            Self::NoSetup => "no_setup",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Milestones of one episode, each in milliseconds from the moment the close
/// was handled (the same start as `rotation_gap_ms`). They are separate on
/// purpose: `connected_ms` ends when the provider finished setup, and the
/// reconnect notice before it, the opener after it and the installation each
/// have their own, so notice delay is not read as provider setup.
#[derive(Default, Clone, Copy)]
pub(super) struct EpisodeMilestones {
    /// The "reconnecting" notice returned.
    pub(super) notice_ms: Option<u64>,
    /// Physical setup took this long: from opening the replacement connection
    /// to its setup completing. The notice and the old socket's close are
    /// outside it.
    pub(super) setup_ms: Option<u64>,
    /// Setup completed and was counted; equal to `rotation_gap_ms`.
    pub(super) connected_ms: Option<u64>,
    /// The briefing, board resend and queued work were written and the opener
    /// returned.
    pub(super) opener_ms: Option<u64>,
    /// The new socket was installed.
    pub(super) install_ms: Option<u64>,
}

pub(super) struct EpisodeEnd<'a> {
    pub(super) start: EpisodeStart<'a>,
    pub(super) result: EpisodeResult,
    /// The socket number the installed replacement has.
    pub(super) new_socket: Option<u64>,
    pub(super) replacement: Option<u64>,
    /// The consecutive restart attempt the episode reached.
    pub(super) attempt: usize,
    pub(super) resume_tried: bool,
    pub(super) milestones: EpisodeMilestones,
    /// The assessment clock when the episode ended.
    pub(super) interview_ms: Option<i64>,
}

/// `replacement end:` -- the one terminal line of an episode.
pub(super) fn episode_end_line(end: &EpisodeEnd<'_>) -> String {
    let m = &end.milestones;
    format!(
        "replacement end: pid={} session={} episode={} result={} trigger={} old_socket={} new_socket={} replacement={} attempt={} resume_tried={} obligation={} notice_ms={} setup_ms={} connected_ms={} opener_ms={} install_ms={} interview_ms={} room={}",
        std::process::id(),
        end.start.session,
        end.start.episode,
        end.result.label(),
        end.start.trigger,
        end.start.old_socket,
        opt(end.new_socket),
        opt(end.replacement),
        end.attempt,
        end.resume_tried,
        opt(end.start.obligation),
        opt(m.notice_ms),
        opt(m.setup_ms),
        opt(m.connected_ms),
        opt(m.opener_ms),
        opt(m.install_ms),
        opt(end.interview_ms),
        end.start.room
    )
}

/// Why a socket's pressure summary was written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PressureEnd {
    /// The socket was replaced by a new one.
    Replacement,
    /// The interview ended with this socket installed.
    Terminal,
    /// A socket set up for a replacement was dropped before it was installed.
    Discarded,
}

impl PressureEnd {
    fn label(self) -> &'static str {
        match self {
            Self::Replacement => "replacement",
            Self::Terminal => "terminal",
            Self::Discarded => "discarded",
        }
    }
}

/// `socket_pressure` -- the one summary of how full a socket's event channel
/// got and how long its reader waited for a slot. Written when the socket is
/// let go, never per frame, and made only of counts and durations. The wait is
/// time blocked on the channel; a reader that ended on the 45 second read
/// limit says so in `reader_end` and has no blocked time for it.
pub(super) fn pressure_record(
    room: &str,
    session: u64,
    socket: u64,
    end: PressureEnd,
    interview_ms: Option<i64>,
    pressure: &crate::gemini::SocketPressure,
) -> Value {
    let ms = crate::agent::saturating_ms;
    json!({
        "event": "socket_pressure", "version": SCHEMA_VERSION,
        "pid": std::process::id(), "room": room,
        "session": session, "socket": socket,
        "end": end.label(), "reader_end": pressure.end.label(),
        "interview_ms": interview_ms,
        "capacity": pressure.capacity,
        "occupancy_high_water": pressure.high_water,
        "blocked_sends": pressure.blocked_sends,
        "blocked_ms_total": ms(pressure.blocked_total),
        "blocked_ms_max": ms(pressure.blocked_max),
        "received": pressure.received,
        "consumed": pressure.consumed,
    })
}

#[cfg(test)]
#[path = "../../tests/unit/livekit/latency.rs"]
mod tests;

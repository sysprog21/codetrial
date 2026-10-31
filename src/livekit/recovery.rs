//! Room-owned transport waits. No mutable interview state crosses into the
//! pending operation; terminal intents run the report only after its cleanup.

use super::*;
use ::livekit::prelude::RemoteTrack;
use ::livekit::webrtc::audio_frame::AudioFrame;
use std::collections::VecDeque;
use std::future::Future;

pub(super) type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug)]
pub(super) enum Terminal {
    End {
        topic: String,
        payload: serde_json::Value,
    },
    Finish(String),
    Deadline,
    Absent,
    RoomClosed,
}

#[derive(Debug)]
pub(super) struct Cancelled;

pub(super) struct Packet {
    pub(super) topic: Option<String>,
    pub(super) sender: Option<String>,
    pub(super) payload: Vec<u8>,
}

const INGRESS_FRAMES: usize = 4000;
const INGRESS_BYTES: usize = 40 * 16_000 * 2;
const INGRESS_AGE: Duration = Duration::from_secs(40);

#[derive(Default)]
pub(super) struct Ingress {
    frames: VecDeque<(Instant, CandidateInput)>,
    bytes: usize,
    leading_yield: bool,
    pub(super) dropped_bytes: usize,
}

impl Ingress {
    pub(super) fn is_empty(&self) -> bool {
        self.frames.is_empty() && !self.leading_yield
    }

    fn has_audio(&mut self) -> bool {
        self.expire(Instant::now());
        self.bytes != 0
    }

    fn discard_front(&mut self) {
        if let Some((_, frame)) = self.frames.pop_front() {
            let bytes = match frame {
                CandidateInput::BufferedFrame(frame) => frame.data.len().saturating_mul(2),
                CandidateInput::Yield => {
                    self.leading_yield = true;
                    0
                }
                _ => 0,
            };
            self.bytes -= bytes;
            self.dropped_bytes = self.dropped_bytes.saturating_add(bytes);
        }
    }

    fn expire(&mut self, now: Instant) {
        while self
            .frames
            .front()
            .is_some_and(|(at, _)| now.saturating_duration_since(*at) >= INGRESS_AGE)
        {
            self.discard_front();
        }
    }

    pub(super) fn push(&mut self, frame: AudioFrame<'static>, now: Instant) {
        self.expire(now);
        let bytes = frame.data.len().saturating_mul(2);
        if bytes == 0 {
            return;
        }
        if bytes > INGRESS_BYTES {
            self.dropped_bytes = self.dropped_bytes.saturating_add(bytes);
            return;
        }
        while self.frames.len() >= INGRESS_FRAMES || self.bytes + bytes > INGRESS_BYTES {
            self.discard_front();
        }
        self.bytes += bytes;
        self.frames
            .push_back((now, CandidateInput::BufferedFrame(frame)));
    }

    pub(super) fn pop(&mut self, now: Instant) -> Option<CandidateInput> {
        self.expire(now);
        if std::mem::take(&mut self.leading_yield) {
            return Some(CandidateInput::Yield);
        }
        let (_, input) = self.frames.pop_front()?;
        if let CandidateInput::BufferedFrame(frame) = &input {
            self.bytes -= frame.data.len().saturating_mul(2);
        }
        Some(input)
    }

    pub(super) fn yielded(&mut self, now: Instant) {
        self.expire(now);
        if (self.leading_yield && self.frames.is_empty())
            || self
                .frames
                .back()
                .is_some_and(|(_, item)| matches!(item, CandidateInput::Yield))
        {
            return;
        }
        while self.frames.len() >= INGRESS_FRAMES {
            self.discard_front();
        }
        self.frames.push_back((now, CandidateInput::Yield));
    }
}

struct Action {
    sequence: u64,
    text: String,
    earlier: Vec<&'static str>,
}

/// A test-only barrier after input preparation and before transport polling.
#[cfg(test)]
pub(super) struct WriteBarrier {
    pub(super) phase: &'static str,
    pub(super) entered: std::sync::Arc<tokio::sync::Notify>,
    pub(super) release: std::sync::Arc<tokio::sync::Notify>,
}

/// A test-only action run in the poll that completes a labelled write, before
/// the driver can observe any later input.
#[cfg(test)]
pub(super) struct AfterWrite {
    pub(super) phase: &'static str,
    pub(super) action: Box<dyn FnOnce() + Send>,
}

pub(super) struct Driver<'r, 'c, 'i> {
    pub(super) room: &'r RoomLink,
    pub(super) context: &'r mut GeminiEventContext<'c>,
    pub(super) loops: &'r mut RoomLoop,
    pub(super) interview: InterviewContext<'i>,
    pub(super) ingress: Ingress,
    pub(super) revision: u64,
    pub(super) terminal: Option<Terminal>,
    held_contexts: VecDeque<String>,
    held_bytes: usize,
    held_omitted: usize,
    held_omitted_total: usize,
    held_steps: Vec<&'static str>,
    action: Option<Action>,
    last_action_toggle: bool,
    obligation_revision: u64,
    evictions_omitted: usize,
    failed_thinking: Option<bool>,
    failed_framework: Option<Vec<&'static str>>,
    round: Option<&'static str>,
    evictions: VecDeque<String>,
    tick: tokio::time::Interval,
    agent: String,
}

impl<'r, 'c, 'i> Driver<'r, 'c, 'i> {
    pub(super) fn new(
        room: &'r RoomLink,
        context: &'r mut GeminiEventContext<'c>,
        loops: &'r mut RoomLoop,
        interview: InterviewContext<'i>,
    ) -> Self {
        let ingress = std::mem::take(&mut context.media.recovery_audio);
        Self {
            room,
            context,
            loops,
            interview,
            ingress,
            revision: 0,
            terminal: None,
            held_contexts: VecDeque::new(),
            held_bytes: 0,
            held_omitted: 0,
            held_omitted_total: 0,
            held_steps: Vec::new(),
            action: None,
            last_action_toggle: false,
            obligation_revision: 0,
            evictions_omitted: 0,
            failed_thinking: None,
            failed_framework: None,
            round: None,
            #[cfg(test)]
            evictions: room.take_eviction_seed().into(),
            #[cfg(not(test))]
            evictions: VecDeque::new(),
            tick: tokio::time::interval(Duration::from_secs_f64(WATCH_TICK_S)),
            agent: agent_identity(room.name()),
        }
    }

    fn packet(&mut self, topic: Option<&str>, sender: Option<&str>, payload: &[u8]) {
        let Some((topic, payload)) =
            interview_packet(topic, sender, self.interview.candidate_identity, payload)
        else {
            return;
        };
        if control_type(topic, &payload) == Some("end_interview") {
            self.terminal = Some(Terminal::End {
                topic: topic.into(),
                payload,
            });
            return;
        }
        if packets::park_round_transition(self.context, self.interview, topic, &payload, true) {
            self.revision = self.revision.saturating_add(1);
            return;
        }
        let code_revision = self.context.state.evidence_ledger.code.revision;
        let language = self.context.state.language.clone();
        let held_debt = self.context.state.owed_reply_on_resume.clone();
        let result = packets::reduce_recovery_packet(
            self.room,
            self.context,
            self.interview,
            topic,
            &payload,
        );
        if result.thinking_changed == Some(true) && self.last_action_toggle {
            self.context.state.owed_reply_on_resume = held_debt.or(Some(None));
        }
        if result != crate::agent::DataEventResult::default()
            || code_revision != self.context.state.evidence_ledger.code.revision
            || language != self.context.state.language
        {
            self.revision = self.revision.saturating_add(1);
        }
        if result.yield_turn || result.thinking_changed.is_some() {
            self.ingress.yielded(Instant::now());
        }
        let mut earlier = if result.generate_reply.is_some() || result.held_context.is_some() {
            std::mem::take(&mut self.context.state.earlier_steps_pending)
        } else {
            Vec::new()
        };
        if result.thinking_changed.is_some()
            || result.generate_reply.is_some()
            || result.held_context.is_some()
        {
            self.obligation_revision = self.obligation_revision.saturating_add(1);
        }
        if result.generate_reply.is_some() {
            self.last_action_toggle = control_type(topic, &payload) == Some("thinking");
        }
        if let Some(prompt) = &result.generate_reply {
            if let Some(old) = self.action.take() {
                for step in old.earlier {
                    if !earlier.contains(&step) {
                        earlier.push(step);
                    }
                }
            }
            let text = format!("{prompt}{}", earlier_references(&earlier));
            self.context.activity.prompt_text = Some(text.clone());
            self.action = Some(Action {
                sequence: self.context.activity.prompt_sequence,
                text,
                earlier: earlier.clone(),
            });
        }
        if let Some(round) = result.round_changed {
            self.round = Some(round);
        }
        if let Some(context) = result.held_context {
            for step in earlier {
                if !self.held_steps.contains(&step) {
                    self.held_steps.push(step);
                }
            }
            if context.len() > 64 * 1024 {
                self.held_omitted += 1;
                self.held_omitted_total += 1;
            } else {
                while self.held_contexts.len() >= 32 || self.held_bytes + context.len() > 64 * 1024
                {
                    if let Some(old) = self.held_contexts.pop_front() {
                        self.held_bytes -= old.len();
                        self.held_omitted += 1;
                        self.held_omitted_total += 1;
                    }
                }
                self.held_bytes += context.len();
                self.held_contexts.push_back(context);
            }
        }
        if let Some(reason) = result.finish_interview {
            self.terminal = Some(Terminal::Finish(reason));
        }
    }

    fn preserve_room_work(&mut self) {
        if let Some(round) = self.round.take() {
            self.loops.pending_round_notice = Some(round);
        }
        self.evictions_omitted += self.evictions.len();
        self.evictions.clear();
    }

    fn restore_steps(&mut self, steps: impl IntoIterator<Item = &'static str>) {
        for step in steps {
            if !self.context.state.earlier_steps_pending.contains(&step) {
                self.context.state.earlier_steps_pending.push(step);
            }
        }
    }

    fn event(&mut self, event: RoomEvent) {
        if handle_board_event(
            self.context.state.interview_mode,
            self.context.state.example_drawing_enabled,
            self.interview.candidate_identity,
            self.context.board,
            &event,
        ) {
            return;
        }
        match event {
            RoomEvent::DataReceived {
                payload,
                topic,
                participant,
                ..
            } => {
                let sender = participant.as_ref().map(|p| p.identity().0);
                self.packet(topic.as_deref(), sender.as_deref(), &payload);
            }
            RoomEvent::ParticipantDisconnected(p)
                if p.identity().0 == self.interview.candidate_identity =>
            {
                self.loops.presence.left(Instant::now());
            }
            RoomEvent::ParticipantConnected(p)
                if p.identity().0 == self.interview.candidate_identity =>
            {
                self.loops.presence.returned();
                self.context.state.announce_thinking();
                self.context.state.framework_published.clear();
            }
            RoomEvent::ParticipantConnected(p) if is_duplicate_agent(&p, &self.agent) => {
                let identity = p.identity().0;
                if !self.evictions.contains(&identity) && self.evictions.len() < 32 {
                    self.evictions.push_back(identity);
                } else if !self.evictions.contains(&identity) {
                    self.evictions_omitted += 1;
                }
            }
            RoomEvent::TrackSubscribed {
                track: RemoteTrack::Audio(track),
                participant,
                ..
            } if is_interview_participant(
                Some(&participant.identity().0),
                self.interview.candidate_identity,
            ) =>
            {
                self.context.media.attach_audio(&track, &participant);
                self.ingress.yielded(Instant::now());
            }
            RoomEvent::TrackUnsubscribed {
                track: RemoteTrack::Audio(_),
                participant,
                ..
            } if is_interview_participant(
                Some(&participant.identity().0),
                self.interview.candidate_identity,
            ) =>
            {
                self.context.media.audio = None;
                self.context.activity.clear_candidate_voice_activity();
                self.ingress.yielded(Instant::now());
            }
            RoomEvent::TrackSubscribed {
                track: RemoteTrack::Video(track),
                participant,
                ..
            } if self.interview.boot.candidate_video
                && is_interview_participant(
                    Some(&participant.identity().0),
                    self.interview.candidate_identity,
                ) =>
            {
                self.context.media.video = Some(
                    ::livekit::webrtc::video_stream::native::NativeVideoStream::new(
                        track.rtc_track(),
                    ),
                );
            }
            RoomEvent::TrackUnsubscribed {
                track: RemoteTrack::Video(_),
                participant,
                ..
            } if is_interview_participant(
                Some(&participant.identity().0),
                self.interview.candidate_identity,
            ) =>
            {
                self.context.media.video = None;
            }
            RoomEvent::Disconnected { reason } => {
                eprintln!(
                    "room disconnected during Live recovery: reason={reason:?} room={}",
                    self.room.name()
                );
                self.terminal = Some(Terminal::RoomClosed);
            }
            _ => {}
        }
    }

    async fn step<T>(
        &mut self,
        phase: &'static str,
        op: impl Future<Output = T>,
    ) -> Result<T, Cancelled> {
        #[cfg(test)]
        {
            let probe = self.loops.recovery_phase.clone();
            let mut announced = false;
            tokio::pin!(op);
            let observed = std::future::poll_fn(|cx| {
                let state = op.as_mut().poll(cx);
                if state.is_pending() && !announced {
                    announced = true;
                    if let Some(probe) = &probe {
                        let _ = probe.send(phase);
                    }
                }
                state
            });
            self.wait(observed).await
        }
        #[cfg(not(test))]
        {
            let _ = phase;
            self.wait(op).await
        }
    }

    pub(super) async fn wait<T>(&mut self, op: impl Future<Output = T>) -> Result<T, Cancelled> {
        tokio::pin!(op);
        loop {
            if self.terminal.is_some() {
                return Err(Cancelled);
            }
            if !self.context.state.ended && tokio::time::Instant::now() >= self.loops.deadline_at {
                self.terminal = Some(Terminal::Deadline);
                return Err(Cancelled);
            }
            if self.loops.presence.gave_up(Instant::now()) {
                self.terminal = Some(Terminal::Absent);
                return Err(Cancelled);
            }
            tokio::select! {
                biased;
                () = tokio::time::sleep_until(self.loops.deadline_at), if !self.context.state.ended => {
                    self.terminal = Some(Terminal::Deadline);
                }
                event = async { self.interview.events.lock().await.recv().await } => {
                    match event { Some(event) => self.event(event), None => self.terminal = Some(Terminal::RoomClosed) }
                }
                packet = async {
                    #[cfg(test)]
                    if let Some(inputs) = &mut self.loops.recovery_inputs { return inputs.recv().await; }
                    std::future::pending::<Option<Packet>>().await
                } => {
                    if let Some(packet) = packet {
                        self.packet(packet.topic.as_deref(), packet.sender.as_deref(), &packet.payload);
                    } else {
                        #[cfg(test)]
                        { self.loops.recovery_inputs = None; }
                    }
                }
                _ = self.tick.tick() => {}
                Some(snapshot) = self.context.board.rx.recv(), if !self.context.state.ended => {
                    if board::record(self.context.board, self.context.state, snapshot) {
                        self.context.activity.reset_completed_coding_idle();
                    }
                    self.context.activity.last_code_change = Instant::now();
                    self.revision = self.revision.saturating_add(1);
                }
                frame = next_audio_frame(&mut self.context.media.audio), if self.context.media.audio.is_some() => {
                    let now = Instant::now();
                    if note_candidate_audio_frame(self.context.state, self.context.activity, frame.as_ref(), now) {
                        self.context.media.voiced_at = Some(now);
                    }
                    match frame {
                        Some(frame) => self.ingress.push(frame, now),
                        None => {
                            self.context.media.audio = None;
                            self.context.activity.clear_candidate_voice_activity();
                            self.ingress.yielded(now);
                        }
                    }
                }
                frame = next_video_frame(&mut self.context.media.video), if self.context.media.video.is_some() => {
                    if frame.is_none() { self.context.media.video = None; }
                }
                value = &mut op => return Ok(value),
            }
        }
    }

    async fn guarded<T>(
        &mut self,
        phase: &'static str,
        op: impl Future<Output = T>,
    ) -> Result<T, Cancelled> {
        #[cfg(test)]
        let gate = self
            .loops
            .recovery_writes
            .iter()
            .position(|gate| gate.phase == phase)
            .and_then(|index| self.loops.recovery_writes.remove(index));
        #[cfg(test)]
        let op = async move {
            if let Some(gate) = gate {
                gate.entered.notify_one();
                gate.release.notified().await;
            }
            op.await
        };
        #[cfg(test)]
        let after = self
            .loops
            .recovery_after
            .iter()
            .position(|after| after.phase == phase)
            .and_then(|index| self.loops.recovery_after.remove(index));
        #[cfg(test)]
        let op = async move {
            let value = op.await;
            if let Some(after) = after {
                (after.action)();
            }
            value
        };
        self.step(phase, op).await
    }

    async fn send(
        &mut self,
        phase: &'static str,
        input: session::PreparedInput,
        session: &mut GeminiLiveSession,
    ) -> Result<Result<(), Error>, Cancelled> {
        self.guarded(phase, input.send(session)).await
    }

    async fn notice(&mut self, message: &serde_json::Value, what: &str) -> Result<bool, Cancelled> {
        let packet = match browser_packet(TOPIC_CONTROL, message) {
            Ok(packet) => packet,
            Err(error) => {
                eprintln!("publishing {what} failed ({error}); continuing the interview");
                return Ok(false);
            }
        };
        let room = self.room;
        match self.wait(room.publish_data(packet)).await? {
            Ok(()) => Ok(true),
            Err(error) => {
                eprintln!("publishing {what} failed ({error}); continuing the interview");
                Ok(false)
            }
        }
    }

    pub(super) async fn flush_notices(&mut self) -> Result<(), Cancelled> {
        if let Some(round) = self.round {
            let message =
                serde_json::json!({"type":"round_state", "round":"behavioral", "status":round});
            if self.notice(&message, "the recovered round state").await? {
                if self.round == Some(round) {
                    self.round = None;
                }
                // This publication supersedes any earlier failed snapshot.
                self.loops.pending_round_notice = None;
            } else {
                self.loops.pending_round_notice = self.round;
                self.round = None;
            }
        }
        if self.context.state.thinking_notice != self.failed_thinking
            && let Some(thinking) = self.context.state.take_thinking_notice()
        {
            let message = session::thinking_state_message(thinking);
            if !self
                .notice(&message, "the recovered thinking state")
                .await?
            {
                self.failed_thinking = Some(thinking);
                self.context.state.announce_thinking();
            } else {
                self.failed_thinking = None;
            }
        }
        if let Some(phases) = session::framework_progress_unpublished(self.context.state)
            && self.failed_framework.as_ref() != Some(&phases)
        {
            let message = serde_json::json!({"type":"framework_state", "phases":phases});
            if self
                .notice(&message, "the recovered framework state")
                .await?
            {
                self.context.state.framework_published = phases;
                self.failed_framework = None;
            } else {
                self.failed_framework = Some(phases);
            }
        }
        while let Some(identity) = self.evictions.pop_front() {
            let config = self.interview.config;
            let room = self.room.name();
            if let Err(error) = self
                .step(
                    "duplicate_eviction",
                    evict_duplicate_agent(
                        config,
                        room,
                        &identity,
                        crate::current_epoch_millis() / 1000,
                    ),
                )
                .await?
            {
                eprintln!(
                    "removing the duplicate agent failed ({error}); continuing the interview"
                );
            }
        }
        Ok(())
    }
    async fn flush_contexts(&mut self, session: &mut GeminiLiveSession) -> Result<bool, Cancelled> {
        while !self.held_contexts.is_empty() || self.held_omitted != 0 {
            let contexts = std::mem::take(&mut self.held_contexts);
            self.held_bytes = 0;
            let steps = std::mem::take(&mut self.held_steps);
            let omitted = std::mem::take(&mut self.held_omitted);
            let mut body = contexts.into_iter().collect::<Vec<_>>().join("\n");
            if omitted != 0 {
                body.push_str(&format!("\n[SYSTEM EVENT] {omitted} earlier context summaries exceeded recovery retention and are omitted here. Do not infer their details; use the retained platform evidence and latest test report."));
            }
            body.push_str(&earlier_references(&steps));
            let body = crate::agent::with_timer(self.context.state, body);
            let input = session::prepare_input(
                self.context.state,
                ModelInputKind::Turn,
                &body,
                None,
                ContinuityScope::Ordinary,
                session::Transport::Context,
            );
            if self.send("held_context", input, session).await?.is_ok() {
                for step in steps {
                    if !self.context.state.earlier_steps_named.contains(&step) {
                        self.context.state.earlier_steps_named.push(step);
                    }
                }
            } else {
                self.context.state.needs_cold_brief = true;
                self.restore_steps(steps);
                return Ok(false);
            }
        }
        Ok(true)
    }

    async fn flush_actions(
        &mut self,
        session: &mut GeminiLiveSession,
    ) -> Result<(bool, bool), Cancelled> {
        let mut spoke = false;
        while !self.context.state.interviewer_silenced() {
            let Some(mut action) = self.action.take() else {
                break;
            };
            let mut extra = Vec::new();
            for step in std::mem::take(&mut self.context.state.earlier_steps_pending) {
                if !action.earlier.contains(&step) {
                    action.earlier.push(step);
                    extra.push(step);
                }
            }
            let revision = self.obligation_revision;
            let body = crate::agent::with_timer(
                self.context.state,
                crate::agent::with_thinking_debt(
                    self.context.state,
                    &format!("{}{}", action.text, earlier_references(&extra)),
                ),
            );
            let deferred = self.ingress.has_audio();
            let input = session::prepare_input(
                self.context.state,
                ModelInputKind::Turn,
                &body,
                (!deferred).then_some(TurnCause::Recovery),
                ContinuityScope::Ordinary,
                session::Transport::Context,
            );
            if self.send("held_action", input, session).await?.is_err() {
                self.restore_steps(action.earlier);
                return Ok((false, spoke));
            }
            if self.context.activity.prompt_sequence == action.sequence {
                self.context
                    .activity
                    .mark_recovery_prompted(Instant::now(), Some(&action.text));
            }
            if self.obligation_revision == revision {
                let sent_code = self.context.state.code_shown.clone();
                self.context.state.clear_thinking_debt();
                self.context.state.code_shown = sent_code;
            }
            for step in action.earlier {
                if !self.context.state.earlier_steps_named.contains(&step) {
                    self.context.state.earlier_steps_named.push(step);
                }
            }
            if deferred {
                self.ingress.yielded(Instant::now());
            } else {
                spoke = true;
            }
            self.wait(std::future::ready(())).await?;
        }
        Ok((true, spoke))
    }

    fn pending(&self) -> bool {
        !self.held_contexts.is_empty()
            || self.held_omitted != 0
            || self.round.is_some()
            || !self.evictions.is_empty()
            || (self.action.is_some() && !self.context.state.interviewer_silenced())
            || (self.context.state.thinking_notice.is_some()
                && self.context.state.thinking_notice != self.failed_thinking)
            || session::framework_progress_unpublished(self.context.state)
                .is_some_and(|phases| self.failed_framework.as_ref() != Some(&phases))
    }

    async fn drain(&mut self, session: &mut GeminiLiveSession) -> Result<bool, Cancelled> {
        let mut spoke = false;
        loop {
            let contexts_ok = self.flush_contexts(session).await?;
            self.flush_notices().await?;
            if !contexts_ok {
                break;
            }
            let (ok, replied) = self.flush_actions(session).await?;
            spoke |= replied;
            if !ok {
                // Model transport failure must not discard independent room
                // notices or duplicate-agent cleanup received during the write.
                self.flush_notices().await?;
                break;
            }
            self.wait(std::future::ready(())).await?;
            if !self.pending() {
                break;
            }
        }
        Ok(spoke)
    }
}

impl Driver<'_, '_, '_> {
    pub(super) async fn prefix(
        &mut self,
        old: &mut GeminiLiveSession,
        audio: &mut Vec<u8>,
    ) -> Result<Result<session::SpeechHandoff, Error>, Cancelled> {
        if !audio.is_empty() {
            if let Err(error) = self
                .step("prefix_audio_write", old.send_audio_pcm_16khz(audio))
                .await?
            {
                return Ok(Err(error));
            }
            audio.clear();
        }
        if let Err(error) = self
            .step("prefix_end_audio_write", old.end_audio_turn())
            .await?
        {
            return Ok(Err(error));
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut fragments = Vec::new();
        let mut checkpoint_after = None;
        let mut discarded = 0;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let wait = if fragments.is_empty() {
                left
            } else {
                left.min(Duration::from_millis(200))
            };
            match self
                .step(
                    if fragments.is_empty() {
                        "prefix_collect_initial"
                    } else {
                        "prefix_collect"
                    },
                    tokio::time::timeout(wait, old.next_event()),
                )
                .await?
            {
                Ok(Some(GeminiEvent::InputTranscript(text))) if !text.trim().is_empty() => {
                    fragments.push(text.clone());
                    if session::apply_recovery_transcript(
                        self.room,
                        self.context,
                        &text,
                        old.event_received_at().unwrap_or_else(Instant::now),
                    ) {
                        self.obligation_revision = self.obligation_revision.saturating_add(1);
                        self.ingress.yielded(Instant::now());
                    }
                    if checkpoint_after.is_none() {
                        checkpoint_after = Some(Instant::now());
                        let input = session::prepare_plain_context(
                            self.context.state,
                            "[SYSTEM EVENT] The candidate continues speaking. Keep listening; no reply is requested.",
                        );
                        if let Err(error) = self
                            .guarded("prefix_listening_context", input.send(old))
                            .await?
                        {
                            return Ok(Err(error));
                        }
                    }
                }
                Ok(Some(GeminiEvent::UsageRecorded | GeminiEvent::InputTranscript(_))) => {}
                Ok(Some(_)) => discarded += 1,
                Ok(None) | Err(_) => break,
            }
        }
        Ok(match checkpoint_after {
            Some(checkpoint_after) => Ok(session::SpeechHandoff {
                checkpoint_after,
                transcript_fragments: fragments,
                discarded,
            }),
            None => Err(std::io::Error::other(format!(
                "speech handoff received no transcription; discarded_events={discarded}"
            ))
            .into()),
        })
    }
}

/// What an episode's closing line needs beyond its result, gathered once so
/// each way out of [`replace`] logs the same fields.
struct EpisodeTail<'a> {
    start: latency::EpisodeStart<'a>,
    attempt: usize,
    resume_tried: bool,
}

impl EpisodeTail<'_> {
    fn log(
        self,
        result: latency::EpisodeResult,
        new_socket: Option<u64>,
        replacement: Option<u64>,
        interview_ms: Option<i64>,
        milestones: latency::EpisodeMilestones,
    ) {
        eprintln!(
            "{}",
            latency::episode_end_line(&latency::EpisodeEnd {
                start: self.start,
                result,
                new_socket,
                replacement,
                attempt: self.attempt,
                resume_tried: self.resume_tried,
                milestones,
                interview_ms,
            })
        );
    }
}

enum Opened {
    Ready {
        resumed: bool,
        fallback: Fallback,
        connected: Instant,
        spoke: bool,
    },
    Failed(Error),
}

pub(super) async fn replace(
    room: &RoomLink,
    context: &mut GeminiEventContext<'_>,
    interview: InterviewContext<'_>,
    loops: &mut RoomLoop,
    trigger: Trigger,
    mut handoff: Option<session::SpeechHandoff>,
    finalize_prefix: bool,
) -> Result<ControlFlow<()>, Error> {
    let rotation_started = Instant::now();
    let (episode, obligation) = context.activity.latency.begin_episode();
    let old_socket = context.activity.live_socket;
    let advisories = context.activity.latency.take_advisories();
    let episode_start = latency::EpisodeStart {
        room: room.name(),
        session: context.activity.live_session_id,
        episode,
        trigger: trigger.as_str(),
        old_socket,
        obligation,
        obligation_origin: obligation.and(context.activity.latency.owed_origin()),
        advisories,
        interview_ms: context.activity.latency.interview_ms(rotation_started),
    };
    eprintln!("{}", latency::episode_start_line(&episode_start));
    session::finish_turn_timing(
        room.name(),
        context.activity,
        &log_clock(context.state),
        timing::TimingOutcome::Replaced,
        &mut std::io::stderr().lock(),
    );
    if context.gemini.closed_for_billing(interview.keys) {
        context.activity.live_exit = LiveOutcome::Billing;
        EpisodeTail {
            start: episode_start,
            attempt: loops.restarts,
            resume_tried: false,
        }
        .log(
            latency::EpisodeResult::NoSetup,
            None,
            None,
            context.activity.latency.interview_ms(Instant::now()),
            Default::default(),
        );
        leave_room(room).await;
        return Ok(ControlFlow::Break(()));
    }
    let mut handle = context.gemini.recovery_handle(interview.keys);
    let mut had_handle = handle.is_some();
    let mut old = context.gemini.retire();
    let (owed, debt, original_prompt) =
        hand_over(context.state, context.activity, context.output_audio);
    if owed {
        context.activity.owe_prompt(Instant::now(), original_prompt);
    }
    let mut audio = std::mem::take(&mut context.media.audio_bytes);
    let mut audio_unconfirmed = std::mem::take(&mut context.media.audio_unconfirmed);

    // A previous write may have reached Gemini. Do not retry its bytes during
    // prefix finalization or carry them onto a second socket.
    let quarantined_bytes = std::mem::take(&mut context.media.quarantined_audio_bytes)
        + if audio_unconfirmed {
            let bytes = audio.len();
            audio.clear();
            audio_unconfirmed = false;
            bytes
        } else {
            0
        };
    let mut fresh = None;
    let mut driver = Driver::new(room, context, loops, interview);
    let mut checkpoint_age = String::new();
    let mut initial_owed = owed;
    let mut marks = latency::EpisodeMilestones::default();
    let since_start = |at: Instant| {
        Some(crate::agent::saturating_ms(
            at.saturating_duration_since(rotation_started),
        ))
    };
    let mut resume_tried = false;
    let mut setup_tried = false;
    let result = async {
        driver.wait(std::future::ready(())).await?;
        if finalize_prefix {
            audio_unconfirmed |= !audio.is_empty();
            handoff = match driver.prefix(&mut old, &mut audio).await? {
                Ok(prefix) => Some(prefix),
                Err(error) => { eprintln!("speech handoff failed ({error}); recovering transport room={}", room.name()); None }
            };
            if audio.is_empty() { audio_unconfirmed = false; }
            driver.loops.deferred_restart.cancel();
            handle = old.recovery_handle(interview.keys);
            had_handle = handle.is_some();
            if let Some(prefix) = &handoff {
                eprintln!("speech handoff: prefix_fragments={} discarded_events={} at={} room={}",
                    prefix.transcript_fragments.len(), prefix.discarded, log_clock(driver.context.state), room.name());
            }
        }
        let age = replaced_socket_age(old.age(), trigger == Trigger::Watchdog || owed, driver.context.activity);
        if !take_restart_attempt(&mut driver.loops.restarts, age) {
            return Ok(Opened::Failed(std::io::Error::other("consecutive Gemini restart budget exhausted").into()));
        }
        let allow_resume = driver.loops.resume_recovery.allow_resume(old.age());
        if driver.loops.resume_recovery.suppressed {
            eprintln!("{}; rebuilding from local state room={}",
                if driver.loops.resume_recovery.resumed { "Gemini resumed socket was replaced young, so resumption is suppressed" }
                else { "Gemini cold socket was replaced young" }, room.name());
        }
        let handle = handle.filter(|_| allow_resume && handoff.as_ref().is_none_or(|h|
            old.checkpoint_age().is_some_and(|age| age <= h.checkpoint_after.elapsed())));
        let handle_offered = handle.is_some();
        checkpoint_age = match (&handle, old.checkpoint_age()) {
            (None, _) => "none".into(),
            (Some(_), Some(age)) => format!("{}s", age.as_secs()),
            _ => "inherited".into(),
        };
        close_turns(room, driver.context);
        let message = interviewer_state(true);
        let _ = driver.notice(&message, "the reconnect notice").await?;
        marks.notice_ms = since_start(Instant::now());
        let _ = driver.step("close", old.shutdown()).await?;
        session::drain_retired_usage(
            room,
            driver.context,
            &mut old,
            old_socket,
            latency::PressureEnd::Replacement,
        );
        // The writer must be released even when close failed.
        drop(old.retire());
        let setup_started = Instant::now();
        setup_tried = true;
        let resumed_session = if let Some((key, handle)) = handle {
            resume_tried = true;
            match driver.step("resume", interview.open_live(Some((&key, &handle)))).await? {
                Ok(session) => Some(session),
                Err(error) => { eprintln!("Gemini refused to resume ({error}); starting a fresh session instead"); None }
            }
        } else { None };
        let resumed = resumed_session.is_some();
        let fallback = fallback_reason(resumed, handoff.is_some(), allow_resume, had_handle, handle_offered);
        let opened = if let Some(session) = resumed_session {
            fresh = Some(session);
            Ok(Ok(()))
        } else {
            let mut restarts = driver.loops.restarts;
            let opened = driver.wait(open_cold_session_into(interview, &mut restarts, Some(room), &mut fresh)).await;
            driver.loops.restarts = restarts;
            opened
        };

        // Cancellation may interrupt a notice after setup has completed. The
        // caller-owned slot must be counted before propagating that terminal.
        if let Some(session) = fresh.as_ref() {
            #[cfg(test)]
            if let Some(probe) = &driver.loops.recovery_reader {
                let _ = probe.send(session.inspect_reader_for_test().expect("setup owns its reader"));
            }
            #[cfg(not(test))]
            let _ = session;
            driver.context.state.evidence_ledger.record_model_input(ModelInputKind::LiveSetup, &interview.boot.instructions);
            driver.context.activity.live_socket += 1;
            marks.setup_ms = Some(crate::agent::saturating_ms(setup_started.elapsed()));
        }
        if let Err(error) = opened? { return Ok(Opened::Failed(error)); }
        let session = fresh.as_mut().expect("opened transport is owned before another wait");
        driver.context.activity.reset_context_observations(resumed);
        driver.context.activity.sync_cut_plausible(driver.context.state);
        let connected = Instant::now();
        marks.connected_ms = since_start(connected);
        driver.wait(std::future::ready(())).await?;
        let message = interviewer_state(false);
        let _ = driver.notice(&message, "the reconnected notice").await?;
        let mut spoke = false;
        for correction in 0..2 {
            let revision = driver.revision;
            let obligation_revision = driver.obligation_revision;
            let was_held = driver.context.state.interviewer_silenced();
            let sequence = driver.context.activity.prompt_sequence;
            let owed = driver.context.activity.reply_unfinished();
            initial_owed = owed;
            let prompt = driver.context.activity.prompt_text.clone();
            let code = driver.context.state.code.clone();
            let editor = (driver.context.state.evidence_ledger.code.substantive_revision, driver.context.state.language.clone());
            let (input, reply, clear_debt, clear_cold, listening) = if let Some(h) = &handoff {
                match session::prepare_listening_handoff(driver.context.state, h, resumed, prompt.as_deref()) {
                    Ok((input, briefed)) => (input, false, !driver.context.state.interviewer_silenced(), true, briefed),
                    Err(error) => { eprintln!("preparing speech handoff failed ({error})"); driver.context.state.needs_cold_brief = true; break; }
                }
            } else {
                let plan = prepare_recovery_brief(driver.context.state,
                    if resumed { Replacement::Resumed { owed } } else { Replacement::Cold { owed } }, prompt.as_deref());
                (plan.input, plan.reply, plan.clear_debt, plan.clear_cold, false)
            };
            let mut earlier = if driver.action.as_ref().is_some_and(|action| action.sequence == sequence) {
                driver.action.take().map_or_else(Vec::new, |action| action.earlier)
            } else { Vec::new() };
            for step in std::mem::take(&mut driver.context.state.earlier_steps_pending) {
                if !earlier.contains(&step) { earlier.push(step); }
            }
            let sent = driver
                .send(
                    if handoff.is_some() { "listening_brief" } else { "recovery_brief" },
                    input,
                    session,
                )
                .await?;
            if sent.is_ok() {
                if handoff.is_none() || listening {
                    driver.context.activity.silent_editor_delivery = Some(editor);
                    driver.context.state.code_shown = code;
                }
                if driver.obligation_revision == obligation_revision {
                    if clear_debt {
                        let sent_code = driver.context.state.code_shown.clone();
                        driver.context.state.clear_thinking_debt();
                        driver.context.state.code_shown = sent_code;
                    }
                    if clear_cold { driver.context.state.needs_cold_brief = false; }
                }
                for step in earlier { if !driver.context.state.earlier_steps_named.contains(&step) { driver.context.state.earlier_steps_named.push(step); } }
                if sequence == driver.context.activity.prompt_sequence {
                    if reply && !driver.context.state.interviewer_silenced() {
                        driver.context.activity.mark_recovery_prompted(Instant::now(), prompt.as_deref());
                        driver.context.state.recovery_reply_pending = true;
                        spoke = true;
                    } else if handoff.is_some() && owed {
                        driver.context.activity.owe_prompt(Instant::now(), prompt);
                        driver.context.activity.arm_handoff_reply(Instant::now());
                    }
                }
            } else {
                driver.restore_steps(earlier);
                if !resumed || handoff.is_some() { driver.context.state.needs_cold_brief = true; }
                if sequence == driver.context.activity.prompt_sequence && owed {
                    driver.context.activity.owe_prompt(Instant::now(), prompt);
                }
                break;
            }
            if driver.revision == revision || (driver.action.is_none() && was_held == driver.context.state.interviewer_silenced()) { break; }
            if correction == 1 { break; }
            if reply { driver.context.activity.socket.discarding_output = true; }
        }
        if handoff.is_some() && resumed
            && let Some(body) = crate::agent::editor_update_body(driver.context.state) {
                let code = driver.context.state.code.clone();
                let editor = (driver.context.state.evidence_ledger.code.substantive_revision, driver.context.state.language.clone());
                let body = format!("[SYSTEM EVENT] Silent editor state update; keep listening to the candidate. This requests no reply. Use this current code when answering their latest question.\n{body}");
                let input = session::prepare_plain_context(driver.context.state, &body);
                if driver.send("silent_editor", input, session).await?.is_ok() {
                    driver.context.state.code_shown = code;
                    driver.context.activity.silent_editor_delivery = Some(editor);
                }
        }
        if !resumed && driver.context.state.interview_mode.is_whiteboard()
            && let Some(bytes) = driver.context.board.recovery_image()
            && driver.guarded("board_image", session.send_video_frame(&bytes, crate::gemini::GEMINI_IMAGE_MIME_TYPE)).await?.is_ok() {
            driver.context.board.recovered_image(&bytes, Instant::now());
        }
        spoke |= driver.drain(session).await?;
        marks.opener_ms = since_start(Instant::now());
        spoke &= !driver.context.state.interviewer_silenced();
        Ok::<_, Cancelled>(Opened::Ready { resumed, fallback, connected, spoke })
    }.await;
    driver.preserve_room_work();
    if let Some(action) = driver.action.take() {
        driver.restore_steps(action.earlier);
    }
    let held_steps = std::mem::take(&mut driver.held_steps);
    driver.restore_steps(held_steps);

    // Explicit joins precede draining on every path, including a cancelled
    // close and a newly set-up socket rejected by a terminal control.
    old.stop_reader().await;
    session::drain_retired_usage(
        room,
        driver.context,
        &mut old,
        old_socket,
        latency::PressureEnd::Replacement,
    );
    drop(old);
    if matches!(result, Err(_) | Ok(Opened::Failed(_)))
        && let Some(mut session) = fresh.take()
    {
        session.stop_reader().await;
        let discarded = driver.context.activity.live_socket;
        session::drain_retired_usage(
            room,
            driver.context,
            &mut session,
            discarded,
            latency::PressureEnd::Discarded,
        );
        eprintln!(
            "recovery setup discarded: socket={} room={}",
            driver.context.activity.live_socket,
            room.name()
        );
    }
    let unconfirmed_bytes = quarantined_bytes + if audio_unconfirmed { audio.len() } else { 0 };
    if audio_unconfirmed {
        audio = Vec::new();
    }
    eprintln!(
        "recovery ingress: dropped_bytes={} unconfirmed_bytes={} unsent_bytes={} omitted_contexts={} omitted_evictions={} room={}",
        driver.ingress.dropped_bytes,
        unconfirmed_bytes,
        audio.len(),
        driver.held_omitted_total,
        driver.evictions_omitted,
        room.name()
    );
    let attempt = driver.loops.restarts;
    let end = EpisodeTail {
        start: episode_start,
        attempt,
        resume_tried,
    };
    match result {
        Err(_) => {
            end.log(
                latency::EpisodeResult::Cancelled,
                None,
                None,
                driver.context.activity.latency.interview_ms(Instant::now()),
                marks,
            );
            let terminal = driver
                .terminal
                .take()
                .expect("cancelled wait retains its terminal intent");
            #[cfg(test)]
            if let Some(probe) = &driver.loops.recovery_terminal {
                let _ = probe.send(terminal);
                return Ok(ControlFlow::Break(()));
            }
            match terminal {
                Terminal::End { topic, payload } => {
                    packets::handle_data_packet(
                        room,
                        driver.context,
                        interview,
                        &topic,
                        &payload,
                        true,
                    )
                    .await
                }
                Terminal::Finish(reason) => {
                    packets::finish_interview(room, driver.context, interview, reason).await
                }
                Terminal::Deadline => {
                    on_hard_deadline(room, driver.context, driver.loops, interview).await
                }
                Terminal::Absent | Terminal::RoomClosed => {
                    leave_room(room).await;
                    Ok(ControlFlow::Break(()))
                }
            }
        }
        Ok(Opened::Failed(error)) => {
            end.log(
                if setup_tried {
                    latency::EpisodeResult::Failed
                } else {
                    latency::EpisodeResult::NoSetup
                },
                None,
                None,
                driver.context.activity.latency.interview_ms(Instant::now()),
                marks,
            );
            driver.context.activity.live_exit =
                LiveOutcome::from_error(error.as_ref(), LiveOutcome::GeminiUnreachable);
            if driver.context.activity.live_exit == LiveOutcome::Billing {
                leave_room(room).await;
                Ok(ControlFlow::Break(()))
            } else {
                end_without_interviewer(room, driver.context, interview, driver.loops).await
            }
        }
        Ok(Opened::Ready {
            resumed,
            fallback,
            connected,
            spoke,
        }) => {
            *driver.context.gemini = fresh.take().expect("only ready transport is committed");
            driver.context.media.presence_frame_sent = false;
            driver.context.media.audio_bytes = audio;
            driver.context.media.audio_unconfirmed = false;
            driver.context.media.recovery_audio = std::mem::take(&mut driver.ingress);
            driver.loops.resume_recovery.connected(resumed);
            let owed_prompt = driver.context.activity.prompt_text.clone();
            let installed_at = Instant::now();
            marks.install_ms = since_start(installed_at);
            let replacement = driver.context.activity.latency.installed_replacement();
            let identity = format!(
                "pid={} session={} episode={episode} replacement={replacement} old_socket={old_socket} new_socket={} obligation={} interview_ms={}",
                std::process::id(),
                driver.context.activity.live_session_id,
                driver.context.activity.live_socket,
                obligation.map_or_else(|| "none".to_string(), |id| id.to_string()),
                driver
                    .context
                    .activity
                    .latency
                    .interview_ms(installed_at)
                    .map_or_else(|| "none".to_string(), |ms| ms.to_string()),
            );
            end.log(
                latency::EpisodeResult::Installed,
                Some(driver.context.activity.live_socket),
                Some(replacement),
                driver.context.activity.latency.interview_ms(installed_at),
                marks,
            );
            eprintln!(
                "replacement: at={} resumed={resumed} {} rotation_gap_ms={} checkpoint_age={checkpoint_age} owed={initial_owed} ({debt}) owed_chars={} editor_chars={} {} {identity} room={}",
                log_clock(driver.context.state),
                replacement_fields(trigger, handover_kind(fallback), fallback),
                crate::agent::saturating_ms(connected.saturating_duration_since(rotation_started)),
                owed_prompt.as_deref().map_or(0, |p| p.chars().count()),
                driver.context.state.code.chars().count(),
                prompt_fields(driver.context.state, driver.context.activity),
                room.name()
            );
            eprintln!(
                "{}",
                replacement_notice_line(handover_kind(fallback), room.name())
            );
            driver.context.activity.begin_rotation_watch(
                spoke,
                RotationWatch {
                    started: rotation_started,
                    connected,
                    resumed,
                    candidate_spoke_at: None,
                },
            );
            if spoke {
                eprintln!(
                    "{}",
                    prompt_line(
                        driver.context.state,
                        driver.context.activity,
                        "kind=briefing",
                        room.name()
                    )
                );
            }
            Ok(ControlFlow::Continue(()))
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/livekit/recovery.rs"]
mod tests;
